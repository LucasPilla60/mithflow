//! Spike: ¿`rdev::grab` consume la tecla para que no llegue a la app enfocada?
//!
//! Es el bug que la versión Python resolvió con `suppress=True`. Sin supresión,
//! F9 llega también a la aplicación donde está el cursor, lo mueve, y el texto
//! se pega en otro lado.
//!
//! Usa **F9**, no F8: la versión Python en producción usa F8 y correr los dos a
//! la vez dispararía una grabación real que contaminaría la prueba.
//!
//! Uso: `cargo run --release`, después probar con el foco en cada aplicación.

use rdev::{grab, Event, EventType, Key};
use std::sync::atomic::{AtomicU32, Ordering};

/// `grab` toma un callback `Fn`, no `FnMut`: no se puede mutar una variable
/// capturada. El contador va en un atómico.
static CAPTURAS: AtomicU32 = AtomicU32::new(0);

fn main() {
    println!("Escuchando F9. Probá con el cursor en cada una de estas:");
    println!("  1. Bloc de notas");
    println!("  2. VS Code");
    println!("  3. Chrome, en un campo de texto");
    println!("  4. PowerShell ABIERTO COMO ADMINISTRADOR  <- se espera que ACÁ FALLE");
    println!();
    println!("Si la supresión funciona: aparece el mensaje de abajo y la app NO reacciona");
    println!("(el cursor no se mueve, no pasa nada raro).");
    println!();
    println!("El caso 4 se espera que falle: ningún hook de teclado en modo usuario");
    println!("puede suprimir una tecla destinada a un proceso de mayor integridad.");
    println!("Confirmarlo es el objetivo, no arreglarlo.");
    println!();
    println!("Ctrl+C para salir.");
    println!();

    if let Err(e) = grab(|event: Event| -> Option<Event> {
        match event.event_type {
            EventType::KeyPress(Key::F9) => {
                let n = CAPTURAS.fetch_add(1, Ordering::Relaxed) + 1;
                println!("F9 capturada y SUPRIMIDA (#{n})");
                None // devolver None descarta el evento
            }
            EventType::KeyRelease(Key::F9) => None,
            _ => Some(event),
        }
    }) {
        eprintln!("Error al enganchar el teclado: {e:?}");
        std::process::exit(1);
    }
}
