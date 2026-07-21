/// Vocabulario propio: el modelo lo usa como contexto para no inventar palabras.
pub const INITIAL_PROMPT: &str = concat!(
    "Transcripción de dictado en español rioplatense sobre negocios y tecnología. ",
    "Términos frecuentes: MithData, PyME, dashboard, frontend, backend, UX, UI, ",
    "lead, CRM, IA, ciencia de datos, machine learning, API, Canva."
);

/// Idioma del dictado. Fijo a propósito: la autodetección confunde el español
/// rioplatense con portugués en clips cortos y cuesta tiempo en cada dictado.
pub const LANGUAGE: &str = "es";

/// El vocabulario del usuario **se suma** al de fábrica, no lo reemplaza.
///
/// Es la diferencia entre un ajuste que ayuda y uno que rompe: quien escriba
/// "Jaé, MithCore" en Ajustes quiere que el modelo aprenda esos dos términos,
/// no perder "MithData", "CRM" y el resto de la lista que ya venía funcionando.
/// Con la caja vacía —el caso de todo usuario que nunca la toque— devuelve
/// exactamente [`INITIAL_PROMPT`], así que el comportamiento por defecto no
/// cambia en un solo carácter.
pub fn prompt_con_vocabulario(propio: &str) -> String {
    let propio = propio.trim();
    if propio.is_empty() {
        return INITIAL_PROMPT.to_string();
    }
    format!("{INITIAL_PROMPT} También aparecen: {propio}")
}

/// Muletillas que se eliminan cuando quedaron aisladas por comas.
/// Deliberadamente NO incluidas: "bueno", "nada", "a ver" — son muletillas
/// frecuentes pero también arranques legítimos, y sacarlas cambia el tono.
pub const FILLERS: &[&str] = &[
    "eh", "ehh", "em", "mmm", "este", "esto", "o sea", "osea", "digamos", "viste", "tipo",
];

/// Palabras que se tartamudean al dictar ("el el informe"). Lista explícita
/// para no tocar repeticiones intencionales ("muy muy bueno", "no no").
pub const STUTTER_WORDS: &[&str] = &[
    "el", "la", "los", "las", "un", "una", "unos", "unas", "de", "del", "que", "y", "a", "en",
    "con", "por", "para", "se", "lo", "le", "les", "es", "al", "su", "mi", "te", "me",
];

/// Frecuencia de muestreo que espera el modelo.
pub const SAMPLE_RATE: u32 = 16_000;

/// Audio más corto que esto se descarta.
pub const MIN_AUDIO_SECS: f32 = 0.5;

/// Nivel RMS por debajo del cual se considera que no hubo voz y ni siquiera se
/// llama al modelo.
///
/// Medido sobre los fixtures (10 s cada uno): silencio 0.000000, ruido de fondo
/// 0.002891, voz 0.071088 y 0.100555. La separación es de 25x, y este umbral
/// queda algo por debajo del punto medio geométrico (0.0143) a propósito: entre
/// transcribir ruido y perder un dictado flojito, el error caro es el segundo.
pub const MIN_SPEECH_RMS: f32 = 0.01;

/// El tono de inicio suena por los parlantes y el micrófono lo capta, sobre
/// todo en notebooks. Se descarta ese tramo del buffer.
pub const TONE_GUARD_SECS: f32 = 0.2;

/// Tope de duración de una grabación. El buffer a tasa nativa consume
/// ~384 KB/s (48 kHz, 2 canales, f32).
pub const MAX_RECORDING_SECS: f32 = 180.0;

/// Frases que el modelo inventa sobre silencio o ruido: residuos de los
/// subtítulos con los que se entrenó. Acá el texto se pega directo en el
/// documento del usuario, así que hay que filtrarlas.
pub const HALLUCINATION_PHRASES: &[&str] = &[
    "gracias por ver el video",
    "gracias por ver el vídeo",
    "suscribete al canal",
    "suscríbete al canal",
    "subtitulos realizados por la comunidad de amara org",
    "subtítulos realizados por la comunidad de amara org",
    "mas informacion en www",
    "más información en www",
    "gracias",
    "muchas gracias",
    "adios",
    "adiós",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Sin vocabulario propio el prompt tiene que ser IDÉNTICO al de siempre:
    /// es lo que garantiza que parametrizar esto no le cambie la transcripción
    /// a quien nunca abra Ajustes.
    #[test]
    fn sin_vocabulario_propio_el_prompt_no_cambia() {
        assert_eq!(prompt_con_vocabulario(""), INITIAL_PROMPT);
        assert_eq!(prompt_con_vocabulario("   \n  "), INITIAL_PROMPT);
    }

    /// Y con vocabulario propio se SUMA: los términos de fábrica siguen ahí.
    #[test]
    fn el_vocabulario_propio_se_suma_al_de_fabrica() {
        let prompt = prompt_con_vocabulario("MithCore, Jaé, pgTAP");
        assert!(prompt.starts_with(INITIAL_PROMPT), "se perdió el prompt base");
        assert!(prompt.contains("MithData"), "se perdió el vocabulario de fábrica");
        assert!(prompt.contains("MithCore"));
        assert!(prompt.contains("Jaé"));
    }
}
