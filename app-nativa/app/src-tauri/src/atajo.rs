//! El atajo global, con `rdev::grab` en un hilo propio.
//!
//! # Por qué `rdev` y no `tauri-plugin-global-shortcut`
//!
//! Porque hay que **suprimir** la tecla, no sólo enterarse de ella: si F9 llega
//! además a la aplicación enfocada, el cursor se mueve (o se pone un breakpoint,
//! que fue como se verificó — ver `DECISIONES.md`) y el texto termina pegado en
//! otro lado. `grab` devuelve `None` para descartar el evento; el plugin de
//! Tauri no ofrece esa vía.
//!
//! # Las tres restricciones que dan forma a este módulo
//!
//! 1. **`grab` bloquea para siempre.** Instala un hook de bajo nivel de Windows
//!    y se queda en su bucle de mensajes. Va en un hilo dedicado, y la única
//!    salida hacia el resto del programa es un canal.
//! 2. **El callback es `Fn`, no `FnMut`.** No se puede mutar nada capturado, así
//!    que todo el estado configurable vive en atómicos globales. El `Sender` sí
//!    se puede capturar por valor: `Sender::send` toma `&self`.
//! 3. **El hook corre en el camino crítico del teclado.** Si tarda más que
//!    `LowLevelHooksTimeout` (300 ms por defecto), Windows lo desengancha sin
//!    avisar y el atajo deja de funcionar. Por eso el callback sólo hace
//!    atómicos y un `send` que no bloquea; nada de I/O, logs ni locks.

use crate::director::Mensaje;
use rdev::{grab, Event, EventType, Key};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;

/// Teclas que se pueden elegir como atajo, con su nombre para los ajustes.
///
/// Sólo teclas que en la práctica no escriben nada: el atajo se SUPRIME, así
/// que permitir la "a" dejaría al usuario sin poder escribir esa letra.
///
/// **F9 es el default y no F8 a propósito**: la versión Python en producción usa
/// F8 y las dos conviven en esta máquina durante la transición.
const TECLAS: &[(&str, Key)] = &[
    ("F1", Key::F1),
    ("F2", Key::F2),
    ("F3", Key::F3),
    ("F4", Key::F4),
    ("F5", Key::F5),
    ("F6", Key::F6),
    ("F7", Key::F7),
    ("F8", Key::F8),
    ("F9", Key::F9),
    ("F10", Key::F10),
    ("F11", Key::F11),
    ("F12", Key::F12),
    ("Insert", Key::Insert),
    ("ScrollLock", Key::ScrollLock),
    ("Pause", Key::Pause),
];

/// Índice de F9 dentro de [`TECLAS`]. Se verifica en un test: si alguien
/// reordena la tabla, el default no se muda en silencio.
const INDICE_POR_DEFECTO: usize = 8;

pub const TECLA_POR_DEFECTO: &str = "F9";

/// La tecla configurada, como índice de [`TECLAS`]. Es un atómico porque el
/// callback de `grab` no puede capturar estado mutable, y se lee en cada evento
/// para que cambiar el atajo desde Ajustes tenga efecto sin reiniciar el hilo.
static TECLA: AtomicUsize = AtomicUsize::new(INDICE_POR_DEFECTO);

/// Con la pausa activa el hook sigue instalado pero deja pasar la tecla: es
/// "soltar el atajo sin cerrar la app", que es lo que pide el menú.
static PAUSADO: AtomicBool = AtomicBool::new(false);

/// ¿La tecla está apretada? Windows repite `KeyPress` mientras se mantiene
/// apretada una tecla; sin esta guarda, dejar el dedo sobre F9 medio segundo
/// dispararía decenas de arranques y paradas de grabación.
static ABAJO: AtomicBool = AtomicBool::new(false);

/// Los nombres de tecla admitidos, para que la interfaz ofrezca exactamente
/// éstos y no una lista escrita a mano que se desincronice.
pub fn teclas_admitidas() -> Vec<&'static str> {
    TECLAS.iter().map(|(nombre, _)| *nombre).collect()
}

/// ¿Es un nombre de tecla que este módulo sabe enganchar?
pub fn nombre_valido(nombre: &str) -> bool {
    indice_de(nombre).is_some()
}

fn indice_de(nombre: &str) -> Option<usize> {
    TECLAS
        .iter()
        .position(|(n, _)| n.eq_ignore_ascii_case(nombre))
}

/// Cambia la tecla del atajo. Un nombre desconocido no cambia nada y devuelve
/// `false`: fallar cerrado, con el atajo anterior todavía funcionando, es mejor
/// que quedarse sin atajo por un ajuste mal escrito.
pub fn fijar_tecla(nombre: &str) -> bool {
    match indice_de(nombre) {
        Some(i) => {
            TECLA.store(i, Ordering::Relaxed);
            // Si se cambió la tecla con la anterior apretada, el `KeyRelease`
            // de la vieja ya no va a coincidir y `ABAJO` quedaría trabado.
            ABAJO.store(false, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

/// El nombre de la tecla enganchada ahora mismo.
pub fn tecla_actual() -> &'static str {
    TECLAS[TECLA.load(Ordering::Relaxed).min(TECLAS.len() - 1)].0
}

/// Activa o suelta el atajo. Al pausar se limpia `ABAJO` para que la tecla no
/// quede "apretada" si la pausa llegó entre el press y el release.
pub fn fijar_pausa(pausado: bool) {
    PAUSADO.store(pausado, Ordering::SeqCst);
    if pausado {
        ABAJO.store(false, Ordering::SeqCst);
    }
}

/// Arranca el hilo del atajo. No vuelve nunca de `grab` salvo error, y ese
/// error viaja por el canal: sin atajo la aplicación no sirve para dictar, pero
/// tampoco tiene por qué cerrarse — el usuario puede querer ver el dashboard.
pub fn lanzar(al_director: Sender<Mensaje>) {
    std::thread::Builder::new()
        .name("mithflow-atajo".into())
        .spawn(move || {
            let avisos = al_director.clone();
            if let Err(e) = grab(move |evento| filtrar(evento, &al_director)) {
                let _ = avisos.send(Mensaje::AtajoRoto(format!(
                    "no pude enganchar el teclado ({e:?}). \
                     Otra aplicación puede estar usando el mismo atajo."
                )));
            }
        })
        // Si no hay hilos disponibles el sistema está en un estado en el que
        // nada más va a funcionar tampoco.
        .expect("no pude crear el hilo del atajo");
}

/// El callback del hook. Devolver `None` descarta el evento (lo suprime);
/// devolver `Some` lo deja seguir su camino hacia la aplicación enfocada.
fn filtrar(evento: Event, al_director: &Sender<Mensaje>) -> Option<Event> {
    let tecla = TECLAS[TECLA.load(Ordering::Relaxed).min(TECLAS.len() - 1)].1;
    match evento.event_type {
        EventType::KeyPress(k) if k == tecla => {
            // `ABAJO` se actualiza SIEMPRE, también estando en pausa: si no, un
            // press durante la pausa dejaría el flag desalineado con la
            // realidad del teclado.
            let repetida = ABAJO.swap(true, Ordering::SeqCst);
            if PAUSADO.load(Ordering::SeqCst) {
                return Some(evento);
            }
            if !repetida {
                // Si el director murió no hay nada que hacer con el error acá:
                // este hilo no puede informar por su cuenta.
                let _ = al_director.send(Mensaje::Pulso);
            }
            None
        }
        EventType::KeyRelease(k) if k == tecla => {
            ABAJO.store(false, Ordering::SeqCst);
            if PAUSADO.load(Ordering::SeqCst) {
                return Some(evento);
            }
            None
        }
        _ => Some(evento),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Los tests de este módulo comparten los atómicos globales, así que se
    /// serializan a mano: `cargo test` corre cada `#[test]` en su propio hilo.
    static CANDADO: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn el_default_es_f9_y_no_f8() {
        let _g = CANDADO.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(TECLAS[INDICE_POR_DEFECTO].0, TECLA_POR_DEFECTO);
        assert_eq!(TECLA_POR_DEFECTO, "F9");
        assert_eq!(TECLAS[INDICE_POR_DEFECTO].1, Key::F9);
    }

    #[test]
    fn cambiar_la_tecla_acepta_conocidas_y_rechaza_el_resto() {
        let _g = CANDADO.lock().unwrap_or_else(|e| e.into_inner());
        assert!(fijar_tecla("F7"));
        assert_eq!(tecla_actual(), "F7");
        // Insensible a mayúsculas, pero conserva el nombre canónico.
        assert!(fijar_tecla("scrolllock"));
        assert_eq!(tecla_actual(), "ScrollLock");

        assert!(!fijar_tecla("Ctrl+Shift+Q"));
        assert_eq!(tecla_actual(), "ScrollLock", "un nombre inválido no cambia nada");

        fijar_tecla(TECLA_POR_DEFECTO);
    }

    #[test]
    fn no_hay_nombres_repetidos_en_la_tabla() {
        let mut nombres: Vec<&str> = teclas_admitidas();
        let total = nombres.len();
        nombres.sort_unstable();
        nombres.dedup();
        assert_eq!(nombres.len(), total);
    }

    #[test]
    fn la_pausa_destraba_la_tecla_apretada() {
        let _g = CANDADO.lock().unwrap_or_else(|e| e.into_inner());
        ABAJO.store(true, Ordering::SeqCst);
        fijar_pausa(true);
        assert!(PAUSADO.load(Ordering::SeqCst));
        assert!(!ABAJO.load(Ordering::SeqCst), "pausar tiene que soltar la tecla");
        fijar_pausa(false);
        assert!(!PAUSADO.load(Ordering::SeqCst));
    }

    /// La repetición automática de Windows manda muchos `KeyPress` seguidos:
    /// sólo el primero puede convertirse en un pulso.
    #[test]
    fn la_repeticion_automatica_manda_un_solo_pulso() {
        let _g = CANDADO.lock().unwrap_or_else(|e| e.into_inner());
        fijar_pausa(false);
        fijar_tecla(TECLA_POR_DEFECTO);
        let (tx, rx) = std::sync::mpsc::channel();

        let press = evento(EventType::KeyPress(Key::F9));
        for _ in 0..5 {
            assert!(filtrar(press.clone(), &tx).is_none(), "la tecla se suprime");
        }
        assert!(filtrar(evento(EventType::KeyRelease(Key::F9)), &tx).is_none());
        assert!(filtrar(press.clone(), &tx).is_none());

        let pulsos = rx.try_iter().count();
        assert_eq!(pulsos, 2, "un pulso por pulsación real, no por repetición");
    }

    #[test]
    fn en_pausa_la_tecla_pasa_de_largo_y_no_manda_pulsos() {
        let _g = CANDADO.lock().unwrap_or_else(|e| e.into_inner());
        fijar_tecla(TECLA_POR_DEFECTO);
        fijar_pausa(true);
        let (tx, rx) = std::sync::mpsc::channel();

        assert!(
            filtrar(evento(EventType::KeyPress(Key::F9)), &tx).is_some(),
            "pausado, F9 tiene que llegar a la aplicación enfocada"
        );
        assert!(filtrar(evento(EventType::KeyRelease(Key::F9)), &tx).is_some());
        assert_eq!(rx.try_iter().count(), 0);

        fijar_pausa(false);
    }

    #[test]
    fn las_teclas_ajenas_pasan_siempre() {
        let _g = CANDADO.lock().unwrap_or_else(|e| e.into_inner());
        fijar_pausa(false);
        fijar_tecla(TECLA_POR_DEFECTO);
        let (tx, rx) = std::sync::mpsc::channel();

        // F8 es la de la versión Python: NO se puede tocar mientras conviven.
        assert!(filtrar(evento(EventType::KeyPress(Key::F8)), &tx).is_some());
        assert!(filtrar(evento(EventType::KeyPress(Key::KeyA)), &tx).is_some());
        assert_eq!(rx.try_iter().count(), 0);
    }

    fn evento(tipo: EventType) -> Event {
        Event {
            time: std::time::SystemTime::now(),
            name: None,
            event_type: tipo,
        }
    }
}
