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
pub mod metricas;
pub mod models;
pub mod paste;
pub mod stt;

use std::path::Path;
use std::time::Instant;

pub use cleanup::{Limpieza, ModoLimpieza};

/// Lo que el usuario configuró y el dictado tiene que respetar.
///
/// Existe para que [`dictate_con`] reciba **una** cosa y no cuatro sueltas, y
/// para que agregar una preferencia mañana no cambie la firma otra vez. El
/// vocabulario no está acá porque no lo usa el pipeline sino el motor
/// ([`stt::Transcriber::fijar_vocabulario`]): es una opción de decodificación,
/// no un paso de la limpieza.
pub struct Preferencias {
    pub limpieza: Limpieza,
    /// Con `false`, el historial guarda las métricas del dictado pero **no el
    /// texto**. Es la opción de privacidad: el historial es lo único de esta
    /// app que guarda en claro todo lo que el usuario dijo, y esta app se
    /// instala también en máquinas compartidas.
    pub guardar_texto: bool,
}

impl Default for Preferencias {
    /// Lo de siempre: limpieza rápida de fábrica y el texto guardado.
    ///
    /// **No se deriva a propósito.** `bool::default()` es `false`, así que un
    /// `#[derive(Default)]` dejaría el historial sin texto para todo el que no
    /// configure nada: el default silencioso opuesto al que la app promete.
    fn default() -> Self {
        Self {
            limpieza: Limpieza::default(),
            guardar_texto: true,
        }
    }
}

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
    /// La entrada tal como quedó en el historial, o `None` si no se pudo
    /// guardar.
    ///
    /// Se devuelve en vez de dejar que quien llame relea el archivo: releer un
    /// `.jsonl` de miles de líneas después de cada dictado es trabajo O(n) por
    /// dictado, y además con el texto sin guardar (`guardar_texto = false`) no
    /// habría forma de reconocer cuál de las entradas es ésta.
    pub entrada: Option<history::Entry>,
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
    dictate_con(transcriber, audio, history_path, &Preferencias::default())
}

/// El dictado respetando lo que el usuario configuró.
///
/// Es la misma función que [`dictate`], que no es más que ésta con las
/// preferencias por defecto. Ver allá para el orden de las etapas y los
/// errores.
pub fn dictate_con(
    transcriber: &mut impl Transcribe,
    audio: &[f32],
    history_path: &Path,
    preferencias: &Preferencias,
) -> Result<Option<DictationResult>, String> {
    dictar_interno(transcriber, audio, history_path, preferencias, paste::paste)
}

/// El pipeline con el pegado inyectado.
///
/// Existe por la misma razón que el trait [`Transcribe`]: para poder probar las
/// reglas sin el efecto. `paste::paste` escribe el portapapeles del usuario y
/// **manda un Ctrl+V real a la ventana que tenga el foco**, así que un test que
/// llegara hasta ahí le pegaría texto en medio de lo que esté haciendo. Con
/// esta costura los tests ejercitan el pipeline entero con un pegado que no
/// hace nada, y el código de producción sigue teniendo un solo camino.
fn dictar_interno<P>(
    transcriber: &mut impl Transcribe,
    audio: &[f32],
    history_path: &Path,
    preferencias: &Preferencias,
    pegar: P,
) -> Result<Option<DictationResult>, String>
where
    P: FnOnce(&str) -> Result<(), String>,
{
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
    // La limpieza sólo devuelve vacío si la entrada era vacía (su red de
    // seguridad restituye el original cuando las reglas se comen más de la
    // mitad), y con el modo `ninguno` devuelve el crudo: acá `final_text` nunca
    // queda vacío.
    let final_text = preferencias.limpieza.aplicar(&raw);
    let cleanup_s = t0.elapsed().as_secs_f32();

    let pegado = pegar(&final_text);

    let mut entrada = history::Entry {
        // Hora local sin zona, el mismo formato que `time.strftime` en Python:
        // el historial es un solo archivo compartido con el dashboard.
        ts: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        audio_s: redondear(audio_s),
        transcribe_s: redondear(transcribe_s),
        cleanup_s: redondear(cleanup_s),
        words: final_text.split_whitespace().count(),
        cleaned: final_text != raw,
        mode: preferencias.limpieza.modo().etiqueta().to_string(),
        raw: raw.clone(),
        final_text: final_text.clone(),
    };
    // Las métricas quedan; el texto no. Se vacía ACÁ y no antes para que
    // `words` y `cleaned` sigan siendo los del dictado real: el dashboard tiene
    // que poder seguir contando palabras aunque no guarde lo que se dijo.
    if !preferencias.guardar_texto {
        entrada.raw = String::new();
        entrada.final_text = String::new();
    }

    let guardada = match history::append(history_path, &entrada) {
        Ok(()) => Some(entrada),
        Err(e) => {
            eprintln!(
                "no pude guardar el historial en {}: {e}",
                history_path.display()
            );
            None
        }
    };

    // Recién ahora: el historial se escribe aunque el pegado haya fallado.
    pegado?;

    Ok(Some(DictationResult {
        raw,
        final_text,
        audio_s,
        transcribe_s,
        cleanup_s,
        entrada: guardada,
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

    // ---- Preferencias del usuario (Plan 4) -------------------------------
    //
    // Ejercitan el pipeline COMPLETO —transcripción, limpieza, historial— con
    // el pegado inyectado: `paste::paste` manda un Ctrl+V real a la ventana con
    // foco, así que un `cargo test` que lo llamara le escribiría al usuario en
    // medio de lo que esté haciendo.

    /// Un pegado que no pega. Devuelve lo que le mandaron para poder afirmar
    /// QUÉ se habría pegado, que es la mitad de lo que estos tests verifican.
    fn pegado_falso(recibido: &std::cell::RefCell<String>) -> impl FnOnce(&str) -> Result<(), String> + '_ {
        move |texto| {
            recibido.borrow_mut().push_str(texto);
            Ok(())
        }
    }

    /// Doble que devuelve un texto crudo con muletilla y tartamudeo: sirve para
    /// ver si la limpieza corrió o no.
    struct ModeloQueDicta(&'static str);

    impl Transcribe for ModeloQueDicta {
        fn transcribe(&mut self, _audio: &[f32]) -> Result<String, String> {
            Ok(self.0.to_string())
        }
    }

    const CRUDO: &str = "Eh, quería ver el el dashboard.";
    const LIMPIO: &str = "Quería ver el dashboard.";

    fn audio_valido() -> Vec<f32> {
        vec![0.1f32; config::SAMPLE_RATE as usize * 2]
    }

    /// El modo `ninguno` pega y guarda el texto CRUDO. Es el ajuste que hasta
    /// el Plan 4 se guardaba y no hacía nada.
    #[test]
    fn el_modo_ninguno_deja_el_texto_crudo() {
        let historial = ruta_temporal("modo_ninguno");
        let preferencias = Preferencias {
            limpieza: Limpieza::nueva(ModoLimpieza::Ninguno, config::FILLERS),
            ..Default::default()
        };
        let pegado = std::cell::RefCell::new(String::new());

        let resultado = dictar_interno(
            &mut ModeloQueDicta(CRUDO),
            &audio_valido(),
            &historial,
            &preferencias,
            pegado_falso(&pegado),
        )
        .expect("el dictado no tenía que fallar")
        .expect("con texto hay dictado");

        assert_eq!(pegado.into_inner(), CRUDO, "se pegó texto limpiado");
        assert_eq!(resultado.final_text, CRUDO, "el modo ninguno limpió igual");
        let entrada = resultado.entrada.expect("tenía que guardarse");
        assert_eq!(entrada.final_text, CRUDO);
        assert!(!entrada.cleaned, "no hubo limpieza que declarar");
        assert_eq!(entrada.mode, "none", "el historial tiene que decir qué modo fue");

        std::fs::remove_file(&historial).ok();
    }

    /// Y el modo rápido —el default— sigue limpiando como siempre.
    #[test]
    fn el_modo_rapido_limpia_y_lo_registra() {
        let historial = ruta_temporal("modo_rapido");
        let pegado = std::cell::RefCell::new(String::new());

        let resultado = dictar_interno(
            &mut ModeloQueDicta(CRUDO),
            &audio_valido(),
            &historial,
            &Preferencias::default(),
            pegado_falso(&pegado),
        )
        .expect("el dictado no tenía que fallar")
        .expect("con texto hay dictado");

        assert_eq!(pegado.into_inner(), LIMPIO);
        assert_eq!(resultado.final_text, LIMPIO);
        let entrada = resultado.entrada.expect("tenía que guardarse");
        assert!(entrada.cleaned);
        assert_eq!(entrada.mode, "fast");
        assert_eq!(entrada.words, 4);

        std::fs::remove_file(&historial).ok();
    }

    /// Las muletillas del usuario se aplican de punta a punta, no sólo en el
    /// módulo de limpieza.
    #[test]
    fn las_muletillas_propias_llegan_hasta_el_historial() {
        let historial = ruta_temporal("muletillas_propias");
        let preferencias = Preferencias {
            limpieza: Limpieza::nueva(ModoLimpieza::Rapido, &["viste"]),
            ..Default::default()
        };
        let pegado = std::cell::RefCell::new(String::new());

        let resultado = dictar_interno(
            &mut ModeloQueDicta("El informe, viste, ya está."),
            &audio_valido(),
            &historial,
            &preferencias,
            pegado_falso(&pegado),
        )
        .expect("el dictado no tenía que fallar")
        .expect("con texto hay dictado");

        assert_eq!(resultado.final_text, "El informe, ya está.");
        assert_eq!(
            resultado.entrada.expect("tenía que guardarse").final_text,
            "El informe, ya está."
        );
        std::fs::remove_file(&historial).ok();
    }

    /// Con `guardar_texto = false` el historial conserva las métricas y pierde
    /// el texto. Lo que se pega NO cambia: la privacidad es sobre lo que queda
    /// escrito en el disco, no sobre el dictado.
    #[test]
    fn sin_guardar_texto_quedan_las_metricas_y_no_las_palabras() {
        let historial = ruta_temporal("sin_texto");
        let preferencias = Preferencias {
            guardar_texto: false,
            ..Default::default()
        };
        let pegado = std::cell::RefCell::new(String::new());

        let resultado = dictar_interno(
            &mut ModeloQueDicta(CRUDO),
            &audio_valido(),
            &historial,
            &preferencias,
            pegado_falso(&pegado),
        )
        .expect("el dictado no tenía que fallar")
        .expect("con texto hay dictado");

        assert_eq!(pegado.into_inner(), LIMPIO, "el pegado no se toca");
        assert_eq!(resultado.final_text, LIMPIO);

        let guardadas = history::load(&historial).expect("el historial tiene que leerse");
        assert_eq!(guardadas.len(), 1);
        assert_eq!(guardadas[0].final_text, "", "quedó el texto dictado en disco");
        assert_eq!(guardadas[0].raw, "", "quedó el crudo en disco");
        assert_eq!(guardadas[0].words, 4, "se perdieron las métricas");
        assert!(guardadas[0].audio_s > 0.0);

        std::fs::remove_file(&historial).ok();
    }

    /// El default no puede ser el que borra el texto: sería la peor sorpresa
    /// posible para quien nunca abrió Ajustes.
    #[test]
    fn las_preferencias_por_defecto_guardan_el_texto() {
        let preferencias = Preferencias::default();
        assert!(preferencias.guardar_texto);
        assert_eq!(preferencias.limpieza.modo(), ModoLimpieza::Rapido);
    }
}
