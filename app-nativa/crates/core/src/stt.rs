//! Transcripción: convierte el audio en texto y descarta lo que el modelo
//! inventa cuando no hay voz.
//!
//! # Por qué hay dos filtros y no uno
//!
//! El texto de este módulo se pega directo en el documento del usuario, así que
//! una alucinación no es un error cosmético. Se midió qué tan lejos llega cada
//! defensa (barrido sobre 10 s de silencio y 10 s de ruido de fondo):
//!
//! 1. **Los umbrales de decodificación de whisper no alcanzan.** La compuerta
//!    interna exige `no_speech_prob > no_speech_thold` **y**
//!    `avg_logprob < logprob_thold`. Se probaron seis combinaciones
//!    (`no_speech_thold` de 0.1 a 0.8, `logprob_thold` de -1.0 a -0.3) y en
//!    TODAS el modelo devolvió texto inventado sobre silencio y sobre ruido
//!    ("Perm Friendship ¿Qué es lo que te ocurre?", "911", "Moderna,
//!    Civilization..."). Subir `no_speech_thold` filtra MENOS, no más: hace la
//!    condición más difícil de cumplir.
//! 2. **La lista de bloqueo tampoco alcanza sola.** Sobre silencio el modelo
//!    repite frases de subtítulos que sí se pueden enumerar, pero sobre ruido
//!    inventa cadenas distintas en cada corrida: no hay lista que las cubra.
//!
//! De ahí las dos capas: una compuerta de energía ANTES del modelo
//! ([`tiene_voz`]), que es determinista y no puede inventar nada, y la lista de
//! bloqueo DESPUÉS ([`filtrar_alucinacion`]) para lo que igual se escape.

use crate::config::HALLUCINATION_PHRASES;
use std::sync::LazyLock;

/// Umbral de la compuerta interna de whisper. Hoy coincide con el default de la
/// librería; se fija explícito para que un cambio río arriba no mueva el
/// comportamiento sin que nos enteremos.
const NO_SPEECH_THOLD: f32 = 0.6;

/// Nivel RMS del audio. `0.0` para un buffer vacío.
///
/// Acumula en `f64`: en una grabación al tope (180 s son 2.88 M de muestras) la
/// suma de cuadrados se acerca a la mantisa de 24 bits de `f32` y el resultado
/// empezaría a perder precisión.
fn rms(audio: &[f32]) -> f32 {
    if audio.is_empty() {
        return 0.0;
    }
    let suma: f64 = audio.iter().map(|m| (*m as f64) * (*m as f64)).sum();
    (suma / audio.len() as f64).sqrt() as f32
}

/// ¿Vale la pena preguntarle al modelo?
///
/// Es la única defensa que NO puede producir texto falso, porque decide sin
/// mirar el modelo. Ver la nota del módulo para las mediciones que fijan el
/// umbral ([`crate::config::MIN_SPEECH_RMS`]).
pub fn tiene_voz(audio: &[f32]) -> bool {
    rms(audio) >= crate::config::MIN_SPEECH_RMS
}

/// Reduce el texto a su núcleo comparable: minúsculas, sin signos, con los
/// espacios colapsados. `"¡Muchas gracias!"` y `"muchas gracias"` convergen.
///
/// Cualquier carácter no alfanumérico actúa de separador (y no de borrado):
/// así `"www.mithdata.com"` da tres palabras y no un pegote. Los acentos se
/// conservan — `is_alphanumeric` es Unicode — porque `HALLUCINATION_PHRASES`
/// ya trae las dos variantes de cada frase ("adios" y "adiós").
fn normalizar(texto: &str) -> String {
    let mut salida = String::with_capacity(texto.len());
    let mut separador_pendiente = false;
    for c in texto.chars() {
        if c.is_alphanumeric() {
            if separador_pendiente && !salida.is_empty() {
                salida.push(' ');
            }
            separador_pendiente = false;
            salida.extend(c.to_lowercase());
        } else {
            separador_pendiente = true;
        }
    }
    salida
}

/// Las frases de bloqueo ya normalizadas. Se calculan una sola vez: comparar
/// contra ellas pasa en cada dictado.
static ALUCINACIONES: LazyLock<Vec<String>> = LazyLock::new(|| {
    HALLUCINATION_PHRASES
        .iter()
        .map(|f| normalizar(f))
        .collect()
});

/// Devuelve cadena vacía si el texto ES una alucinación conocida; si no, el
/// texto tal cual (sin espacios sobrantes en los bordes).
///
/// La comparación es por IGUALDAD DEL TEXTO COMPLETO, nunca por contención.
/// La lista incluye "gracias" y "muchas gracias", que son también dictados
/// legítimos frecuentes: buscarlas como subcadena mutilaría
/// "Gracias por el reporte, lo reviso mañana." Sólo se descarta cuando la
/// frase inventada es TODO lo que el modelo devolvió, que es exactamente el
/// caso del silencio y el ruido de fondo.
pub fn filtrar_alucinacion(texto: &str) -> String {
    let normalizado = normalizar(texto);
    if normalizado.is_empty() || ALUCINACIONES.contains(&normalizado) {
        return String::new();
    }
    texto.trim().to_string()
}

#[cfg(windows)]
pub use motor::Transcriber;

#[cfg(windows)]
mod motor {
    use super::{filtrar_alucinacion, tiene_voz};
    use crate::config::{INITIAL_PROMPT, LANGUAGE};
    use std::path::Path;
    use std::sync::OnceLock;
    use transcribe_cpp::{Model, RunExtension, RunOptions, Session, WhisperRunOptions};

    /// Resultado de inicializar los backends, calculado una única vez por
    /// proceso. Es un `OnceLock` y no un `Once` pelado para poder devolverle
    /// el mensaje de error a cada llamador: si faltan las DLLs de ggml al lado
    /// del ejecutable, ése es el único diagnóstico que va a existir.
    static BACKENDS: OnceLock<Result<(), String>> = OnceLock::new();

    /// Con `dynamic-backends` los backends de ggml son DLLs sueltas que hay
    /// que cargar ANTES del primer `Model::load`, o falla con
    /// "backend error (status 8)".
    fn init_backends_una_vez() -> Result<(), String> {
        BACKENDS
            .get_or_init(|| {
                transcribe_cpp::init_backends_default()
                    .map_err(|e| format!("no pude inicializar los backends de ggml: {e}"))
            })
            .clone()
    }

    /// Opciones de decodificación del dictado.
    ///
    /// Las cuatro que se fijan acá no son preferencias, sostienen requisitos:
    /// - `language`: sin esto autodetecta, confunde el rioplatense con
    ///   portugués en clips cortos y paga la detección en cada dictado.
    /// - `initial_prompt`: el vocabulario propio. Sin esto el modelo escribe
    ///   "Middata" en vez de "MithData" (medido en el spike).
    /// - `no_speech_thold`: compuerta interna de whisper. Ayuda pero no basta
    ///   sola; ver la nota del módulo.
    /// - `condition_on_prev_tokens`: apagado, evita la degradación en audio
    ///   largo (en Python, 120 s bajaron de 4.92 s a 2.25 s).
    ///
    /// El resto queda en el default del crate, que es lo que se midió en el
    /// spike (0.221 s sobre 9.5 s de audio con Vulkan).
    fn opciones_de_dictado() -> RunOptions {
        RunOptions {
            language: Some(LANGUAGE.to_string()),
            family: Some(RunExtension::Whisper(WhisperRunOptions {
                initial_prompt: Some(INITIAL_PROMPT.to_string()),
                no_speech_thold: Some(super::NO_SPEECH_THOLD),
                condition_on_prev_tokens: Some(false),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    /// El modelo cargado y su sesión de decodificación.
    ///
    /// Cargar el modelo cuesta ~1.8 s, así que se hace una vez y el
    /// `Transcriber` vive lo que dura la aplicación. La sesión no se comparte
    /// entre hilos (`run` toma `&mut self`), pero sí se puede mover a uno.
    pub struct Transcriber {
        session: Session,
        opciones: RunOptions,
    }

    impl Transcriber {
        pub fn new(model_path: &Path) -> Result<Self, String> {
            init_backends_una_vez()?;
            if !model_path.is_file() {
                return Err(format!(
                    "no encuentro el modelo en {}",
                    model_path.display()
                ));
            }
            // La `Session` retiene el modelo por dentro (Arc), así que dejar
            // caer `modelo` acá no lo libera.
            let modelo = Model::load(model_path)
                .map_err(|e| format!("no pude cargar el modelo {}: {e}", model_path.display()))?;
            let session = modelo
                .session()
                .map_err(|e| format!("no pude crear la sesión: {e}"))?;
            Ok(Self {
                session,
                opciones: opciones_de_dictado(),
            })
        }

        /// Transcribe audio mono de 16 kHz en f32 y devuelve el texto ya
        /// filtrado. Cadena vacía significa "no había nada que transcribir".
        pub fn transcribe(&mut self, audio: &[f32]) -> Result<String, String> {
            // Antes del modelo: sobre silencio o ruido de fondo whisper
            // inventa, y lo inventado se pega en el documento del usuario.
            if !tiene_voz(audio) {
                return Ok(String::new());
            }
            let transcripcion = self
                .session
                .run(audio, &self.opciones)
                .map_err(|e| format!("falló la transcripción: {e}"))?;
            Ok(filtrar_alucinacion(&transcripcion.text))
        }
    }

    /// El pipeline transcribe en un hilo aparte para no congelar la interfaz:
    /// si `Transcriber` dejara de ser `Send`, eso no compilaría.
    #[cfg(test)]
    const _: fn() = || {
        fn assert_send<T: Send>() {}
        let _ = assert_send::<Transcriber>;
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtra_alucinaciones_conocidas() {
        assert_eq!(filtrar_alucinacion("Gracias por ver el video."), "");
        assert_eq!(filtrar_alucinacion("¡Muchas gracias!"), "");
        assert_eq!(filtrar_alucinacion("   "), "");
    }

    #[test]
    fn no_filtra_texto_legitimo_que_contiene_esas_palabras() {
        let real = "Gracias por el reporte, lo reviso mañana.";
        assert_eq!(filtrar_alucinacion(real), real);
        let otro = "Muchas gracias por mandarme el dashboard.";
        assert_eq!(filtrar_alucinacion(otro), otro);
    }

    /// Los tres niveles medidos sobre los fixtures. Es el test que sostiene el
    /// criterio de aceptación 9 sin necesitar el modelo de 1.5 GB.
    #[test]
    fn la_compuerta_de_energia_separa_voz_de_silencio_y_ruido() {
        assert!(!tiene_voz(&[]), "un buffer vacío no tiene voz");
        assert!(!tiene_voz(&vec![0.0; 16_000]), "el silencio no tiene voz");

        // Ruido de fondo: el mismo LCG y la misma amplitud que el test de
        // integración, RMS medido 0.002891.
        let mut estado: u32 = 0x1234_5678;
        let ruido: Vec<f32> = (0..16_000)
            .map(|_| {
                estado = estado.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((estado >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 0.005
            })
            .collect();
        assert!(!tiene_voz(&ruido), "el ruido de fondo no es voz");

        // Una senoide al nivel de voz medido en los fixtures (RMS ~0.07).
        let voz: Vec<f32> = (0..16_000)
            .map(|i| (2.0 * std::f32::consts::PI * 200.0 * i as f32 / 16_000.0).sin() * 0.1)
            .collect();
        assert!(tiene_voz(&voz), "una señal al nivel de la voz sí es voz");
    }
}
