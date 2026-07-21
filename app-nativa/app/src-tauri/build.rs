//! Además de generar el contexto de Tauri, junta las DLLs nativas que el
//! instalador tiene que llevar.
//!
//! # El problema que resuelve
//!
//! Con `dynamic-backends`, whisper.cpp no queda adentro del ejecutable: son 13
//! DLLs sueltas (~84 MB; `ggml-vulkan.dll` sola pesa 71) que
//! `init_backends_default()` busca AL LADO del ejecutable. Compilando funciona
//! de casualidad, porque el build de `transcribe-cpp-sys` las copia al
//! directorio del perfil de cargo — que es justo donde queda el `.exe`. **El
//! instalador no hereda esa casualidad**: si no se las nombra explícitamente,
//! el bundler de Tauri no las empaqueta y la app instalada falla al cargar el
//! modelo con "backend error (status 8)".
//!
//! # Cómo
//!
//! El crate publica dónde quedaron sus artefactos en `DEP_TRANSCRIBE_CPP_*`,
//! pero cargo sólo se las pasa al build script del que depende DIRECTAMENTE del
//! crate con `links`. Por eso `transcribe-cpp` figura como dependencia de esta
//! app aunque el código no lo use: es la única forma de recibir esas rutas.
//! Se copian a `transcribe-libs/`, una carpeta de ruta fija que
//! `tauri.conf.json` puede nombrar en `bundle.resources` (los directorios de
//! cargo llevan un hash y no se pueden escribir en un archivo de configuración).
//!
//! En Windows los recursos de Tauri se instalan al lado del ejecutable, que es
//! exactamente donde `init_backends_default()` los busca.

use std::path::{Path, PathBuf};
use std::{env, fs};

/// Dónde se dejan las librerías para que el bundler las encuentre.
const DESTINO: &str = "transcribe-libs";

fn main() {
    // ANTES de `tauri_build::build()`, no después: ahí es donde se resuelve el
    // glob de `bundle.resources`, y un `transcribe-libs/` todavía vacío aborta
    // la compilación con "path not found or didn't match any files".
    reunir_librerias_nativas();
    tauri_build::build();
}

fn reunir_librerias_nativas() {
    println!("cargo:rerun-if-env-changed=DEP_TRANSCRIBE_CPP_RUNTIME_DIR");
    println!("cargo:rerun-if-env-changed=DEP_TRANSCRIBE_CPP_MODULE_DIR");

    // Ausentes en un build estático: no hay nada que llevar.
    let Some(runtime) = env::var_os("DEP_TRANSCRIBE_CPP_RUNTIME_DIR") else {
        return;
    };

    let destino = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DESTINO);
    if let Err(e) = fs::create_dir_all(&destino) {
        panic!("no pude crear {}: {e}", destino.display());
    }

    let copiadas = copiar(Path::new(&runtime), &destino);
    // Falla ruidosa a propósito: un directorio anunciado y vacío significa que
    // el instalador saldría sin motor, y eso se descubriría recién al
    // instalarlo en otra máquina.
    assert!(
        copiadas > 0,
        "no encontré librerías en {}",
        Path::new(&runtime).display()
    );

    // Con `dynamic-backends` los backends de cómputo son módulos aparte, en
    // otro directorio.
    if let Some(modulos) = env::var_os("DEP_TRANSCRIBE_CPP_MODULE_DIR") {
        let copiados = copiar(Path::new(&modulos), &destino);
        assert!(
            copiados > 0,
            "no encontré módulos de backend en {}",
            Path::new(&modulos).display()
        );
    }
}

/// Copia las librerías dinámicas de `origen` a `destino` y devuelve cuántas.
///
/// Se filtra por NOMBRE y no por extensión: en Linux las librerías van
/// versionadas (`libtranscribe.so.0`) y el cargador necesita el SONAME, así que
/// un filtro por extensión copiaría sólo el enlace de desarrollo. En Windows la
/// distinción no importa, pero el criterio correcto no cuesta más.
fn copiar(origen: &Path, destino: &Path) -> usize {
    let entradas = match fs::read_dir(origen) {
        Ok(e) => e,
        Err(e) => panic!("no pude leer {}: {e}", origen.display()),
    };

    let mut copiadas = 0;
    for entrada in entradas.flatten() {
        let ruta = entrada.path();
        let Some(nombre) = ruta.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let es_libreria = nombre.ends_with(".dll")
            || nombre.ends_with(".dylib")
            || nombre.ends_with(".so")
            || nombre.contains(".so.");
        if !es_libreria {
            continue;
        }
        if let Err(e) = fs::copy(&ruta, destino.join(nombre)) {
            panic!("no pude copiar {}: {e}", ruta.display());
        }
        copiadas += 1;
    }
    copiadas
}
