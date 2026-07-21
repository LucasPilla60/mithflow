//! Pegado del texto dictado en la ventana con foco.
//!
//! Dos invariantes que vienen de la versión Python y no se negocian:
//!
//! 1. Si el Ctrl+V falla, el texto dictado QUEDA en el portapapeles. Restaurar
//!    el contenido previo perdería la transcripción, que es lo único que el
//!    usuario no puede recuperar (ver `tests/test_paste.py`).
//! 2. No se espera al portapapeles con `sleep` de duración fija. La versión
//!    Python gastaba 0,45 s en dos esperas ciegas (62 % de la latencia
//!    percibida); acá se espera a que el portapapeles CONFIRME el cambio.

use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::{
    thread::sleep,
    time::{Duration, Instant},
};

/// Tope de espera a que el portapapeles confirme el `set_text`.
const TOPE_CONFIRMACION: Duration = Duration::from_millis(300);

/// Margen para que la app destino procese el Ctrl+V antes de restaurar el
/// portapapeles previo. Sigue siendo fijo porque no hay señal del sistema que
/// avise "ya pegué": es la única espera ciega que queda, y es 120 ms contra
/// los 450 ms de Python.
const MARGEN_PROCESADO: Duration = Duration::from_millis(120);

/// Intervalo de sondeo del número de secuencia del portapapeles.
const SONDEO: Duration = Duration::from_millis(5);

#[cfg(windows)]
fn secuencia_portapapeles() -> u32 {
    // Contador global de Windows: se incrementa cada vez que CUALQUIER proceso
    // modifica el portapapeles. Es la señal de "ya está escrito" que reemplaza
    // al sleep fijo.
    unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

#[cfg(not(windows))]
fn secuencia_portapapeles() -> u32 {
    0
}

/// Espera a que el portapapeles refleje el cambio, con tope. Sustituye al
/// `sleep(150 ms)` fijo de la versión Python.
///
/// Al agotarse el tope retorna igual: es preferible intentar el pegado con un
/// portapapeles quizá desactualizado que colgar el dictado indefinidamente.
fn esperar_portapapeles(anterior: u32, tope: Duration) {
    let t0 = Instant::now();
    while t0.elapsed() < tope {
        if secuencia_portapapeles() != anterior {
            return;
        }
        sleep(SONDEO);
    }
}

/// Pega el texto donde esté el cursor, preservando el portapapeles previo.
///
/// Si el pegado falla, el texto queda en el portapapeles a propósito: es
/// preferible que el usuario pegue a mano antes que perder lo que dictó.
pub fn paste(text: &str) -> Result<(), String> {
    let mut clip = arboard::Clipboard::new().map_err(|e| format!("sin portapapeles: {e}"))?;
    // `ok()` y no `?`: un portapapeles vacío o con una imagen no es un error,
    // solo significa que después no hay nada de texto que restaurar.
    let anterior_texto = clip.get_text().ok();
    let seq = secuencia_portapapeles();

    clip.set_text(text).map_err(|e| format!("no pude copiar: {e}"))?;
    esperar_portapapeles(seq, TOPE_CONFIRMACION);

    match enviar_ctrl_v() {
        Ok(()) => {
            sleep(MARGEN_PROCESADO);
            // Restaurar es best-effort: su fallo no invalida el dictado, que ya
            // llegó a destino.
            if let Some(prev) = anterior_texto {
                let _ = clip.set_text(prev);
            }
            Ok(())
        }
        // Sin restauración a propósito: el dictado se queda en el portapapeles.
        Err(e) => Err(format!("{e} — el texto quedó en el portapapeles")),
    }
}

fn enviar_ctrl_v() -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo
        .key(Key::Control, Direction::Press)
        .map_err(|e| e.to_string())?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| e.to_string())?;
    enigo
        .key(Key::Control, Direction::Release)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Si la secuencia ya cambió, no espera nada: el tope de 5 s no se toca.
    #[test]
    fn esperar_portapapeles_retorna_ya_si_la_secuencia_cambio() {
        let distinta = secuencia_portapapeles().wrapping_add(1);
        let t0 = Instant::now();
        esperar_portapapeles(distinta, Duration::from_secs(5));
        let transcurrido = t0.elapsed();
        assert!(
            transcurrido < Duration::from_millis(100),
            "debía retornar de inmediato, tardó {transcurrido:?}"
        );
    }

    /// Si la secuencia nunca cambia, respeta el tope y NO se cuelga.
    #[test]
    fn esperar_portapapeles_respeta_el_tope_si_no_cambia_nunca() {
        let tope = Duration::from_millis(150);
        let antes = secuencia_portapapeles();
        let t0 = Instant::now();
        esperar_portapapeles(antes, tope);
        let transcurrido = t0.elapsed();

        // Cota superior: la propiedad que importa es que termina.
        assert!(
            transcurrido < tope * 3,
            "se colgó o se pasó del tope: {transcurrido:?} para un tope de {tope:?}"
        );
        // Cota inferior: solo si nadie más tocó el portapapeles mientras corría
        // el test. El contador es global al sistema, así que otro proceso
        // copiando algo es una salida temprana LEGÍTIMA, no una falla.
        if secuencia_portapapeles() == antes {
            assert!(
                transcurrido >= tope,
                "cortó antes del tope: {transcurrido:?} < {tope:?}"
            );
        }
    }

    /// Ida y vuelta por `arboard`. `#[ignore]`: pisa el portapapeles real de la
    /// máquina, no queremos hacerlo en cada `cargo test`.
    #[test]
    #[ignore = "toca el portapapeles real del usuario"]
    fn arboard_escribe_y_lee_el_mismo_texto() {
        let mut clip = arboard::Clipboard::new().expect("no hay portapapeles disponible");
        let previo = clip.get_text().ok();

        let escrito = "MithFlow: prueba de ida y vuelta ñáéí";
        clip.set_text(escrito).expect("no pude escribir");
        esperar_portapapeles(secuencia_portapapeles(), TOPE_CONFIRMACION);
        let leido = clip.get_text().expect("no pude leer");

        if let Some(prev) = previo {
            let _ = clip.set_text(prev);
        }
        assert_eq!(leido, escrito);
    }

    /// Mide `paste()` de punta a punta contra el portapapeles real, N=10.
    ///
    /// `#[ignore]` porque ENVÍA Ctrl+V DE VERDAD: pega el texto de prueba en
    /// la ventana que tenga el foco en ese momento. Ejecutar a conciencia con
    /// un bloc de notas en primer plano:
    /// `cargo test -p mithflow-core --lib bench -- --ignored --nocapture`
    ///
    /// Mide TIEMPO, no entrega: `paste()` no puede saber si la app destino
    /// aceptó el Ctrl+V, así que este test pasa igual aunque no haya foco.
    /// Que el texto llegue se verifica a mano.
    ///
    /// Referencia: la versión Python gasta 0,460 s solo en esperas fijas.
    #[test]
    #[ignore = "envía Ctrl+V real a la ventana con foco"]
    fn bench_paste_punta_a_punta() {
        const N: usize = 10;
        let mut muestras = Vec::with_capacity(N);

        for i in 0..N {
            let t0 = Instant::now();
            paste(&format!("MithFlow bench {i}")).expect("paste falló");
            muestras.push(t0.elapsed());
        }

        muestras.sort_unstable();
        let mediana = (muestras[N / 2 - 1] + muestras[N / 2]) / 2;
        println!("paste() N={N}");
        println!("  min     {:?}", muestras[0]);
        println!("  mediana {mediana:?}");
        println!("  max     {:?}", muestras[N - 1]);
        println!("  (Python: 0.460 s solo de esperas fijas)");
    }
}
