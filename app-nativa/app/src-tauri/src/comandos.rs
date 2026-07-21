//! Los comandos que el frontend puede invocar.
//!
//! # La regla que gobierna este módulo
//!
//! **El dashboard tiene que poder reconstruir todo al montarse.** Los eventos
//! cuentan novedades y sólo llegan a quien esté escuchando; un dictado hecho
//! con la ventana cerrada no le llega a nadie. Por eso cada evento tiene su
//! comando espejo: `estado-cambiado`/[`leer_estado`],
//! `dictado-nuevo`/[`leer_historial`]. Sin esa mitad, abrir la ventana
//! mostraría una lista vacía.
//!
//! Los tres comandos que arrancan trabajo largo (descargar, perfilar) devuelven
//! enseguida y avisan por evento: bloquear el `invoke` colgaría la interfaz por
//! minutos, y encima Tauri lo corre en su pool, no en el hilo del director.

use crate::ajustes::{self, clave_modelo, Ajustes};
use crate::director::{AlDirector, Mensaje};
use crate::estado::{EstadoCompartido, EstadoDto};
use crate::eventos::{self, ProgresoDescarga};
use crate::sonidos::Sonidos;
use crate::{atajo, rutas};
use mithflow_core::history::Entry;
use mithflow_core::models::{self, Modelo};
use mithflow_core::{hardware, history};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_autostart::ManagerExt;

/// Tope de entradas que se devuelven de una. El historial crece para siempre y
/// serializar 50.000 dictados a la interfaz no ayuda a nadie.
const MAX_HISTORIAL: usize = 500;

/// Una descarga a la vez. Dos descargas del mismo modelo escribirían el mismo
/// `.part` y el archivo resultante no verificaría: media hora perdida y un
/// error confuso.
static DESCARGANDO: AtomicBool = AtomicBool::new(false);

/// Un perfilado a la vez: carga un modelo entero en memoria.
static PERFILANDO: AtomicBool = AtomicBool::new(false);

/// Libera el testigo aunque el hilo paniquee. Sin esto, un panic dejaría la
/// función inutilizable hasta reiniciar la aplicación.
struct Testigo(&'static AtomicBool);

impl Testigo {
    /// `None` si ya había alguien adentro.
    fn tomar(bandera: &'static AtomicBool) -> Option<Self> {
        bandera
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Testigo(bandera))
    }
}

impl Drop for Testigo {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[tauri::command]
pub fn leer_estado(espejo: State<'_, Arc<EstadoCompartido>>) -> EstadoDto {
    espejo.leer()
}

#[tauri::command]
pub fn leer_ajustes(app: AppHandle) -> Ajustes {
    ajustes::cargar(&app)
}

/// Guarda y **aplica** lo que se puede aplicar en caliente: la tecla del atajo,
/// los sonidos y el autoarranque. El modelo recién cambia al reiniciar, porque
/// cargarlo son 1,8 s y 1,5 GB de memoria: se avisa en vez de hacerlo a
/// escondidas.
#[tauri::command]
pub fn escribir_ajustes(
    app: AppHandle,
    nuevos: Ajustes,
    sonidos: State<'_, Sonidos>,
) -> Result<Ajustes, String> {
    let anteriores = ajustes::cargar(&app);
    let guardados = ajustes::guardar(&app, nuevos)?;

    atajo::fijar_tecla(&guardados.tecla);
    sonidos.configurar(guardados.sonidos, guardados.volumen);

    if guardados.arranque_con_windows != anteriores.arranque_con_windows {
        aplicar_autoarranque(&app, guardados.arranque_con_windows);
    }
    if guardados.modelo != anteriores.modelo {
        eventos::aviso(
            &app,
            "El modelo cambia la próxima vez que abras MithFlow.",
            "info",
        );
    }
    Ok(guardados)
}

/// Sincroniza el autoarranque de Windows con el ajuste.
///
/// Se consulta el estado real ANTES de tocar nada, y no sólo por prolijidad:
/// `disable()` borra un valor del registro y falla si no existe, así que
/// llamarlo a ciegas en cada arranque —el caso normal, porque el autoarranque
/// viene apagado— tiraría un error espurio todas las veces.
///
/// Un fallo acá no puede tumbar nada: se avisa y el ajuste queda guardado.
pub fn aplicar_autoarranque(app: &AppHandle, activo: bool) {
    let gestor = app.autolaunch();
    match gestor.is_enabled() {
        Ok(actual) if actual == activo => return,
        Ok(_) => {}
        // Si no se puede ni consultar, se intenta igual: el estado deseado es
        // lo que manda.
        Err(e) => eprintln!("no pude consultar el arranque con Windows: {e}"),
    }

    let salida = if activo { gestor.enable() } else { gestor.disable() };
    if let Err(e) = salida {
        let texto = format!("No pude cambiar el arranque con Windows: {e}");
        eprintln!("{texto}");
        eventos::aviso(app, &texto, "error");
    }
}

/// Los últimos `limite` dictados, **del más nuevo al más viejo**, que es el
/// orden en que se muestran y el mismo en que `dictado-nuevo` los agrega.
#[tauri::command]
pub fn leer_historial(app: AppHandle, limite: usize) -> Result<Vec<Entry>, String> {
    let ruta = rutas::historial(&app)?;
    let mut entradas = history::load(&ruta)
        .map_err(|e| format!("no pude leer el historial {}: {e}", ruta.display()))?;
    entradas.reverse();
    entradas.truncate(limite.clamp(1, MAX_HISTORIAL));
    Ok(entradas)
}

#[tauri::command]
pub fn pausar(al_director: State<'_, AlDirector>) {
    al_director.enviar(Mensaje::Pausa(true));
}

#[tauri::command]
pub fn reanudar(al_director: State<'_, AlDirector>) {
    al_director.enviar(Mensaje::Pausa(false));
}

#[tauri::command]
pub fn alternar_pausa(al_director: State<'_, AlDirector>) {
    al_director.enviar(Mensaje::AlternarPausa);
}

/// Lo que la interfaz necesita para armar la pantalla de Ajustes sin repetir
/// listas que se desincronizarían.
#[derive(Serialize)]
pub struct Catalogo {
    pub teclas: Vec<&'static str>,
    pub modos_limpieza: Vec<&'static str>,
    pub modelos: Vec<ModeloDto>,
    pub version: String,
}

#[derive(Serialize)]
pub struct ModeloDto {
    pub clave: &'static str,
    pub etiqueta: String,
    pub megabytes: u64,
    pub descargado: bool,
}

#[tauri::command]
pub fn leer_catalogo() -> Catalogo {
    Catalogo {
        teclas: atajo::teclas_admitidas(),
        modos_limpieza: ajustes::MODOS_LIMPIEZA.to_vec(),
        modelos: Modelo::TODOS
            .into_iter()
            .map(|m| ModeloDto {
                clave: clave_modelo(m),
                etiqueta: m.to_string(),
                megabytes: m.megabytes(),
                descargado: models::esta_descargado(m),
            })
            .collect(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Lo que midió el perfilado. `PerfilHardware` no es serializable y tampoco
/// debería serlo: este DTO es el contrato con la interfaz y puede cambiar sin
/// tocar el núcleo.
#[derive(Serialize, Clone)]
pub struct PerfilDto {
    pub ram_total_gb: f32,
    pub gpu_nombre: Option<String>,
    pub backend: String,
    pub factor_tiempo_real: f32,
    pub modelo_recomendado: &'static str,
    pub etiqueta_recomendado: String,
    pub error: Option<String>,
}

/// Mide esta máquina y emite `perfilado-listo` cuando termina (~20 s).
///
/// Necesita el modelo de perfilado descargado: es el más liviano justamente
/// porque esto corre antes de saber cuál conviene.
#[tauri::command]
pub fn perfilar_hardware(app: AppHandle) -> Result<(), String> {
    let Some(testigo) = Testigo::tomar(&PERFILANDO) else {
        return Err("ya estoy midiendo la máquina".to_string());
    };
    let de_prueba = Modelo::DE_PERFILADO;
    if !models::esta_descargado(de_prueba) {
        return Err(format!(
            "para medir la máquina hace falta {de_prueba} ({} MB); descargalo primero",
            de_prueba.megabytes()
        ));
    }
    let ruta = models::ruta(de_prueba)?;

    std::thread::Builder::new()
        .name("mithflow-perfilado".into())
        .spawn(move || {
            let _testigo = testigo;
            let carga = match hardware::perfilar(&ruta) {
                Ok(p) => PerfilDto {
                    ram_total_gb: p.ram_total_gb,
                    gpu_nombre: p.gpu_nombre,
                    backend: p.backend,
                    factor_tiempo_real: p.factor_tiempo_real,
                    modelo_recomendado: clave_modelo(p.modelo_recomendado),
                    etiqueta_recomendado: p.modelo_recomendado.to_string(),
                    error: None,
                },
                Err(e) => PerfilDto {
                    ram_total_gb: 0.0,
                    gpu_nombre: None,
                    backend: String::new(),
                    factor_tiempo_real: 0.0,
                    modelo_recomendado: "",
                    etiqueta_recomendado: String::new(),
                    error: Some(e),
                },
            };
            if let Err(e) = app.emit(eventos::PERFILADO_LISTO, carga) {
                eprintln!("no pude emitir el perfilado: {e}");
            }
        })
        .map_err(|e| format!("no pude crear el hilo de perfilado: {e}"))?;
    Ok(())
}

/// Baja un modelo del catálogo informando por `progreso-descarga`.
///
/// La clave se valida contra el catálogo compilado: la URL y el SHA-256 salen
/// de ahí, nunca de lo que mande el frontend.
#[tauri::command]
pub fn descargar_modelo(app: AppHandle, clave: String) -> Result<(), String> {
    let modelo = ajustes::modelo_de_clave(&clave)
        .ok_or_else(|| format!("no conozco el modelo '{clave}'"))?;
    let Some(testigo) = Testigo::tomar(&DESCARGANDO) else {
        return Err("ya hay una descarga en curso".to_string());
    };
    let etiqueta = clave_modelo(modelo);

    std::thread::Builder::new()
        .name("mithflow-descarga".into())
        .spawn(move || {
            let _testigo = testigo;
            let app_progreso = app.clone();
            let salida = models::descargar(modelo, |bytes, total| {
                eventos::progreso_descarga(
                    &app_progreso,
                    ProgresoDescarga::en_curso(etiqueta, bytes, total),
                );
            });
            let final_ = match salida {
                Ok(_) => ProgresoDescarga {
                    terminado: true,
                    ..ProgresoDescarga::en_curso(etiqueta, modelo.bytes(), modelo.bytes())
                },
                Err(e) => ProgresoDescarga::fallada(etiqueta, e),
            };
            eventos::progreso_descarga(&app, final_);
        })
        .map_err(|e| format!("no pude crear el hilo de descarga: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_testigo_deja_pasar_a_uno_solo_y_se_libera_al_soltarlo() {
        static BANDERA: AtomicBool = AtomicBool::new(false);
        let primero = Testigo::tomar(&BANDERA).expect("el primero entra");
        assert!(Testigo::tomar(&BANDERA).is_none(), "el segundo no");
        drop(primero);
        assert!(Testigo::tomar(&BANDERA).is_some(), "una vez libre, se puede de nuevo");
    }

    /// Un `limite` de 0 o disparatado no puede dar un panic ni un volcado
    /// entero del historial.
    #[test]
    fn el_limite_del_historial_esta_acotado_en_los_dos_extremos() {
        assert_eq!(0usize.clamp(1, MAX_HISTORIAL), 1);
        assert_eq!(usize::MAX.clamp(1, MAX_HISTORIAL), MAX_HISTORIAL);
    }
}
