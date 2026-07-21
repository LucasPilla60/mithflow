//! MithFlow: dictado por voz local. El cascarón de escritorio.
//!
//! # Los hilos
//!
//! | Hilo | Qué hace | Por qué está solo |
//! |---|---|---|
//! | principal | ventana, bandeja, comandos | lo exige el sistema operativo |
//! | `atajo` | `rdev::grab` | bloquea para siempre |
//! | `director` | la máquina de estados | único escritor del estado |
//! | `motor` | carga el modelo y transcribe | 20 s de arranque, segundos por dictado |
//! | `sonidos` | los cuatro tonos | retiene el `Stream` de salida, que no es `Sync` |
//!
//! Se hablan por canales, nunca por memoria compartida mutable. La única
//! memoria compartida es el espejo de sólo lectura del estado.
//!
//! # Qué cierra la aplicación
//!
//! Sólo "Salir" en el menú de la bandeja. Ni el micrófono ocupado, ni un
//! pegado fallido, ni una transcripción vacía, ni siquiera que no haya un
//! modelo descargado: todo eso se informa y la app sigue viva, porque desde ahí
//! el usuario todavía puede llegar a Ajustes y arreglarlo.

// Sin esto, la app de release abre una consola negra al arrancar. En debug se
// deja a propósito: es donde salen los mensajes de diagnóstico.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ajustes;
mod atajo;
mod bandeja;
mod comandos;
mod director;
mod estado;
mod eventos;
mod motor;
mod rutas;
mod sonidos;
mod ventana;

use crate::director::{AlDirector, Mensaje};
use crate::estado::EstadoCompartido;
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
            if let Some(v) = app.get_webview_window(ventana::PRINCIPAL) {
                ventana::enfocar(&v);
            }
        }))
        .plugin(tauri_plugin_store::Builder::default().build())
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
            comandos::leer_catalogo,
            comandos::pausar,
            comandos::reanudar,
            comandos::alternar_pausa,
            comandos::perfilar_hardware,
            comandos::descargar_modelo,
        ])
        .setup(preparar)
        // Cerrar la ventana esconde, no termina: esta app vive en la bandeja y
        // cerrar la vista no puede dejar al usuario sin atajo sin avisarle.
        .on_window_event(|ventana, evento| {
            if let WindowEvent::CloseRequested { api, .. } = evento {
                api.prevent_close();
                if let Err(e) = ventana.hide() {
                    eprintln!("no pude esconder la ventana: {e}");
                }
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

    if std::env::args().any(|a| a == ARG_OCULTO) {
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

    let espejo = Arc::new(EstadoCompartido::nuevo());
    handle.manage(Arc::clone(&espejo));
    handle.manage(AlDirector::nuevo(al_director.clone()));
    handle.manage(sonidos.clone());

    let al_motor = arrancar_motor(&handle, &cfg, &al_director);
    atajo::lanzar(al_director.clone());
    director::lanzar(handle.clone(), cola, al_motor, espejo, sonidos);
    Ok(())
}

/// Resuelve el modelo y arranca el motor.
///
/// Si no hay ningún modelo descargado se devuelve un canal muerto y se le avisa
/// al director, que lo convierte en estado `Error` con el motivo. **No se sale
/// de la aplicación**: descargar el modelo se hace desde Ajustes, o sea desde
/// esta misma app corriendo.
fn arrancar_motor(
    handle: &tauri::AppHandle,
    cfg: &ajustes::Ajustes,
    al_director: &mpsc::Sender<Mensaje>,
) -> mpsc::Sender<Vec<f32>> {
    let historial = match rutas::historial(handle) {
        Ok(h) => h,
        Err(e) => return motor_ausente(al_director, e),
    };
    println!("historial: {}", historial.display());

    match rutas::modelo(cfg) {
        Ok((ruta, aviso)) => {
            if let Some(aviso) = aviso {
                println!("{aviso}");
                eventos::aviso(handle, &aviso, "info");
            }
            println!("cargando el modelo {}…", ruta.display());
            motor::lanzar(al_director.clone(), ruta, historial)
        }
        Err(e) => motor_ausente(al_director, e),
    }
}

/// Un canal sin nadie del otro lado. Mandar audio ahí falla enseguida, que es
/// exactamente lo que el director sabe informar.
fn motor_ausente(al_director: &mpsc::Sender<Mensaje>, motivo: String) -> mpsc::Sender<Vec<f32>> {
    eprintln!("el motor no arranca: {motivo}");
    let _ = al_director.send(Mensaje::MotorListo(Err(motivo)));
    let (huerfano, _) = mpsc::channel();
    huerfano
}
