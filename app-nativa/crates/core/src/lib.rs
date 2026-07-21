//! MithFlow: dictado por voz local.
//!
//! Los módulos son independientes entre sí; [`dictate`] es el único lugar donde
//! se componen, y el orden en que lo hace no es arbitrario: ver su
//! documentación.

pub mod audio;
pub mod cleanup;
pub mod config;
pub mod hardware;
pub mod history;
pub mod models;
pub mod paste;
pub mod stt;

use std::path::Path;
use std::time::Instant;

/// Valor del campo `mode` del historial. La limpieza por reglas reemplazó al
/// LLM: el dashboard distingue las dos épocas por este campo, así que las
/// entradas nuevas tienen que decir `"fast"` como las de `mithflow.py`.
const MODO_LIMPIEZA: &str = "fast";

/// Lo único que el pipeline necesita del motor de transcripción.
///
/// Existe para que [`dictate`] no dependa de [`stt::Transcriber`], que sólo
/// compila en Windows y necesita 1,5 GB de modelo en disco. Con el trait, las
/// reglas de descarte se verifican con un doble que paniquea si lo llaman:
/// así el test prueba que el modelo NO se toca, en vez de suponerlo.
pub trait Transcribe {
    /// Texto ya filtrado. Cadena vacía significa "no había nada que transcribir".
    fn transcribe(&mut self, audio: &[f32]) -> Result<String, String>;
}

#[cfg(windows)]
impl Transcribe for stt::Transcriber {
    fn transcribe(&mut self, audio: &[f32]) -> Result<String, String> {
        // Ruta explícita al método inherente: `self.transcribe(...)` acá
        // resolvería a lo mismo, pero por precedencia y no por intención.
        stt::Transcriber::transcribe(self, audio)
    }
}

/// Un dictado que llegó hasta el final: lo que dijo el modelo, lo que se pegó
/// y cuánto costó cada etapa.
///
/// Los tiempos van con toda la precisión medida; el redondeo a 2 decimales es
/// del historial, no de la medición.
#[derive(Debug, Clone)]
pub struct DictationResult {
    /// Lo que devolvió el modelo, antes de la limpieza.
    pub raw: String,
    /// Lo que efectivamente se pegó.
    pub final_text: String,
    pub audio_s: f32,
    pub transcribe_s: f32,
    pub cleanup_s: f32,
}

/// Redondea a 2 decimales, igual que `save_history` en la versión Python.
fn redondear(segundos: f32) -> f32 {
    (segundos * 100.0).round() / 100.0
}

/// El dictado de punta a punta: audio capturado adentro, texto pegado afuera.
///
/// `audio` tiene que venir mono a [`config::SAMPLE_RATE`], que es lo que
/// devuelve [`audio::Recorder::stop`].
///
/// `Ok(None)` es el caso "no había nada que dictar" y NO es un error: audio más
/// corto que [`config::MIN_AUDIO_SECS`], silencio, ruido de fondo o una
/// alucinación filtrada. Quien llame lo informa distinto de un fallo.
///
/// # El orden importa
///
/// 1. **La duración se chequea antes de transcribir**: un roce del atajo no
///    puede costar una inferencia.
/// 2. **Se pega antes de registrar el historial**: el usuario está esperando el
///    texto; el dashboard puede esperar.
/// 3. **Un fallo al guardar no invalida el dictado**: se informa por `stderr` y
///    se sigue, porque el texto ya llegó a destino.
///
/// # Errores
///
/// Devuelve `Err` si falla la transcripción o el pegado. Si lo que falló fue el
/// pegado, el historial YA se escribió a propósito: el texto queda en el
/// portapapeles (ver [`paste::paste`]) y en el historial, que son las dos vías
/// que le quedan al usuario para recuperarlo.
pub fn dictate(
    transcriber: &mut impl Transcribe,
    audio: &[f32],
    history_path: &Path,
) -> Result<Option<DictationResult>, String> {
    let audio_s = audio.len() as f32 / config::SAMPLE_RATE as f32;
    if audio_s < config::MIN_AUDIO_SECS {
        return Ok(None);
    }

    let t0 = Instant::now();
    let raw = transcriber.transcribe(audio)?;
    let transcribe_s = t0.elapsed().as_secs_f32();
    // Silencio, ruido o una alucinación que el filtro descartó.
    if raw.is_empty() {
        return Ok(None);
    }

    let t0 = Instant::now();
    // `fast_cleanup` sólo devuelve vacío si la entrada era vacía (su red de
    // seguridad restituye el original cuando las reglas se comen más de la
    // mitad), así que acá `final_text` nunca queda vacío.
    let final_text = cleanup::fast_cleanup(&raw);
    let cleanup_s = t0.elapsed().as_secs_f32();

    let pegado = paste::paste(&final_text);

    let entrada = history::Entry {
        // Hora local sin zona, el mismo formato que `time.strftime` en Python:
        // el historial es un solo archivo compartido con el dashboard.
        ts: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        audio_s: redondear(audio_s),
        transcribe_s: redondear(transcribe_s),
        cleanup_s: redondear(cleanup_s),
        words: final_text.split_whitespace().count(),
        cleaned: final_text != raw,
        mode: MODO_LIMPIEZA.to_string(),
        raw: raw.clone(),
        final_text: final_text.clone(),
    };
    if let Err(e) = history::append(history_path, &entrada) {
        eprintln!(
            "no pude guardar el historial en {}: {e}",
            history_path.display()
        );
    }

    // Recién ahora: el historial se escribe aunque el pegado haya fallado.
    pegado?;

    Ok(Some(DictationResult {
        raw,
        final_text,
        audio_s,
        transcribe_s,
        cleanup_s,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Doble que hace fallar el test si el pipeline lo llama.
    struct ModeloQueNoDebeLlamarse;

    impl Transcribe for ModeloQueNoDebeLlamarse {
        fn transcribe(&mut self, _audio: &[f32]) -> Result<String, String> {
            panic!("dictate llamó al modelo con audio que tenía que descartar");
        }
    }

    /// Doble que responde "no escuché nada", como el modelo sobre silencio.
    struct ModeloQueNoEscuchoNada;

    impl Transcribe for ModeloQueNoEscuchoNada {
        fn transcribe(&mut self, _audio: &[f32]) -> Result<String, String> {
            Ok(String::new())
        }
    }

    fn ruta_temporal(nombre: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mithflow_dictate_{}_{nombre}.jsonl",
            std::process::id()
        ));
        std::fs::remove_file(&path).ok();
        path
    }

    /// Un roce del atajo no puede costar una inferencia: la duración se chequea
    /// ANTES de transcribir, y por eso este test corre sin modelo.
    #[test]
    fn audio_demasiado_corto_no_toca_el_modelo() {
        let muestras =
            (config::MIN_AUDIO_SECS * config::SAMPLE_RATE as f32) as usize - 1;
        // Con amplitud real: lo que descarta el audio es su duración, no su
        // nivel — si fueran ceros, el test pasaría por el motivo equivocado.
        let audio = vec![0.1f32; muestras];
        let historial = ruta_temporal("corto");

        let salida = dictate(&mut ModeloQueNoDebeLlamarse, &audio, &historial)
            .expect("descartar por duración no es un error");

        assert!(salida.is_none(), "audio corto tiene que dar Ok(None)");
        assert!(
            !historial.exists(),
            "un dictado descartado no debe dejar entrada en el historial"
        );
    }

    /// Un buffer vacío es el caso degenerado del anterior (0 s < 0,5 s).
    #[test]
    fn audio_vacio_no_toca_el_modelo() {
        let historial = ruta_temporal("vacio");
        let salida = dictate(&mut ModeloQueNoDebeLlamarse, &[], &historial)
            .expect("un buffer vacío no es un error");
        assert!(salida.is_none());
        assert!(!historial.exists());
    }

    /// Transcripción vacía (silencio, ruido o alucinación filtrada): corta
    /// antes de pegar, así que este test tampoco toca el portapapeles real.
    #[test]
    fn transcripcion_vacia_no_pega_ni_registra() {
        let audio = vec![0.1f32; config::SAMPLE_RATE as usize * 2];
        let historial = ruta_temporal("silencio");

        let salida = dictate(&mut ModeloQueNoEscuchoNada, &audio, &historial)
            .expect("no escuchar nada no es un error");

        assert!(salida.is_none(), "sin texto no hay dictado");
        assert!(
            !historial.exists(),
            "no se registra un dictado que no existió"
        );
    }

    #[test]
    fn redondea_como_la_version_python() {
        assert_eq!(redondear(0.221_4), 0.22);
        assert_eq!(redondear(9.5), 9.5);
        assert_eq!(redondear(0.000_9), 0.0);
    }
}
