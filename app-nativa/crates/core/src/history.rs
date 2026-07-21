use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// Valor por defecto de `mode` para las entradas anteriores al 21/7/2026,
/// que son de la época en que la limpieza la hacía un LLM.
fn modo_historico() -> String {
    "llm".to_string()
}

/// Una entrada del historial, con el mismo formato que `save_history` de la
/// versión Python.
///
/// Todos los campos salvo `ts` y `final` llevan `#[serde(default)]`: el
/// historial real tiene entradas de varias épocas del programa y 7 de 20 no
/// tienen `mode`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub ts: String,
    #[serde(default)]
    pub audio_s: f32,
    #[serde(default)]
    pub transcribe_s: f32,
    #[serde(default)]
    pub cleanup_s: f32,
    #[serde(default)]
    pub words: usize,
    #[serde(default)]
    pub cleaned: bool,
    #[serde(default = "modo_historico")]
    pub mode: String,
    #[serde(default)]
    pub raw: String,
    /// `final` es palabra reservada en Rust; en el archivo se llama "final".
    #[serde(rename = "final")]
    pub final_text: String,
}

/// Agrega una entrada. Un fallo acá no debe invalidar el dictado: quien llame
/// registra el error, pero el texto ya fue pegado.
pub fn append(path: &Path, entry: &Entry) -> std::io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(entry)?)
}

/// Borra el historial entero.
///
/// **Borra, no vacía**: dejar el archivo en cero bytes serviría igual, pero un
/// archivo que no existe es una respuesta más clara a "¿queda algo de lo que
/// dicté?". Que el archivo ya no esté NO es un error: borrar dos veces tiene
/// que ser igual que borrar una, porque el botón está a un clic y el usuario no
/// tiene por qué saber si había algo.
pub fn clear(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Saca el BOM UTF-8 (`EF BB BF`, que decodifica a U+FEFF) del principio del
/// archivo. `mithflow.py` escribe sin BOM, pero `dashboard.py` lee con
/// `utf-8-sig`, así que el proyecto ya asume que puede aparecer uno. Sin esto,
/// el `filter_map` de `load` se traga el error de parseo y la primera entrada
/// del historial se pierde en silencio.
fn sin_bom(mut linea: String) -> String {
    if linea.starts_with('\u{feff}') {
        linea.remove(0);
    }
    linea
}

/// Lee todas las entradas, salteando líneas corruptas en vez de fallar.
pub fn load(path: &Path) -> std::io::Result<Vec<Entry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = std::fs::File::open(path)?;
    Ok(BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .enumerate()
        // El BOM solo puede estar en los primeros bytes del archivo.
        .map(|(i, l)| if i == 0 { sin_bom(l) } else { l })
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializa_igual_que_la_version_python() {
        let e = Entry {
            ts: "2026-07-21T12:00:00".into(),
            audio_s: 5.0,
            transcribe_s: 0.3,
            cleanup_s: 0.001,
            words: 7,
            cleaned: true,
            mode: "fast".into(),
            raw: "eh hola".into(),
            final_text: "Hola".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        // El campo se llama "final" en el archivo, aunque en Rust sea reservado
        assert!(json.contains(r#""final":"Hola""#), "json real: {json}");
        assert!(json.contains(r#""ts":"2026-07-21T12:00:00""#));
        assert!(json.contains(r#""mode":"fast""#));
    }

    #[test]
    fn lee_el_formato_actual() {
        let linea = r#"{"ts": "2026-07-21T11:49:40", "audio_s": 6.58, "transcribe_s": 0.51, "cleanup_s": 2.69, "words": 22, "cleaned": true, "mode": "fast", "raw": "crudo", "final": "limpio"}"#;
        let e: Entry = serde_json::from_str(linea).unwrap();
        assert_eq!(e.words, 22);
        assert_eq!(e.final_text, "limpio");
        assert_eq!(e.mode, "fast");
    }

    /// 7 de las 20 entradas reales NO tienen `mode`. Sin `#[serde(default)]`
    /// esto falla y se pierde el 35% del historial del usuario.
    #[test]
    fn lee_entradas_viejas_sin_campo_mode() {
        let linea = r#"{"ts": "2026-07-21T10:15:00", "audio_s": 9.5, "transcribe_s": 0.5, "cleanup_s": 1.2, "words": 18, "cleaned": true, "raw": "crudo", "final": "limpio"}"#;
        let e: Entry = serde_json::from_str(linea).expect("debe leer entradas sin `mode`");
        assert_eq!(e.words, 18);
        assert_eq!(e.mode, "llm", "las entradas viejas son de la época del LLM");
    }

    #[test]
    fn saltea_lineas_invalidas_sin_abortar() {
        let dir = std::env::temp_dir().join(format!("mithflow_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("h.jsonl");
        std::fs::write(
            &path,
            "{\"ts\":\"2026-07-21T10:00:00\",\"final\":\"uno\"}\n\
             esto no es json\n\
             \n\
             {\"ts\":\"2026-07-21T10:01:00\",\"final\":\"dos\"}\n",
        )
        .unwrap();
        let entries = load(&path).unwrap();
        assert_eq!(
            entries.len(),
            2,
            "debe leer las 2 válidas e ignorar la basura"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn append_y_load_hacen_ida_y_vuelta() {
        let dir = std::env::temp_dir().join(format!("mithflow_rt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rt.jsonl");
        std::fs::remove_file(&path).ok();
        let e = Entry {
            ts: "2026-07-21T13:00:00".into(),
            audio_s: 1.5,
            transcribe_s: 0.2,
            cleanup_s: 0.0,
            words: 2,
            cleaned: false,
            mode: "fast".into(),
            raw: "hola mundo".into(),
            final_text: "Hola mundo".into(),
        };
        append(&path, &e).unwrap();
        append(&path, &e).unwrap();
        let leidas = load(&path).unwrap();
        assert_eq!(leidas.len(), 2);
        assert_eq!(leidas[0].final_text, "Hola mundo");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `mithflow.py` escribe con `utf-8` (sin BOM) pero `dashboard.py` lee con
    /// `utf-8-sig`: el propio proyecto ya asume que puede aparecer un BOM
    /// (Notepad, `Set-Content` de PowerShell 5.1). Sin manejarlo, `filter_map`
    /// se traga el error y la primera entrada del usuario se pierde en silencio.
    #[test]
    fn lee_un_archivo_con_bom_utf8() {
        let dir = std::env::temp_dir().join(format!("mithflow_bom_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bom.jsonl");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            b"{\"ts\":\"2026-07-21T10:00:00\",\"final\":\"primera\"}\n\
              {\"ts\":\"2026-07-21T10:01:00\",\"final\":\"segunda\"}\n",
        );
        std::fs::write(&path, bytes).unwrap();
        let entries = load(&path).unwrap();
        assert_eq!(
            entries.len(),
            2,
            "el BOM no debe hacer perder la primera entrada"
        );
        assert_eq!(entries[0].final_text, "primera");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Borrar el historial lo deja vacío, y borrar dos veces no es un error:
    /// el botón está a un clic y no puede fallar por llegar tarde.
    #[test]
    fn borrar_el_historial_lo_vacia_y_es_idempotente() {
        let dir = std::env::temp_dir().join(format!("mithflow_clear_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("h.jsonl");
        std::fs::write(&path, "{\"ts\":\"2026-07-21T10:00:00\",\"final\":\"uno\"}\n").unwrap();
        assert_eq!(load(&path).unwrap().len(), 1);

        clear(&path).expect("borrar tiene que funcionar");
        assert!(!path.exists(), "el archivo tiene que desaparecer");
        assert!(load(&path).unwrap().is_empty(), "sin archivo, historial vacío");

        clear(&path).expect("borrar de nuevo no puede fallar");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// El historial real del usuario debe leerse completo (criterio 7 del spec).
    /// Ignorado por defecto: depende de un archivo fuera del crate.
    /// Correr con: cargo test -p mithflow-core history -- --ignored --nocapture
    #[test]
    #[ignore]
    fn lee_el_historial_real_del_proyecto() {
        let path = std::path::Path::new(r"D:\MithFlow\history.jsonl");
        if !path.exists() {
            eprintln!("no está el historial real, se saltea");
            return;
        }
        let entries = load(path).expect("debe poder leerse");
        let lineas = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
        println!(
            "historial real: {} lineas, {} entradas parseadas",
            lineas,
            entries.len()
        );
        assert_eq!(entries.len(), lineas, "se perdieron entradas al parsear");
    }
}
