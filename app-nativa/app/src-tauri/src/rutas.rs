//! Dónde vive cada cosa en el disco.
//!
//! El historial **no** es el de la versión Python. Mientras las dos convivan
//! escriben archivos distintos: `history.jsonl` es de `mithflow.py` y lo lee su
//! dashboard; esta app usa `history-nativo.jsonl` en su propio directorio de
//! datos. Mezclarlos haría que un bug de la versión nueva corrompa el registro
//! de la que el usuario usa todos los días.

use crate::ajustes::{modelo_de_clave, Ajustes};
use mithflow_core::models::{self, Modelo};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

/// Deliberadamente distinto de `history.jsonl`.
pub const ARCHIVO_HISTORIAL: &str = "history-nativo.jsonl";

/// Variable de entorno para apuntar a un `.gguf` concreto.
///
/// Existe para desarrollo (los modelos del spike viven en `app-nativa/models/`,
/// no en `%APPDATA%`) y para una instalación portable. Se valida que sea un
/// archivo existente antes de usarla.
pub const VAR_MODELO: &str = "MITHFLOW_MODELO";

/// `%APPDATA%\com.mithdata.mithflow\`, creado si no existe.
pub fn dir_datos<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no pude resolver el directorio de datos: {e}"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("no pude crear {}: {e}", dir.display()))?;
    Ok(dir)
}

pub fn historial<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(dir_datos(app)?.join(ARCHIVO_HISTORIAL))
}

/// El `.gguf` que hay que cargar, en orden de preferencia:
///
/// 1. lo que diga [`VAR_MODELO`], si apunta a un archivo que existe;
/// 2. el modelo elegido en Ajustes, si está descargado;
/// 3. cualquier otro descargado, del más grande al más chico.
///
/// El paso 3 no es un capricho: si el usuario eligió el `F16` pero todavía sólo
/// bajó el `Q4_K_M`, dictar con el chico es mucho mejor que no dictar. Devuelve
/// también si hubo que sustituir, para poder avisarlo.
pub fn modelo(ajustes: &Ajustes) -> Result<(PathBuf, Option<String>), String> {
    if let Some(desde_entorno) = desde_variable(std::env::var_os(VAR_MODELO).map(PathBuf::from)) {
        return desde_entorno;
    }

    let elegido = modelo_de_clave(&ajustes.modelo);
    if let Some(m) = elegido {
        if models::esta_descargado(m) {
            return Ok((models::ruta(m)?, None));
        }
    }

    let Some(disponible) = Modelo::TODOS.into_iter().find(|m| models::esta_descargado(*m)) else {
        return Err(
            "no hay ningún modelo descargado. Abrí Ajustes y descargá uno para poder dictar."
                .to_string(),
        );
    };
    let aviso = elegido.map(|m| {
        format!("{m} todavía no está descargado; uso {disponible} mientras tanto.")
    });
    Ok((models::ruta(disponible)?, aviso))
}

/// La rama de [`VAR_MODELO`], separada de la lectura del entorno para poder
/// probarla sin tocar variables globales del proceso.
///
/// `None` significa "la variable no está, seguí con el catálogo". Si está pero
/// apunta a cualquier cosa, es un ERROR y no un aviso: quien la fijó lo hizo a
/// propósito, y caerse en silencio a otro modelo escondería el problema.
fn desde_variable(valor: Option<PathBuf>) -> Option<Result<(PathBuf, Option<String>), String>> {
    let ruta = valor?;
    if ruta.is_file() {
        let aviso = format!("modelo tomado de {VAR_MODELO}: {}", ruta.display());
        return Some(Ok((ruta, Some(aviso))));
    }
    Some(Err(format!(
        "{VAR_MODELO} apunta a {}, que no es un archivo",
        ruta.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La regla que sostiene la convivencia con la versión Python.
    #[test]
    fn el_historial_no_es_el_de_python() {
        assert_ne!(ARCHIVO_HISTORIAL, "history.jsonl");
        assert_eq!(ARCHIVO_HISTORIAL, "history-nativo.jsonl");
    }

    /// Una variable mal puesta tiene que dar un error claro, no caerse a otro
    /// modelo por su cuenta: si alguien la fijó, es a propósito.
    #[test]
    fn la_variable_apuntando_a_la_nada_es_un_error() {
        let salida = desde_variable(Some(PathBuf::from("D:\\no\\existe\\modelo.gguf")))
            .expect("con la variable puesta, la rama decide");
        let error = salida.expect_err("una ruta inexistente no puede pasar");
        assert!(error.contains("no es un archivo"), "mensaje: {error}");
    }

    #[test]
    fn sin_variable_la_decision_sigue_por_el_catalogo() {
        assert!(desde_variable(None).is_none());
    }

    /// Un archivo que sí existe se acepta y se avisa de dónde salió.
    #[test]
    fn la_variable_apuntando_a_un_archivo_real_manda() {
        let archivo = std::env::temp_dir().join(format!("mithflow_ruta_{}.gguf", std::process::id()));
        std::fs::write(&archivo, b"no es un modelo de verdad").expect("no pude crear el archivo");

        let (ruta, aviso) = desde_variable(Some(archivo.clone()))
            .expect("con la variable puesta, la rama decide")
            .expect("un archivo existente se acepta");
        assert_eq!(ruta, archivo);
        assert!(aviso.unwrap_or_default().contains(VAR_MODELO));

        std::fs::remove_file(&archivo).ok();
    }
}
