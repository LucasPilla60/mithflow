/// Vocabulario propio: el modelo lo usa como contexto para no inventar palabras.
pub const INITIAL_PROMPT: &str = concat!(
    "Transcripción de dictado en español rioplatense sobre negocios y tecnología. ",
    "Términos frecuentes: MithData, PyME, dashboard, frontend, backend, UX, UI, ",
    "lead, CRM, IA, ciencia de datos, machine learning, API, Canva."
);

/// Idioma del dictado. Fijo a propósito: la autodetección confunde el español
/// rioplatense con portugués en clips cortos y cuesta tiempo en cada dictado.
pub const LANGUAGE: &str = "es";

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
