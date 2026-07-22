//! Los eventos que la app le manda al frontend, en un solo lugar.
//!
//! Los nombres son constantes y no cadenas sueltas por una razón concreta: un
//! `emit("estado-cambiado")` contra un `listen("estado_cambiado")` compila,
//! arranca y no hace nada. Acá al menos el lado Rust es un solo sitio para
//! cambiar, y `EVENTOS` los expone para que el frontend no los adivine.
//!
//! **Los eventos cuentan novedades, no el presente.** Un dictado hecho con la
//! ventana cerrada no llega a ningún oyente, así que el dashboard tiene que
//! poder pedir el estado y el historial al montarse (ver `comandos`). Sin esa
//! mitad, abrir la ventana mostraría una lista vacía.

use crate::estado::EstadoDto;
use crate::superpuesta::NivelAudio;
use mithflow_core::history;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

pub const ESTADO_CAMBIADO: &str = "estado-cambiado";
pub const DICTADO_NUEVO: &str = "dictado-nuevo";
pub const PROGRESO_DESCARGA: &str = "progreso-descarga";
pub const PERFILADO_LISTO: &str = "perfilado-listo";
pub const AVISO: &str = "aviso";
/// Cuánto está entrando por el micrófono, unas veinticinco veces por segundo
/// **y sólo mientras se graba** (ver `superpuesta::hay_que_medir`).
pub const NIVEL_AUDIO: &str = "nivel-audio";

/// Algo que contarle al usuario sin cambiar de estado: el micrófono estaba
/// ocupado, no se escuchó nada, la grabación llegó al tope.
#[derive(Debug, Clone, Serialize)]
pub struct Aviso {
    pub texto: String,
    /// `"info"` o `"error"`. Decide el color, no el flujo.
    pub nivel: &'static str,
}

/// Cómo va una descarga de modelo.
#[derive(Debug, Clone, Serialize)]
pub struct ProgresoDescarga {
    pub modelo: String,
    pub bytes: u64,
    pub total: u64,
    pub porcentaje: f32,
    pub terminado: bool,
    /// Presente sólo si la descarga falló.
    pub error: Option<String>,
}

impl ProgresoDescarga {
    pub fn en_curso(modelo: &str, bytes: u64, total: u64) -> Self {
        Self {
            modelo: modelo.to_string(),
            bytes,
            total,
            // El total sale del catálogo compilado, nunca es cero; la guarda
            // está igual porque una división por cero acá daría `NaN` y el
            // frontend dibujaría una barra imposible.
            porcentaje: if total == 0 {
                0.0
            } else {
                (bytes as f64 / total as f64 * 100.0) as f32
            },
            terminado: false,
            error: None,
        }
    }

    pub fn fallada(modelo: &str, error: String) -> Self {
        Self {
            modelo: modelo.to_string(),
            bytes: 0,
            total: 0,
            porcentaje: 0.0,
            terminado: true,
            error: Some(error),
        }
    }
}

/// Emite y, si falla, lo registra. Un evento que no llega no puede tumbar nada:
/// significa que no hay ventana escuchando, que es el caso normal de esta app.
fn emitir<R: Runtime, P: Serialize + Clone>(app: &AppHandle<R>, nombre: &str, carga: P) {
    if let Err(e) = app.emit(nombre, carga) {
        eprintln!("no pude emitir {nombre}: {e}");
    }
}

pub fn estado_cambiado<R: Runtime>(app: &AppHandle<R>, dto: &EstadoDto) {
    emitir(app, ESTADO_CAMBIADO, dto.clone());
}

pub fn dictado_nuevo<R: Runtime>(app: &AppHandle<R>, entrada: &history::Entry) {
    emitir(app, DICTADO_NUEVO, entrada.clone());
}

pub fn aviso<R: Runtime>(app: &AppHandle<R>, texto: &str, nivel: &'static str) {
    emitir(
        app,
        AVISO,
        Aviso {
            texto: texto.to_string(),
            nivel,
        },
    );
}

pub fn progreso_descarga<R: Runtime>(app: &AppHandle<R>, progreso: ProgresoDescarga) {
    emitir(app, PROGRESO_DESCARGA, progreso);
}

/// El nivel de entrada para la ventanita de grabación.
///
/// A diferencia del resto, **éste no tiene comando espejo y no lo necesita**: no
/// cuenta un hecho que haya que reconstruir al abrir una ventana, sino el
/// instante que está pasando. Un nivel viejo no significa nada, y el siguiente
/// llega en cuarenta milisegundos.
///
/// # Por qué se emite a todas las ventanas y no sólo a la ventanita
///
/// `emit_to(etiqueta, …)` parece lo correcto —es el único que lo mira— pero
/// exige que el oyente se haya registrado con esa misma etiqueta como destino:
/// un `listen()` pelado del frontend queda como `EventTarget::Any`, y el filtro
/// de Tauri (`manager::emit_to`) **no** hace coincidir `Any` con `AnyLabel`. O
/// sea que un descuido del lado JavaScript deja la ventanita sin un solo evento,
/// que es la falla total de esta función y además silenciosa. Con `emit` el
/// costo de más es que la ventana principal, si está abierta durante un dictado,
/// recibe veinticinco mensajes por segundo que ignora.
pub fn nivel_audio<R: Runtime>(app: &AppHandle<R>, nivel: NivelAudio) {
    emitir(app, NIVEL_AUDIO, nivel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_porcentaje_es_el_esperado_y_no_divide_por_cero() {
        let p = ProgresoDescarga::en_curso("F16", 50, 200);
        assert_eq!(p.porcentaje, 25.0);
        assert!(!p.terminado);

        let p = ProgresoDescarga::en_curso("F16", 0, 0);
        assert_eq!(p.porcentaje, 0.0, "sin total no hay NaN");
    }

    #[test]
    fn una_descarga_fallada_llega_terminada_y_con_motivo() {
        let p = ProgresoDescarga::fallada("F16", "se cortó la red".into());
        assert!(p.terminado);
        assert_eq!(p.error.as_deref(), Some("se cortó la red"));
    }
}
