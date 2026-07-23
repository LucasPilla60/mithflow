//! La ventanita de grabación: la única realimentación visual que existe cuando
//! el usuario no tiene MithFlow abierto.
//!
//! # El problema que resuelve
//!
//! Con la ventana principal cerrada —que es como se usa esta app casi siempre—
//! la única señal de que se está grabando eran los cuatro tonos. Un tono dice
//! "arrancó", pero no dice **"te estoy escuchando"**: quien habla con el
//! micrófono silenciado por Windows, o con el auricular equivocado como entrada,
//! escucha exactamente lo mismo que quien está dictando bien, y se entera recién
//! cuando no aparece ningún texto.
//!
//! # La restricción que le da forma a todo: no puede tomar el foco
//!
//! Esta app pega texto en la ventana que el usuario tenga enfocada. Si la
//! ventanita se lleva el foco, el cursor de texto se va del campo donde el
//! usuario estaba escribiendo y el dictado termina pegado en otro lado — que es
//! exactamente el defecto que el proyecto ya resolvió una vez suprimiendo la
//! tecla con `rdev` (ver `DECISIONES.md`, hallazgo del atajo global).
//!
//! Se cierra por **tres** vías, y ninguna sobra:
//!
//! 1. **`focusable(false)`** → `WS_EX_NOACTIVATE` en el estilo extendido, puesto
//!    en el `CreateWindowEx`. Es la que de verdad garantiza la invariante:
//!    Windows no convierte en ventana de primer plano a una ventana con ese
//!    estilo, ni al mostrarla ni al hacerle clic — **ni al arrastrarla**. Es la
//!    que **no se puede sacar**: `WebviewWindow::show()` termina en
//!    `ShowWindow(SW_SHOW)`, que *activa* la ventana, y sin `WS_EX_NOACTIVATE`
//!    cada grabación le robaría el foco al usuario.
//! 2. **`focused(false)`** → `SW_SHOWNOACTIVATE` en el primer `show`. Cubre el
//!    arranque, pero **sólo el primero**: `tao` consume esa marca al mostrarla
//!    por primera vez. Por eso no alcanza sola.
//! 3. **Nunca se llama a `set_focus`.** La ventana principal tiene su
//!    `ventana::enfocar`; ésta no tiene equivalente y no debe tenerlo.
//!
//! Y dos más que no son sobre el foco pero van en el mismo paquete:
//! `skip_taskbar(true)` (no es una aplicación, es un indicador) y
//! `decorations(false)` (una barra de título tendría botones que no se pueden
//! apretar).
//!
//! # Por qué SÍ recibe clics (y antes no)
//!
//! Hasta la 1.0 la ventanita llamaba a `set_ignore_cursor_events(true)`
//! (`WS_EX_TRANSPARENT`): los clics la atravesaban. Eso evitaba que estorbara,
//! pero también hacía imposible agarrarla, y el usuario pidió poder correrla
//! para leer lo que le tapa.
//!
//! `WS_EX_TRANSPARENT` y `WS_EX_NOACTIVATE` son propiedades **distintas**: la
//! primera decide si el clic llega a la ventana, la segunda si el clic la
//! activa. Que se pueda tener la segunda sin la primera —o sea, una ventana que
//! se puede agarrar y no roba el foco— no se dio por sabido: se midió sobre
//! Windows en `spike-superpuesta/`, con tres configuraciones y un testigo que
//! prueba que el aparato detecta un robo de foco cuando lo hay.
//!
//! | Configuración | clic → ventana | foco intacto |
//! |---|---|---|
//! | `NOACTIVATE` + `TRANSPARENT` (la vieja) | no | sí |
//! | `NOACTIVATE` sin `TRANSPARENT` (ésta) | **sí** | **sí** |
//! | sin `NOACTIVATE` (testigo) | sí | **no** |
//!
//! El costo es real y está aceptado: ahora un clic sobre la ventanita no llega
//! a lo que haya debajo. Por eso es chica y por eso se puede mover.
//!
//! # Cuándo se crea: perezosa, con un adelanto cuando es seguro
//!
//! Crear una ventana con su webview cuesta decenas de milisegundos y hay un
//! presupuesto de latencia que respetar, así que la idea era crearla escondida
//! al arrancar y que apretar la tecla no hiciera más que moverla y mostrarla.
//! Pero crearla en el `setup` de Tauri tiene una trampa: al **arrancar con
//! Windows** (autostart) ese `setup` corre tempranísimo en el inicio de sesión,
//! antes de que el shell (`explorer.exe`) y el compositor (DWM) estén listos, y
//! una ventana `WS_EX_NOACTIVATE` + topmost creada en ese momento queda en un
//! estado en el que `show()` devuelve `Ok` pero **no pinta**: la ventanita no
//! aparecía nunca al grabar, aunque el dictado anduviera perfecto.
//!
//! Por eso la creación es **perezosa**: la hace [`asegurar`] la primera vez que
//! de verdad hay que mostrarla, que es cuando el usuario aprieta la tecla para
//! dictar —necesariamente después de iniciada la sesión y con el escritorio ya
//! compuesto—. Cuando la app se abre **a mano** (sin `--oculto`) el escritorio
//! ya está listo, así que se conserva el adelanto: `main` la pre-crea en el
//! `setup` para que el primer dictado no pague la latencia. El adelanto es sólo
//! una optimización; la corrección vive en [`asegurar`], que la crea igual si no
//! estaba, y crearla al primer dictado no le come audio a nadie porque pasa
//! después de abrir el micrófono (ver `director::Director::publicar`).
//!
//! Si el indicador está desactivado en Ajustes **no se crea nada**: ni ventana,
//! ni eventos, ni medición (ver [`mithflow_core::audio::Medidor::activar`]).

use crate::ajustes;
use crate::estado::EstadoDto;
use mithflow_core::audio::Nivel;
use mithflow_core::config;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{
    AppHandle, LogicalSize, Manager, Monitor, PhysicalPosition, Runtime, WebviewUrl,
    WebviewWindowBuilder,
};
use tauri_plugin_store::StoreExt;

/// Etiqueta de la ventana. No es `"main"`: los permisos, los eventos de cierre
/// y la capability se resuelven por etiqueta.
pub const ETIQUETA: &str = "superpuesta";

/// Ancho y alto de la ventanita, en píxeles lógicos. Entran una palabra, el
/// reloj y el medidor; nada más, que es el punto.
///
/// Era 232x64 en la 1.0 y el usuario la encontró grande ("bastante grande" con
/// sus palabras). 168x48 es **46 % menos superficie** dejando el medidor en
/// ~21 px de alto —contra ~25 antes—, que es lo que no se podía tocar: su razón
/// de ser es distinguir de un vistazo "te escucho" de "estoy grabando silencio",
/// y esa diferencia la lleva sobre todo el color, no la altura.
const ANCHO: f64 = 168.0;
const ALTO: f64 = 48.0;

/// Cuánto se despega del borde del área de trabajo. Suficiente para no quedar
/// pegada a la barra de tareas y para no tapar lo que se está escribiendo.
const MARGEN: f64 = 48.0;

/// Dónde se puede poner la ventanita. La primera es el default.
pub const POSICIONES: &[&str] = &["abajo-centro", "arriba-centro", "abajo-derecha"];

/// Si ahora mismo la ventanita está a la vista.
///
/// Es un átomo y no una consulta a la ventana porque preguntarle a Tauri
/// `is_visible()` despacha al hilo principal y **espera**: el director publica
/// estados desde su propio hilo y no puede quedarse esperando al hilo de la
/// interfaz para decidir si tiene que mostrar un indicador.
static A_LA_VISTA: AtomicBool = AtomicBool::new(false);

/// El usuario tiene la ventanita agarrada con el mouse ahora mismo.
///
/// Está armada **exactamente** mientras corre el bucle modal de arrastre: la
/// arma y la desarma [`empezar_a_arrastrar`], que es una sola función y sabe
/// cuándo empieza y cuándo termina. Existe para **distinguir un arrastre del
/// usuario de un movimiento nuestro**: la ventana también informa `Moved` cuando
/// la ubica [`ubicar`] y cuando Windows la reacomoda sola al cruzar a un monitor
/// con otro factor de escala. Sin esta bandera, esos dos casos se guardarían
/// como "acá la quiere el usuario" y la ventanita dejaría de seguir al mouse
/// entre pantallas sin que nadie se lo haya pedido.
static AGARRADA: AtomicBool = AtomicBool::new(false);

/// La posición que el usuario eligió arrastrando y que todavía no se escribió al
/// disco. `Some` significa "hay algo que guardar".
///
/// No se escribe en cada `Moved` porque durante un arrastre llegan decenas por
/// segundo y cada uno sería un archivo reescrito. Se vuelca al esconder la
/// ventanita —o sea, una vez por dictado como mucho, y después de que el texto
/// ya se pegó.
static PENDIENTE: Mutex<Option<(i32, i32)>> = Mutex::new(None);

/// Claves de la posición recordada dentro de `ajustes.json`.
///
/// Viven en el mismo archivo que los ajustes pero **no** en [`ajustes::Ajustes`]:
/// no son algo que el usuario elija en un formulario sino dónde dejó una
/// ventana, y mezclarlas tendría un costo concreto — guardar Ajustes escribiría
/// la posición que el formulario leyó al abrirse, pisando la que el usuario
/// eligió arrastrando la ventanita mientras tanto. `ajustes::guardar` sólo
/// escribe los campos de su struct y `ajustes::cargar` ignora lo que no conoce,
/// así que las dos mitades conviven sin tocarse.
const CLAVE_X: &str = "indicador_x";
const CLAVE_Y: &str = "indicador_y";

/// El nivel de entrada de este instante, tal como lo dibuja la ventanita.
///
/// Los tres números derivados —[`intensidad`], `hay_voz` y `cerca_del_tope`— se
/// calculan **acá y no en el frontend** por la misma razón que las métricas del
/// dashboard: son reglas, se pueden equivocar y tienen que poder probarse. En
/// particular `hay_voz`, que es la misma compuerta con la que el motor decide si
/// vale la pena transcribir: si la ventanita usara su propio umbral, le diría al
/// usuario "te escucho" de un audio que la app va a descartar.
#[derive(Debug, Clone, Serialize)]
pub struct NivelAudio {
    pub rms: f32,
    pub pico: f32,
    /// Altura relativa de la barra, de 0 a 1.
    pub intensidad: f32,
    /// ¿Lo que está entrando llega a voz, o es ruido de fondo?
    pub hay_voz: bool,
    /// Segundos grabados hasta ahora.
    pub segundos: f32,
    /// Tope de duración vigente, para poder dibujar cuánto queda.
    pub limite_s: f32,
    pub cerca_del_tope: bool,
}

impl NivelAudio {
    /// `None` significa que no entró ni una muestra desde la lectura anterior
    /// —el dispositivo se trabó, o todavía no llegó el primer chunk— y se
    /// dibuja como silencio: es lo único honesto que se puede mostrar.
    pub fn nuevo(nivel: Option<Nivel>, segundos: f32, limite_s: f32) -> Self {
        let Nivel { rms, pico } = nivel.unwrap_or(Nivel { rms: 0.0, pico: 0.0 });
        Self {
            rms,
            pico,
            intensidad: intensidad(rms),
            hay_voz: hay_voz(rms),
            segundos,
            limite_s,
            cerca_del_tope: cerca_del_tope(segundos, limite_s),
        }
    }
}

/// Piso de la escala del medidor, en dBFS. Por debajo la barra queda en cero.
///
/// −60 dBFS es un RMS de 0,001: bastante por debajo del ruido de fondo medido
/// (0,0029), así que una habitación en silencio no deja el medidor plano —se ve
/// que el micrófono está vivo— pero tampoco lo levanta.
const PISO_DB: f32 = -60.0;

/// Techo de la escala, en dBFS. −12 dBFS es un RMS de 0,25: hablar fuerte y
/// cerca del micrófono. Poner el techo en 0 dBFS —la saturación digital— dejaría
/// la voz normal a media altura y sin recorrido visible.
const TECHO_DB: f32 = -12.0;

/// La altura de la barra para un nivel dado, de 0 a 1.
///
/// # Por qué logarítmica
///
/// En escala lineal la voz vive pegada al piso: el ruido de fondo mide 0,0029 y
/// la voz 0,071 — con el eje de 0 a 1 las dos son la misma raya de un píxel, que
/// es justo lo que el usuario tiene que poder distinguir de un vistazo. En
/// decibeles, con esta escala, quedan:
///
/// | Señal | RMS | Altura |
/// |---|---|---|
/// | silencio digital | 0,000 | 0 % |
/// | ruido de fondo | 0,0029 | ~19 % |
/// | umbral de voz ([`config::MIN_SPEECH_RMS`]) | 0,010 | ~42 % |
/// | voz normal | 0,071 | ~77 % |
/// | voz fuerte | 0,25 | 100 % |
pub fn intensidad(rms: f32) -> f32 {
    if !rms.is_finite() || rms <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * rms.log10();
    ((db - PISO_DB) / (TECHO_DB - PISO_DB)).clamp(0.0, 1.0)
}

/// ¿Lo que está entrando es voz, o ruido de fondo?
///
/// **El mismo umbral con el que el motor decide si transcribe**
/// ([`mithflow_core::stt::tiene_voz`] sobre [`config::MIN_SPEECH_RMS`]), y no
/// uno propio: lo que el usuario ve tiene que coincidir con lo que la app va a
/// hacer con ese audio. Un indicador que se pone verde con un audio que después
/// vuelve "no se escuchó nada" es peor que no tener indicador.
pub fn hay_voz(rms: f32) -> bool {
    rms.is_finite() && rms >= config::MIN_SPEECH_RMS
}

/// Fracción del tope a partir de la cual se avisa que se está por cortar.
const AVISO_DEL_TOPE: f32 = 0.15;

/// Nunca menos de esto de aviso, aunque el tope sea el mínimo de 15 s.
const AVISO_MINIMO_S: f32 = 5.0;

/// Ni más de esto, aunque el tope sea el máximo de 10 minutos: media hora
/// mirando un reloj en ámbar no avisa nada.
const AVISO_MAXIMO_S: f32 = 30.0;

/// ¿La grabación está por llegar al tope y cortarse sola?
///
/// Es proporcional y acotada porque el tope es configurable entre 15 s y 10
/// minutos: un aviso fijo de 20 s estaría prendido desde el principio con el
/// tope mínimo, y sería un parpadeo al final con el máximo.
pub fn cerca_del_tope(pasados: f32, limite: f32) -> bool {
    if !pasados.is_finite() || !limite.is_finite() || limite <= 0.0 {
        return false;
    }
    let aviso = (limite * AVISO_DEL_TOPE).clamp(AVISO_MINIMO_S, AVISO_MAXIMO_S);
    limite - pasados <= aviso
}

/// ¿Corresponde que la ventanita esté a la vista con este estado?
///
/// `transcribiendo` cuenta tanto como `grabando`: el usuario acaba de apretar la
/// tecla por segunda vez y está esperando su texto. Si la ventanita desapareciera
/// ahí, el segundo o dos de transcripción se verían igual que un dictado que se
/// perdió.
pub fn visible_en(estado: &str) -> bool {
    matches!(estado, "grabando" | "transcribiendo")
}

/// ¿Corresponde emitir el nivel de audio, o es trabajo que nadie va a mirar?
///
/// Sólo mientras se graba: en `transcribiendo` ya no entra audio, y con el
/// indicador desactivado no hay a quién contarle.
pub fn hay_que_medir(estado: &str, indicador: bool) -> bool {
    indicador && estado == "grabando"
}

/// La esquina superior izquierda de la ventanita, en píxeles físicos.
///
/// Se calcula sobre el **área de trabajo** del monitor y no sobre su resolución:
/// abajo y centrada sobre la resolución quedaría por debajo de la barra de
/// tareas, o sea escondida justo en la posición por defecto.
///
/// Es una función suelta y no un método para poder probarla sin una aplicación
/// de Tauri y sin un segundo monitor enchufado: es la parte de multi-monitor que
/// se puede equivocar (un monitor a la izquierda del principal tiene posición
/// negativa, y una pantalla al 150 % mide distinto en lógicos que en físicos).
fn esquina(
    area: (i32, i32, u32, u32),
    escala: f64,
    ventana: (f64, f64),
    posicion: &str,
) -> PhysicalPosition<i32> {
    let (izquierda, arriba, ancho, alto) = area;
    let (ancho, alto) = (ancho as f64, alto as f64);
    let escala = if escala.is_finite() && escala > 0.0 {
        escala
    } else {
        1.0
    };
    // La ventana se pide en lógicos y el monitor se informa en físicos: sin
    // esta conversión, en una pantalla al 150 % la ventanita queda corrida un
    // tercio de su ancho.
    let ancho_ventana = ventana.0 * escala;
    let alto_ventana = ventana.1 * escala;
    let margen = MARGEN * escala;

    let centrada = (ancho - ancho_ventana) / 2.0;
    let a_la_derecha = ancho - ancho_ventana - margen;
    let abajo = alto - alto_ventana - margen;

    let (x, y) = match posicion {
        "arriba-centro" => (centrada, margen),
        "abajo-derecha" => (a_la_derecha, abajo),
        // Cualquier otra cosa —un ajuste viejo, un archivo editado a mano— cae
        // en el default en vez de mandar la ventana fuera de la pantalla.
        _ => (centrada, abajo),
    };

    // Un monitor más chico que la ventanita no puede dejarla fuera de la
    // pantalla: se pega al borde y se ve recortada, que es preferible a no
    // verse.
    PhysicalPosition::new(
        izquierda + x.clamp(0.0, (ancho - ancho_ventana).max(0.0)) as i32,
        arriba + y.clamp(0.0, (alto - alto_ventana).max(0.0)) as i32,
    )
}

/// El área de trabajo de un monitor, como la quiere [`esquina`].
fn area_de(monitor: &Monitor) -> (i32, i32, u32, u32) {
    let area = monitor.work_area();
    (
        area.position.x,
        area.position.y,
        area.size.width,
        area.size.height,
    )
}

// ------------------------------------------------- la posición que se recuerda

/// Una pantalla como la necesita [`acomodar`]: **todo** el monitor en píxeles
/// físicos, y su factor de escala.
///
/// El monitor entero y no su área de trabajo, al revés que [`esquina`]. La
/// diferencia son los ~40 px de la barra de tareas y no es un detalle: la
/// ventanita está siempre por encima de todo, así que soltarla sobre la barra
/// de tareas la deja perfectamente visible. Medir contra el área de trabajo
/// haría que esa posición —que es justo donde uno la manda para sacársela de
/// encima— se descartara como "fuera de pantalla" y la ventanita volviera sola
/// al centro en el dictado siguiente, sin explicación.
///
/// La posición **de fábrica** sí se calcula sobre el área de trabajo
/// ([`esquina`]): ahí no hay nadie eligiendo, y taparle la barra de tareas a
/// quien no lo pidió sería de mal gusto.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pantalla {
    /// Origen y tamaño del monitor completo, en píxeles físicos.
    pub limites: (i32, i32, u32, u32),
    pub escala: f64,
}

impl Pantalla {
    fn de(monitor: &Monitor) -> Self {
        Self {
            limites: (
                monitor.position().x,
                monitor.position().y,
                monitor.size().width,
                monitor.size().height,
            ),
            escala: monitor.scale_factor(),
        }
    }
}

/// Dónde poner la ventanita para respetar la posición que el usuario eligió
/// arrastrándola, o `None` si esa posición ya no corresponde a ninguna pantalla.
///
/// # Los dos casos que hay que atender, y que no son el mismo
///
/// - **El monitor ya no está.** La notebook se desenchufó del segundo monitor y
///   la posición guardada cae en coordenadas que hoy no dibuja nadie: la
///   ventanita quedaría invisible y sin forma de recuperarla salvo editando el
///   archivo de ajustes a mano. Devuelve `None`, y quien llama la manda a la
///   posición de fábrica, que siempre se ve.
/// - **El monitor está pero la ventanita quedó colgando del borde.** La pantalla
///   cambió de resolución, o el usuario la soltó a medio salir. Acá su elección
///   se respeta y sólo se la mete adentro: descartarla sería mover la ventanita
///   de pantalla por un par de píxeles.
///
/// Se mide contra **el monitor entero** y no contra su área de trabajo (ver
/// [`Pantalla`]): sobre la barra de tareas la ventanita se ve, porque está
/// siempre por encima de todo, y ahí es donde uno la manda para sacársela de
/// encima.
///
/// El criterio de "sigue estando" es el **centro** de la ventanita y no una
/// esquina: es lo que decide en qué pantalla la ve el usuario, y es estable si
/// dos monitores se tocan. Elegida la pantalla, la ventanita entra entera en
/// ella —una ventanita partida entre dos monitores es justo el caso en el que
/// el medidor deja de leerse de un vistazo, que es para lo único que existe.
///
/// Es una función suelta y pura por la misma razón que [`esquina`]: es la parte
/// que se equivoca (orígenes negativos, escalas distintas por monitor, un
/// archivo editado a mano con un número absurdo) y hay que poder probarla sin
/// desenchufar un monitor de verdad.
pub fn acomodar(
    guardada: (i32, i32),
    ventana: (f64, f64),
    pantallas: &[Pantalla],
) -> Option<PhysicalPosition<i32>> {
    for pantalla in pantallas {
        let escala = if pantalla.escala.is_finite() && pantalla.escala > 0.0 {
            pantalla.escala
        } else {
            1.0
        };
        // La ventana se declara en lógicos y los monitores se informan en
        // físicos: en una pantalla al 150 % ocupa la mitad más de lo que dice.
        let ancho = (ventana.0 * escala).round() as i32;
        let alto = (ventana.1 * escala).round() as i32;

        let (izquierda, arriba, ancho_pantalla, alto_pantalla) = pantalla.limites;
        let derecha = izquierda.saturating_add(ancho_pantalla as i32);
        let abajo = arriba.saturating_add(alto_pantalla as i32);

        let centro_x = guardada.0.saturating_add(ancho / 2);
        let centro_y = guardada.1.saturating_add(alto / 2);
        if centro_x < izquierda || centro_x >= derecha || centro_y < arriba || centro_y >= abajo {
            continue;
        }

        // Un monitor más chico que la propia ventanita no puede producir un
        // `clamp` invertido (que paniquearía): el tope nunca baja del origen.
        let tope_x = derecha.saturating_sub(ancho).max(izquierda);
        let tope_y = abajo.saturating_sub(alto).max(arriba);
        return Some(PhysicalPosition::new(
            guardada.0.clamp(izquierda, tope_x),
            guardada.1.clamp(arriba, tope_y),
        ));
    }
    None
}

/// La posición que el usuario dejó elegida, si hay alguna.
///
/// **Nada de lo que sale del archivo se cree sin revisar** (la regla de
/// `ajustes`): un valor que no sea un entero de 32 bits se descarta y la
/// ventanita vuelve a la posición de fábrica, que es el fallo cerrado.
pub fn posicion_recordada<R: Runtime>(app: &AppHandle<R>) -> Option<(i32, i32)> {
    let store = app.store(ajustes::ARCHIVO).ok()?;
    let x = store.get(CLAVE_X)?.as_i64()?;
    let y = store.get(CLAVE_Y)?.as_i64()?;
    Some((i32::try_from(x).ok()?, i32::try_from(y).ok()?))
}

/// Guarda dónde quedó la ventanita.
fn recordar_posicion<R: Runtime>(app: &AppHandle<R>, (x, y): (i32, i32)) -> Result<(), String> {
    let store = app
        .store(ajustes::ARCHIVO)
        .map_err(|e| format!("no pude abrir {}: {e}", ajustes::ARCHIVO))?;
    store.set(CLAVE_X, x);
    store.set(CLAVE_Y, y);
    store
        .save()
        .map_err(|e| format!("no pude guardar {}: {e}", ajustes::ARCHIVO))
}

/// Olvida la posición elegida: la ventanita vuelve a aparecer donde diga
/// Ajustes y a seguir al mouse entre monitores. Es el botón "volver a la
/// posición por defecto".
pub fn olvidar_posicion<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    // Primero lo que está en vuelo: si quedara un pendiente, el próximo hide lo
    // volvería a escribir y el botón no habría hecho nada.
    if let Ok(mut pendiente) = PENDIENTE.lock() {
        *pendiente = None;
    }
    let store = app
        .store(ajustes::ARCHIVO)
        .map_err(|e| format!("no pude abrir {}: {e}", ajustes::ARCHIVO))?;
    store.delete(CLAVE_X);
    store.delete(CLAVE_Y);
    store
        .save()
        .map_err(|e| format!("no pude guardar {}: {e}", ajustes::ARCHIVO))
}

/// Arrastra la ventanita con el mouse, de punta a punta.
///
/// # Por qué el arrastre lo pide Rust y no el JavaScript
///
/// La ventanita podría llamar a `startDragging()` de `@tauri-apps/api` y
/// ahorrarse este comando, pero entonces harían falta **dos** viajes de IPC
/// —uno para armar [`AGARRADA`] y otro para arrastrar— y entre los dos no hay
/// orden garantizado: si el arrastre llegara primero, el `Moved` de los
/// primeros píxeles no contaría. Acá las dos cosas pasan en la misma función y
/// en el orden correcto por construcción.
///
/// De paso, la ventanita se queda **sin** el permiso
/// `core:window:allow-start-dragging`, que no está acotado a quien lo pide: ese
/// comando acepta la etiqueta de cualquier ventana (`tauri::window::plugin`),
/// así que dárselo al webview que está siempre por encima de todo era regalar
/// más de lo necesario. Desde Rust no hay ACL de por medio y el alcance es
/// exactamente esta ventana.
///
/// # Bloquea, y tiene que bloquear
///
/// `start_dragging` termina en `ReleaseCapture` + `WM_NCLBUTTONDOWN` con
/// `HTCAPTION`, o sea el bucle modal de movimiento del sistema: **no devuelve
/// el control hasta que el usuario suelta el botón**. Eso es justo lo que
/// permite saber cuándo terminó el arrastre —no hay ningún evento que lo
/// diga— y escribir ahí la posición final, en vez de al esconder la ventanita:
/// si la grabación terminara en medio de un arrastre, esconderla guardaría una
/// posición a mitad de camino que el usuario nunca eligió.
///
/// Que bloquee depende de que el comando que llama acá corra en el **hilo
/// principal** (`tauri_runtime_wry::send_user_message` ejecuta el mensaje en el
/// acto si ya está ahí, y lo encola si no). Por eso `comandos::arrastrar_indicador`
/// es sincrónico y no puede volverse `async`: ahí está explicado qué se rompe.
pub fn empezar_a_arrastrar<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let Some(ventana) = app.get_webview_window(ETIQUETA) else {
        return Err("el indicador de grabación ya no existe".to_string());
    };
    // Armar ANTES: el primer `Moved` puede llegar apenas arranque el bucle.
    AGARRADA.store(true, Ordering::SeqCst);
    let resultado = ventana
        .start_dragging()
        .map_err(|e| format!("no pude arrastrar el indicador: {e}"));
    // Y guardar acá, con el botón ya soltado y la última posición anotada.
    guardar_lo_arrastrado(app);
    resultado
}

/// La ventanita se movió. Sólo cuenta si el usuario la está arrastrando: ver
/// [`AGARRADA`] para los dos movimientos que no son suyos.
pub fn anotar_movimiento(posicion: PhysicalPosition<i32>) {
    if !AGARRADA.load(Ordering::SeqCst) {
        return;
    }
    if let Ok(mut pendiente) = PENDIENTE.lock() {
        *pendiente = Some((posicion.x, posicion.y));
    }
}

/// Vuelca al disco lo que haya quedado de un arrastre y desarma la bandera.
///
/// Se llama al esconder o destruir la ventanita: una escritura por dictado como
/// mucho, y después de que el texto ya se pegó.
fn guardar_lo_arrastrado<R: Runtime>(app: &AppHandle<R>) {
    AGARRADA.store(false, Ordering::SeqCst);
    let pendiente = PENDIENTE.lock().ok().and_then(|mut p| p.take());
    let Some(posicion) = pendiente else {
        return;
    };
    if let Err(e) = recordar_posicion(app, posicion) {
        eprintln!("no pude recordar dónde dejaste el indicador: {e}");
    }
}

/// Crea la ventanita, escondida. Ver la nota del módulo para cada opción.
pub fn crear(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(ETIQUETA).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(app, ETIQUETA, WebviewUrl::App("superpuesta.html".into()))
        .title("MithFlow — grabando")
        .inner_size(ANCHO, ALTO)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        // Las dos del foco. Ver la nota del módulo: `focusable` es la que
        // garantiza la invariante, `focused` sólo cubre el primer `show`.
        .focusable(false)
        .focused(false)
        .visible(false)
        .build()?;

    // Acá iba `set_ignore_cursor_events(true)` (`WS_EX_TRANSPARENT`) y ya no
    // va: es lo único que impedía agarrarla para moverla, y no es lo que
    // sostiene la garantía del foco. Ver la nota del módulo.
    //
    // `A_LA_VISTA` NO se toca acá: lo dueña [`reflejar`] (que lo pone antes de
    // llamar a `asegurar` en la creación perezosa) y [`destruir`]. Reponerlo a
    // `false` desde acá pisaría el `swap(true)` que `reflejar` acaba de hacer y
    // dejaría el flag mintiendo. La ventanita nace escondida (`visible(false)`)
    // y el estático arranca en `false`, así que no hace falta.
    AGARRADA.store(false, Ordering::SeqCst);
    println!("indicador de grabación listo (escondido)");
    Ok(())
}

/// Saca la ventanita del proceso. Se usa al desactivar el indicador en Ajustes:
/// desactivado no queda ni un webview vivo.
///
/// `destroy` y no `close`: `close` pide permiso con un `CloseRequested`, y el
/// manejador global de `main` lo cancela para que cerrar la ventana principal
/// esconda en vez de terminar la app.
pub fn destruir(app: &AppHandle) {
    A_LA_VISTA.store(false, Ordering::SeqCst);
    // Apagar el indicador en medio de un arrastre no puede tirar a la basura
    // dónde lo dejó el usuario.
    guardar_lo_arrastrado(app);
    if let Some(ventana) = app.get_webview_window(ETIQUETA) {
        if let Err(e) = ventana.destroy() {
            eprintln!("no pude cerrar el indicador de grabación: {e}");
        }
    }
}

/// Aplica el ajuste: crea la ventanita si se activó, la destruye si se apagó.
pub fn aplicar_ajuste(app: &AppHandle, activado: bool) {
    if !activado {
        destruir(app);
        return;
    }
    if let Err(e) = crear(app) {
        eprintln!("no pude crear el indicador de grabación: {e}");
    }
}

/// Qué hacer con la ventanita, comparando lo que decía [`A_LA_VISTA`] con lo que
/// corresponde ahora.
///
/// Es la máquina de estados del flag, **pura y aparte** para poder probarla sin
/// un `AppHandle`: `reflejar` la usa sobre el valor viejo que devuelve el `swap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transicion {
    /// Ya estaba como tiene que estar: no se despacha nada al hilo principal.
    Ninguna,
    Mostrar,
    Esconder,
}

/// La regla, en un solo lugar: cambió → mostrar u esconder según a dónde va; no
/// cambió → nada.
fn transicion(estaba_a_la_vista: bool, debe_verse: bool) -> Transicion {
    if estaba_a_la_vista == debe_verse {
        Transicion::Ninguna
    } else if debe_verse {
        Transicion::Mostrar
    } else {
        Transicion::Esconder
    }
}

/// Devuelve la ventanita, **creándola si todavía no existe**.
///
/// Ésta es la corrección del bug del arranque con Windows: la ventanita se crea
/// perezosamente la primera vez que hay que mostrarla, no en el `setup` de
/// Tauri. En autostart el `setup` corre antes de que el shell y el compositor
/// (DWM) estén listos, y una ventana `WS_EX_NOACTIVATE` + topmost creada ahí
/// queda en un estado donde `show()` devuelve `Ok` pero no pinta. Para cuando el
/// usuario aprieta la tecla para dictar —lo único que lleva a mostrarla— la
/// sesión ya está iniciada y el escritorio compuesto, así que la ventana nace
/// sana. Ver la nota del módulo.
///
/// `None` sólo si la creación falla de verdad: ahí no hay nada que mostrar y el
/// dictado sigue igual.
fn asegurar(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(ventana) = app.get_webview_window(ETIQUETA) {
        return Some(ventana);
    }
    if let Err(e) = crear(app) {
        eprintln!("no pude crear el indicador de grabación: {e}");
        return None;
    }
    app.get_webview_window(ETIQUETA)
}

/// Muestra o esconde la ventanita según el estado que se acaba de publicar.
///
/// Se llama en **cada** publicación del director, así que lo primero que hace es
/// contestar rápido cuando no hay nada que cambiar: mostrar una ventana que ya
/// está a la vista despacha al hilo principal y espera, y el director publica
/// también al pausar, al despausar y en cada cambio de estado.
///
/// Cuando corresponde mostrarla y la ventanita todavía no existe, la crea
/// [`asegurar`] en el acto: en el arranque con Windows es la primera vez que se
/// puede crear sana (ver la nota del módulo).
pub fn reflejar(app: &AppHandle, dto: &EstadoDto, activado: bool, posicion: &str) {
    let debe_verse = activado && visible_en(dto.estado);
    match transicion(A_LA_VISTA.swap(debe_verse, Ordering::SeqCst), debe_verse) {
        Transicion::Ninguna => {}
        Transicion::Esconder => {
            // La ventana puede no existir todavía —perezosa: nunca se llegó a
            // mostrar en esta sesión—, y entonces no hay nada que esconder.
            if let Some(ventana) = app.get_webview_window(ETIQUETA) {
                if let Err(e) = ventana.hide() {
                    eprintln!("no pude esconder el indicador: {e}");
                }
            }
            // Recién acá se escribe al disco lo que el usuario haya arrastrado:
            // el dictado ya terminó y el texto ya se pegó, así que una escritura
            // de 200 bytes no le cuesta nada a nadie.
            guardar_lo_arrastrado(app);
        }
        Transicion::Mostrar => {
            let Some(ventana) = asegurar(app) else {
                // No se pudo crear: el dictado sigue igual. Se rearma el flag
                // para reintentar en la próxima aparición.
                A_LA_VISTA.store(false, Ordering::SeqCst);
                return;
            };
            // La posición se recalcula en cada aparición y no una vez al
            // crearla: el usuario puede haberse mudado de monitor, o haber
            // enchufado uno nuevo, desde la grabación anterior.
            ubicar(app, &ventana, posicion);
            if let Err(e) = ventana.show() {
                eprintln!("no pude mostrar el indicador: {e}");
            }
        }
    }
}

/// Deja la ventanita donde corresponde antes de mostrarla.
///
/// # Manda lo que el usuario haya arrastrado
///
/// Si arrastró la ventanita, ahí se queda: entre dictados y entre reinicios, y
/// en el monitor donde la dejó. Elegir a mano y que la aplicación te lo pise es
/// peor que no poder elegir. Sólo se descarta cuando esa posición ya no existe
/// —el monitor se desenchufó—, que es el caso en el que respetarla la dejaría
/// invisible (ver [`acomodar`]).
///
/// # Y si no arrastró nada, el monitor donde está el mouse
///
/// Se elige por dónde está el puntero y no por dónde estaba la ventana: quien
/// tiene dos pantallas dicta en la que está mirando, y ahí es donde está el
/// mouse. Si no se puede saber, cae al monitor donde ya estaba y después al
/// principal; si tampoco hay ninguno, se deja donde esté antes que mandarla a
/// una coordenada inventada.
fn ubicar<R: Runtime>(app: &AppHandle<R>, ventana: &tauri::WebviewWindow<R>, posicion: &str) {
    let monitor = ventana
        .cursor_position()
        .ok()
        .and_then(|p| ventana.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| ventana.current_monitor().ok().flatten())
        .or_else(|| ventana.primary_monitor().ok().flatten());

    let Some(monitor) = monitor else {
        eprintln!("no pude enumerar monitores; dejo el indicador donde estaba");
        return;
    };

    // El tamaño se vuelve a fijar acá porque el factor de escala puede haber
    // cambiado al mudarse de monitor: sin esto, la ventanita queda del tamaño
    // de la pantalla anterior.
    if let Err(e) = ventana.set_size(LogicalSize::new(ANCHO, ALTO)) {
        eprintln!("no pude fijar el tamaño del indicador: {e}");
    }

    let destino = posicion_recordada(app)
        .and_then(|guardada| acomodar(guardada, (ANCHO, ALTO), &pantallas(ventana)))
        .unwrap_or_else(|| {
            esquina(
                area_de(&monitor),
                monitor.scale_factor(),
                (ANCHO, ALTO),
                posicion,
            )
        });
    if let Err(e) = ventana.set_position(destino) {
        eprintln!("no pude ubicar el indicador: {e}");
    }
}

/// Los monitores que existen ahora mismo. Si no se pueden enumerar, la lista
/// vacía hace que [`acomodar`] devuelva `None` y la ventanita caiga en la
/// posición de fábrica: fallo cerrado, nunca en una coordenada sin pantalla.
fn pantallas<R: Runtime>(ventana: &tauri::WebviewWindow<R>) -> Vec<Pantalla> {
    ventana
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(Pantalla::de)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mithflow_core::stt;

    /// Un monitor Full HD con la barra de tareas de Windows abajo (40 px).
    const PRINCIPAL: (i32, i32, u32, u32) = (0, 0, 1920, 1040);

    // ---- Qué ve el usuario ------------------------------------------------

    /// La ventanita aparece al grabar y **se queda** mientras se transcribe: es
    /// el segundo o dos en el que el usuario ya soltó la tecla y todavía no
    /// tiene su texto. Sin eso, ese hueco se ve igual que un dictado perdido.
    #[test]
    fn la_ventanita_se_ve_grabando_y_transcribiendo_y_en_ningun_otro_estado() {
        assert!(visible_en("grabando"));
        assert!(visible_en("transcribiendo"));
        for otro in ["cargando", "listo", "sin-modelo", "error"] {
            assert!(!visible_en(otro), "{otro} no está dictando nada");
        }
    }

    /// Y el ajuste manda sobre todo lo demás: desactivado, no se ve nunca. Es
    /// la misma conjunción que evalúa `reflejar`, escrita una sola vez.
    #[test]
    fn desactivado_el_indicador_no_se_ve_en_ningun_estado() {
        let se_ve = |activado: bool, estado: &str| activado && visible_en(estado);
        for estado in ["grabando", "transcribiendo", "listo", "error"] {
            assert!(!se_ve(false, estado), "{estado} lo mostró con el ajuste apagado");
        }
        assert!(se_ve(true, "grabando"), "activado y grabando tiene que verse");
        assert!(
            crate::ajustes::Ajustes::default().indicador,
            "el default de fábrica es con el indicador encendido"
        );
    }

    /// El nivel sólo se mide y se emite mientras entra audio. En
    /// `transcribiendo` la ventanita sigue a la vista pero el micrófono ya está
    /// cerrado: seguir emitiendo sería mandar ceros veinticinco veces por
    /// segundo.
    #[test]
    fn el_nivel_solo_se_emite_grabando_y_con_el_indicador_activado() {
        assert!(hay_que_medir("grabando", true));
        assert!(!hay_que_medir("grabando", false), "desactivado no se mide nada");
        assert!(!hay_que_medir("transcribiendo", true), "ya no entra audio");
        for otro in ["listo", "cargando", "error", "sin-modelo"] {
            assert!(!hay_que_medir(otro, true));
        }
    }

    // ---- La máquina de estados del flag A_LA_VISTA ------------------------

    /// El `swap` de `reflejar` sólo dispara trabajo cuando la visibilidad
    /// **cambia**: mostrar u esconder despachan al hilo principal y esperan, y
    /// el director publica también al pausar, al despausar y en cada estado.
    #[test]
    fn la_transicion_solo_actua_cuando_la_visibilidad_cambia() {
        // No cambió: nada que despachar, en los dos sentidos.
        assert_eq!(transicion(false, false), Transicion::Ninguna);
        assert_eq!(transicion(true, true), Transicion::Ninguna);
        // Cambió: mostrar u esconder según a dónde va.
        assert_eq!(transicion(false, true), Transicion::Mostrar);
        assert_eq!(transicion(true, false), Transicion::Esconder);
    }

    /// El recorrido de un dictado, tal cual lo produce el director, no muestra
    /// ni esconde de más. Es lo que evita que `grabando → transcribiendo` vuelva
    /// a ubicar y mostrar la ventanita (un salto en pantalla) o que la creación
    /// perezosa dispare un segundo `show` redundante.
    ///
    /// Se simula el `swap`: cada paso compara contra lo que dejó el anterior.
    #[test]
    fn un_dictado_entero_muestra_una_vez_y_esconde_una_vez() {
        // (estaba_a_la_vista, debe_verse) en el orden en que publica el director.
        let pasos = [
            (false, true),  // listo → grabando: aparece
            (true, true),   // grabando → transcribiendo: sigue, sin re-mostrar
            (true, false),  // transcribiendo → listo: desaparece
            (false, false), // listo (otra publicación, p. ej. pausa): nada
        ];
        let esperado = [
            Transicion::Mostrar,
            Transicion::Ninguna,
            Transicion::Esconder,
            Transicion::Ninguna,
        ];
        let mut estaba = false;
        for (i, (_, debe)) in pasos.iter().enumerate() {
            let t = transicion(estaba, *debe);
            assert_eq!(t, esperado[i], "paso {i}: {estaba} -> {debe}");
            // Lo que hace el `swap`: el flag queda en el valor nuevo.
            estaba = *debe;
        }
        assert!(!estaba, "al final del dictado la ventanita queda escondida");
    }

    // ---- El medidor -------------------------------------------------------

    /// La altura de la barra tiene que **separar a simple vista** el silencio,
    /// el ruido de fondo y la voz. Es el punto entero del pedido: distinguir
    /// "te estoy escuchando" de "estoy grabando silencio".
    ///
    /// Los tres RMS son los medidos sobre los fixtures y documentados en
    /// [`config::MIN_SPEECH_RMS`].
    #[test]
    fn la_barra_separa_el_silencio_del_ruido_y_de_la_voz() {
        let silencio = intensidad(0.0);
        let ruido = intensidad(0.002_891);
        let voz = intensidad(0.071_088);
        let grito = intensidad(0.4);

        assert_eq!(silencio, 0.0, "el silencio digital deja la barra en cero");
        assert!(
            (0.10..0.30).contains(&ruido),
            "el ruido de fondo tiene que verse vivo pero bajo: {ruido}"
        );
        assert!(
            voz > 0.70,
            "la voz normal tiene que llenar la mayor parte de la barra: {voz}"
        );
        assert!(
            voz - ruido > 0.45,
            "ruido {ruido} y voz {voz} se parecen demasiado: no se distinguirían de un vistazo"
        );
        assert_eq!(grito, 1.0, "por encima del techo la barra se llena y no se pasa");
    }

    /// Monótona y acotada: más nivel nunca puede dar una barra más baja, y
    /// ningún valor puede dibujar una barra fuera de su caja.
    #[test]
    fn la_altura_de_la_barra_es_monotona_y_esta_acotada() {
        let mut anterior = 0.0;
        for paso in 0..=200 {
            let rms = paso as f32 / 200.0;
            let altura = intensidad(rms);
            assert!(
                (0.0..=1.0).contains(&altura),
                "rms {rms} dibujó una barra de {altura}"
            );
            assert!(altura >= anterior, "rms {rms} bajó la barra");
            anterior = altura;
        }
        // Y lo imposible cae en cero en vez de propagarse al `style` del CSS.
        //
        // El infinito también, y no al tope: un RMS infinito no es "muy fuerte"
        // sino un número roto —el medidor ya los convierte en cero antes de
        // llegar acá— y la barra llena mentiría diciendo que se está midiendo
        // algo. Misma regla que `hay_voz`, que tampoco los da por voz.
        for imposible in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
            assert_eq!(intensidad(imposible), 0.0, "{imposible} no puede dibujar nada");
            assert!(!hay_voz(imposible), "{imposible} no puede contar como voz");
        }
    }

    /// El indicador y el motor usan **el mismo umbral**. Si divergieran, la
    /// ventanita diría "te escucho" de un audio que después vuelve como "no se
    /// escuchó nada".
    #[test]
    fn el_indicador_dice_que_hay_voz_exactamente_cuando_el_motor_transcribiria() {
        for amplitud in [0.0, 0.001, 0.005, 0.009, 0.01, 0.05, 0.2, 0.9] {
            let muestras = vec![amplitud; 4_000];
            assert_eq!(
                hay_voz(amplitud),
                stt::tiene_voz(&muestras),
                "con amplitud {amplitud} el indicador y el motor no coinciden"
            );
        }
        assert!(!hay_voz(f32::NAN));
    }

    /// La marca de "hay voz" tiene que caer dentro de la barra dibujada, no
    /// arriba del techo ni pegada al piso: si no, el usuario no vería la
    /// diferencia entre estar por encima o por debajo del umbral.
    #[test]
    fn el_umbral_de_voz_cae_en_un_lugar_visible_de_la_barra() {
        let en_el_umbral = intensidad(config::MIN_SPEECH_RMS);
        assert!(
            (0.25..0.60).contains(&en_el_umbral),
            "el umbral quedó en {en_el_umbral} de la barra: no se ve dónde empieza la voz"
        );
        assert!(hay_voz(config::MIN_SPEECH_RMS), "el umbral es inclusivo");
    }

    // ---- El reloj ---------------------------------------------------------

    /// El aviso del tope es proporcional: con el tope de fábrica avisa faltando
    /// menos de medio minuto, y con el mínimo de 15 s no puede estar prendido
    /// desde el arranque.
    #[test]
    fn el_aviso_del_tope_se_adapta_al_tope_configurado() {
        let de_fabrica = config::MAX_RECORDING_SECS; // 180 s
        assert!(!cerca_del_tope(0.0, de_fabrica));
        assert!(!cerca_del_tope(140.0, de_fabrica));
        assert!(cerca_del_tope(155.0, de_fabrica), "faltando 25 s ya tiene que avisar");
        assert!(cerca_del_tope(180.0, de_fabrica), "en el tope, obviamente");
        assert!(cerca_del_tope(999.0, de_fabrica), "pasado el tope no se apaga");

        // Con el tope mínimo (15 s) el aviso es de 5 s, no de 27.
        assert!(!cerca_del_tope(0.0, 15.0), "arrancar no puede ser 'casi el tope'");
        assert!(!cerca_del_tope(9.0, 15.0));
        assert!(cerca_del_tope(11.0, 15.0));

        // Y con el máximo (600 s) el aviso no se estira a hora y media.
        assert!(!cerca_del_tope(500.0, 600.0));
        assert!(cerca_del_tope(575.0, 600.0));
    }

    /// Nada de lo que llegue del reloj puede prender el aviso por accidente.
    #[test]
    fn un_tope_imposible_no_prende_el_aviso() {
        for (pasados, limite) in [
            (0.0, 0.0),
            (0.0, -10.0),
            (f32::NAN, 180.0),
            (10.0, f32::NAN),
            (10.0, f32::INFINITY),
        ] {
            assert!(
                !cerca_del_tope(pasados, limite),
                "({pasados}, {limite}) no puede avisar nada"
            );
        }
    }

    // ---- La carga que viaja al frontend -----------------------------------

    #[test]
    fn la_carga_del_nivel_lleva_todo_lo_que_la_ventanita_dibuja() {
        let carga = NivelAudio::nuevo(Some(Nivel { rms: 0.071, pico: 0.3 }), 12.5, 180.0);
        assert!(carga.hay_voz);
        assert!(carga.intensidad > 0.7);
        assert_eq!(carga.segundos, 12.5);
        assert_eq!(carga.limite_s, 180.0);
        assert!(!carga.cerca_del_tope);

        // Sin muestras nuevas se dibuja silencio, no un valor viejo.
        let sin_audio = NivelAudio::nuevo(None, 1.0, 180.0);
        assert_eq!(sin_audio.rms, 0.0);
        assert_eq!(sin_audio.intensidad, 0.0);
        assert!(!sin_audio.hay_voz);
        assert_eq!(sin_audio.segundos, 1.0, "el reloj sigue corriendo igual");
    }

    // ---- Dónde aparece ----------------------------------------------------

    /// La posición por defecto: abajo y centrada, como Wispr Flow, y **sobre el
    /// área de trabajo** — calcularla sobre la resolución la dejaría debajo de
    /// la barra de tareas, o sea escondida justo en el default.
    #[test]
    fn abajo_al_centro_queda_centrada_y_sobre_la_barra_de_tareas() {
        let p = esquina(PRINCIPAL, 1.0, (ANCHO, ALTO), "abajo-centro");
        assert_eq!(p.x, ((1920.0 - ANCHO) / 2.0) as i32);
        assert_eq!(p.y, (1040.0 - ALTO - MARGEN) as i32);
        assert!(
            (p.y as f64 + ALTO) < 1040.0,
            "la ventanita se metió debajo de la barra de tareas"
        );
    }

    /// Las tres posiciones son tres lugares distintos y ninguna se sale de la
    /// pantalla.
    #[test]
    fn las_tres_posiciones_caen_dentro_del_area_y_no_se_repiten() {
        let puestos: Vec<PhysicalPosition<i32>> = POSICIONES
            .iter()
            .map(|p| esquina(PRINCIPAL, 1.0, (ANCHO, ALTO), p))
            .collect();

        for (i, a) in puestos.iter().enumerate() {
            assert!(a.x >= 0 && a.y >= 0, "{} se salió por arriba/izquierda", POSICIONES[i]);
            assert!(
                (a.x as f64 + ANCHO) <= 1920.0 && (a.y as f64 + ALTO) <= 1040.0,
                "{} se salió por abajo/derecha",
                POSICIONES[i]
            );
            for b in &puestos[i + 1..] {
                assert_ne!(a, b, "dos posiciones que caen en el mismo lugar son una sola");
            }
        }
        assert_eq!(
            esquina(PRINCIPAL, 1.0, (ANCHO, ALTO), "arriba-centro").y,
            MARGEN as i32
        );
    }

    /// Un ajuste desconocido —un archivo editado a mano, una versión vieja— cae
    /// en el default en vez de mandar la ventanita a una coordenada inventada.
    #[test]
    fn una_posicion_desconocida_cae_en_la_de_fabrica() {
        assert_eq!(
            esquina(PRINCIPAL, 1.0, (ANCHO, ALTO), "en-el-techo"),
            esquina(PRINCIPAL, 1.0, (ANCHO, ALTO), POSICIONES[0])
        );
        assert_eq!(
            esquina(PRINCIPAL, 1.0, (ANCHO, ALTO), ""),
            esquina(PRINCIPAL, 1.0, (ANCHO, ALTO), POSICIONES[0])
        );
    }

    /// Multi-monitor: el segundo monitor **suma su origen**. Sin eso la
    /// ventanita aparecería siempre en el principal, o fuera de la pantalla si
    /// el segundo está a la izquierda (origen negativo).
    #[test]
    fn en_un_segundo_monitor_la_ventanita_aparece_en_ese_monitor() {
        let a_la_derecha = (1920, 0, 2560, 1400);
        let p = esquina(a_la_derecha, 1.0, (ANCHO, ALTO), "abajo-centro");
        assert!(p.x >= 1920, "cayó en el monitor principal: {}", p.x);
        assert_eq!(p.x, 1920 + ((2560.0 - ANCHO) / 2.0) as i32);

        // Un monitor a la izquierda del principal tiene origen negativo.
        let a_la_izquierda = (-1920, 0, 1920, 1040);
        let q = esquina(a_la_izquierda, 1.0, (ANCHO, ALTO), "abajo-derecha");
        assert!(q.x < 0, "un monitor a la izquierda vive en coordenadas negativas");
        assert!(
            (q.x as f64 + ANCHO) <= 0.0,
            "se metió en el monitor principal: {}",
            q.x
        );
    }

    /// Una pantalla al 150 % informa el área en físicos y la ventana se pide en
    /// lógicos: sin convertir, la ventanita queda corrida medio ancho.
    #[test]
    fn con_la_pantalla_escalada_la_ventanita_sigue_centrada() {
        // 2880x1620 físicos al 150 % = 1920x1080 lógicos.
        let escalado = (0, 0, 2880, 1560);
        let p = esquina(escalado, 1.5, (ANCHO, ALTO), "abajo-centro");
        assert_eq!(p.x, ((2880.0 - ANCHO * 1.5) / 2.0) as i32);
        assert_eq!(p.y, (1560.0 - ALTO * 1.5 - MARGEN * 1.5) as i32);

        // Un factor imposible no puede dar una coordenada absurda.
        for roto in [0.0, -1.0, f64::NAN] {
            let q = esquina(escalado, roto, (ANCHO, ALTO), "abajo-centro");
            assert!(q.x >= 0 && q.y >= 0, "escala {roto} tiró la ventanita fuera");
        }
    }

    /// Un monitor más chico que la ventanita no puede dejarla fuera de la
    /// pantalla: es el caso del proyector de 640x480 y el de una escala
    /// disparatada.
    #[test]
    fn un_monitor_diminuto_no_deja_la_ventanita_fuera_de_pantalla() {
        // Más chico que la ventanita en los dos ejes: se pega al origen y se ve
        // recortada, que es preferible a no verse.
        let diminuto = (0, 0, 120, 40);
        for posicion in POSICIONES {
            let p = esquina(diminuto, 1.0, (ANCHO, ALTO), posicion);
            assert_eq!((p.x, p.y), (0, 0), "{posicion} se salió de un monitor chico");
        }
        // Y uno que entra a lo alto pero no a lo ancho —una pantalla vertical
        // angosta—: se pega a la izquierda y sigue entrando por arriba y abajo.
        let angosto = (0, 0, 120, 400);
        for posicion in POSICIONES {
            let p = esquina(angosto, 1.0, (ANCHO, ALTO), posicion);
            assert_eq!(p.x, 0, "{posicion} se salió por el ancho");
            assert!(
                p.y >= 0 && (p.y as f64 + ALTO) <= 400.0,
                "{posicion} se salió por el alto: {}",
                p.y
            );
        }
    }

    /// Cuánto de la pantalla tapa. No es estética: el usuario pidió achicarla
    /// porque le tapaba lo que estaba leyendo, y desde que se puede arrastrar
    /// además se come los clics que le caen encima. Si alguien la agranda, que
    /// sea a propósito y no por acumulación de "un par de píxeles más".
    ///
    /// Con 232x64 —la de la 1.0— este test falla: tapaba el 0,74 %.
    #[test]
    fn la_ventanita_tapa_una_fraccion_minima_de_la_pantalla() {
        let (ancho_pantalla, alto_pantalla) = (PRINCIPAL.2 as f64, PRINCIPAL.3 as f64);
        let porcion = (ANCHO * ALTO) / (ancho_pantalla * alto_pantalla);
        assert!(
            porcion < 0.005,
            "tapa el {:.2} % de una pantalla Full HD",
            porcion * 100.0
        );

        // Y el piso: por debajo de esto no entran el rótulo con el reloj al lado
        // ni veinte barras que se distingan, que es para lo que existe.
        let (ancho, alto) = (ANCHO, ALTO);
        assert!(ancho >= 140.0, "{ancho} px de ancho no dibujan un medidor");
        assert!(alto >= 40.0, "{alto} px de alto no dibujan un medidor");
    }

    // ---- La posición que el usuario elige arrastrándola --------------------

    /// El monitor Full HD entero, barra de tareas incluida. `acomodar` mide
    /// contra esto y no contra [`PRINCIPAL`], que es el área de trabajo.
    const PRINCIPAL_ENTERO: (i32, i32, u32, u32) = (0, 0, 1920, 1080);

    /// Dos monitores Full HD, el segundo a la derecha, los dos al 100 %.
    fn dos_pantallas() -> Vec<Pantalla> {
        vec![
            Pantalla {
                limites: PRINCIPAL_ENTERO,
                escala: 1.0,
            },
            Pantalla {
                limites: (1920, 0, 1920, 1080),
                escala: 1.0,
            },
        ]
    }

    /// **La ventanita está por encima de todo, incluida la barra de tareas.**
    /// Soltarla ahí —que es justo donde uno la manda para sacársela del medio—
    /// la deja perfectamente visible, así que esa posición se respeta.
    ///
    /// Medir contra el área de trabajo la descartaba en silencio: el dictado
    /// siguiente la devolvía al centro sin explicación, y Ajustes seguía
    /// diciendo "manda dónde la dejaste con el mouse".
    #[test]
    fn una_ventanita_soltada_sobre_la_barra_de_tareas_se_queda_ahi() {
        let pantallas = vec![Pantalla {
            limites: PRINCIPAL_ENTERO,
            escala: 1.0,
        }];
        // 1020 + 48 = 1068: pisa la barra de tareas (el área de trabajo termina
        // en 1040) pero entra entera en la pantalla.
        let sobre_la_barra = acomodar((600, 1020), (ANCHO, ALTO), &pantallas).expect("se ve");
        assert_eq!((sobre_la_barra.x, sobre_la_barra.y), (600, 1020));

        // Y el borde de verdad, el de la pantalla, sí la mete adentro.
        let colgando = acomodar((600, 1050), (ANCHO, ALTO), &pantallas).expect("el centro entra");
        assert_eq!(colgando.y, 1080 - ALTO as i32);
    }

    /// Lo primero y lo más importante: si la arrastró y ahí sigue habiendo
    /// pantalla, ahí se queda. Tal cual, sin corregirle nada.
    #[test]
    fn una_posicion_arrastrada_que_sigue_en_pantalla_se_respeta_intacta() {
        let elegida = (640, 300);
        let p = acomodar(elegida, (ANCHO, ALTO), &dos_pantallas()).expect("esa posición existe");
        assert_eq!((p.x, p.y), elegida);

        // Y en el segundo monitor también: la elección incluye en qué pantalla.
        let en_el_segundo = (2400, 800);
        let q = acomodar(en_el_segundo, (ANCHO, ALTO), &dos_pantallas()).expect("existe");
        assert_eq!((q.x, q.y), en_el_segundo);
    }

    /// El monitor que se desenchufó: la posición guardada cae en coordenadas que
    /// ya no dibuja nadie. Devolver `None` es lo que manda la ventanita a la
    /// posición de fábrica; respetarla la dejaría invisible y sin forma de
    /// recuperarla salvo editando `ajustes.json` a mano.
    #[test]
    fn si_el_monitor_ya_no_esta_la_posicion_guardada_se_descarta() {
        let solo_el_principal = vec![Pantalla {
            limites: PRINCIPAL_ENTERO,
            escala: 1.0,
        }];
        // Estaba en el segundo monitor, que hoy no existe.
        assert_eq!(acomodar((2400, 800), (ANCHO, ALTO), &solo_el_principal), None);
        // Un monitor a la izquierda que tampoco está.
        assert_eq!(acomodar((-900, 400), (ANCHO, ALTO), &solo_el_principal), None);
        // Y sin ningún monitor enumerado, nada puede acomodarse.
        assert_eq!(acomodar((10, 10), (ANCHO, ALTO), &[]), None);
    }

    /// El monitor sigue estando pero la ventanita quedó colgando del borde —la
    /// pantalla cambió de resolución, o el usuario la soltó a medio salir—. Su
    /// elección se respeta y sólo se la mete adentro: descartarla sería moverla
    /// de pantalla por un par de píxeles.
    #[test]
    fn una_ventanita_colgando_del_borde_se_mete_adentro_en_vez_de_descartarse() {
        let pantallas = vec![Pantalla {
            limites: PRINCIPAL_ENTERO,
            escala: 1.0,
        }];

        // Colgando por la derecha: el centro todavía cae adentro.
        let derecha = acomodar((1800, 500), (ANCHO, ALTO), &pantallas).expect("el centro entra");
        assert_eq!(derecha.x, 1920 - ANCHO as i32);
        assert_eq!(derecha.y, 500, "el eje que estaba bien no se toca");

        // Colgando por abajo del borde de la PANTALLA: ahí sí desaparecería de
        // verdad (la barra de tareas no la tapa, ver el test de arriba).
        let abajo = acomodar((300, 1040), (ANCHO, ALTO), &pantallas).expect("el centro entra");
        assert_eq!(abajo.y, 1080 - ALTO as i32);

        // Y arriba a la izquierda, contra el origen.
        let arriba_izq = acomodar((-40, -10), (ANCHO, ALTO), &pantallas).expect("el centro entra");
        assert_eq!((arriba_izq.x, arriba_izq.y), (0, 0));
    }

    /// Un monitor con origen negativo (a la izquierda del principal) es el caso
    /// que más se equivoca: ahí las coordenadas válidas son negativas.
    #[test]
    fn un_monitor_a_la_izquierda_admite_coordenadas_negativas() {
        let pantallas = vec![
            Pantalla {
                limites: PRINCIPAL_ENTERO,
                escala: 1.0,
            },
            Pantalla {
                limites: (-1920, 0, 1920, 1080),
                escala: 1.0,
            },
        ];
        let p = acomodar((-1500, 200), (ANCHO, ALTO), &pantallas).expect("ese monitor existe");
        assert_eq!((p.x, p.y), (-1500, 200));

        // Y el borde izquierdo de ESE monitor, no el del principal.
        let pegada = acomodar((-2000, 200), (ANCHO, ALTO), &pantallas).expect("el centro entra");
        assert_eq!(pegada.x, -1920);
    }

    /// Una pantalla al 150 % informa su área en físicos mientras la ventana se
    /// declara en lógicos: sin convertir, el tope de la derecha quedaría medio
    /// ancho corrido y la ventanita colgando.
    #[test]
    fn con_la_pantalla_escalada_el_tope_usa_el_tamano_fisico() {
        let pantallas = vec![Pantalla {
            // 2880x1620 físicos al 150 % = 1920x1080 lógicos.
            limites: (0, 0, 2880, 1620),
            escala: 1.5,
        }];
        let p = acomodar((2750, 100), (ANCHO, ALTO), &pantallas).expect("el centro entra");
        assert_eq!(p.x, 2880 - (ANCHO * 1.5) as i32);

        // Una escala imposible no puede producir una coordenada absurda.
        for rota in [0.0, -2.0, f64::NAN] {
            let raras = vec![Pantalla {
                limites: (0, 0, 1920, 1080),
                escala: rota,
            }];
            let q = acomodar((100, 100), (ANCHO, ALTO), &raras).expect("el centro entra");
            assert_eq!((q.x, q.y), (100, 100), "escala {rota}");
        }
    }

    /// Un monitor más chico que la propia ventanita no puede producir un
    /// `clamp` invertido, que paniquearía. Es el proyector de 640x480 otra vez.
    #[test]
    fn un_monitor_mas_chico_que_la_ventanita_no_paniquea() {
        let pantallas = vec![Pantalla {
            limites: (0, 0, 120, 40),
            escala: 1.0,
        }];
        let p = acomodar((10, 5), (ANCHO, ALTO), &pantallas).expect("el centro entra");
        assert_eq!((p.x, p.y), (0, 0), "se pega al origen y se ve recortada");
    }

    /// `ajustes.json` se puede editar a mano y el archivo de una versión vieja
    /// puede tener cualquier cosa: ningún número puede paniquear ni mandar la
    /// ventanita a una coordenada inventada.
    #[test]
    fn una_posicion_guardada_disparatada_no_rompe_nada() {
        let pantallas = dos_pantallas();
        for imposible in [
            (i32::MAX, i32::MAX),
            (i32::MIN, i32::MIN),
            (i32::MAX, 0),
            (0, i32::MIN),
        ] {
            // Lo único que se exige es que no paniquee y que, si contesta algo,
            // sea una posición que cae en alguna de las pantallas.
            if let Some(p) = acomodar(imposible, (ANCHO, ALTO), &pantallas) {
                assert!(
                    pantallas.iter().any(|pant| {
                        let (x, y, w, h) = pant.limites;
                        p.x >= x && p.y >= y && p.x < x + w as i32 && p.y < y + h as i32
                    }),
                    "{imposible:?} devolvió {p:?}, que no cae en ninguna pantalla"
                );
            }
        }

        // Y un área de monitor absurda tampoco.
        let absurda = vec![Pantalla {
            limites: (i32::MAX - 10, i32::MAX - 10, u32::MAX, u32::MAX),
            escala: 1.0,
        }];
        let _ = acomodar((0, 0), (ANCHO, ALTO), &absurda);
    }

    /// Soltada justo sobre la juntura de dos monitores. Manda **el centro**: es
    /// lo que decide en cuál de las dos pantallas la ve el usuario. Y una vez
    /// elegida la pantalla, la ventanita entra entera: dejarla partida entre dos
    /// monitores sería el único caso en que el medidor no se lee de un vistazo.
    #[test]
    fn a_caballo_de_dos_monitores_entra_entera_en_el_que_tiene_el_centro() {
        let pantallas = dos_pantallas();

        // Más de la mitad en el segundo monitor: el centro cae pasado 1920.
        let mayormente_derecha = (1920 - (ANCHO as i32 / 2) + 10, 500);
        let p = acomodar(mayormente_derecha, (ANCHO, ALTO), &pantallas).expect("el centro entra");
        assert_eq!(p.x, 1920, "tenía que terminar de entrar en el segundo monitor");
        assert_eq!(p.y, 500, "el eje que estaba bien no se toca");

        // Más de la mitad en el principal: se mete entera en el principal.
        let mayormente_izquierda = (1920 - (ANCHO as i32 / 2) - 10, 500);
        let q = acomodar(mayormente_izquierda, (ANCHO, ALTO), &pantallas).expect("el centro entra");
        assert_eq!(q.x, 1920 - ANCHO as i32);
    }
}
