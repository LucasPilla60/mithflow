//! MithFlow: dictado por voz local. El cascarón de escritorio.
//!
//! # Los hilos
//!
//! | Hilo | Qué hace | Por qué está solo |
//! |---|---|---|
//! | principal | ventana, bandeja, comandos | lo exige el sistema operativo |
//! | `atajo` | `rdev::grab` | bloquea para siempre |
//! | `director` | la máquina de estados | único escritor del estado |
//! | `motor` | carga el modelo y transcribe | entre 20 s y un minuto de arranque, segundos por dictado |
//! | `sonidos` | los cuatro tonos | retiene el `Stream` de salida, que no es `Sync` |
//!
//! Se hablan por canales, nunca por memoria compartida mutable. La única
//! memoria compartida es el espejo de sólo lectura del estado.
//!
//! El del motor es el único que puede arrancar **dos veces**: acá al abrir la
//! aplicación y, si acá no había ningún modelo, apenas una descarga termina bien
//! (ver [`motor::resolver_y_lanzar`] y `director::Director::modelo_descargado`).
//! Los dos caminos pasan por la misma función a propósito.
//!
//! # Qué cierra la aplicación
//!
//! "Salir" en el menú de la bandeja, y desinstalar desde Ajustes (que cierra
//! porque el desinstalador tiene que poder borrar este ejecutable). Ni el
//! micrófono ocupado, ni un pegado fallido, ni una transcripción vacía, ni
//! siquiera que no haya un modelo descargado: todo eso se informa y la app
//! sigue viva, porque desde ahí el usuario todavía puede llegar a Ajustes y
//! arreglarlo.

// Sin esto, la app de release abre una consola negra al arrancar. En debug se
// deja a propósito: es donde salen los mensajes de diagnóstico.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actualizador;
mod ajustes;
mod atajo;
mod bandeja;
mod comandos;
mod desinstalar;
mod director;
mod estado;
mod eventos;
mod motor;
mod rutas;
mod sonidos;
mod superpuesta;
mod ventana;

use crate::director::{AlDirector, Mensaje};
use crate::estado::{EstadoCompartido, FalloDelMotor};
use crate::sonidos::Sonidos;
use std::sync::mpsc;
use std::sync::Arc;
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

/// Con este argumento la app arranca sin ventana, sólo con el ícono de bandeja.
const ARG_OCULTO: &str = "--oculto";

fn main() {
    tauri::Builder::default()
        // Una sola instancia: abrir el acceso directo dos veces tiene que traer
        // la ventana al frente, no enganchar el teclado dos veces (que sería un
        // atajo que arranca y para la grabación de un solo golpe).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            ventana::mostrar(app, None);
        }))
        .plugin(tauri_plugin_store::Builder::default().build())
        // Actualizaciones contra GitHub Releases. El endpoint y la clave pública
        // que verifica la firma salen de `plugins.updater` en `tauri.conf.json`.
        // No se le da ningún permiso al JavaScript (ver `capabilities/`): todo
        // pasa por los comandos de `actualizador`, que es donde están las
        // guardas de "no interrumpir un dictado" y "no bajar de versión".
        .plugin(tauri_plugin_updater::Builder::new().build())
        // El argumento se agrega SÓLO al acceso directo del arranque de
        // Windows: arrancar con la sesión no puede tirarte una ventana encima,
        // pero abrir la app a mano sí tiene que mostrarla.
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![ARG_OCULTO]),
        ))
        .invoke_handler(tauri::generate_handler![
            comandos::leer_estado,
            comandos::leer_ajustes,
            comandos::escribir_ajustes,
            comandos::leer_historial,
            comandos::leer_metricas,
            comandos::borrar_historial,
            comandos::leer_catalogo,
            comandos::pausar,
            comandos::reanudar,
            comandos::alternar_pausa,
            comandos::perfilar_hardware,
            comandos::descargar_modelo,
            comandos::arrastrar_indicador,
            comandos::indicador_movido,
            comandos::restablecer_posicion_indicador,
            desinstalar::resumen_desinstalacion,
            desinstalar::desinstalar,
            actualizador::leer_actualizacion,
            actualizador::buscar_actualizacion,
            actualizador::instalar_actualizacion,
        ])
        .setup(preparar)
        // Cerrar la ventana esconde, no termina: esta app vive en la bandeja y
        // cerrar la vista no puede dejar al usuario sin atajo sin avisarle.
        //
        // Sólo la principal: la ventanita de grabación se cierra a propósito
        // cuando el usuario desactiva el indicador, y cancelarle el cierre acá
        // la dejaría viva para siempre.
        .on_window_event(|ventana, evento| {
            if matches!(evento, WindowEvent::CloseRequested { .. })
                && ventana.label() == ventana::PRINCIPAL
            {
                if let WindowEvent::CloseRequested { api, .. } = evento {
                    api.prevent_close();
                }
                if let Err(e) = ventana.hide() {
                    eprintln!("no pude esconder la ventana: {e}");
                }
            }
            // Dónde dejó el usuario la ventanita de grabación. Sólo cuenta
            // mientras la tenga agarrada con el mouse: acá también llegan los
            // movimientos que hace el propio programa. Ver `superpuesta`.
            if let (superpuesta::ETIQUETA, WindowEvent::Moved(posicion)) =
                (ventana.label(), evento)
            {
                superpuesta::anotar_movimiento(*posicion);
            }
        })
        .run(tauri::generate_context!())
        .expect("no pude arrancar la aplicación");
}

/// Arma todo el aparato: ajustes, bandeja y los hilos.
///
/// El orden importa en un solo punto: la bandeja se construye ANTES de lanzar
/// al director, para que el primer estado que éste publique ya tenga dónde
/// mostrarse.
fn preparar(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let cfg = ajustes::cargar(&handle);
    println!(
        "MithFlow — atajo {}, modelo '{}', sonidos {}",
        cfg.tecla,
        cfg.modelo,
        if cfg.sonidos { "sí" } else { "no" }
    );

    // `--oculto` lo agrega SÓLO el acceso directo del arranque con Windows, así
    // que también es la señal de que el escritorio puede no estar listo todavía
    // (ver la pre-creación del indicador más abajo).
    let arranco_oculto = std::env::args().any(|a| a == ARG_OCULTO);
    if arranco_oculto {
        if let Some(v) = handle.get_webview_window(ventana::PRINCIPAL) {
            if let Err(e) = v.hide() {
                eprintln!("no pude arrancar sin ventana: {e}");
            }
        }
    }

    let sonidos = Sonidos::lanzar();
    sonidos.configurar(cfg.sonidos, cfg.volumen);
    atajo::fijar_tecla(&cfg.tecla);
    comandos::aplicar_autoarranque(&handle, cfg.arranque_con_windows);

    let (al_director, cola) = mpsc::channel::<Mensaje>();
    bandeja::construir(&handle)?;

    // La ventanita se pre-crea ACÁ, escondida, **sólo si la app se abrió a
    // mano** (no por autostart): crearla adelanta la latencia del primer
    // dictado, pero en el arranque con Windows el escritorio (shell + DWM)
    // todavía no está listo y la ventana queda en un estado donde `show()` no
    // pinta. En autostart se difiere: `superpuesta::asegurar` la crea sana la
    // primera vez que hay que mostrarla, ya con la sesión iniciada. Ver la nota
    // del módulo `superpuesta`. Que falle no es fatal: se dicta igual, sólo sin
    // indicador.
    if cfg.indicador && !arranco_oculto {
        superpuesta::aplicar_ajuste(&handle, true);
    }

    let espejo = Arc::new(EstadoCompartido::nuevo());
    handle.manage(Arc::clone(&espejo));
    handle.manage(AlDirector::nuevo(al_director.clone()));
    handle.manage(sonidos.clone());
    // El espejo de la última consulta de actualizaciones. Se registra ANTES de
    // programar la consulta, que es quien lo escribe.
    handle.manage(Arc::new(actualizador::UltimaConsulta::nueva()));

    // Si acá no se puede arrancar el motor, la aplicación sigue viva con un
    // canal muerto: el director publica `SinModelo` o `Error` según el caso, y
    // una descarga que termine bien lo vuelve a intentar sin reiniciar nada.
    let motor = match motor::resolver_y_lanzar(&handle, &al_director, &cfg) {
        Ok(motor) => motor,
        Err(fallo) => motor_ausente(&al_director, fallo),
    };
    atajo::lanzar(al_director.clone());
    // Última de todo y en un hilo que primero duerme: si esta línea no existiera
    // la aplicación funcionaría igual, que es exactamente la relación que tiene
    // que tener el actualizador con el dictado.
    actualizador::consultar_periodicamente(&handle);
    director::lanzar(
        handle.clone(),
        al_director,
        cola,
        motor,
        espejo,
        sonidos,
        &cfg,
    );
    Ok(())
}

/// El motor que no se pudo resolver: se le cuenta al director —que es quien
/// sabe convertirlo en un estado visible— y se sigue con un canal huérfano.
fn motor_ausente(al_director: &mpsc::Sender<Mensaje>, fallo: FalloDelMotor) -> motor::Motor {
    eprintln!("el motor no arranca: {}", fallo.motivo());
    let _ = al_director.send(Mensaje::MotorListo(Err(fallo)));
    motor::Motor::ausente()
}
