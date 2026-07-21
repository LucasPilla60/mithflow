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
use mithflow_core::{hardware, history, metricas};
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
    al_director: State<'_, AlDirector>,
) -> Result<Ajustes, String> {
    let anteriores = ajustes::cargar(&app);
    let guardados = ajustes::guardar(&app, nuevos)?;

    atajo::fijar_tecla(&guardados.tecla);
    sonidos.configurar(guardados.sonidos, guardados.volumen);
    // Vocabulario, muletillas, modo de limpieza, privacidad del historial y
    // tope de grabación: el director aplica lo suyo y le pasa al motor el
    // resto. Sin esta línea los cinco se guardarían y no harían nada, que es
    // exactamente el defecto que este plan vino a cerrar.
    al_director.enviar(Mensaje::Ajustados(Box::new(guardados.clone())));

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

/// Una página del historial y cuánto hay detrás.
#[derive(Serialize)]
pub struct PaginaHistorial {
    pub entradas: Vec<Entry>,
    /// Cuántas entradas hay en total **después de buscar**: es el número que
    /// sostiene "mostrando 50 de 312".
    pub total: usize,
    pub hay_mas: bool,
}

/// Una página de dictados, **del más nuevo al más viejo**, que es el orden en
/// que se muestran y el mismo en que `dictado-nuevo` los agrega.
///
/// # Por qué se pagina y se busca acá
///
/// El dashboard de Streamlit levantaba el archivo entero y dibujaba todas las
/// filas. Paginar sólo en la interfaz no arreglaría lo que importa: el texto
/// dictado de años cruzando al webview para mostrar veinte líneas. Buscar acá
/// además busca en TODO el historial y no sólo en lo ya cargado, que es lo que
/// el usuario espera de un buscador.
#[tauri::command]
pub fn leer_historial(
    app: AppHandle,
    limite: usize,
    desplazamiento: usize,
    buscar: Option<String>,
) -> Result<PaginaHistorial, String> {
    let ruta = rutas::historial(&app)?;
    let entradas = history::load(&ruta)
        .map_err(|e| format!("no pude leer el historial {}: {e}", ruta.display()))?;
    Ok(paginar(&entradas, limite, desplazamiento, buscar.as_deref()))
}

/// El filtrado y el corte, sin el disco: es la parte que tiene bordes (el
/// desplazamiento más allá del final, el buscador vacío, el límite absurdo) y
/// la que se puede probar sin levantar una aplicación de Tauri.
fn paginar(
    entradas: &[Entry],
    limite: usize,
    desplazamiento: usize,
    buscar: Option<&str>,
) -> PaginaHistorial {
    // La búsqueda es por subcadena sin distinguir mayúsculas, NUNCA una
    // expresión regular: escribir "(" en el buscador no puede romper la vista
    // (es el mismo bug que ya mordió en `dashboard.py`).
    let aguja = buscar
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty());
    let coincide = |e: &Entry| match &aguja {
        None => true,
        Some(aguja) => e.final_text.to_lowercase().contains(aguja),
    };

    // `rev()`: del más nuevo al más viejo, que es como se muestran y el mismo
    // orden en que `dictado-nuevo` agrega arriba.
    let filtradas: Vec<&Entry> = entradas.iter().rev().filter(|e| coincide(e)).collect();
    let total = filtradas.len();
    let pagina: Vec<Entry> = filtradas
        .into_iter()
        .skip(desplazamiento)
        .take(limite.clamp(1, MAX_HISTORIAL))
        .cloned()
        .collect();

    PaginaHistorial {
        hay_mas: desplazamiento + pagina.len() < total,
        entradas: pagina,
        total,
    }
}

/// Todo lo que el dashboard muestra arriba, ya calculado sobre el historial
/// completo. Ver [`mithflow_core::metricas`] para las cuentas y para el error
/// de latencia que corrigen.
#[tauri::command]
pub fn leer_metricas(app: AppHandle) -> Result<metricas::Metricas, String> {
    let ruta = rutas::historial(&app)?;
    let entradas = history::load(&ruta)
        .map_err(|e| format!("no pude leer el historial {}: {e}", ruta.display()))?;
    Ok(metricas::calcular(&entradas, chrono::Local::now().date_naive()))
}

/// Borra el historial entero, sin vuelta atrás.
///
/// Es una acción destructiva y a propósito no tiene "deshacer": la interfaz
/// pide confirmación antes de llamar. Existe porque el historial es lo único
/// que esta app deja escrito en claro con todo lo que el usuario dijo, y no
/// poder borrarlo sería una trampa.
#[tauri::command]
pub fn borrar_historial(app: AppHandle) -> Result<(), String> {
    let ruta = rutas::historial(&app)?;
    history::clear(&ruta)
        .map_err(|e| format!("no pude borrar el historial {}: {e}", ruta.display()))?;
    eventos::aviso(&app, "Historial borrado.", "info");
    Ok(())
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
    /// Con cuál se mide la máquina. El asistente tiene que bajarlo antes de
    /// poder perfilar, así que necesita saber cuál es sin adivinarlo.
    pub modelo_de_perfilado: &'static str,
    pub limite_grabacion_minimo: u32,
    pub limite_grabacion_maximo: u32,
    pub version: String,
}

#[derive(Serialize)]
pub struct ModeloDto {
    pub clave: &'static str,
    pub etiqueta: String,
    pub megabytes: u64,
    pub descargado: bool,
    /// Cuánta RAM del sistema pide si tiene que correr sin GPU dedicada.
    /// Es lo que sostiene el aviso "este modelo no te entra".
    pub ram_minima_gb: f32,
}

/// La RAM que este modelo necesita cuando los pesos viven en memoria del
/// sistema. Sale de los mismos umbrales con los que decide el perfilado, para
/// que el aviso de la interfaz y la recomendación no puedan contradecirse.
fn ram_minima_gb(modelo: Modelo) -> f32 {
    match modelo {
        Modelo::F16 => hardware::RAM_MINIMA_F16_GB,
        Modelo::Q5KM => hardware::RAM_MINIMA_Q5_GB,
        Modelo::Q4KM => 0.0,
    }
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
                ram_minima_gb: ram_minima_gb(m),
            })
            .collect(),
        modelo_de_perfilado: clave_modelo(Modelo::DE_PERFILADO),
        limite_grabacion_minimo: ajustes::LIMITE_GRABACION_MINIMO,
        limite_grabacion_maximo: ajustes::LIMITE_GRABACION_MAXIMO,
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
    /// `"dedicada"`, `"integrada"`, `"cpu"`… o `None` si no se enumeró ninguna.
    pub gpu_clase: Option<&'static str>,
    /// El asistente lo necesita para avisar que un modelo elegido a mano no
    /// entra en memoria: sin placa propia, los pesos compiten con el sistema.
    pub gpu_dedicada: bool,
    pub backend: String,
    pub factor_tiempo_real: f32,
    pub modelo_recomendado: &'static str,
    pub etiqueta_recomendado: String,
    pub error: Option<String>,
}

impl PerfilDto {
    /// El perfil que se emite cuando la medición no se pudo hacer. Los ceros no
    /// son datos: van acompañados del `error`, que es lo que la interfaz
    /// muestra.
    fn fallado(motivo: String) -> Self {
        Self {
            ram_total_gb: 0.0,
            gpu_nombre: None,
            gpu_clase: None,
            gpu_dedicada: false,
            backend: String::new(),
            factor_tiempo_real: 0.0,
            modelo_recomendado: "",
            etiqueta_recomendado: String::new(),
            error: Some(motivo),
        }
    }
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
                    gpu_clase: p.gpu_clase,
                    gpu_dedicada: p.gpu_dedicada,
                    backend: p.backend,
                    factor_tiempo_real: p.factor_tiempo_real,
                    modelo_recomendado: clave_modelo(p.modelo_recomendado),
                    etiqueta_recomendado: p.modelo_recomendado.to_string(),
                    error: None,
                },
                Err(e) => PerfilDto::fallado(e),
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
                Ok(_) => {
                    // Cerrar el lazo: el motor carga el modelo UNA vez, al
                    // arrancar. Sin este aviso, alguien que abrió la app sin
                    // ningún modelo, la vio en Error y bajó uno desde Ajustes
                    // se queda mirando el mismo Error sin saber que ya está.
                    eventos::aviso(
                        &app,
                        "Modelo descargado. Reiniciá MithFlow para empezar a usarlo.",
                        "info",
                    );
                    ProgresoDescarga {
                        terminado: true,
                        ..ProgresoDescarga::en_curso(etiqueta, modelo.bytes(), modelo.bytes())
                    }
                }
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

    /// La RAM mínima que se le muestra al usuario tiene que salir de los mismos
    /// umbrales con los que decide el perfilado: si se copiaran a mano, la
    /// interfaz podría avisar "no te entra" de un modelo que el perfilado sí
    /// recomienda.
    #[test]
    fn la_ram_minima_que_se_informa_es_la_que_usa_el_perfilado() {
        assert_eq!(ram_minima_gb(Modelo::F16), hardware::RAM_MINIMA_F16_GB);
        assert_eq!(ram_minima_gb(Modelo::Q5KM), hardware::RAM_MINIMA_Q5_GB);
        assert_eq!(
            ram_minima_gb(Modelo::Q4KM),
            0.0,
            "el modelo más chico es el piso: nunca se descarta por memoria"
        );
    }

    fn entrada(ts: &str, texto: &str) -> Entry {
        Entry {
            ts: ts.to_string(),
            audio_s: 1.0,
            transcribe_s: 0.2,
            cleanup_s: 0.0,
            words: texto.split_whitespace().count(),
            cleaned: false,
            mode: "fast".into(),
            raw: texto.into(),
            final_text: texto.into(),
        }
    }

    fn historial_de_prueba() -> Vec<Entry> {
        vec![
            entrada("2026-07-21T10:00:00", "el primero, sobre el CRM"),
            entrada("2026-07-21T11:00:00", "el segundo, sobre el dashboard"),
            entrada("2026-07-21T12:00:00", "el tercero (con paréntesis)"),
        ]
    }

    /// La página llega del más nuevo al más viejo y dice cuánto queda detrás.
    #[test]
    fn la_primera_pagina_trae_lo_mas_nuevo_y_avisa_que_hay_mas() {
        let pagina = paginar(&historial_de_prueba(), 2, 0, None);
        assert_eq!(pagina.entradas.len(), 2);
        assert_eq!(pagina.entradas[0].ts, "2026-07-21T12:00:00", "no vino el más nuevo");
        assert_eq!(pagina.total, 3);
        assert!(pagina.hay_mas);
    }

    /// Y la última no promete una página que no existe: sin esto, el botón
    /// "Cargar más" quedaría para siempre y cada clic traería una lista vacía.
    #[test]
    fn la_ultima_pagina_no_promete_mas() {
        let pagina = paginar(&historial_de_prueba(), 2, 2, None);
        assert_eq!(pagina.entradas.len(), 1);
        assert!(!pagina.hay_mas);

        let vacia = paginar(&historial_de_prueba(), 2, 99, None);
        assert!(vacia.entradas.is_empty(), "pasado el final no hay nada");
        assert!(!vacia.hay_mas);
        assert_eq!(vacia.total, 3, "el total sigue siendo el del historial");
    }

    /// La búsqueda mira TODO el historial, no sólo la página cargada, e ignora
    /// mayúsculas.
    #[test]
    fn la_busqueda_filtra_todo_el_historial_sin_distinguir_mayusculas() {
        let pagina = paginar(&historial_de_prueba(), 50, 0, Some("DASHBOARD"));
        assert_eq!(pagina.total, 1);
        assert_eq!(pagina.entradas[0].ts, "2026-07-21T11:00:00");

        let nada = paginar(&historial_de_prueba(), 50, 0, Some("no está"));
        assert_eq!(nada.total, 0);
        assert!(nada.entradas.is_empty());
    }

    /// Un buscador vacío (o con espacios) es "sin filtro", no "buscar la cadena
    /// vacía", que daría lo mismo pero por casualidad.
    #[test]
    fn un_buscador_vacio_no_filtra() {
        assert_eq!(paginar(&historial_de_prueba(), 50, 0, Some("   ")).total, 3);
        assert_eq!(paginar(&historial_de_prueba(), 50, 0, Some("")).total, 3);
    }

    /// El buscador recibe texto del usuario: un paréntesis es un carácter, no
    /// el principio de un grupo de captura.
    #[test]
    fn el_buscador_trata_los_metacaracteres_como_texto() {
        let pagina = paginar(&historial_de_prueba(), 50, 0, Some("(con paréntesis)"));
        assert_eq!(pagina.total, 1, "la búsqueda literal no encontró el texto");

        let ninguna = paginar(&historial_de_prueba(), 50, 0, Some(".*"));
        assert_eq!(ninguna.total, 0, "se interpretó como expresión regular");
    }

    /// El catálogo es lo único que la interfaz ve del modelo de perfilado: si
    /// nombrara uno que no está en la lista, el asistente intentaría descargar
    /// algo inexistente.
    #[test]
    fn el_catalogo_nombra_un_modelo_de_perfilado_que_existe() {
        let catalogo = leer_catalogo();
        assert!(
            catalogo
                .modelos
                .iter()
                .any(|m| m.clave == catalogo.modelo_de_perfilado),
            "el modelo de perfilado no está en el catálogo"
        );
        assert!(catalogo.limite_grabacion_minimo < catalogo.limite_grabacion_maximo);
        assert!(!catalogo.teclas.is_empty());
    }
}
