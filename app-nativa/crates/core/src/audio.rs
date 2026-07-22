use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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

/// Cuánto nivel entró por el micrófono desde la última lectura.
///
/// El RMS es el que se compara contra [`crate::config::MIN_SPEECH_RMS`] —el
/// mismo umbral con el que el motor decide si vale la pena transcribir—, así que
/// un indicador dibujado con este número no puede decir "te escucho" de algo que
/// después se va a descartar. El pico sirve para que un golpe corto (una
/// consonante fuerte) se vea aunque el promedio de la ventana quede bajo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nivel {
    pub rms: f32,
    pub pico: f32,
}

/// Medidor de nivel de entrada: **lo escribe el callback de audio y lo lee otro
/// hilo**, sin lock de por medio.
///
/// # Por qué atómicos y no leer el buffer
///
/// La alternativa era que quien dibuja el indicador tomara el `Mutex` del buffer
/// veinticinco veces por segundo para mirarle la cola. Ese mutex es el mismo que
/// toma el callback de `cpal` en cada chunk, y el callback de audio no puede
/// esperar a nadie: si se pasa del plazo del dispositivo, lo que se pierde es
/// audio del usuario. Acá el callback nunca bloquea —sólo suma— y el lector
/// tampoco.
///
/// # Lo que cuesta en el camino crítico
///
/// Una pasada por el chunk (una multiplicación y dos sumas por muestra) y tres
/// operaciones atómicas por chunk, contra la copia al buffer que ya se hacía. Y
/// se puede apagar entero con [`Medidor::activar`]: con el indicador
/// desactivado en Ajustes, el callback lee un `bool` y vuelve.
#[derive(Debug)]
pub struct Medidor {
    activo: AtomicBool,
    /// Suma de cuadrados de las muestras **ya mezcladas a mono**, en bits de
    /// `f64`: en `f32` una grabación larga pierde los aportes chicos contra un
    /// acumulador grande.
    suma_cuadrados: AtomicU64,
    /// Cuántas muestras mono entraron en esa suma.
    muestras: AtomicU64,
    /// Mayor valor absoluto visto, en bits de `f32`.
    pico: AtomicU32,
}

impl Medidor {
    pub fn nuevo() -> Self {
        Self {
            activo: AtomicBool::new(false),
            suma_cuadrados: AtomicU64::new(0),
            muestras: AtomicU64::new(0),
            pico: AtomicU32::new(0),
        }
    }

    /// Enciende o apaga la medición. Apagado, [`Medidor::acumular`] no hace
    /// **nada** más que leer un átomo: es lo que hace que desactivar el
    /// indicador en Ajustes no cueste un solo ciclo en el camino del audio.
    pub fn activar(&self, activo: bool) {
        self.activo.store(activo, Ordering::Relaxed);
    }

    pub fn esta_activo(&self) -> bool {
        self.activo.load(Ordering::Relaxed)
    }

    /// Suma un chunk recién capturado. Se llama **desde el callback de `cpal`**.
    ///
    /// Recibe un iterador y no un `&[f32]` porque el dispositivo puede entregar
    /// `i16` o `u16`: convertir a un `Vec` intermedio sería una asignación de
    /// memoria en el camino crítico del audio, que es justo lo que no se puede
    /// hacer ahí.
    ///
    /// Las muestras vienen intercaladas por canal y se promedian igual que
    /// [`downmix`], que es lo que va a recibir el modelo: medir sobre el
    /// intercalado daría otro número que el del audio real.
    pub fn acumular<I: Iterator<Item = f32>>(&self, muestras: I, canales: u16) {
        if !self.activo.load(Ordering::Relaxed) {
            return;
        }
        let canales = canales.max(1) as usize;
        let mut suma = 0.0f64;
        let mut pico = 0.0f32;
        let mut cuenta = 0u64;
        let mut del_cuadro = 0.0f32;
        let mut vistos = 0usize;

        for muestra in muestras {
            del_cuadro += muestra;
            vistos += 1;
            if vistos < canales {
                continue;
            }
            let mono = del_cuadro / canales as f32;
            suma += f64::from(mono) * f64::from(mono);
            pico = pico.max(mono.abs());
            cuenta += 1;
            del_cuadro = 0.0;
            vistos = 0;
        }

        if cuenta == 0 {
            return;
        }
        sumar(&self.suma_cuadrados, suma);
        self.muestras.fetch_add(cuenta, Ordering::Relaxed);
        subir_el_pico(&self.pico, pico);
    }

    /// El nivel de lo capturado desde la lectura anterior, y **arranca una
    /// ventana nueva**. `None` cuando no entró ni una muestra: es distinto de
    /// "entró silencio", y quien dibuja no tiene por qué mostrar un cero que
    /// nadie midió.
    ///
    /// Los tres átomos no se leen de un saque, así que un chunk que llegue justo
    /// en el medio puede quedar contado en uno y no en el otro. Es intencional:
    /// evitarlo pediría un lock, y a veinticinco lecturas por segundo el sesgo
    /// máximo es una fracción de un chunk de diez milisegundos.
    pub fn tomar(&self) -> Option<Nivel> {
        let muestras = self.muestras.swap(0, Ordering::Relaxed);
        let suma = f64::from_bits(self.suma_cuadrados.swap(0, Ordering::Relaxed));
        let pico = f32::from_bits(self.pico.swap(0, Ordering::Relaxed));
        if muestras == 0 {
            return None;
        }
        let rms = (suma / muestras as f64).sqrt() as f32;
        // Una sola muestra `NaN` del driver envenenaría la suma. Se contesta
        // cero en vez de propagarla: la ventana siguiente arranca limpia.
        Some(Nivel {
            rms: if rms.is_finite() { rms } else { 0.0 },
            pico: if pico.is_finite() { pico } else { 0.0 },
        })
    }
}

impl Default for Medidor {
    fn default() -> Self {
        Self::nuevo()
    }
}

/// Suma `delta` a un `f64` guardado como bits en un átomo.
fn sumar(celda: &AtomicU64, delta: f64) {
    let _ = celda.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
        Some((f64::from_bits(bits) + delta).to_bits())
    });
}

/// Deja en `celda` el mayor entre lo que había y `valor`.
fn subir_el_pico(celda: &AtomicU32, valor: f32) {
    let _ = celda.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
        (valor > f32::from_bits(bits)).then(|| valor.to_bits())
    });
}

/// Grabadora que acumula audio en memoria mientras está activa.
///
/// El dispositivo se abre al empezar a grabar y se cierra al terminar. La
/// versión Python lo dejaba abierto siempre, lo que mantiene encendido el
/// indicador de micrófono de Windows y consume batería en las notebooks.
pub struct Recorder {
    buffer: Arc<Mutex<Vec<f32>>>,
    /// El nivel en vivo, para el indicador de grabación. Va en `Arc` porque lo
    /// escribe el callback de `cpal`, que vive mientras viva el stream.
    medidor: Arc<Medidor>,
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
            medidor: Arc::new(Medidor::nuevo()),
            stream: None,
            sample_rate: crate::config::SAMPLE_RATE,
            channels: 1,
            tope_secs: crate::config::MAX_RECORDING_SECS,
        })
    }

    /// Enciende la medición de nivel. Apagada —que es como arranca— el callback
    /// de audio no hace ni una operación de más: quien no quiere el indicador no
    /// paga nada por él.
    pub fn medir_nivel(&self, activo: bool) {
        self.medidor.activar(activo);
    }

    /// El nivel capturado desde la lectura anterior. Ver [`Medidor::tomar`].
    pub fn tomar_nivel(&self) -> Option<Nivel> {
        self.medidor.tomar()
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
        // La grabación anterior pudo terminar sin que nadie leyera la última
        // ventana: sin este descarte, el primer nivel de esta grabación sería el
        // de la de antes.
        self.medidor.tomar();

        let buf = Arc::clone(&self.buffer);
        let err_cb = |e| eprintln!("error de captura: {e}");
        let stream_cfg = cfg.config();
        // Se capturan por valor y no se leen de `self`: el callback vive en el
        // hilo de audio y no puede tocar la grabadora.
        let medidor = Arc::clone(&self.medidor);
        let canales = self.channels;

        // El formato nativo no siempre es f32: construir un stream f32 sobre un
        // dispositivo que entrega i16 falla con StreamConfigNotSupported, o
        // paniquea al interpretar el buffer.
        //
        // El nivel se acumula ANTES de copiar al buffer para que un buffer
        // envenenado —que ya no se puede recuperar— no se lleve puesto además
        // el indicador, que es lo único que le queda al usuario para darse
        // cuenta de que algo dejó de andar.
        let stream = match cfg.sample_format() {
            SampleFormat::F32 => device.build_input_stream(
                &stream_cfg,
                move |data: &[f32], _: &_| {
                    medidor.acumular(data.iter().copied(), canales);
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
                    medidor.acumular(data.iter().map(|s| *s as f32 / 32768.0), canales);
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
                    medidor.acumular(data.iter().map(|s| (*s as f32 - 32768.0) / 32768.0), canales);
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

    // ---- Medidor de nivel ------------------------------------------------

    /// Un medidor encendido, que es como lo deja el director cuando el
    /// indicador está activado.
    fn medidor_encendido() -> Medidor {
        let m = Medidor::nuevo();
        m.activar(true);
        m
    }

    /// El RMS del medidor tiene que ser **el mismo** que el del audio que se le
    /// manda al modelo. Si divergieran, el indicador diría "te escucho" de algo
    /// que el motor después descarta (o al revés), que es exactamente lo que un
    /// indicador de grabación no puede hacer.
    #[test]
    fn el_rms_del_medidor_es_el_del_audio_que_ve_el_modelo() {
        let señal: Vec<f32> = (0..16_000)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin())
            .collect();
        let esperado =
            (señal.iter().map(|m| f64::from(*m) * f64::from(*m)).sum::<f64>() / señal.len() as f64)
                .sqrt() as f32;

        let medidor = medidor_encendido();
        // En chunks, como los entrega cpal: el acumulado no puede depender del
        // tamaño con el que llega el audio.
        for chunk in señal.chunks(480) {
            medidor.acumular(chunk.iter().copied(), 1);
        }
        let nivel = medidor.tomar().expect("entraron muestras");

        assert!(
            (nivel.rms - esperado).abs() < 1e-4,
            "esperaba {esperado}, medí {}",
            nivel.rms
        );
        assert!(
            (nivel.pico - 0.3).abs() < 0.01,
            "el pico de una senoide de 0,3 es 0,3: {}",
            nivel.pico
        );
    }

    /// Estéreo: el medidor promedia los canales igual que [`downmix`]. Medir
    /// sobre las muestras intercaladas daría otro número que el del audio real.
    #[test]
    fn el_medidor_mezcla_los_canales_igual_que_el_downmix() {
        // Canales opuestos: mezclados dan silencio, intercalados dan 0,5.
        let estereo = vec![0.5, -0.5, 0.5, -0.5, 0.5, -0.5];
        let medidor = medidor_encendido();
        medidor.acumular(estereo.iter().copied(), 2);
        let nivel = medidor.tomar().expect("entraron muestras");

        assert_eq!(downmix(&estereo, 2), vec![0.0, 0.0, 0.0]);
        assert!(nivel.rms < 1e-6, "canales opuestos se cancelan: {}", nivel.rms);
    }

    /// Silencio y voz tienen que separarse por el mismo umbral con el que el
    /// motor decide si transcribe: es lo que hace que lo que el usuario ve
    /// coincida con lo que la app va a hacer.
    #[test]
    fn el_medidor_separa_el_silencio_de_la_voz_por_el_umbral_del_motor() {
        let umbral = crate::config::MIN_SPEECH_RMS;

        let silencio = medidor_encendido();
        silencio.acumular(std::iter::repeat_n(0.0f32, 1_000), 1);
        assert!(silencio.tomar().expect("entraron muestras").rms < umbral);

        let voz = medidor_encendido();
        // 0,071 es el RMS medido sobre el fixture de voz (ver `MIN_SPEECH_RMS`).
        voz.acumular(std::iter::repeat_n(0.071f32, 1_000), 1);
        let nivel = voz.tomar().expect("entraron muestras");
        assert!(nivel.rms >= umbral, "voz real por debajo del umbral: {}", nivel.rms);
        assert!(
            crate::stt::tiene_voz(&vec![0.071f32; 1_000]),
            "el medidor y el motor tienen que coincidir sobre la misma señal"
        );
    }

    /// Leer arranca una ventana nueva: dos lecturas seguidas no pueden contar
    /// dos veces el mismo audio, o el indicador se quedaría clavado en el
    /// último grito después de que el usuario se callara.
    #[test]
    fn cada_lectura_arranca_una_ventana_nueva() {
        let medidor = medidor_encendido();
        medidor.acumular(std::iter::repeat_n(0.5f32, 100), 1);
        assert!(medidor.tomar().expect("hubo audio").rms > 0.4);

        assert_eq!(medidor.tomar(), None, "sin audio nuevo no hay nivel que dar");

        medidor.acumular(std::iter::repeat_n(0.0f32, 100), 1);
        let callado = medidor.tomar().expect("entró silencio, que no es lo mismo que nada");
        assert_eq!(callado.rms, 0.0);
        assert_eq!(callado.pico, 0.0, "el pico también se reinicia");
    }

    /// Apagado no mide nada. Es lo que sostiene la promesa de Ajustes: quien
    /// desactiva el indicador no paga ni una operación por él en el camino
    /// crítico del audio.
    #[test]
    fn apagado_el_medidor_no_acumula_nada() {
        let medidor = Medidor::nuevo();
        assert!(!medidor.esta_activo(), "arranca apagado");
        medidor.acumular(std::iter::repeat_n(0.5f32, 1_000), 1);
        assert_eq!(medidor.tomar(), None, "apagado no puede haber medido nada");

        medidor.activar(true);
        medidor.acumular(std::iter::repeat_n(0.5f32, 1_000), 1);
        assert!(medidor.tomar().is_some(), "encendido vuelve a medir");
    }

    /// Un driver que entrega un `NaN` no puede dejar el indicador en un valor
    /// imposible para siempre: se contesta cero y la ventana siguiente arranca
    /// limpia.
    #[test]
    fn una_muestra_imposible_no_envenena_el_indicador() {
        let medidor = medidor_encendido();
        medidor.acumular([0.1f32, f32::NAN, 0.1].into_iter(), 1);
        let nivel = medidor.tomar().expect("entraron muestras");
        assert_eq!(nivel.rms, 0.0);
        assert!(nivel.pico.is_finite());

        medidor.acumular(std::iter::repeat_n(0.2f32, 100), 1);
        let siguiente = medidor.tomar().expect("entraron muestras");
        assert!((siguiente.rms - 0.2).abs() < 1e-4, "la ventana siguiente quedó sucia");
    }

    /// El medidor lo escribe el hilo de audio y lo lee el del director: si
    /// dejara de ser compartible entre hilos, `Recorder::start` no compilaría
    /// con un error entendible.
    #[test]
    fn el_medidor_se_puede_compartir_entre_hilos() {
        fn exigir<T: Send + Sync + 'static>() {}
        exigir::<Medidor>();
        exigir::<Arc<Medidor>>();
    }

    /// La grabadora arranca sin medir: el indicador es opcional y se enciende
    /// desde Ajustes, no al revés.
    #[test]
    fn la_grabadora_arranca_sin_medir_y_se_puede_encender() {
        let r = Recorder::new().expect("crear la grabadora no toca el hardware");
        assert_eq!(r.tomar_nivel(), None, "sin grabar no hay nivel");
        r.medir_nivel(true);
        assert!(r.medidor.esta_activo());
        r.medir_nivel(false);
        assert!(!r.medidor.esta_activo());
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
