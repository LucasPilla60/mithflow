//! La máquina de estados de la aplicación y su espejo para la interfaz.
//!
//! ```text
//! Cargando ──(el motor cargó y calentó)──> Listo
//!                                            │  atajo
//!                                            ▼
//!                                        Grabando
//!                                            │  atajo (o tope de duración)
//!                                            ▼
//!                                      Transcribiendo ──> Listo
//! ```
//!
//! **Un solo hilo escribe el estado** (el director). Todos los demás lo leen
//! por el espejo [`EstadoCompartido`]. Esa asimetría es deliberada: mientras el
//! estado viva en un solo lugar con un solo escritor, no hay transición que dos
//! hilos puedan pisarse.
//!
//! `Pausado` NO es un estado de esta máquina sino una condición ortogonal: se
//! puede estar pausado y `Listo`, y la pausa no cambia lo que pasa con el audio
//! ya grabado. Vive aparte para que el `match` del director no tenga que
//! duplicar cada rama.

use serde::Serialize;
use std::sync::RwLock;

/// En qué anda la aplicación. `Error` lleva el motivo porque es lo único que
/// el usuario puede accionar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Estado {
    /// Cargando el modelo y compilando los shaders. El atajo todavía no sirve.
    Cargando,
    Listo,
    Grabando,
    Transcribiendo,
    /// Algo que impide dictar y que el usuario tiene que resolver (no hay
    /// modelo descargado, el atajo no se pudo enganchar).
    Error(String),
}

impl Estado {
    /// Identificador estable para la interfaz. Es lo que viaja en los eventos,
    /// así que cambiarlo rompe el frontend: se elige acá y no se deriva de
    /// `Debug`, que es para el programador.
    pub fn clave(&self) -> &'static str {
        match self {
            Estado::Cargando => "cargando",
            Estado::Listo => "listo",
            Estado::Grabando => "grabando",
            Estado::Transcribiendo => "transcribiendo",
            Estado::Error(_) => "error",
        }
    }

    /// Etiqueta para la bandeja, en español y sin jerga.
    pub fn etiqueta(&self) -> &'static str {
        match self {
            Estado::Cargando => "Cargando…",
            Estado::Listo => "Listo",
            Estado::Grabando => "Grabando",
            Estado::Transcribiendo => "Transcribiendo",
            Estado::Error(_) => "Error",
        }
    }

    pub fn detalle(&self) -> Option<&str> {
        match self {
            Estado::Error(motivo) => Some(motivo),
            _ => None,
        }
    }
}

/// Lo que ven la interfaz y la bandeja: el estado más la pausa, que son dos
/// cosas independientes pero siempre se muestran juntas.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EstadoDto {
    pub estado: &'static str,
    pub etiqueta: &'static str,
    pub detalle: Option<String>,
    pub pausado: bool,
}

impl EstadoDto {
    pub fn nuevo(estado: &Estado, pausado: bool) -> Self {
        Self {
            estado: estado.clave(),
            etiqueta: estado.etiqueta(),
            detalle: estado.detalle().map(str::to_string),
            pausado,
        }
    }
}

/// Copia de sólo lectura del estado, para los comandos de Tauri y la bandeja.
///
/// Es un `RwLock` y no un canal porque el dashboard tiene que poder preguntar
/// "¿en qué andás?" al montarse, sin esperar a que algo cambie. Un evento sólo
/// cuenta las novedades; esto cuenta el presente.
pub struct EstadoCompartido(RwLock<EstadoDto>);

impl EstadoCompartido {
    pub fn nuevo() -> Self {
        Self(RwLock::new(EstadoDto::nuevo(&Estado::Cargando, false)))
    }

    /// Lectura. Si el lock quedó envenenado se devuelve igual el último valor:
    /// un `RwLock` envenenado no corrompe el `EstadoDto` (no hay invariante que
    /// mantener entre campos) y dejar la interfaz sin estado sería peor.
    pub fn leer(&self) -> EstadoDto {
        match self.0.read() {
            Ok(g) => g.clone(),
            Err(envenenado) => envenenado.into_inner().clone(),
        }
    }

    pub fn escribir(&self, nuevo: EstadoDto) {
        match self.0.write() {
            Ok(mut g) => *g = nuevo,
            Err(envenenado) => *envenenado.into_inner() = nuevo,
        }
    }
}

impl Default for EstadoCompartido {
    fn default() -> Self {
        Self::nuevo()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn las_claves_son_estables_y_distintas() {
        let todos = [
            Estado::Cargando,
            Estado::Listo,
            Estado::Grabando,
            Estado::Transcribiendo,
            Estado::Error("x".into()),
        ];
        let claves: Vec<&str> = todos.iter().map(Estado::clave).collect();
        assert_eq!(
            claves,
            ["cargando", "listo", "grabando", "transcribiendo", "error"]
        );
    }

    #[test]
    fn solo_el_estado_de_error_lleva_detalle() {
        assert_eq!(Estado::Listo.detalle(), None);
        assert_eq!(Estado::Error("sin modelo".into()).detalle(), Some("sin modelo"));
    }

    #[test]
    fn el_espejo_arranca_cargando_y_refleja_lo_que_se_escribe() {
        let compartido = EstadoCompartido::nuevo();
        assert_eq!(compartido.leer().estado, "cargando");
        assert!(!compartido.leer().pausado);

        compartido.escribir(EstadoDto::nuevo(&Estado::Grabando, true));
        let dto = compartido.leer();
        assert_eq!(dto.estado, "grabando");
        assert_eq!(dto.etiqueta, "Grabando");
        assert!(dto.pausado);
    }
}
