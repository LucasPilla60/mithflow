//! Limpieza del texto transcripto.
//!
//! # Por qué la lista de muletillas es un parámetro y no una constante
//!
//! Las muletillas son personales: quien dice "ponele" en cada frase necesita
//! sacarla, y quien nunca dice "tipo" no gana nada teniéndola en la lista (y
//! pierde si dicta sobre tipos de datos). La app las ofrece en Ajustes, así que
//! el núcleo tiene que poder recibirlas.
//!
//! Los patrones se compilan **una vez por configuración** y no por dictado:
//! [`Limpiador`] guarda los `Regex` ya armados. Recompilar veinte expresiones
//! regulares en cada pegado sería trabajo puro en el camino crítico, que es lo
//! que este módulo existe para evitar.
//!
//! Las muletillas del usuario se escapan con [`regex::escape`] antes de entrar
//! al patrón: son texto, nunca una expresión regular. Sin eso, escribir "(" en
//! Ajustes rompería la limpieza para siempre — es el mismo bug que ya mordió en
//! el buscador de `dashboard.py`.

use crate::config::{FILLERS, STUTTER_WORDS};
use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

static RE_ESPACIO_ANTES_PUNTUACION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+([,.;:!?])").unwrap());
static RE_COMAS_REPETIDAS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r",\s*(?:,\s*)+").unwrap());
static RE_INICIO_SUCIO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\s,]+").unwrap());

/// Qué se le hace al texto antes de pegarlo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModoLimpieza {
    /// Reglas instantáneas: muletillas, tartamudeos, espaciado y mayúscula.
    #[default]
    Rapido,
    /// Se pega exactamente lo que dijo el modelo.
    Ninguno,
}

impl ModoLimpieza {
    /// Valor del campo `mode` del historial. **Es un dato persistido**: el
    /// dashboard segmenta la latencia por acá, así que cambiar estas cadenas
    /// parte el historial en dos épocas artificiales.
    ///
    /// `"fast"` es el mismo que escribe `mithflow.py`, a propósito.
    pub fn etiqueta(self) -> &'static str {
        match self {
            ModoLimpieza::Rapido => "fast",
            ModoLimpieza::Ninguno => "none",
        }
    }

    /// El modo que nombra esta etiqueta. Desconocida cae en el default, que es
    /// la regla de siempre: ante un valor imposible, el comportamiento normal.
    pub fn desde_etiqueta(etiqueta: &str) -> Self {
        match etiqueta {
            "none" => ModoLimpieza::Ninguno,
            _ => ModoLimpieza::Rapido,
        }
    }
}

/// Los patrones de muletillas ya compilados para una lista concreta.
pub struct Limpiador {
    /// Muletillas aisladas por comas: ", este," -> ","
    medio: Vec<Regex>,
    /// Muletillas al inicio o después de punto: "Eh, quería" -> "quería"
    inicio: Vec<Regex>,
}

impl Limpiador {
    /// Compila los patrones de esta lista de muletillas.
    ///
    /// Una muletilla que no se pueda compilar (sólo puede pasar si es
    /// disparatadamente larga, porque el texto va escapado) **se saltea**: es
    /// entrada del usuario y no puede tumbar el dictado. El resto sigue
    /// funcionando.
    pub fn nuevo<S: AsRef<str>>(muletillas: &[S]) -> Self {
        let mut medio = Vec::with_capacity(muletillas.len());
        let mut inicio = Vec::with_capacity(muletillas.len());
        for muletilla in muletillas {
            let literal = regex::escape(muletilla.as_ref().trim());
            if literal.is_empty() {
                continue;
            }
            if let Ok(re) = Regex::new(&format!(r"(?i),\s*{literal}\s*,")) {
                medio.push(re);
            }
            if let Ok(re) = Regex::new(&format!(r"(?i)(^|[.!?]\s){literal}\s*,\s*")) {
                inicio.push(re);
            }
        }
        Self { medio, inicio }
    }

    /// Limpieza por reglas: instantánea, sin LLM. Conservadora por diseño —
    /// ante la duda deja el texto como está, porque el modelo ya puntúa y
    /// capitaliza bien por su cuenta.
    pub fn limpiar(&self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let mut out = text.to_string();

        // 1. Muletillas
        // `replace_all` devuelve `Cow`: sólo se reasigna cuando hubo match, así
        // se evita copiar el string entero en cada una de las ~20 reglas.
        for re in &self.medio {
            if let Cow::Owned(reemplazado) = re.replace_all(&out, ",") {
                out = reemplazado;
            }
        }
        for re in &self.inicio {
            if let Cow::Owned(reemplazado) = re.replace_all(&out, "$1") {
                out = reemplazado;
            }
        }

        // 2. Tartamudeos
        out = colapsar_tartamudeos(&out);

        // 3. Espaciado y puntuación
        // No hace falta normalizar `\s+`: `colapsar_tartamudeos` termina en
        // `join(" ")`, que ya deja espacios simples y sin bordes.
        if let Cow::Owned(reemplazado) = RE_ESPACIO_ANTES_PUNTUACION.replace_all(&out, "$1") {
            out = reemplazado;
        }
        if let Cow::Owned(reemplazado) = RE_COMAS_REPETIDAS.replace_all(&out, ", ") {
            out = reemplazado;
        }
        if let Cow::Owned(reemplazado) = RE_INICIO_SUCIO.replace_all(&out, "") {
            out = reemplazado;
        }
        // `RE_COMAS_REPETIDAS` reemplaza por ", " y puede dejar un espacio final.
        out = out.trim().to_string();

        // 4. Mayúscula inicial, sin tocar el resto (puede haber siglas: CRM, API)
        if let Some(primera) = out.chars().next() {
            if primera.is_lowercase() {
                out = primera.to_uppercase().collect::<String>() + &out[primera.len_utf8()..];
            }
        }

        // 5. Red de seguridad: si se comió más de la mitad, algo salió mal.
        //    `a * 2 < b` es equivalente a `a < b * 0.5` sin pasar por f32.
        if out.chars().count() * 2 < text.chars().count() {
            return text.to_string();
        }
        out
    }
}

impl Default for Limpiador {
    /// Las muletillas de fábrica ([`crate::config::FILLERS`]).
    fn default() -> Self {
        Self::nuevo(FILLERS)
    }
}

/// El limpiador de fábrica, compilado una sola vez por proceso.
static POR_DEFECTO: LazyLock<Limpiador> = LazyLock::new(Limpiador::default);

/// Cómo limpiar: el modo elegido y, si corresponde, los patrones ya compilados.
///
/// Se arma una vez al arrancar y otra cada vez que el usuario guarda Ajustes;
/// el dictado sólo la usa.
#[derive(Default)]
pub struct Limpieza {
    modo: ModoLimpieza,
    /// Patrones propios del usuario. `None` significa "los de fábrica", que
    /// viven en [`POR_DEFECTO`] y se compilan una sola vez por proceso: por eso
    /// no se guarda acá una copia idéntica en cada configuración.
    propios: Option<Limpiador>,
}

impl Limpieza {
    /// Con `muletillas` vacío no se saca ninguna muletilla: es una lista que el
    /// usuario vació, no una lista ausente.
    pub fn nueva<S: AsRef<str>>(modo: ModoLimpieza, muletillas: &[S]) -> Self {
        let propios = match modo {
            // No tiene sentido compilar patrones que nadie va a ejecutar.
            ModoLimpieza::Ninguno => None,
            ModoLimpieza::Rapido => Some(Limpiador::nuevo(muletillas)),
        };
        Self { modo, propios }
    }

    pub fn modo(&self) -> ModoLimpieza {
        self.modo
    }

    /// El texto listo para pegar. Con [`ModoLimpieza::Ninguno`] devuelve lo que
    /// dijo el modelo, sin tocar un carácter.
    pub fn aplicar(&self, texto: &str) -> String {
        match self.modo {
            ModoLimpieza::Ninguno => texto.to_string(),
            ModoLimpieza::Rapido => match &self.propios {
                Some(limpiador) => limpiador.limpiar(texto),
                None => POR_DEFECTO.limpiar(texto),
            },
        }
    }
}

/// Núcleo alfanumérico de un token: lo mismo que encierran los `\b` de la
/// expresión regular de Python. `"el."` -> `"el"`, `"(el"` -> `"el"`, `","` -> `""`.
fn nucleo(token: &str) -> &str {
    token.trim_matches(|c: char| !c.is_alphanumeric())
}

/// Colapsa repeticiones consecutivas de palabras funcionales: "el el informe"
/// se vuelve "el informe".
///
/// No se puede resolver con una expresión regular como en Python: el crate
/// `regex` de Rust NO soporta retrorreferencias (`\1`) por diseño, para
/// garantizar tiempo lineal. Recorrer tokens además es más rápido y claro.
///
/// Se compara el NÚCLEO alfanumérico y no el token entero porque
/// `split_whitespace` deja la puntuación pegada: `"el el."` produce los tokens
/// `"el"` y `"el."`, que nunca son iguales entre sí, y el tartamudeo más
/// frecuente al dictar (justo antes de la pausa que el modelo transcribe como
/// coma o punto) quedaba sin colapsar. Python no tiene el problema porque su
/// `\b` cierra contra la puntuación.
///
/// Paridad con Python, que reemplaza el par por el grupo `\1` (la PRIMERA
/// ocurrencia) y deja fuera del match lo que la rodea:
/// - se conserva el token previo entero — su prefijo y sus mayúsculas
///   (`"(El el)"` -> `"(El)"`);
/// - se le arrastra la puntuación final del token actual (`"el el."` -> `"el."`);
/// - la puntuación INTERIOR del par rompe la repetición, porque el `(\s+\1)+`
///   de Python sólo admite espacios entre las dos ocurrencias: `"el, el"` y
///   `"el ,el"` quedan intactos.
fn colapsar_tartamudeos(text: &str) -> String {
    let mut salida: Vec<Cow<'_, str>> = Vec::new();
    for token in text.split_whitespace() {
        let nucleo_actual = nucleo(token);
        // Un token de pura puntuación (`","` suelto) tiene núcleo vacío: dos
        // seguidos NO son un tartamudeo, así que se exige núcleo no vacío.
        let repetido = !nucleo_actual.is_empty()
            && token.starts_with(nucleo_actual)
            && STUTTER_WORDS
                .iter()
                .any(|w| w.eq_ignore_ascii_case(nucleo_actual))
            && salida.last().is_some_and(|previo| {
                let nucleo_previo = nucleo(previo);
                nucleo_previo.eq_ignore_ascii_case(nucleo_actual)
                    && previo.ends_with(nucleo_previo)
            });

        if !repetido {
            salida.push(Cow::Borrowed(token));
            continue;
        }
        let sufijo = &token[nucleo_actual.len()..];
        if sufijo.is_empty() {
            continue;
        }
        if let Some(previo) = salida.last_mut() {
            previo.to_mut().push_str(sufijo);
        }
    }
    salida.join(" ")
}

/// Limpieza por reglas con las muletillas de fábrica.
///
/// Es el atajo para quien no configura nada (la CLI, los tests); la aplicación
/// usa [`Limpieza`], que además sabe respetar el modo elegido por el usuario.
pub fn fast_cleanup(text: &str) -> String {
    POR_DEFECTO.limpiar(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (entrada, esperado). Si esperado es None, el texto NO debe cambiar.
    const CASES: &[(&str, Option<&str>)] = &[
        // --- Debe limpiar ---
        ("Eh, quería decirte que el dashboard está listo.",
         Some("Quería decirte que el dashboard está listo.")),
        ("Bueno, este, quería mostrarte el CRM.",
         Some("Bueno, quería mostrarte el CRM.")),
        ("El cliente, o sea, pidió el reporte.",
         Some("El cliente, pidió el reporte.")),
        ("Vamos a ver el el dashboard de MithData.",
         Some("Vamos a ver el dashboard de MithData.")),
        ("Hola  ,  qué tal .", Some("Hola, qué tal.")),
        ("el reporte ya está.", Some("El reporte ya está.")),
        // --- NO debe tocar ---
        ("Este dashboard está listo para el cliente.", None),
        ("No quedó nada pendiente.", None),
        ("Bueno el resultado, malo el proceso.", None),
        ("Vamos a ver qué dice la API del CRM.", None),
        ("Ahora me parece que sí está funcionando, aunque en el frontend local el motor no se inicia.", None),
        ("Muy muy bueno el resultado.", None),
    ];

    #[test]
    fn los_doce_casos() {
        let mut fallos = Vec::new();
        for (entrada, esperado) in CASES {
            let objetivo = esperado.unwrap_or(entrada);
            let salida = fast_cleanup(entrada);
            if salida != objetivo {
                fallos.push(format!(
                    "\n  entrada:  {entrada:?}\n  esperado: {objetivo:?}\n  obtenido: {salida:?}"
                ));
            }
        }
        assert!(fallos.is_empty(), "{} caso(s) fallaron:{}", fallos.len(), fallos.join(""));
    }

    #[test]
    fn texto_vacio_no_rompe() {
        assert_eq!(fast_cleanup(""), "");
    }

    #[test]
    fn red_de_seguridad_devuelve_original_si_borra_demasiado() {
        let entrada = "Eh, o sea, digamos, viste,";
        assert_eq!(fast_cleanup(entrada), entrada);
    }

    #[test]
    fn tartamudeos_solo_en_palabras_funcionales() {
        assert_eq!(colapsar_tartamudeos("el el informe"), "el informe");
        assert_eq!(colapsar_tartamudeos("de de la casa"), "de la casa");
        assert_eq!(colapsar_tartamudeos("muy muy bueno"), "muy muy bueno");
        assert_eq!(colapsar_tartamudeos("no no gracias"), "no no gracias");
    }

    /// Los tartamudeos también se colapsan cuando el segundo token trae
    /// puntuación pegada. Es el caso más frecuente al dictar: el tartamudeo
    /// ocurre justo antes de la pausa que el modelo transcribe como coma o
    /// punto. Verificado contra la implementación Python en producción.
    #[test]
    fn tartamudeos_con_puntuacion_adherida() {
        assert_eq!(fast_cleanup("Vamos a ver el el."), "Vamos a ver el.");
        assert_eq!(fast_cleanup("Hoy vi todo esto de de."), "Hoy vi todo esto de.");
        assert_eq!(
            fast_cleanup("Vamos a ver el el, dashboard de MithData."),
            "Vamos a ver el, dashboard de MithData."
        );
    }

    /// Con acentos NO debe colapsar: `STUTTER_WORDS` es ASCII puro y ninguna
    /// palabra acentuada está en la lista.
    #[test]
    fn no_colapsa_palabras_acentuadas_fuera_de_la_lista() {
        assert_eq!(fast_cleanup("Él él dijo que sí."), "Él él dijo que sí.");
    }

    /// Aplicar la limpieza dos veces debe dar el mismo resultado que una.
    /// Si esto se rompe, alguna regla está deshaciendo lo que hizo otra.
    #[test]
    fn la_limpieza_es_idempotente() {
        for (entrada, _) in CASES {
            let una = fast_cleanup(entrada);
            let dos = fast_cleanup(&una);
            assert_eq!(una, dos, "no es idempotente para {entrada:?}");
        }
    }

    // ---- Muletillas configurables (Plan 4) --------------------------------

    /// El ajuste tiene que APLICARSE, no sólo guardarse: una muletilla propia
    /// se saca, y una de fábrica que el usuario sacó de su lista se conserva.
    #[test]
    fn una_lista_propia_de_muletillas_reemplaza_a_la_de_fabrica() {
        let limpieza = Limpieza::nueva(ModoLimpieza::Rapido, &["ponele", "nomás"]);

        assert_eq!(
            limpieza.aplicar("El informe, ponele, ya está listo."),
            "El informe, ya está listo.",
            "no se aplicó la muletilla propia"
        );
        assert_eq!(
            limpieza.aplicar("Ponele, arranquemos por el CRM."),
            "Arranquemos por el CRM.",
            "tampoco al inicio de la frase"
        );
        assert_eq!(
            limpieza.aplicar("El cliente, o sea, pidió el reporte."),
            "El cliente, o sea, pidió el reporte.",
            "«o sea» ya no está en la lista del usuario: no se puede sacar"
        );
    }

    /// Una lista vacía es una decisión ("no me saques nada"), no una ausencia.
    /// El resto de la limpieza —tartamudeos, espaciado, mayúscula— sigue.
    #[test]
    fn una_lista_vacia_de_muletillas_no_saca_ninguna() {
        let limpieza = Limpieza::nueva(ModoLimpieza::Rapido, &[] as &[&str]);
        assert_eq!(
            limpieza.aplicar("Eh, quería ver el el dashboard ."),
            "Eh, quería ver el dashboard."
        );
    }

    /// El texto del usuario es texto, nunca una expresión regular: un paréntesis
    /// en Ajustes no puede romper la limpieza (ni para siempre ni una vez).
    #[test]
    fn una_muletilla_con_metacaracteres_no_rompe_nada() {
        let limpieza = Limpieza::nueva(ModoLimpieza::Rapido, &["(", "a*b", "["]);
        assert_eq!(
            limpieza.aplicar("El reporte, ya está."),
            "El reporte, ya está."
        );
        // Y si el usuario escribe literalmente eso, se saca como literal.
        assert_eq!(limpieza.aplicar("El reporte, a*b, ya está."), "El reporte, ya está.");
    }

    /// Sin configurar nada, `Limpieza` tiene que dar exactamente lo mismo que
    /// la función de siempre. Es la garantía de que parametrizar no cambió el
    /// comportamiento por defecto.
    #[test]
    fn la_limpieza_por_defecto_es_la_de_siempre() {
        let limpieza = Limpieza::default();
        assert_eq!(limpieza.modo(), ModoLimpieza::Rapido);
        for (entrada, _) in CASES {
            assert_eq!(limpieza.aplicar(entrada), fast_cleanup(entrada), "{entrada:?}");
        }
    }

    // ---- Modo de limpieza -------------------------------------------------

    /// El modo `ninguno` deja el texto CRUDO, tal como salió del modelo.
    #[test]
    fn el_modo_ninguno_no_toca_el_texto() {
        let limpieza = Limpieza::nueva(ModoLimpieza::Ninguno, FILLERS);
        for (entrada, _) in CASES {
            assert_eq!(limpieza.aplicar(entrada), *entrada, "el modo ninguno tocó el texto");
        }
        assert_eq!(limpieza.aplicar("  eh, esto ,  no se toca "), "  eh, esto ,  no se toca ");
    }

    /// Las etiquetas van al historial: cambiarlas parte la serie de latencia en
    /// dos épocas que no existieron.
    #[test]
    fn las_etiquetas_de_modo_son_estables_y_van_y_vuelven() {
        assert_eq!(ModoLimpieza::Rapido.etiqueta(), "fast", "es la de mithflow.py");
        assert_eq!(ModoLimpieza::Ninguno.etiqueta(), "none");
        for modo in [ModoLimpieza::Rapido, ModoLimpieza::Ninguno] {
            assert_eq!(ModoLimpieza::desde_etiqueta(modo.etiqueta()), modo);
        }
        assert_eq!(
            ModoLimpieza::desde_etiqueta("llm"),
            ModoLimpieza::Rapido,
            "una etiqueta desconocida cae en el comportamiento normal"
        );
    }
}
