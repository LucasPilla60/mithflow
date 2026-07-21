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

use crate::director::Mensaje;
use mithflow_core::{dictate, hardware, history, stt::Transcriber};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::time::Instant;

/// Arranca el motor y devuelve el extremo por donde mandarle audio.
///
/// Nunca falla acá: todo lo que pueda salir mal (el modelo no carga) viaja por
/// `al_director` como [`Mensaje::MotorListo`], que es quien sabe convertirlo en
/// un estado visible. Devolver un `Result` obligaría a quien llama a decidir
/// dos veces lo mismo.
pub fn lanzar(
    al_director: Sender<Mensaje>,
    modelo: PathBuf,
    historial: PathBuf,
) -> Sender<Vec<f32>> {
    let (al_motor, cola) = mpsc::channel::<Vec<f32>>();

    let creado = std::thread::Builder::new()
        .name("mithflow-motor".into())
        .spawn(move || trabajar(cola, al_director, modelo, historial));
    if let Err(e) = creado {
        eprintln!("no pude crear el hilo del motor: {e}");
    }
    al_motor
}

fn trabajar(
    cola: mpsc::Receiver<Vec<f32>>,
    al_director: Sender<Mensaje>,
    modelo: PathBuf,
    historial: PathBuf,
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

    let backend = transcriber.backend();
    let calentamiento = calentar(&mut transcriber);
    let _ = al_director.send(Mensaje::MotorListo(Ok(format!(
        "motor listo sobre {backend} ({calentamiento})"
    ))));

    // El `for` termina cuando el director suelta su extremo, o sea al cerrar.
    for audio in cola {
        let salida = dictate(&mut transcriber, &audio, &historial);
        let entrada = match &salida {
            // Se relee del archivo en vez de reconstruirla: así lo que ve el
            // dashboard es exactamente lo que quedó persistido, incluida la
            // marca de tiempo que puso `dictate`. La comparación del texto
            // evita mandar una entrada vieja si el guardado falló.
            Ok(Some(resultado)) => ultima_entrada(&historial)
                .filter(|e| e.final_text == resultado.final_text),
            _ => None,
        };
        if al_director.send(Mensaje::Transcripcion { salida, entrada }).is_err() {
            return; // el director se fue; no hay a quién contarle
        }
    }
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

/// La última entrada del historial, o `None` si no se pudo leer.
///
/// Un fallo acá no puede romper nada: el dashboard igual carga el historial
/// entero al montarse, así que lo peor que pasa es que una fila aparezca recién
/// al refrescar.
fn ultima_entrada(historial: &std::path::Path) -> Option<history::Entry> {
    match history::load(historial) {
        Ok(entradas) => entradas.into_iter().next_back(),
        Err(e) => {
            eprintln!("no pude releer el historial {}: {e}", historial.display());
            None
        }
    }
}
