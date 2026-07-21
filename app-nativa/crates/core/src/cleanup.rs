use crate::config::{FILLERS, STUTTER_WORDS};
use once_cell::sync::Lazy;
use regex::Regex;

static RE_ESPACIOS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static RE_ESPACIO_ANTES_PUNTUACION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s+([,.;:!?])").unwrap());
static RE_COMAS_REPETIDAS: Lazy<Regex> = Lazy::new(|| Regex::new(r",\s*(?:,\s*)+").unwrap());
static RE_INICIO_SUCIO: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[\s,]+").unwrap());

/// Muletillas aisladas por comas: ", este," -> ","
static RE_FILLERS_MEDIO: Lazy<Vec<Regex>> = Lazy::new(|| {
    FILLERS
        .iter()
        .map(|f| Regex::new(&format!(r"(?i),\s*{}\s*,", regex::escape(f))).unwrap())
        .collect()
});

/// Muletillas al inicio o después de punto: "Eh, quería" -> "quería"
static RE_FILLERS_INICIO: Lazy<Vec<Regex>> = Lazy::new(|| {
    FILLERS
        .iter()
        .map(|f| Regex::new(&format!(r"(?i)(^|[.!?]\s){}\s*,\s*", regex::escape(f))).unwrap())
        .collect()
});

/// Colapsa repeticiones consecutivas de palabras funcionales: "el el informe"
/// se vuelve "el informe".
///
/// No se puede resolver con una expresión regular como en Python: el crate
/// `regex` de Rust NO soporta retrorreferencias (`\1`) por diseño, para
/// garantizar tiempo lineal. Recorrer tokens además es más rápido y claro.
fn colapsar_tartamudeos(text: &str) -> String {
    let mut salida: Vec<&str> = Vec::new();
    for token in text.split_whitespace() {
        let repetido = salida
            .last()
            .map(|previo: &&str| {
                previo.eq_ignore_ascii_case(token)
                    && STUTTER_WORDS.iter().any(|w| w.eq_ignore_ascii_case(token))
            })
            .unwrap_or(false);
        if !repetido {
            salida.push(token);
        }
    }
    salida.join(" ")
}

/// Limpieza por reglas: instantánea, sin LLM. Conservadora por diseño — ante
/// la duda deja el texto como está, porque el modelo ya puntúa y capitaliza
/// bien por su cuenta.
pub fn fast_cleanup(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = text.to_string();

    // 1. Muletillas
    for re in RE_FILLERS_MEDIO.iter() {
        out = re.replace_all(&out, ",").into_owned();
    }
    for re in RE_FILLERS_INICIO.iter() {
        out = re.replace_all(&out, "$1").into_owned();
    }

    // 2. Tartamudeos
    out = colapsar_tartamudeos(&out);

    // 3. Espaciado y puntuación
    out = RE_ESPACIOS.replace_all(&out, " ").into_owned();
    out = RE_ESPACIO_ANTES_PUNTUACION.replace_all(&out, "$1").into_owned();
    out = RE_COMAS_REPETIDAS.replace_all(&out, ", ").into_owned();
    out = RE_INICIO_SUCIO.replace_all(&out, "").into_owned();
    out = out.trim().to_string();

    // 4. Mayúscula inicial, sin tocar el resto (puede haber siglas: CRM, API)
    if let Some(primera) = out.chars().next() {
        if primera.is_lowercase() {
            out = primera.to_uppercase().collect::<String>() + &out[primera.len_utf8()..];
        }
    }

    // 5. Red de seguridad: si se comió más de la mitad, algo salió mal.
    if (out.chars().count() as f32) < (text.chars().count() as f32) * 0.5 {
        return text.to_string();
    }
    out
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
}
