//! ¿La ventanita de grabación le puede robar el foco al usuario?
//!
//! Es la única restricción innegociable de `app/src-tauri/src/superpuesta.rs`:
//! MithFlow pega texto en la ventana que el usuario tenga enfocada, así que un
//! indicador que se lleve el foco pega el dictado en otro lado. Este spike la
//! verifica **de verdad, sobre Windows**, sin arrancar MithFlow.
//!
//! Crea una ventana con exactamente las mismas opciones que la de producción
//! —las de `tao`, que es lo que Tauri usa por debajo— y comprueba tres cosas:
//!
//! 1. Que el estilo extendido tenga `WS_EX_NOACTIVATE` (la que garantiza la
//!    invariante) y `WS_EX_TRANSPARENT` (los clics pasan de largo).
//! 2. Que **mostrarla no cambie la ventana de primer plano**, ni el foco de
//!    teclado del hilo que la tenía. Ése es el efecto que movería el cursor de
//!    texto del usuario.
//! 3. Que **mostrarla por segunda vez tampoco**. Es el caso que se escapa: `tao`
//!    usa `SW_SHOWNOACTIVATE` sólo en el primer `show` (consume la marca
//!    `MARKER_DONT_FOCUS`) y a partir del segundo usa `SW_SHOW`, que *activa*.
//!    Sin `WS_EX_NOACTIVATE`, la segunda grabación robaría el foco.
//!
//! Correr con el foco en OTRA aplicación (una ventana de texto, idealmente):
//!
//! ```text
//! cargo run --release --manifest-path spike-superpuesta/Cargo.toml
//! ```
//!
//! Sale con código 0 si la garantía se cumple y 1 si no.

use std::time::{Duration, Instant};
use tao::dpi::LogicalSize;
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::WindowBuilder;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetGUIThreadInfo, GetWindowLongPtrW, GetWindowTextW,
    GetWindowThreadProcessId, GUITHREADINFO, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
};

/// Cuánto se le da a Windows para procesar el cambio antes de mirar el foco.
const RESPIRO: Duration = Duration::from_millis(400);

fn titulo(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let largo = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..largo.max(0) as usize])
}

/// Quién tiene el foco de teclado, visto desde afuera del proceso.
///
/// `GetFocus` sólo contesta sobre la cola de mensajes propia, así que para saber
/// dónde está el cursor de texto de OTRA aplicación hay que preguntarle a su
/// hilo con `GetGUIThreadInfo`. Es literalmente el dato que este spike vino a
/// mirar: `hwndCaret` es el control con el cursor parpadeando.
fn foco_del_hilo(hwnd: HWND) -> (isize, isize) {
    let hilo = unsafe { GetWindowThreadProcessId(hwnd, None) };
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(hilo, &mut info) }.is_ok() {
        (info.hwndFocus.0 as isize, info.hwndCaret.0 as isize)
    } else {
        (0, 0)
    }
}

struct Comprobacion {
    que: &'static str,
    bien: bool,
    detalle: String,
}

fn main() {
    let antes = unsafe { GetForegroundWindow() };
    let foco_antes = foco_del_hilo(antes);
    println!("ventana enfocada al arrancar: «{}» ({:?})", titulo(antes), antes.0);
    println!("foco de teclado / cursor de texto de ese hilo: {foco_antes:?}\n");

    let mut evento = EventLoop::new();
    // EXACTAMENTE las mismas opciones que `superpuesta::crear`, que es lo que
    // Tauri traduce a esta capa. Copiarlas de memoria no serviría: lo que se
    // está verificando es esta combinación y ninguna otra.
    let ventana = WindowBuilder::new()
        .with_title("MithFlow — grabando (spike)")
        .with_inner_size(LogicalSize::new(232.0, 64.0))
        .with_resizable(false)
        .with_maximizable(false)
        .with_minimizable(false)
        .with_closable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_undecorated_shadow(false)
        .with_always_on_top(true)
        .with_skip_taskbar(true)
        .with_focusable(false)
        .with_focused(false)
        .with_visible(false)
        .build(&evento)
        .expect("no pude crear la ventana del spike");
    ventana.set_ignore_cursor_events(true).ok();

    let hwnd = HWND(ventana.hwnd() as *mut _);
    let mut resultados = Vec::new();

    // 1. Los estilos, que es de dónde sale la garantía.
    let estilo = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    resultados.push(Comprobacion {
        que: "WS_EX_NOACTIVATE en el estilo extendido",
        bien: estilo & WS_EX_NOACTIVATE.0 != 0,
        detalle: format!("GWL_EXSTYLE = 0x{estilo:08x}"),
    });
    resultados.push(Comprobacion {
        que: "WS_EX_TRANSPARENT (los clics la atraviesan)",
        bien: estilo & WS_EX_TRANSPARENT.0 != 0,
        detalle: format!("GWL_EXSTYLE = 0x{estilo:08x}"),
    });

    // 2 y 3. Mostrar, esconder y volver a mostrar: el segundo `show` es el que
    // usa `SW_SHOW` y el que se escaparía sin `WS_EX_NOACTIVATE`.
    let mut pasos: Vec<(&'static str, bool)> = Vec::new();
    for (nombre, visible) in [
        ("primer show", true),
        ("hide", false),
        ("SEGUNDO show (el que usa SW_SHOW)", true),
    ] {
        ventana.set_visible(visible);
        bombear(&mut evento, RESPIRO);
        let ahora = unsafe { GetForegroundWindow() };
        let foco_ahora = foco_del_hilo(antes);
        let intacto = ahora == antes && foco_ahora == foco_antes;
        if !intacto {
            println!(
                "  ✗ tras «{nombre}»: primer plano «{}» ({:?}), foco {foco_ahora:?}",
                titulo(ahora),
                ahora.0
            );
        }
        pasos.push((nombre, intacto));
    }
    for (nombre, bien) in pasos {
        resultados.push(Comprobacion {
            que: Box::leak(format!("el foco no se movió tras «{nombre}»").into_boxed_str()),
            bien,
            detalle: String::new(),
        });
    }

    // 4. Y la propia ventana nunca se cree enfocada.
    resultados.push(Comprobacion {
        que: "la ventanita no tiene el foco de teclado",
        bien: unsafe { GetFocus() } != hwnd,
        detalle: String::new(),
    });

    ventana.set_visible(false);
    drop(ventana);

    println!();
    let mut todo_bien = true;
    for r in &resultados {
        println!(
            "{} {}{}",
            if r.bien { "PASA" } else { "FALLA" },
            r.que,
            if r.detalle.is_empty() {
                String::new()
            } else {
                format!("  ({})", r.detalle)
            }
        );
        todo_bien &= r.bien;
    }
    println!(
        "\n{}",
        if todo_bien {
            "OK: la ventanita no le puede robar el foco al usuario."
        } else {
            "ROTO: la ventanita puede robar el foco. NO se puede shippear así."
        }
    );
    std::process::exit(if todo_bien { 0 } else { 1 });
}

/// Corre el bucle de mensajes de `tao` durante `cuanto` y vuelve.
///
/// Hace falta porque `set_visible` de `tao` no toca la ventana en el acto:
/// encola el trabajo para el hilo del bucle. Sin bombear, se estaría mirando el
/// foco antes de que la ventana se haya mostrado — o sea, un PASA que no
/// significa nada.
fn bombear(evento: &mut EventLoop<()>, cuanto: Duration) {
    use tao::platform::run_return::EventLoopExtRunReturn;
    let hasta = Instant::now() + cuanto;
    evento.run_return(|_, _, control| {
        *control = if Instant::now() >= hasta {
            ControlFlow::Exit
        } else {
            ControlFlow::WaitUntil(hasta)
        };
    });
}
