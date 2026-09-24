//! La ventana principal.
//!
//! Esta aplicación vive en la bandeja: la ventana es una vista, no la app. Por
//! eso cerrarla la esconde en vez de terminar el proceso —si cerrar la ventana
//! matara el dictado, el usuario perdería el atajo sin darse cuenta— y por eso
//! "Abrir dashboard" y "Ajustes" abren la misma ventana con un evento que dice
//! qué mostrar, en lugar de crear ventanas distintas.
//!
//! # Si la ventana no existe, se vuelve a crear
//!
//! Después de actualizar a la 1.1.2 el proceso quedó vivo, dictando, pero sin
//! la ventana `main`: ni escondida, directamente no estaba. Abrirla desde la
//! bandeja o el acceso directo no hacía nada, y la app parecía colgada. La
//! causa exacta no se pudo reproducir; sí que pasa en el relanzamiento que hace
//! el instalador. Por eso la corrección no depende de la causa: quien pide la
//! ventana y no la encuentra la crea de nuevo desde su configuración.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow, WebviewWindowBuilder};

pub const PRINCIPAL: &str = "main";

/// Evento que le dice al frontend qué sección abrir.
pub const IR_A: &str = "ir-a";

/// Hay una recreación en curso. Dos clics seguidos en la bandeja no pueden
/// lanzar dos: la segunda fallaría por la etiqueta repetida.
static RECREANDO: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
struct IrA {
    seccion: &'static str,
}

/// Trae la ventana al frente y, si se pide, cambia de sección.
///
/// Cada paso puede fallar por su cuenta y ninguno es fatal: se registra y se
/// sigue.
pub fn mostrar<R: Runtime>(app: &AppHandle<R>, seccion: Option<&'static str>) {
    let Some(ventana) = app.get_webview_window(PRINCIPAL) else {
        recrear(app.clone());
        return;
    };
    enfocar(&ventana);
    if let Some(seccion) = seccion {
        if let Err(e) = ventana.emit(IR_A, IrA { seccion }) {
            eprintln!("no pude pedirle al frontend que abra {seccion}: {e}");
        }
    }
}

/// Crea la ventana principal desde `tauri.conf.json`, en un hilo aparte.
///
/// El hilo no es opcional: quien llama acá es un manejador de eventos (menú de
/// la bandeja, segunda instancia) que corre en el hilo principal, y en Windows
/// `build()` desde ahí se traba para siempre (documentado en
/// `WebviewWindowBuilder::from_config`, wry#583).
///
/// La sección pedida no viaja: la ventana nueva todavía no está escuchando
/// cuando se crea, y abre en el dashboard. Es un camino de recuperación.
fn recrear<R: Runtime>(app: AppHandle<R>) {
    if RECREANDO.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let config = app
            .config()
            .app
            .windows
            .iter()
            .find(|w| w.label == PRINCIPAL)
            .cloned();
        match config {
            None => eprintln!("no hay una ventana '{PRINCIPAL}' en tauri.conf.json"),
            Some(config) => {
                match WebviewWindowBuilder::from_config(&app, &config).and_then(|b| b.build()) {
                    Ok(ventana) => {
                        eprintln!("la ventana '{PRINCIPAL}' no existía: la creé de nuevo");
                        enfocar(&ventana);
                    }
                    Err(e) => eprintln!("no pude volver a crear la ventana '{PRINCIPAL}': {e}"),
                }
            }
        }
        RECREANDO.store(false, Ordering::SeqCst);
    });
}

/// Muestra, desminimiza y enfoca. Los tres hacen falta: una ventana escondida
/// no se enfoca, y una minimizada se enfoca sin verse.
fn enfocar<R: Runtime>(ventana: &WebviewWindow<R>) {
    if let Err(e) = ventana.show() {
        eprintln!("no pude mostrar la ventana: {e}");
    }
    if ventana.is_minimized().unwrap_or(false) {
        if let Err(e) = ventana.unminimize() {
            eprintln!("no pude desminimizar la ventana: {e}");
        }
    }
    if let Err(e) = ventana.set_focus() {
        eprintln!("no pude enfocar la ventana: {e}");
    }
}
