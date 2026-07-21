use crate::config::{FILLERS, STUTTER_WORDS};
use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

static RE_ESPACIO_ANTES_PUNTUACION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+([,.;:!?])").unwrap());
static RE_COMAS_REPETIDAS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r",\s*(?:,\s*)+").unwrap());
static RE_INICIO_SUCIO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\s,]+").unwrap());

/// Muletillas aisladas por comas: ", este," -> ","
static RE_FILLERS_MEDIO: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    FILLERS
        .iter()
        .map(|f| Regex::new(&format!(r"(?i),\s*{}\s*,", regex::escape(f))).unwrap())
        .collect()
});

/// Muletillas al inicio o después de punto: "Eh, quería" -> "quería"
static RE_FILLERS_INICIO: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    FILLERS
        .iter()
        .map(|f| Regex::new(&format!(r"(?i)(^|[.!?]\s){}\s*,\s*", regex::escape(f))).unwrap())
        .collect()
});

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

/// Limpieza por reglas: instantánea, sin LLM. Conservadora por diseño — ante
/// la duda deja el texto como está, porque el modelo ya puntúa y capitaliza
/// bien por su cuenta.
pub fn fast_cleanup(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = text.to_string();

    // 1. Muletillas
    // `replace_all` devuelve `Cow`: sólo se reasigna cuando hubo match, así se
    // evita copiar el string entero en cada una de las ~20 reglas.
    for re in RE_FILLERS_MEDIO.iter() {
        if let Cow::Owned(reemplazado) = re.replace_all(&out, ",") {
            out = reemplazado;
        }
    }
    for re in RE_FILLERS_INICIO.iter() {
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
}
