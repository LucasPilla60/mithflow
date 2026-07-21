//! La ventana principal.
//!
//! Esta aplicación vive en la bandeja: la ventana es una vista, no la app. Por
//! eso cerrarla la esconde en vez de terminar el proceso —si cerrar la ventana
//! matara el dictado, el usuario perdería el atajo sin darse cuenta— y por eso
//! "Abrir dashboard" y "Ajustes" abren la misma ventana con un evento que dice
//! qué mostrar, en lugar de crear ventanas distintas.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};

pub const PRINCIPAL: &str = "main";

/// Evento que le dice al frontend qué sección abrir.
pub const IR_A: &str = "ir-a";

#[derive(Clone, Serialize)]
struct IrA {
    seccion: &'static str,
}

/// Trae la ventana al frente y le pide que muestre una sección.
///
/// Cada paso puede fallar por su cuenta (la ventana pudo ser destruida) y
/// ninguno es fatal: se registra y se sigue.
pub fn mostrar<R: Runtime>(app: &AppHandle<R>, seccion: &'static str) {
    let Some(ventana) = app.get_webview_window(PRINCIPAL) else {
        eprintln!("no encuentro la ventana '{PRINCIPAL}'");
        return;
    };
    enfocar(&ventana);
    if let Err(e) = ventana.emit(IR_A, IrA { seccion }) {
        eprintln!("no pude pedirle al frontend que abra {seccion}: {e}");
    }
}

/// Muestra, desminimiza y enfoca. Los tres hacen falta: una ventana escondida
/// no se enfoca, y una minimizada se enfoca sin verse.
pub fn enfocar<R: Runtime>(ventana: &WebviewWindow<R>) {
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
