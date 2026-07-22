//! El hilo que carga el modelo y transcribe.
//!
//! Está separado del director por dos motivos que valen igual:
//!
//! 1. **Cargar el modelo y calentarlo tarda ~20 s** (1,8 s de carga y 17 s
//!    compilando shaders de Vulkan la primera vez en la máquina). Si eso pasara
//!    en el hilo de la interfaz, la app arrancaría congelada.
//! 2. **El buffer de audio se MUEVE hasta acá.** El director le manda el `Vec`
//!    y se queda sin él: no hay estado compartido que un segundo atajo pueda
//!    pisar mientras esto transcribe. Es el bug real que tuvo la versión
//!    Python, donde `process_recording` y `toggle` escribían los dos sobre el
//!    mismo buffer global; en Rust lo resuelve el sistema de tipos, no un lock.
//!
//! # Por qué los ajustes viajan por el mismo canal que el audio
//!
//! Vocabulario, muletillas y modo de limpieza los aplica este hilo, que está
//! bloqueado esperando audio. Mandarlos por otra vía obligaría a que alguien
//! los escriba mientras acá se leen —o sea un lock en el camino del dictado—;
//! por la misma cola llegan **ordenados** respecto de los dictados y sin
//! sincronización de por medio. Un cambio guardado a mitad de una
//! transcripción se aplica a la siguiente, que es exactamente lo que se espera.

use crate::ajustes::Ajustes;
use crate::director::Mensaje;
use mithflow_core::{dictate_con, hardware, stt::Transcriber, Preferencias};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::time::Instant;

/// Lo que se le puede mandar al motor.
pub enum AlMotor {
    /// Audio mono de 16 kHz listo para transcribir.
    Audio(Vec<f32>),
    /// El usuario guardó Ajustes: hay preferencias nuevas que aplicar.
    ///
    /// Va en `Box` porque `Ajustes` es varias veces más grande que un `Vec`, y
    /// el enum entero mide lo que su variante mayor: sin el `Box`, cada buffer
    /// de audio que pasa por la cola arrastraría ese tamaño.
    Ajustes(Box<Ajustes>),
}

/// Arranca el motor y devuelve el extremo por donde mandarle trabajo.
///
/// Nunca falla acá: todo lo que pueda salir mal (el modelo no carga) viaja por
/// `al_director` como [`Mensaje::MotorListo`], que es quien sabe convertirlo en
/// un estado visible. Devolver un `Result` obligaría a quien llama a decidir
/// dos veces lo mismo.
pub fn lanzar(
    al_director: Sender<Mensaje>,
    modelo: PathBuf,
    historial: PathBuf,
    ajustes: Ajustes,
) -> Sender<AlMotor> {
    let (al_motor, cola) = mpsc::channel::<AlMotor>();

    let creado = std::thread::Builder::new()
        .name("mithflow-motor".into())
        .spawn(move || trabajar(cola, al_director, modelo, historial, ajustes));
    if let Err(e) = creado {
        eprintln!("no pude crear el hilo del motor: {e}");
    }
    al_motor
}

fn trabajar(
    cola: mpsc::Receiver<AlMotor>,
    al_director: Sender<Mensaje>,
    modelo: PathBuf,
    historial: PathBuf,
    ajustes: Ajustes,
) {
    let mut transcriber = match Transcriber::new(&modelo) {
        Ok(t) => t,
        Err(e) => {
            // Fin del hilo. El director pasa a `Error` y la app sigue viva: el
            // usuario todavía puede abrir Ajustes y descargar un modelo, que es
            // justo la forma de arreglar esto.
            let _ = al_director.send(Mensaje::MotorListo(Err(e)));
            return;
        }
    };
    // El vocabulario propio se aplica ANTES del calentamiento: así la primera
    // inferencia real ya usa el prompt definitivo.
    transcriber.fijar_vocabulario(&ajustes.vocabulario);
    let mut preferencias = ajustes.preferencias();

    let backend = transcriber.backend();
    let calentamiento = calentar(&mut transcriber);
    let _ = al_director.send(Mensaje::MotorListo(Ok(format!(
        "motor listo sobre {backend} ({calentamiento})"
    ))));

    // El `for` termina cuando el director suelta su extremo, o sea al cerrar.
    for trabajo in cola {
        match trabajo {
            AlMotor::Ajustes(nuevos) => {
                transcriber.fijar_vocabulario(&nuevos.vocabulario);
                preferencias = nuevos.preferencias();
            }
            AlMotor::Audio(audio) => {
                if dictar(&mut transcriber, &audio, &historial, &preferencias, &al_director)
                    .is_err()
                {
                    return; // el director se fue; no hay a quién contarle
                }
            }
        }
    }
}

/// Un dictado y su informe al director. `Err(())` significa que el director ya
/// no está escuchando.
fn dictar(
    transcriber: &mut Transcriber,
    audio: &[f32],
    historial: &std::path::Path,
    preferencias: &Preferencias,
    al_director: &Sender<Mensaje>,
) -> Result<(), ()> {
    let salida = dictate_con(transcriber, audio, historial, preferencias);
    // La entrada viene del propio pipeline, que es quien la escribió: releer el
    // archivo para buscarla sería O(n) por dictado y, con el texto sin guardar,
    // no habría por dónde reconocerla.
    let entrada = match &salida {
        Ok(Some(resultado)) => resultado.entrada.clone(),
        _ => None,
    };
    al_director
        .send(Mensaje::Transcripcion {
            salida: Box::new(salida),
            entrada,
        })
        .map_err(|_| ())
}

/// Paga por adelantado la compilación de los shaders de Vulkan.
///
/// **No puede usar silencio.** `Transcriber::transcribe` descarta el audio por
/// debajo de `MIN_SPEECH_RMS` ANTES de llamar al modelo, así que un buffer de
/// ceros volvería en microsegundos sin compilar un solo shader: el
/// calentamiento sería un no-op silencioso y el primer dictado del usuario se
/// colgaría 17 s. Por eso se reusa el clip de referencia del perfilado, que
/// está construido justo para estar por encima de esa compuerta.
///
/// Que falle no es fatal: lo único que se pierde es el adelanto.
fn calentar(transcriber: &mut Transcriber) -> String {
    let t0 = Instant::now();
    match transcriber.transcribe(&hardware::audio_de_referencia()) {
        Ok(_) => format!("calentado en {:.1} s", t0.elapsed().as_secs_f32()),
        Err(e) => format!("sin calentar: {e}; el primer dictado va a tardar más"),
    }
}
