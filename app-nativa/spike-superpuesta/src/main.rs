//! ¿La ventanita de grabación le puede robar el foco al usuario?
//! ¿Y si además se la puede agarrar con el mouse para moverla?
//!
//! Es la única restricción innegociable de `app/src-tauri/src/superpuesta.rs`:
//! MithFlow pega texto en la ventana que el usuario tenga enfocada, así que un
//! indicador que se lleve el foco pega el dictado en otro lado. Este spike la
//! verifica **de verdad, sobre Windows**, sin arrancar MithFlow.
//!
//! # Las dos propiedades que la gente confunde
//!
//! | Estilo | De dónde sale | Qué hace |
//! |---|---|---|
//! | `WS_EX_NOACTIVATE` | `focusable(false)` | clickearla **no la activa ni le da el foco** |
//! | `WS_EX_TRANSPARENT` | `set_ignore_cursor_events(true)` | los clics **la atraviesan**: la ventana ni se entera |
//!
//! Son cosas distintas, y la segunda es la que impide agarrarla para moverla.
//! La pregunta que este spike contesta es si se puede sacar `WS_EX_TRANSPARENT`
//! —para poder arrastrarla— **sin** que un clic mueva el cursor de texto del
//! usuario. La respuesta no se puede razonar: se mide.
//!
//! # Cómo se mide
//!
//! Tres ventanas con la misma pila que producción (`tao`, que es lo que Tauri
//! usa por debajo), y sobre cada una un **clic sintético real** (`SendInput`,
//! no un mensaje inyectado: los mensajes posteados no pasan por la activación
//! que hace el sistema, así que probarían otra cosa).
//!
//! | Configuración | `NOACTIVATE` | `TRANSPARENT` | Qué se espera |
//! |---|---|---|---|
//! | A — la de hoy | sí | sí | el clic la atraviesa; el foco no se mueve; **no se puede agarrar** |
//! | B — la propuesta | sí | no | el clic llega a la ventana; el foco **no** se mueve → se puede arrastrar |
//! | C — testigo | no | no | el clic llega **y se lleva el foco** |
//!
//! **La C no es decorativa**: sin ella, un "el foco no se movió" podría
//! significar simplemente que el clic nunca ocurrió. La C prueba que el aparato
//! de medición detecta un robo de foco cuando lo hay.
//!
//! # Por qué hay una ventana "blanco"
//!
//! Con `WS_EX_TRANSPARENT` el clic **atraviesa** la ventanita y aterriza en lo
//! que haya debajo — que en esta máquina es la aplicación del usuario. Clickear
//! a ciegas ahí sería apretarle un botón cualquiera. Así que debajo de las tres
//! se pone una ventana propia, opaca y también `NOACTIVATE`: el clic que pasa
//! de largo cae ahí. De paso es la medición **directa** de que pasó de largo.
//!
//! Correr con el foco en OTRA aplicación (una ventana de texto, idealmente):
//!
//! ```text
//! cargo run --release --manifest-path spike-superpuesta/Cargo.toml
//! ```
//!
//! Sale con código 0 si las tres configuraciones se comportan como se espera y
//! 1 si alguna no.

use std::time::{Duration, Instant};
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{ElementState, Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder, WindowId};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEINPUT, MOUSE_EVENT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetWindowLongPtrW, GetWindowTextW,
    GetWindowThreadProcessId, SetCursorPos, SetForegroundWindow, GUITHREADINFO, GWL_EXSTYLE,
    WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
};

/// Cuánto se le da a Windows para procesar cada cambio antes de mirar el foco.
const RESPIRO: Duration = Duration::from_millis(400);

/// El tamaño real de la ventanita de producción, en píxeles lógicos
/// (`superpuesta::ANCHO`/`ALTO`). El tamaño no cambia el comportamiento del foco
/// que este spike mide, pero se mantiene alineado para no confundir.
const ANCHO: f64 = 168.0;
const ALTO: f64 = 48.0;

// ---------------------------------------------------------------- Win32 crudo

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

fn estilo_extendido(hwnd: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
}

fn donde_esta_el_mouse() -> POINT {
    let mut punto = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut punto);
    }
    punto
}

fn boton(flags: MOUSE_EVENT_FLAGS) {
    let entrada = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(&[entrada], std::mem::size_of::<INPUT>() as i32) };
}

/// Un clic de verdad —del que pasa por la activación del sistema— en un punto
/// de la pantalla.
///
/// `SendInput` y no `PostMessage(WM_LBUTTONDOWN)`: un mensaje posteado se saltea
/// el `WM_MOUSEACTIVATE` que es justamente el que decide si la ventana se
/// activa. Probar con mensajes inyectados daría un PASA que no significa nada.
fn clic_en(x: i32, y: i32) {
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    std::thread::sleep(Duration::from_millis(40));
    boton(MOUSEEVENTF_LEFTDOWN);
    std::thread::sleep(Duration::from_millis(40));
    boton(MOUSEEVENTF_LEFTUP);
}

// -------------------------------------------------------------- el experimento

/// Qué se le pide a cada ventana de prueba.
struct Caso {
    nombre: &'static str,
    /// `focusable(false)` → `WS_EX_NOACTIVATE`.
    noactivate: bool,
    /// `set_ignore_cursor_events(true)` → `WS_EX_TRANSPARENT`.
    transparente: bool,
    /// Lo que tiene que pasar para que el caso se considere entendido.
    espera_clic_en_la_ventana: bool,
    espera_foco_intacto: bool,
}

const CASOS: &[Caso] = &[
    Caso {
        nombre: "A — la de hoy (NOACTIVATE + TRANSPARENT)",
        noactivate: true,
        transparente: true,
        espera_clic_en_la_ventana: false,
        espera_foco_intacto: true,
    },
    Caso {
        nombre: "B — la propuesta (NOACTIVATE, sin TRANSPARENT)",
        noactivate: true,
        transparente: false,
        espera_clic_en_la_ventana: true,
        espera_foco_intacto: true,
    },
    // Última a propósito: es la única que toca el foco del usuario, y al
    // terminar se lo devuelve.
    Caso {
        nombre: "C — testigo (sin NOACTIVATE, sin TRANSPARENT)",
        noactivate: false,
        transparente: false,
        espera_clic_en_la_ventana: true,
        espera_foco_intacto: false,
    },
];

/// Lo que se midió de un caso.
struct Medicion {
    nombre: &'static str,
    estilo: u32,
    clic_en_la_ventana: bool,
    clic_en_el_blanco: bool,
    foco_intacto: bool,
    esperado: bool,
}

struct Comprobacion {
    que: String,
    bien: bool,
    detalle: String,
}

fn main() {
    let antes = unsafe { GetForegroundWindow() };
    let foco_antes = foco_del_hilo(antes);
    let mouse_antes = donde_esta_el_mouse();
    println!(
        "ventana enfocada al arrancar: «{}» ({:?})",
        titulo(antes),
        antes.0
    );
    println!("foco de teclado / cursor de texto de ese hilo: {foco_antes:?}");
    println!("(el mouse se va a mover un momento y vuelve a donde estaba)\n");

    let mut evento = EventLoop::new();

    // Dónde se hace todo: el centro del monitor principal. Da igual cuál sea
    // mientras las cuatro ventanas caigan en el mismo lugar.
    let centro = centro_del_principal(&evento);

    // El blanco: opaco, debajo de las de prueba, y también NOACTIVATE para no
    // robarle el foco al usuario sólo por existir. Acá aterriza el clic que
    // atraviesa una ventanita con WS_EX_TRANSPARENT.
    let blanco = WindowBuilder::new()
        .with_title("MithFlow — blanco del spike")
        .with_inner_size(LogicalSize::new(ANCHO * 2.0, ALTO * 3.0))
        .with_decorations(false)
        .with_always_on_top(true)
        .with_skip_taskbar(true)
        .with_focusable(false)
        .with_focused(false)
        .with_visible(false)
        .build(&evento)
        .expect("no pude crear la ventana blanco");
    centrar(&blanco, centro);
    blanco.set_visible(true);
    bombear(&mut evento, RESPIRO, &mut Vec::new());

    let mut mediciones = Vec::new();
    let mut resultados = Vec::new();

    for caso in CASOS {
        // EXACTAMENTE las mismas opciones que `superpuesta::crear`, que es lo
        // que Tauri traduce a esta capa; sólo cambian las dos que se están
        // comparando. Copiarlas de memoria no serviría: lo que se verifica es
        // esta combinación y ninguna otra.
        let ventana = WindowBuilder::new()
            .with_title("MithFlow — grabando (spike)")
            .with_inner_size(LogicalSize::new(ANCHO, ALTO))
            .with_resizable(false)
            .with_maximizable(false)
            .with_minimizable(false)
            .with_closable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_undecorated_shadow(false)
            .with_always_on_top(true)
            .with_skip_taskbar(true)
            .with_focusable(!caso.noactivate)
            .with_focused(false)
            .with_visible(false)
            .build(&evento)
            .expect("no pude crear la ventana del spike");
        if caso.transparente {
            ventana.set_ignore_cursor_events(true).ok();
        }
        centrar(&ventana, centro);
        ventana.set_visible(true);
        bombear(&mut evento, RESPIRO, &mut Vec::new());

        let hwnd = HWND(ventana.hwnd() as *mut _);
        let estilo = estilo_extendido(hwnd);

        // El clic, y quién lo recibió.
        let mut clics: Vec<WindowId> = Vec::new();
        clic_en(centro.0, centro.1);
        bombear(&mut evento, RESPIRO, &mut clics);

        let ahora = unsafe { GetForegroundWindow() };
        let foco_ahora = foco_del_hilo(antes);
        let foco_intacto = ahora == antes && foco_ahora == foco_antes;
        let clic_en_la_ventana = clics.contains(&ventana.id());
        let clic_en_el_blanco = clics.contains(&blanco.id());

        if !foco_intacto {
            println!(
                "  · tras el clic en «{}»: primer plano «{}» ({:?}), foco {foco_ahora:?}",
                caso.nombre,
                titulo(ahora),
                ahora.0
            );
        }

        // Los estilos son la explicación de todo lo demás: se comprueban aparte.
        resultados.push(Comprobacion {
            que: format!("{}: los estilos quedaron como se pidió", caso.nombre),
            bien: (estilo & WS_EX_NOACTIVATE.0 != 0) == caso.noactivate
                && (estilo & WS_EX_TRANSPARENT.0 != 0) == caso.transparente,
            detalle: format!("GWL_EXSTYLE = 0x{estilo:08x}"),
        });

        let esperado = clic_en_la_ventana == caso.espera_clic_en_la_ventana
            && foco_intacto == caso.espera_foco_intacto;
        mediciones.push(Medicion {
            nombre: caso.nombre,
            estilo,
            clic_en_la_ventana,
            clic_en_el_blanco,
            foco_intacto,
            esperado,
        });
        resultados.push(Comprobacion {
            que: format!("{}: se comportó como dice la tabla", caso.nombre),
            bien: esperado,
            detalle: format!(
                "clic en la ventana: {} (esperado {}), foco intacto: {} (esperado {})",
                clic_en_la_ventana,
                caso.espera_clic_en_la_ventana,
                foco_intacto,
                caso.espera_foco_intacto
            ),
        });

        // Sólo la configuración que se va a shippear paga el resto de las
        // pruebas: mostrar dos veces (el segundo `show` usa `SW_SHOW`, que
        // activa) y el arrastre de verdad.
        if caso.noactivate && !caso.transparente {
            resultados.extend(mostrar_dos_veces(&mut evento, &ventana, antes, foco_antes));
            resultados.extend(arrastrar(&mut evento, &ventana, centro, antes, foco_antes));
            // El arrastre la corrió: vuelve al centro para que lo que sigue mire
            // dónde tiene que mirar.
            centrar(&ventana, centro);
            bombear(&mut evento, Duration::from_millis(150), &mut Vec::new());
            resultados.push(Comprobacion {
                que: format!("{}: la ventanita no tiene el foco de teclado", caso.nombre),
                bien: unsafe { GetFocus() } != hwnd,
                detalle: String::new(),
            });
        }

        ventana.set_visible(false);
        bombear(&mut evento, Duration::from_millis(150), &mut Vec::new());
        drop(ventana);
    }

    blanco.set_visible(false);
    drop(blanco);

    // Devolverle al usuario lo que se le tomó prestado: el testigo (caso C) le
    // sacó el foco a propósito, y el mouse se movió.
    unsafe {
        let _ = SetForegroundWindow(antes);
        let _ = SetCursorPos(mouse_antes.x, mouse_antes.y);
    }

    informar(&mediciones, &resultados);
}

/// El punto de la pantalla donde se hace todo, en píxeles físicos.
fn centro_del_principal(evento: &EventLoop<()>) -> (i32, i32) {
    match evento.primary_monitor() {
        Some(monitor) => {
            let p: PhysicalPosition<i32> = monitor.position();
            let s: PhysicalSize<u32> = monitor.size();
            (p.x + s.width as i32 / 2, p.y + s.height as i32 / 2)
        }
        None => (400, 400),
    }
}

fn centrar(ventana: &Window, centro: (i32, i32)) {
    let tam = ventana.outer_size();
    ventana.set_outer_position(PhysicalPosition::new(
        centro.0 - tam.width as i32 / 2,
        centro.1 - tam.height as i32 / 2,
    ));
}

/// Mostrar, esconder y volver a mostrar. El segundo `show` es el que se
/// escaparía sin `WS_EX_NOACTIVATE`: `tao` usa `SW_SHOWNOACTIVATE` sólo la
/// primera vez (consume la marca `MARKER_DONT_FOCUS`) y después `SW_SHOW`, que
/// *activa*. Sin esto, la segunda grabación robaría el foco.
fn mostrar_dos_veces(
    evento: &mut EventLoop<()>,
    ventana: &Window,
    antes: HWND,
    foco_antes: (isize, isize),
) -> Vec<Comprobacion> {
    let mut salida = Vec::new();
    for (nombre, visible) in [
        ("hide", false),
        ("SEGUNDO show (el que usa SW_SHOW)", true),
    ] {
        ventana.set_visible(visible);
        bombear(evento, RESPIRO, &mut Vec::new());
        let ahora = unsafe { GetForegroundWindow() };
        let intacto = ahora == antes && foco_del_hilo(antes) == foco_antes;
        salida.push(Comprobacion {
            que: format!("el foco no se movió tras «{nombre}»"),
            bien: intacto,
            detalle: String::new(),
        });
    }
    salida
}

/// Cuánto se corre el mouse durante el arrastre de prueba, en píxeles.
const CORRIMIENTO: (i32, i32) = (40, 24);

/// El arrastre de verdad: botón apretado sobre la ventanita, `drag_window` y el
/// mouse moviéndose. `drag_window` es exactamente lo que hace `startDragging()`
/// de Tauri (`ReleaseCapture` + `WM_NCLBUTTONDOWN` con `HTCAPTION`) y entra en
/// el bucle modal de movimiento del sistema, que es la parte que no se puede
/// razonar desde el escritorio.
///
/// Comprueba **dos** cosas, y las dos hacen falta:
///
/// 1. Que el foco no se mueva ni siquiera durante el bucle modal.
/// 2. Que la ventana **informe `Moved`** mientras la arrastran. De eso depende
///    todo el recuerdo de la posición: `superpuesta` se entera de dónde la dejó
///    el usuario por ese evento y por ningún otro. Si `tao` no lo emitiera —o lo
///    emitiera sólo al final—, la ventanita se movería y no se acordaría de nada.
///
/// El botón lo suelta un hilo aparte —**varias veces**, por si el bucle modal
/// arranca tarde—: el bucle no devuelve el control hasta que llegue el
/// `WM_LBUTTONUP`, así que soltarlo desde este mismo hilo sería un abrazo mortal
/// con el mouse del usuario apretado.
fn arrastrar(
    evento: &mut EventLoop<()>,
    ventana: &Window,
    centro: (i32, i32),
    antes: HWND,
    foco_antes: (isize, isize),
) -> Vec<Comprobacion> {
    let desde = ventana.outer_position().ok();

    let soltador = std::thread::spawn(move || {
        // Primero mover: el bucle modal sólo corre la ventana si el mouse se
        // mueve mientras el botón sigue apretado.
        std::thread::sleep(Duration::from_millis(260));
        unsafe {
            let _ = SetCursorPos(centro.0 + CORRIMIENTO.0, centro.1 + CORRIMIENTO.1);
        }
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(220));
            boton(MOUSEEVENTF_LEFTUP);
        }
    });

    unsafe {
        let _ = SetCursorPos(centro.0, centro.1);
    }
    std::thread::sleep(Duration::from_millis(40));
    boton(MOUSEEVENTF_LEFTDOWN);
    bombear(evento, Duration::from_millis(120), &mut Vec::new());

    let arrastre = ventana.drag_window();
    let mut movimientos = 0usize;
    let hasta = Instant::now() + RESPIRO * 3;
    evento.run_return(|ev, _, control| {
        if let Event::WindowEvent {
            event: WindowEvent::Moved(_),
            ..
        } = ev
        {
            movimientos += 1;
        }
        *control = if Instant::now() >= hasta {
            ControlFlow::Exit
        } else {
            ControlFlow::WaitUntil(hasta)
        };
    });
    let _ = soltador.join();
    // Cinturón: que el botón quede apretado sería dejarle el mouse roto al
    // usuario. Un `up` de más no hace nada.
    boton(MOUSEEVENTF_LEFTUP);

    let hasta_donde = ventana.outer_position().ok();
    let se_movio = match (desde, hasta_donde) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    };

    let ahora = unsafe { GetForegroundWindow() };
    let intacto = ahora == antes && foco_del_hilo(antes) == foco_antes;
    vec![
        Comprobacion {
            que: "arrastrarla (drag_window, lo mismo que startDragging) no mueve el foco"
                .to_string(),
            bien: intacto && arrastre.is_ok(),
            detalle: match arrastre {
                Ok(()) => String::new(),
                Err(e) => format!("drag_window falló: {e}"),
            },
        },
        Comprobacion {
            que: "al arrastrarla se movió y avisó con `Moved` (de eso vive el recuerdo de la posición)"
                .to_string(),
            bien: se_movio && movimientos > 0,
            detalle: format!("{movimientos} eventos Moved; {desde:?} -> {hasta_donde:?}"),
        },
    ]
}

/// Corre el bucle de mensajes de `tao` durante `cuanto` y vuelve, anotando en
/// qué ventana cayó cada clic.
///
/// Hace falta porque `set_visible` de `tao` no toca la ventana en el acto:
/// encola el trabajo para el hilo del bucle. Sin bombear, se estaría mirando el
/// foco antes de que la ventana se haya mostrado — o sea, un PASA que no
/// significa nada.
fn bombear(evento: &mut EventLoop<()>, cuanto: Duration, clics: &mut Vec<WindowId>) {
    let hasta = Instant::now() + cuanto;
    evento.run_return(|ev, _, control| {
        if let Event::WindowEvent {
            window_id,
            event:
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    ..
                },
            ..
        } = ev
        {
            clics.push(window_id);
        }
        *control = if Instant::now() >= hasta {
            ControlFlow::Exit
        } else {
            ControlFlow::WaitUntil(hasta)
        };
    });
}

fn informar(mediciones: &[Medicion], resultados: &[Comprobacion]) {
    println!("\n=== Lo que se midió ===\n");
    println!(
        "{:<48} {:>12} {:>10} {:>10} {:>8}",
        "configuración", "GWL_EXSTYLE", "clic→vent.", "clic→abajo", "foco ok"
    );
    for m in mediciones {
        println!(
            "{:<48} {:>12} {:>10} {:>10} {:>8}{}",
            m.nombre,
            format!("0x{:08x}", m.estilo),
            si_no(m.clic_en_la_ventana),
            si_no(m.clic_en_el_blanco),
            si_no(m.foco_intacto),
            if m.esperado { "" } else { "   <-- NO es lo esperado" }
        );
    }

    println!();
    let mut todo_bien = true;
    for r in resultados {
        println!(
            "{} {}{}",
            if r.bien { "PASA " } else { "FALLA" },
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
            "OK: sacando WS_EX_TRANSPARENT la ventanita se puede agarrar con el mouse\n\
             y sigue sin robarle el foco al usuario. Se puede hacer arrastrable."
        } else {
            "ROTO: la configuración propuesta no se comporta como se esperaba.\n\
             Mirá la tabla de arriba: si el foco se movió en el caso B, NO se puede\n\
             hacer arrastrable y hay que dejarla fija."
        }
    );
    std::process::exit(if todo_bien { 0 } else { 1 });
}

fn si_no(v: bool) -> &'static str {
    if v {
        "sí"
    } else {
        "no"
    }
}
