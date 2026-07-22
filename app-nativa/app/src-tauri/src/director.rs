//! El hilo que decide. Es el único que escribe el estado.
//!
//! Todo lo que puede cambiar el rumbo de la aplicación —el atajo, la bandeja,
//! los comandos del frontend, el motor— manda un [`Mensaje`] por el mismo canal
//! y espera. Acá no hay locks sobre el estado ni carreras posibles: hay una
//! cola y un solo lector.
//!
//! # La transición que importa
//!
//! Al pasar de `Grabando` a `Transcribiendo`, el `Vec<f32>` con el audio **se
//! mueve** al motor. El director se queda literalmente sin el buffer, así que
//! un segundo atajo apretado durante la transcripción no tiene nada que pisar.
//! La versión Python tenía ahí un bug real (dos funciones escribiendo el mismo
//! buffer global); acá el compilador no deja escribirlo.

use crate::ajustes::Ajustes;
use crate::bandeja;
use crate::estado::{Estado, EstadoCompartido, EstadoDto};
use crate::motor::AlMotor;
use crate::sonidos::{Sonidos, Tono};
use crate::{atajo, eventos};
use mithflow_core::audio::Recorder;
use mithflow_core::{config, history, DictationResult};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;

/// Cada cuánto se despierta el director mientras graba, para vigilar el tope de
/// duración. 250 ms no se notan y no cuestan nada: el hilo duerme.
const LATIDO: Duration = Duration::from_millis(250);

/// Lo que el resto del programa le puede pedir al director.
pub enum Mensaje {
    /// Se apretó el atajo.
    Pulso,
    /// `true` pausa, `false` reanuda.
    Pausa(bool),
    AlternarPausa,
    /// El motor terminó de cargar y calentar: `Ok` con un resumen, `Err` con el
    /// motivo por el que no se puede dictar.
    MotorListo(Result<String, String>),
    /// Resultado de un dictado, y la entrada que quedó en el historial.
    ///
    /// `salida` va en `Box` por la misma razón que [`Mensaje::Ajustados`]: un
    /// enum mide lo que su variante mayor, y `DictationResult` (dos textos
    /// completos más la entrada del historial) es varias veces más grande que
    /// todo lo demás que pasa por esta cola. Sin el `Box`, cada `Pulso` del
    /// atajo arrastraría ese tamaño.
    Transcripcion {
        salida: Box<Result<Option<DictationResult>, String>>,
        entrada: Option<history::Entry>,
    },
    /// No se pudo enganchar el teclado.
    AtajoRoto(String),
    /// El usuario guardó Ajustes. El director aplica lo suyo (el tope de
    /// grabación) y le reenvía al motor lo que es del dictado.
    ///
    /// Pasa por acá y no directo del comando al motor por la misma razón que
    /// todo lo demás: el director es el único que le habla al motor, así que no
    /// hay dos escritores compitiendo por esa cola.
    Ajustados(Box<Ajustes>),
}

/// El extremo del canal, para guardarlo en el estado de Tauri.
///
/// No necesita `Mutex`: `mpsc::Sender` es `Sync` desde Rust 1.72 y `send` toma
/// `&self`. Un test de este módulo lo comprueba, para que una regresión en esa
/// garantía se vea acá y no como un error de tipos indescifrable en `manage`.
pub struct AlDirector(Sender<Mensaje>);

impl AlDirector {
    pub fn nuevo(emisor: Sender<Mensaje>) -> Self {
        Self(emisor)
    }

    /// Manda un mensaje. Si el director murió no hay nada que hacer desde acá:
    /// se registra y se sigue, porque quien llama suele ser un clic de menú.
    pub fn enviar(&self, mensaje: Mensaje) {
        if self.0.send(mensaje).is_err() {
            eprintln!("el director no está escuchando; se descarta el mensaje");
        }
    }
}

/// Arranca el director en su propio hilo.
pub fn lanzar(
    app: AppHandle,
    cola: Receiver<Mensaje>,
    al_motor: Sender<AlMotor>,
    espejo: Arc<EstadoCompartido>,
    sonidos: Sonidos,
    limite_grabacion_s: f32,
) {
    let creado = std::thread::Builder::new()
        .name("mithflow-director".into())
        .spawn(move || {
            Director {
                app,
                estado: Estado::Cargando,
                pausado: false,
                grabadora: None,
                al_motor,
                espejo,
                sonidos,
                limite_grabacion_s,
            }
            .atender(cola)
        });
    if let Err(e) = creado {
        eprintln!("no pude crear el hilo del director: {e}");
    }
}

struct Director {
    app: AppHandle,
    estado: Estado,
    pausado: bool,
    /// Existe sólo mientras se graba. Que sea `Option` y no un campo siempre
    /// presente es lo que cierra el micrófono al terminar: el indicador de
    /// Windows se apaga y en una notebook eso es batería.
    grabadora: Option<Recorder>,
    al_motor: Sender<AlMotor>,
    espejo: Arc<EstadoCompartido>,
    sonidos: Sonidos,
    /// Tope de duración de una grabación, configurable desde Ajustes.
    limite_grabacion_s: f32,
}

impl Director {
    fn atender(mut self, cola: Receiver<Mensaje>) {
        // El primer estado ya está en el espejo, pero la bandeja todavía no lo
        // vio: se publica una vez al arrancar.
        self.publicar();

        loop {
            let mensaje = if matches!(self.estado, Estado::Grabando) {
                match cola.recv_timeout(LATIDO) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => {
                        self.vigilar_tope();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match cola.recv() {
                    Ok(m) => m,
                    Err(_) => return,
                }
            };
            self.procesar(mensaje);
        }
    }

    fn procesar(&mut self, mensaje: Mensaje) {
        match mensaje {
            Mensaje::Pulso => self.pulso(),
            Mensaje::Pausa(quiero) => self.fijar_pausa(quiero),
            Mensaje::AlternarPausa => self.fijar_pausa(!self.pausado),
            Mensaje::MotorListo(Ok(resumen)) => {
                eprintln!("{resumen}");
                // Si el motor terminó mientras algo ya había fallado (el atajo,
                // por ejemplo), no se pisa ese error con un "Listo" falso.
                if matches!(self.estado, Estado::Cargando) {
                    self.cambiar(Estado::Listo);
                }
            }
            Mensaje::MotorListo(Err(motivo)) => {
                self.avisar_error(&motivo);
                self.cambiar(Estado::Error(motivo));
            }
            Mensaje::AtajoRoto(motivo) => {
                self.avisar_error(&motivo);
                self.cambiar(Estado::Error(motivo));
            }
            // El `Box` es sólo para que la variante no infle el tamaño del enum
            // en la cola; acá ya no hay cola, así que se abre.
            Mensaje::Transcripcion { salida, entrada } => {
                self.termino_de_transcribir(*salida, entrada)
            }
            Mensaje::Ajustados(nuevos) => self.aplicar_ajustes(nuevos),
        }
    }

    /// El atajo. Cada estado tiene una respuesta y **ninguno se queda callado**:
    /// sin realimentación, "no pasó nada" y "no estaba listo" se ven igual.
    fn pulso(&mut self) {
        match &self.estado {
            Estado::Listo => self.empezar_a_grabar(),
            Estado::Grabando => self.dejar_de_grabar(),
            Estado::Cargando => {
                self.avisar_error("Todavía estoy preparando el motor; dame unos segundos.");
            }
            Estado::Transcribiendo => {
                self.avisar_error("Estoy transcribiendo el dictado anterior.");
            }
            Estado::Error(motivo) => {
                let motivo = motivo.clone();
                self.avisar_error(&motivo);
            }
        }
    }

    /// Aplica lo que es del director y le reenvía al motor lo que es del
    /// dictado. El tope nuevo rige desde la grabación siguiente: cambiarlo en
    /// medio de una ya empezada sería cortarle el dictado al usuario mientras
    /// habla.
    fn aplicar_ajustes(&mut self, nuevos: Box<Ajustes>) {
        self.limite_grabacion_s = nuevos.limite_grabacion_s as f32;
        if self.al_motor.send(AlMotor::Ajustes(nuevos)).is_err() {
            eprintln!("el motor no está escuchando: los ajustes del dictado no se aplicaron");
        }
    }

    fn empezar_a_grabar(&mut self) {
        let mut grabadora = match Recorder::new() {
            Ok(g) => g,
            Err(e) => return self.avisar_error(&format!("No pude preparar el micrófono: {e}")),
        };
        grabadora.fijar_tope(self.limite_grabacion_s);
        // El micrófono ocupado por otra aplicación entra por acá. Se informa y
        // se sigue en `Listo`: no cierra nada.
        if let Err(e) = grabadora.start() {
            return self.avisar_error(&format!("No pude abrir el micrófono: {e}"));
        }
        // El tono suena DESPUÉS de abrir el micrófono, así que entra dentro de
        // los `TONE_GUARD_SECS` que `Recorder::stop` descarta del principio: el
        // modelo no llega a escucharlo por los parlantes.
        self.sonidos.tocar(Tono::Inicio);
        self.grabadora = Some(grabadora);
        self.cambiar(Estado::Grabando);
    }

    fn dejar_de_grabar(&mut self) {
        let Some(mut grabadora) = self.grabadora.take() else {
            // Inalcanzable: sólo se entra en `Grabando` con la grabadora puesta.
            self.cambiar(Estado::Listo);
            return;
        };
        self.sonidos.tocar(Tono::Fin);

        let audio = match grabadora.stop() {
            Ok(a) => a,
            Err(e) => {
                self.avisar_error(&format!("Se perdió el audio: {e}"));
                self.cambiar(Estado::Listo);
                return;
            }
        };

        let segundos = audio.len() as f32 / config::SAMPLE_RATE as f32;
        if segundos < config::MIN_AUDIO_SECS {
            // Un roce del atajo. No es un error y no merece tono: el de fin ya
            // sonó y alcanza para saber que se cortó.
            eventos::aviso(&self.app, "Muy corto, no lo transcribo.", "info");
            self.cambiar(Estado::Listo);
            return;
        }

        // Acá está la transición que importa: `audio` se MUEVE al motor.
        if self.al_motor.send(AlMotor::Audio(audio)).is_err() {
            self.avisar_error("El motor de transcripción no está disponible.");
            self.cambiar(Estado::Listo);
            return;
        }
        self.cambiar(Estado::Transcribiendo);
    }

    /// Corta sola una grabación que llegó al tope. Sin esto, un atajo apretado
    /// sin querer deja el micrófono abierto y el buffer creciendo a 384 KB/s
    /// hasta que alguien se dé cuenta.
    fn vigilar_tope(&mut self) {
        let pasados = self.grabadora.as_ref().map_or(0.0, Recorder::elapsed_secs);
        if pasados >= self.limite_grabacion_s {
            eventos::aviso(
                &self.app,
                "Llegué al máximo de grabación; transcribo lo que hay.",
                "info",
            );
            self.dejar_de_grabar();
        }
    }

    fn termino_de_transcribir(
        &mut self,
        salida: Result<Option<DictationResult>, String>,
        entrada: Option<history::Entry>,
    ) {
        match salida {
            Ok(Some(resultado)) => {
                self.sonidos.tocar(Tono::Pegado);
                if let Some(entrada) = entrada {
                    eventos::dictado_nuevo(&self.app, &entrada);
                }
                // Sólo tiempos: el texto dictado NO va a los logs. Es lo más
                // sensible que maneja esta app y ya queda en el historial, que
                // es un archivo local del usuario y no un log que se comparte
                // al pedir ayuda.
                eprintln!(
                    "dictado: {:.1} s de audio · transcripción {:.2} s",
                    resultado.audio_s, resultado.transcribe_s
                );
            }
            // Silencio, ruido o una alucinación filtrada. Con tono, como en la
            // versión Python: sin él, el usuario no distingue "no te escuché"
            // de "me colgué".
            Ok(None) => self.avisar_error("No se escuchó nada (¿silencio o ruido?)."),
            Err(e) => self.avisar_error(&format!("El dictado falló: {e}")),
        }
        self.cambiar(Estado::Listo);
    }

    /// Pausar suelta la tecla sin cerrar la aplicación. Si se pausa en plena
    /// grabación se corta y se descarta el audio: seguir grabando "en pausa"
    /// sería una contradicción, y transcribir algo que el usuario acaba de
    /// abandonar es peor.
    fn fijar_pausa(&mut self, quiero: bool) {
        if quiero && matches!(self.estado, Estado::Grabando) {
            if let Some(mut grabadora) = self.grabadora.take() {
                let _ = grabadora.stop();
            }
            self.sonidos.tocar(Tono::Fin);
            eventos::aviso(&self.app, "Pausado: descarté la grabación en curso.", "info");
            self.cambiar(Estado::Listo);
        }
        self.pausado = quiero;
        atajo::fijar_pausa(quiero);
        self.publicar();
    }

    fn cambiar(&mut self, nuevo: Estado) {
        if self.estado == nuevo {
            return;
        }
        self.estado = nuevo;
        self.publicar();
    }

    /// Un único lugar donde el estado sale del director: espejo para los
    /// comandos, evento para la interfaz y bandeja para cuando no hay ventana.
    fn publicar(&self) {
        let dto = EstadoDto::nuevo(&self.estado, self.pausado);
        println!(
            "estado: {}{}",
            dto.estado,
            if dto.pausado { " (pausado)" } else { "" }
        );
        self.espejo.escribir(dto.clone());
        eventos::estado_cambiado(&self.app, &dto);
        bandeja::actualizar(&self.app, &dto);
    }

    /// Informa un problema recuperable: tono, evento y registro. **Nunca cambia
    /// el estado** — quien llama decide si además hay que moverse.
    fn avisar_error(&self, texto: &str) {
        eprintln!("{texto}");
        self.sonidos.tocar(Tono::Error);
        eventos::aviso(&self.app, texto, "error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AlDirector` y `Sonidos` viven en el estado manejado por Tauri, que
    /// exige `Send + Sync + 'static`. Si `mpsc::Sender` dejara de ser `Sync`,
    /// el error saldría acá con nombre y apellido en vez de en `manage`.
    #[test]
    fn los_handles_compartidos_siguen_siendo_send_y_sync() {
        fn exigir<T: Send + Sync + 'static>() {}
        exigir::<AlDirector>();
        exigir::<Sonidos>();
        exigir::<Arc<EstadoCompartido>>();
    }

    /// Enviar a un director que ya no existe no puede paniquear: los clics del
    /// menú de la bandeja pasan por acá.
    #[test]
    fn enviar_sin_director_no_rompe() {
        let (tx, rx) = std::sync::mpsc::channel();
        let al_director = AlDirector::nuevo(tx);
        drop(rx);
        al_director.enviar(Mensaje::AlternarPausa);
    }
}
