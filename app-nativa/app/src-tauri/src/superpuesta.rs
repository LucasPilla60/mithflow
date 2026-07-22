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
//! Se cierra por **cuatro** vías, y ninguna sobra:
//!
//! 1. **`focusable(false)`** → `WS_EX_NOACTIVATE` en el estilo extendido, puesto
//!    en el `CreateWindowEx`. Es la que de verdad garantiza la invariante:
//!    Windows no convierte en ventana de primer plano a una ventana con ese
//!    estilo, ni al mostrarla ni al hacerle clic. **Es la que no se puede
//!    sacar**: `WebviewWindow::show()` termina en `ShowWindow(SW_SHOW)`, que
//!    *activa* la ventana, y sin `WS_EX_NOACTIVATE` cada grabación le robaría el
//!    foco al usuario.
//! 2. **`focused(false)`** → `SW_SHOWNOACTIVATE` en el primer `show`. Cubre el
//!    arranque, pero **sólo el primero**: `tao` consume esa marca al mostrarla
//!    por primera vez. Por eso no alcanza sola.
//! 3. **`set_ignore_cursor_events(true)`** → `WS_EX_TRANSPARENT`: los clics
//!    atraviesan la ventanita y llegan a lo que haya debajo. Sin esto, un clic
//!    encima —en el medio de la pantalla, mientras el usuario dicta— no llegaría
//!    a destino aunque no diera foco.
//! 4. **Nunca se llama a `set_focus`.** La ventana principal tiene su
//!    `ventana::enfocar`; ésta no tiene equivalente y no debe tenerlo.
//!
//! Y dos más que no son sobre el foco pero van en el mismo paquete:
//! `skip_taskbar(true)` (no es una aplicación, es un indicador) y
//! `decorations(false)` (una barra de título tendría botones que no se pueden
//! apretar).
//!
//! # Por qué se crea al arrancar y no al apretar la tecla
//!
//! Crear una ventana con su webview cuesta decenas de milisegundos y hay un
//! presupuesto de latencia que respetar. Se crea escondida al arrancar la
//! aplicación, y apretar la tecla no hace más que moverla y mostrarla. Si el
//! indicador está desactivado en Ajustes **no se crea nada**: ni ventana, ni
//! eventos, ni medición (ver [`mithflow_core::audio::Medidor::activar`]).

use crate::estado::EstadoDto;
use mithflow_core::audio::Nivel;
use mithflow_core::config;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    AppHandle, LogicalSize, Manager, Monitor, PhysicalPosition, WebviewUrl, WebviewWindowBuilder,
};

/// Etiqueta de la ventana. No es `"main"`: los permisos, los eventos de cierre
/// y la capability se resuelven por etiqueta.
pub const ETIQUETA: &str = "superpuesta";

/// Ancho y alto de la ventanita, en píxeles lógicos. Entran una palabra, el
/// reloj y el medidor; nada más, que es el punto.
const ANCHO: f64 = 232.0;
const ALTO: f64 = 64.0;

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

/// Crea la ventanita, escondida. Ver la nota del módulo para cada opción.
pub fn crear(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(ETIQUETA).is_some() {
        return Ok(());
    }
    let ventana = WebviewWindowBuilder::new(app, ETIQUETA, WebviewUrl::App("superpuesta.html".into()))
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

    // Los clics la atraviesan. No se puede pedir en el builder: es una llamada
    // aparte, y que falle no es fatal —la ventanita se vería igual— pero sí hay
    // que enterarse, porque una ventanita que come clics en el medio de la
    // pantalla es peor que no tenerla.
    if let Err(e) = ventana.set_ignore_cursor_events(true) {
        eprintln!("no pude hacer que los clics atraviesen el indicador: {e}");
    }
    A_LA_VISTA.store(false, Ordering::SeqCst);
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

/// Muestra o esconde la ventanita según el estado que se acaba de publicar.
///
/// Se llama en **cada** publicación del director, así que lo primero que hace es
/// contestar rápido cuando no hay nada que cambiar: mostrar una ventana que ya
/// está a la vista despacha al hilo principal y espera, y el director publica
/// también al pausar, al despausar y en cada cambio de estado.
pub fn reflejar(app: &AppHandle, dto: &EstadoDto, activado: bool, posicion: &str) {
    let debe_verse = activado && visible_en(dto.estado);
    if A_LA_VISTA.swap(debe_verse, Ordering::SeqCst) == debe_verse {
        return;
    }
    let Some(ventana) = app.get_webview_window(ETIQUETA) else {
        // El indicador está activado pero la ventana no existe (falló al
        // crearse). No hay nada que mostrar y el dictado sigue igual.
        A_LA_VISTA.store(false, Ordering::SeqCst);
        return;
    };

    if !debe_verse {
        if let Err(e) = ventana.hide() {
            eprintln!("no pude esconder el indicador: {e}");
        }
        return;
    }

    // La posición se recalcula en cada aparición y no una vez al crearla: el
    // usuario puede haberse mudado de monitor, o haber enchufado uno nuevo,
    // desde la grabación anterior.
    ubicar(&ventana, posicion);
    if let Err(e) = ventana.show() {
        eprintln!("no pude mostrar el indicador: {e}");
    }
}

/// Deja la ventanita en el monitor donde está el usuario.
///
/// El monitor se elige por dónde está el puntero y no por dónde estaba la
/// ventana: quien tiene dos pantallas dicta en la que está mirando, y ahí es
/// donde está el mouse. Si no se puede saber, cae al monitor donde ya estaba y
/// después al principal; si tampoco hay ninguno, se deja donde esté antes que
/// mandarla a una coordenada inventada.
fn ubicar<R: tauri::Runtime>(ventana: &tauri::WebviewWindow<R>, posicion: &str) {
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
    let destino = esquina(
        area_de(&monitor),
        monitor.scale_factor(),
        (ANCHO, ALTO),
        posicion,
    );
    if let Err(e) = ventana.set_position(destino) {
        eprintln!("no pude ubicar el indicador: {e}");
    }
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
        let diminuto = (0, 0, 120, 60);
        for posicion in POSICIONES {
            let p = esquina(diminuto, 1.0, (ANCHO, ALTO), posicion);
            assert_eq!((p.x, p.y), (0, 0), "{posicion} se salió de un monitor chico");
        }
    }
}
