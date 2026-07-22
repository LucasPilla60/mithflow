//! La máquina de estados de la aplicación y su espejo para la interfaz.
//!
//! ```text
//! Cargando ──(el motor cargó y calentó)──> Listo
//!    │                                       │  atajo
//!    │ (no hay .gguf en el disco)            ▼
//!    ▼                                   Grabando
//! SinModelo                                  │  atajo (o tope de duración)
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

/// En qué anda la aplicación. `SinModelo` y `Error` llevan el motivo porque son
/// lo único que el usuario puede accionar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Estado {
    /// Cargando el modelo y compilando los shaders. El atajo todavía no sirve.
    Cargando,
    Listo,
    Grabando,
    Transcribiendo,
    /// Todavía no hay ningún `.gguf` en el disco.
    ///
    /// **No es un error, y por eso no es `Error`.** Es el estado de una
    /// instalación recién hecha: el asistente está bajando el modelo y la app
    /// hace exactamente lo que corresponde. Mostrar "Error" en ese momento
    /// —que es lo que hacía antes— le dice al usuario que algo se rompió justo
    /// cuando nada se rompió, en el primer minuto de uso.
    ///
    /// Funcionalmente se comporta como `Error` (no se puede dictar), pero se
    /// cuenta distinto: otra clave, otra etiqueta, otro color y otro mensaje.
    SinModelo(String),
    /// Algo que impide dictar y que el usuario tiene que resolver: el modelo
    /// está pero no carga, el atajo no se pudo enganchar, no hay dónde escribir
    /// el historial. Esto sí es una falla y se ve en rojo.
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
            Estado::SinModelo(_) => "sin-modelo",
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
            // Describe lo que falta, no un fracaso. La diferencia entre
            // "Falta el modelo" y "Error" es la diferencia entre una app que
            // te dice qué hacer y una que te dice que se rompió.
            Estado::SinModelo(_) => "Falta el modelo",
            Estado::Error(_) => "Error",
        }
    }

    pub fn detalle(&self) -> Option<&str> {
        match self {
            Estado::SinModelo(motivo) | Estado::Error(motivo) => Some(motivo),
            _ => None,
        }
    }

    /// ¿Se rompió algo? **Sólo** `Error`.
    ///
    /// Existe para que "no se puede dictar" y "hay una falla" no se confundan:
    /// `SinModelo` tampoco dicta, pero no es una falla y no se pinta de rojo.
    pub fn es_falla(&self) -> bool {
        matches!(self, Estado::Error(_))
    }
}

/// Por qué el motor no puede dictar.
///
/// Existe para que quien detecta el problema —`rutas`, que busca el `.gguf`, y
/// `motor`, que lo abre— pueda decir CUÁL de los dos casos es, sin que el
/// director tenga que adivinarlo leyendo el mensaje. Es el mismo criterio que
/// [`mithflow_core::stt::ErrorDeModelo`], un escalón más arriba.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FalloDelMotor {
    /// No hay modelo todavía. Se arregla descargando uno.
    SinModelo(String),
    /// Algo se rompió de verdad.
    Roto(String),
}

impl FalloDelMotor {
    /// El estado que le corresponde. Un solo lugar decide este mapeo.
    pub fn estado(self) -> Estado {
        match self {
            FalloDelMotor::SinModelo(motivo) => Estado::SinModelo(motivo),
            FalloDelMotor::Roto(motivo) => Estado::Error(motivo),
        }
    }

    pub fn motivo(&self) -> &str {
        match self {
            FalloDelMotor::SinModelo(motivo) | FalloDelMotor::Roto(motivo) => motivo,
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

    /// Todos los estados, para que agregar uno obligue a pasar por los tests
    /// que se apoyan en la lista.
    fn todos() -> [Estado; 6] {
        [
            Estado::Cargando,
            Estado::Listo,
            Estado::Grabando,
            Estado::Transcribiendo,
            Estado::SinModelo("x".into()),
            Estado::Error("x".into()),
        ]
    }

    #[test]
    fn las_claves_son_estables_y_distintas() {
        let claves: Vec<&str> = todos().iter().map(Estado::clave).collect();
        assert_eq!(
            claves,
            [
                "cargando",
                "listo",
                "grabando",
                "transcribiendo",
                "sin-modelo",
                "error"
            ]
        );
    }

    /// La clave de `SinModelo` es lo que el frontend usa para elegir el color.
    /// Si colisionara con la de `Error`, la pastilla saldría roja igual y todo
    /// este arreglo no serviría de nada.
    #[test]
    fn sin_modelo_no_se_confunde_con_error() {
        let sin_modelo = Estado::SinModelo("todavía no hay modelo".into());
        let error = Estado::Error("el modelo está corrupto".into());

        assert_ne!(sin_modelo.clave(), error.clave());
        assert_ne!(sin_modelo.etiqueta(), error.etiqueta());
        assert!(
            !sin_modelo.etiqueta().to_lowercase().contains("error"),
            "la etiqueta del primer arranque no puede decir 'error': {}",
            sin_modelo.etiqueta()
        );

        assert!(error.es_falla(), "un modelo que no carga sí es una falla");
        assert!(!sin_modelo.es_falla(), "faltar no es fallar");
    }

    /// Y ningún otro estado se cuela como falla: si mañana alguien marca
    /// `Cargando` o `Transcribiendo` como error, se rompe acá.
    #[test]
    fn la_unica_falla_es_error() {
        let fallas: Vec<&str> = todos()
            .iter()
            .filter(|e| e.es_falla())
            .map(Estado::clave)
            .collect();
        assert_eq!(fallas, ["error"]);
    }

    #[test]
    fn solo_los_estados_accionables_llevan_detalle() {
        assert_eq!(Estado::Listo.detalle(), None);
        assert_eq!(Estado::Cargando.detalle(), None);
        assert_eq!(Estado::Error("no carga".into()).detalle(), Some("no carga"));
        assert_eq!(
            Estado::SinModelo("falta bajarlo".into()).detalle(),
            Some("falta bajarlo")
        );
    }

    /// El mapeo de fallo a estado, que es lo que decide el color de todo lo
    /// demás.
    #[test]
    fn cada_fallo_del_motor_lleva_a_su_estado() {
        let falta = FalloDelMotor::SinModelo("bajá un modelo".into());
        assert_eq!(falta.motivo(), "bajá un modelo");
        assert_eq!(falta.estado(), Estado::SinModelo("bajá un modelo".into()));

        let roto = FalloDelMotor::Roto("el GGUF está cortado".into());
        assert_eq!(roto.motivo(), "el GGUF está cortado");
        assert!(
            roto.estado().es_falla(),
            "un fallo real tiene que terminar en Error"
        );
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
