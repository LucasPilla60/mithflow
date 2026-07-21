use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::sync::{Arc, Mutex};

/// Promedia los canales para obtener mono. `cpal` entrega las muestras
/// intercaladas; pasar un buffer estéreo como si fuera mono produce audio al
/// doble de velocidad y transcripción basura.
pub fn downmix(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples
        .chunks(channels as usize)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect()
}

/// Resampleo a la frecuencia que espera el modelo.
///
/// Devuelve `Result` en vez de paniquear: ningún error de audio puede cerrar
/// la aplicación.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Result<Vec<f32>, String> {
    if from == to || samples.is_empty() {
        return Ok(samples.to_vec());
    }
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler =
        SincFixedIn::<f32>::new(to as f64 / from as f64, 2.0, params, samples.len(), 1)
            .map_err(|e| format!("no pude crear el resampler: {e}"))?;
    let out = resampler
        .process(&[samples.to_vec()], None)
        .map_err(|e| format!("falló el resampleo: {e}"))?;
    Ok(out.into_iter().next().unwrap_or_default())
}

/// Grabadora que acumula audio en memoria mientras está activa.
///
/// El dispositivo se abre al empezar a grabar y se cierra al terminar. La
/// versión Python lo dejaba abierto siempre, lo que mantiene encendido el
/// indicador de micrófono de Windows y consume batería en las notebooks.
pub struct Recorder {
    buffer: Arc<Mutex<Vec<f32>>>,
    stream: Option<cpal::Stream>,
    sample_rate: u32,
    channels: u16,
    /// Tope de duración de la grabación, en segundos. Es un campo y no la
    /// constante porque el usuario lo configura: quien dicta párrafos largos
    /// necesita más de tres minutos, y quien sólo dicta frases prefiere que un
    /// atajo apretado sin querer se corte antes.
    tope_secs: f32,
}

impl Recorder {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            sample_rate: crate::config::SAMPLE_RATE,
            channels: 1,
            tope_secs: crate::config::MAX_RECORDING_SECS,
        })
    }

    /// Cambia el tope de duración. Un valor imposible (cero, negativo, `NaN`)
    /// vuelve al de fábrica: dejar pasar un cero convertiría toda grabación en
    /// un buffer vacío, o sea el dictado roto en silencio.
    pub fn fijar_tope(&mut self, segundos: f32) {
        self.tope_secs = if segundos.is_finite() && segundos > 0.0 {
            segundos
        } else {
            crate::config::MAX_RECORDING_SECS
        };
    }

    /// El tope vigente, en segundos.
    pub fn tope_secs(&self) -> f32 {
        self.tope_secs
    }

    pub fn start(&mut self) -> Result<(), String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or("no hay dispositivo de entrada (¿micrófono conectado?)")?;
        let cfg = device
            .default_input_config()
            .map_err(|e| format!("no pude leer la configuración del micrófono: {e}"))?;
        self.sample_rate = cfg.sample_rate().0;
        self.channels = cfg.channels();
        self.buffer.lock().map_err(|_| "buffer envenenado")?.clear();

        let buf = Arc::clone(&self.buffer);
        let err_cb = |e| eprintln!("error de captura: {e}");
        let stream_cfg = cfg.config();

        // El formato nativo no siempre es f32: construir un stream f32 sobre un
        // dispositivo que entrega i16 falla con StreamConfigNotSupported, o
        // paniquea al interpretar el buffer.
        let stream = match cfg.sample_format() {
            SampleFormat::F32 => device.build_input_stream(
                &stream_cfg,
                move |data: &[f32], _: &_| {
                    if let Ok(mut b) = buf.lock() {
                        b.extend_from_slice(data);
                    }
                },
                err_cb,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                &stream_cfg,
                move |data: &[i16], _: &_| {
                    if let Ok(mut b) = buf.lock() {
                        b.extend(data.iter().map(|s| *s as f32 / 32768.0));
                    }
                },
                err_cb,
                None,
            ),
            SampleFormat::U16 => device.build_input_stream(
                &stream_cfg,
                move |data: &[u16], _: &_| {
                    if let Ok(mut b) = buf.lock() {
                        b.extend(data.iter().map(|s| (*s as f32 - 32768.0) / 32768.0));
                    }
                },
                err_cb,
                None,
            ),
            otro => return Err(format!("formato de audio no soportado: {otro:?}")),
        }
        .map_err(|e| format!("no pude abrir el stream: {e}"))?;

        stream.play().map_err(|e| e.to_string())?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Detiene la captura, cierra el micrófono y devuelve el audio listo para
    /// el modelo: mono, 16 kHz, f32.
    pub fn stop(&mut self) -> Result<Vec<f32>, String> {
        self.stream.take(); // al soltarlo se cierra el dispositivo
        let raw = self.buffer.lock().map_err(|_| "buffer envenenado")?.clone();
        let mono = downmix(&raw, self.channels);
        let mut audio = resample(&mono, self.sample_rate, crate::config::SAMPLE_RATE)?;

        // Descartar el tramo donde suena el tono de inicio: el micrófono lo
        // capta por los parlantes y el modelo alucina más sobre ese ruido.
        let guarda = (crate::config::TONE_GUARD_SECS * crate::config::SAMPLE_RATE as f32) as usize;
        if audio.len() > guarda {
            audio.drain(..guarda);
        }

        // Tope de duración: recortar en vez de rechazar, para no perder lo ya dicho.
        let maximo = (self.tope_secs * crate::config::SAMPLE_RATE as f32) as usize;
        if audio.len() > maximo {
            audio.truncate(maximo);
        }
        Ok(audio)
    }

    /// Duración capturada hasta el momento, para avisar cuando se acerca al tope.
    pub fn elapsed_secs(&self) -> f32 {
        let n = self.buffer.lock().map(|b| b.len()).unwrap_or(0);
        if self.channels == 0 || self.sample_rate == 0 {
            return 0.0;
        }
        n as f32 / self.channels as f32 / self.sample_rate as f32
    }
}

/// `cpal` rompió y volvió a arreglar `Send` en `Stream` entre versiones: si
/// deja de serlo, `Recorder` no se puede mover entre hilos y esto no compila.
#[cfg(test)]
const _: fn() = || {
    fn assert_send<T: Send>() {}
    let _ = assert_send::<cpal::Stream>;
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_estereo_promedia_canales() {
        let estereo = vec![1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(downmix(&estereo, 2), vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn downmix_mono_no_cambia_nada() {
        let mono = vec![0.1, 0.2, 0.3];
        assert_eq!(downmix(&mono, 1), mono);
    }

    #[test]
    fn resample_ajusta_la_cantidad_de_muestras() {
        // 48 kHz -> 16 kHz da aproximadamente un tercio. La tolerancia no es
        // de ±1: un resampler sinc tiene retardo de filtro (sinc_len/2 * ratio
        // ≈ 42 muestras acá), así que la primera pasada devuelve algo menos.
        // Lo que este test detecta es un error de FACTOR, no de bordes.
        let entrada = vec![0.0f32; 48_000];
        let salida = resample(&entrada, 48_000, 16_000).unwrap();
        let error_relativo = (salida.len() as f32 - 16_000.0).abs() / 16_000.0;
        assert!(
            error_relativo < 0.01,
            "esperaba ~16000 muestras (±1%), obtuve {}",
            salida.len()
        );
    }

    #[test]
    fn resample_a_la_misma_frecuencia_es_identidad() {
        let entrada = vec![0.1, 0.2, 0.3];
        assert_eq!(resample(&entrada, 16_000, 16_000).unwrap(), entrada);
    }

    /// Un tono puro debe seguir siendo un tono puro después del resampleo:
    /// verifica que no haya aliasing ni pérdida de energía.
    #[test]
    fn resample_preserva_la_energia_de_una_senoide() {
        let f = 440.0;
        let entrada: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / 48_000.0).sin())
            .collect();
        let salida = resample(&entrada, 48_000, 16_000).unwrap();
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let (a, b) = (rms(&entrada), rms(&salida));
        assert!((a - b).abs() / a < 0.1, "RMS cambió demasiado: {a} -> {b}");
    }

    /// El tope arranca en el de fábrica, se puede mover, y un valor imposible
    /// no puede dejar el dictado en cero muestras.
    #[test]
    fn el_tope_de_grabacion_es_configurable_y_falla_cerrado() {
        let mut r = Recorder::new().expect("crear la grabadora no toca el hardware");
        assert_eq!(r.tope_secs(), crate::config::MAX_RECORDING_SECS);

        r.fijar_tope(45.0);
        assert_eq!(r.tope_secs(), 45.0);

        for imposible in [0.0, -10.0, f32::NAN, f32::INFINITY] {
            r.fijar_tope(imposible);
            assert_eq!(
                r.tope_secs(),
                crate::config::MAX_RECORDING_SECS,
                "un tope de {imposible} tiene que caer en el de fábrica"
            );
        }
    }

    /// Diagnóstico: imprime la configuración nativa del micrófono de esta
    /// máquina. Ignorado por defecto porque depende del hardware presente.
    /// Correr con: cargo test -p mithflow-core config_real_del_microfono
    ///             -- --ignored --nocapture
    #[test]
    #[ignore = "depende del hardware de la máquina"]
    fn config_real_del_microfono() {
        let device = cpal::default_host()
            .default_input_device()
            .expect("no hay dispositivo de entrada");
        let nombre = device.name().unwrap_or_else(|_| "<sin nombre>".into());
        let cfg = device
            .default_input_config()
            .expect("no pude leer la configuración por defecto");
        eprintln!("dispositivo:    {nombre}");
        eprintln!("sample_rate:    {} Hz", cfg.sample_rate().0);
        eprintln!("channels:       {}", cfg.channels());
        eprintln!("sample_format:  {:?}", cfg.sample_format());
    }

    /// El pipeline completo sobre el fixture estéreo 48 kHz real.
    #[test]
    fn procesa_el_wav_estereo_de_48khz() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/speech_es_48k_stereo.wav");
        if !path.exists() {
            eprintln!("falta el fixture estéreo, se saltea");
            return;
        }
        let mut r = hound::WavReader::open(&path).unwrap();
        let spec = r.spec();
        assert_eq!(spec.channels, 2, "el fixture debe ser estéreo");
        assert_eq!(spec.sample_rate, 48_000, "el fixture debe ser de 48 kHz");

        let raw: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
        let mono = downmix(&raw, spec.channels);
        assert_eq!(mono.len(), raw.len() / 2, "el downmix debe dar la mitad de muestras");

        let a16 = resample(&mono, spec.sample_rate, 16_000).unwrap();
        let dur_original = mono.len() as f32 / 48_000.0;
        let dur_final = a16.len() as f32 / 16_000.0;
        assert!(
            (dur_original - dur_final).abs() < 0.05,
            "la duración cambió: {dur_original:.2}s -> {dur_final:.2}s"
        );
    }
}
