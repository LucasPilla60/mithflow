//! Banco de pruebas del núcleo: graba con Enter, transcribe y pega.
//!
//! No es la aplicación final (esa lleva atajo global y bandeja): es la forma
//! más chica de ejercitar el pipeline completo a mano.
//!
//! Dos reglas gobiernan este binario:
//!
//! 1. **Sólo se sale si no carga el modelo.** Cualquier otro fallo —micrófono,
//!    transcripción, pegado— se informa y se vuelve al bucle. Nada de
//!    `.expect()` sobre errores de tiempo de ejecución.
//! 2. **Historial propio.** Mientras la versión Python siga en producción,
//!    las dos conviven y NO deben escribir el mismo archivo.

use mithflow_core::{audio::Recorder, config, dictate, stt::Transcriber, DictationResult};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

const MODELO: &str = "models/whisper-large-v3-turbo-F16.gguf";

/// Deliberadamente distinto de `history.jsonl`, que es el de `mithflow.py`.
const HISTORIAL: &str = "history-nativo.jsonl";

/// Duración del calentamiento. Whisper rellena hasta 30 s por dentro, así que
/// alargarlo no compila más shaders: sólo tarda más.
const CALENTAMIENTO_SEGS: f32 = 0.5;

/// Rutas relativas a `app-nativa/`, dos niveles arriba de este crate.
/// El empaquetado (Plan 5) las va a resolver contra el ejecutable.
fn ruta(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

fn main() {
    println!("MithFlow nativo — dictado por voz local");

    let modelo = ruta(MODELO);
    println!("Cargando el modelo desde {}...", modelo.display());
    let mut transcriber = match Transcriber::new(&modelo) {
        Ok(t) => t,
        // El único fallo fatal: sin modelo no hay nada que hacer.
        Err(e) => {
            eprintln!("No pude cargar el modelo: {e}");
            std::process::exit(1);
        }
    };

    calentar(&mut transcriber);
    bucle(&mut transcriber, &ruta(HISTORIAL));
    println!("Listo, hasta luego.");
}

/// Paga por adelantado la compilación de shaders de Vulkan (16,8 s medidos en
/// la primera inferencia del proceso; después el caché de la placa la elimina).
///
/// Sin esto el primer dictado del usuario se cuelga 17 s y parece roto.
fn calentar(transcriber: &mut Transcriber) {
    print!("Preparando el motor (compila los shaders, la primera vez tarda)... ");
    vaciar_salida();
    let t0 = Instant::now();
    match transcriber.transcribe(&buffer_de_calentamiento()) {
        Ok(_) => println!("listo en {:.1} s.", t0.elapsed().as_secs_f32()),
        // No es fatal: el modelo cargó bien y lo único que se pierde es el
        // adelanto. El primer dictado real pagará la compilación.
        Err(e) => println!("no se pudo ({e}); el primer dictado va a tardar más."),
    }
}

/// Tono corto y muy bajo, apenas por encima de la compuerta de energía.
///
/// NO puede ser silencio digital: `Transcriber::transcribe` descarta el audio
/// por debajo de `MIN_SPEECH_RMS` ANTES de llamar al modelo, así que un buffer
/// de ceros volvería en microsegundos sin compilar un solo shader — que es
/// justo lo que este calentamiento viene a pagar. Una senoide de amplitud 0,05
/// tiene RMS 0,035, tres veces y media el umbral.
fn buffer_de_calentamiento() -> Vec<f32> {
    let frecuencia_muestreo = config::SAMPLE_RATE as f32;
    let muestras = (frecuencia_muestreo * CALENTAMIENTO_SEGS) as usize;
    (0..muestras)
        .map(|i| {
            let fase = 2.0 * std::f32::consts::PI * 220.0 * i as f32 / frecuencia_muestreo;
            fase.sin() * 0.05
        })
        .collect()
}

fn bucle(transcriber: &mut Transcriber, historial: &Path) {
    loop {
        print!("\nEnter para grabar, 'q' para salir: ");
        vaciar_salida();
        match leer_linea() {
            // Entrada cerrada (EOF): sin esto el bucle giraría para siempre.
            None => return,
            Some(linea) if linea.eq_ignore_ascii_case("q") => return,
            Some(_) => {}
        }
        match grabar() {
            Ok(audio) => procesar(transcriber, &audio, historial),
            Err(e) => eprintln!("No pude grabar: {e}"),
        }
    }
}

/// Graba hasta el siguiente Enter y devuelve el audio listo para el modelo.
fn grabar() -> Result<Vec<f32>, String> {
    let mut recorder = Recorder::new()?;
    recorder.start()?;
    print!("Grabando... Enter para cortar: ");
    vaciar_salida();
    // Se ignora el resultado a propósito: aunque se cierre la entrada hay que
    // cerrar el micrófono y procesar lo que ya se capturó.
    let _ = leer_linea();
    recorder.stop()
}

fn procesar(transcriber: &mut Transcriber, audio: &[f32], historial: &Path) {
    let segundos = audio.len() as f32 / config::SAMPLE_RATE as f32;
    println!("Transcribiendo {segundos:.1} s de audio...");
    match dictate(transcriber, audio, historial) {
        Ok(Some(resultado)) => mostrar(&resultado),
        Ok(None) => println!("Nada que dictar (audio corto, silencio o ruido)."),
        Err(e) => eprintln!("El dictado falló: {e}"),
    }
}

fn mostrar(resultado: &DictationResult) {
    println!("  crudo:  {}", resultado.raw);
    println!("  limpio: {}", resultado.final_text);
    println!(
        "  {:.1} s de audio · transcripción {:.2} s · limpieza {:.3} s",
        resultado.audio_s, resultado.transcribe_s, resultado.cleanup_s
    );
}

/// Lee una línea sin su salto. `None` si se cerró la entrada o falló la lectura.
///
/// Se saca el BOM además de los espacios: `trim` NO lo considera espacio
/// (U+FEFF no tiene la propiedad `White_Space`), y PowerShell lo antepone al
/// redirigir la entrada de un ejecutable. Sin esto, `"q" | mithflow-cli.exe`
/// no reconoce la 'q'. Mismo problema que ya había mordido en `history::load`.
fn leer_linea() -> Option<String> {
    let mut linea = String::new();
    match std::io::stdin().read_line(&mut linea) {
        Ok(0) => None,
        Ok(_) => Some(linea.trim_start_matches('\u{feff}').trim().to_string()),
        Err(e) => {
            eprintln!("No pude leer la entrada: {e}");
            None
        }
    }
}

/// Los `print!` sin salto quedan en el buffer hasta el próximo `\n`: sin esto,
/// el usuario espera mirando una pantalla en blanco.
fn vaciar_salida() {
    let _ = std::io::stdout().flush();
}
