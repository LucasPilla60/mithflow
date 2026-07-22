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
use crate::{atajo, motor, rutas, superpuesta};
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
///
/// # Por qué ese aviso NO se emite acá
///
/// Porque desde acá no se puede saber si es verdad. Este comando ve que la clave
/// guardada cambió y nada más; **si hay un motor y con qué archivo lo sabe el
/// director**, y en el camino más común del asistente —`"auto"`, se baja el
/// `Q4_K_M`, el motor empieza a cargarlo, el perfilado lo recomienda y se guarda
/// `"Q4_K_M"`— la clave cambia sin que cambie el modelo. Avisar acá era decirle
/// al usuario que reinicie justo cuando el motor está cargando lo que acaba de
/// elegir. La decisión viaja con [`Mensaje::Ajustados`] y la toma
/// `director::aviso_al_cambiar_el_modelo`, sobre la misma regla que las
/// descargas.
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
    /// Dónde puede aparecer la ventanita de grabación. Sale del módulo que la
    /// dibuja, para que Ajustes no ofrezca una posición que nadie sabe ubicar.
    pub posiciones_indicador: Vec<&'static str>,
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
                // Del núcleo y no de una tabla propia: es el mismo piso con el
                // que el perfilado recomienda y con el que el arranque
                // sustituye, así que el aviso de la interfaz no puede
                // contradecirlos.
                ram_minima_gb: hardware::ram_minima_gb(m),
            })
            .collect(),
        modelo_de_perfilado: clave_modelo(Modelo::DE_PERFILADO),
        limite_grabacion_minimo: ajustes::LIMITE_GRABACION_MINIMO,
        limite_grabacion_maximo: ajustes::LIMITE_GRABACION_MAXIMO,
        posiciones_indicador: superpuesta::POSICIONES.to_vec(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// La ventanita avisa que el usuario la está arrastrando. Lo invoca **ella**,
/// que es la única que ve el mouse.
///
/// **Sincrónico a propósito, y NO se puede volver `async`.** Tauri corre los
/// comandos que no son `async` en el hilo de la IPC —que en Windows es el
/// principal—, y `tauri-runtime-wry` tiene ahí un atajo: `send_user_message`
/// ejecuta el mensaje **en el acto** si ya está en el hilo principal, en vez de
/// encolarlo en el bucle de eventos. Eso es lo que hace que `start_dragging`
/// corra el bucle modal de movimiento acá y **no devuelva hasta que el usuario
/// suelta el botón**, que es de lo que depende poder guardar la posición final.
///
/// Con `async` el comando saldría a un hilo del pool, el mensaje se encolaría y
/// la llamada volvería enseguida: se guardaría la posición **inicial** y el
/// arrastre entero se perdería, sin ningún error a la vista. Ver
/// [`superpuesta::empezar_a_arrastrar`].
///
/// No recibe la posición: ésa la informa la ventana al moverse, y el frontend no
/// tiene por qué poder decidir dónde queda.
#[tauri::command]
pub fn arrastrar_indicador(app: AppHandle) -> Result<(), String> {
    superpuesta::empezar_a_arrastrar(&app)
}

/// ¿El usuario movió la ventanita alguna vez? Ajustes lo necesita para saber si
/// el botón de volver a la posición de fábrica tiene algo que hacer.
///
/// `async` para que Tauri lo saque del hilo principal: toma el mutex del store,
/// que el hilo del director puede estar sosteniendo mientras escribe el archivo.
#[tauri::command]
pub async fn indicador_movido(app: AppHandle) -> bool {
    superpuesta::posicion_recordada(&app).is_some()
}

/// Olvida la posición arrastrada: la ventanita vuelve a aparecer donde diga el
/// desplegable y a seguir al mouse entre monitores.
///
/// `async` por lo mismo, y con más razón: escribe `ajustes.json`, y una escritura
/// sincrónica en el hilo principal con el antivirus mirando congela la interfaz.
#[tauri::command]
pub async fn restablecer_posicion_indicador(app: AppHandle) -> Result<(), String> {
    superpuesta::olvidar_posicion(&app)
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
    fn medido(perfil: hardware::PerfilHardware) -> Self {
        Self {
            ram_total_gb: perfil.ram_total_gb,
            gpu_nombre: perfil.gpu_nombre,
            gpu_clase: perfil.gpu_clase,
            gpu_dedicada: perfil.gpu_dedicada,
            backend: perfil.backend,
            factor_tiempo_real: perfil.factor_tiempo_real,
            modelo_recomendado: clave_modelo(perfil.modelo_recomendado),
            etiqueta_recomendado: perfil.modelo_recomendado.to_string(),
            error: None,
        }
    }

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
///
/// # Por qué espera al motor y por qué reusa su medición
///
/// Desde que una descarga arranca el motor sin reiniciar, el asistente baja el
/// modelo de medición, eso lanza el motor, y menos de un segundo después esto
/// empieza a medir. Medir con otra carga peleando por la misma GPU daría una
/// máquina más lenta de lo que es. Peor todavía: los dos `Transcriber` quedarían
/// residentes a la vez, y en una notebook con gráficos integrados y 8 GB el
/// segundo puede caer a CPU en silencio. Por eso se espera el testigo **y**, si
/// el motor ya midió con el mismo modelo, se usa su número en vez de abrir una
/// segunda copia (ver `motor::medir_si_es_el_de_perfilado`).
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
            // Espera a que el motor termine de cargar. Con tope: si del otro
            // lado se colgó un driver, medir mal es mejor que no medir nunca.
            let exclusiva = motor::testigo_de_carga(motor::TOPE_DE_ESPERA);
            if exclusiva.is_none() {
                eprintln!(
                    "el motor no soltó el testigo en {} s; mido igual, el número puede salir bajo",
                    motor::TOPE_DE_ESPERA.as_secs()
                );
            }

            let residente = motor::residente();
            let carga = medir(&ruta, residente.as_ref());
            // Se suelta ACÁ y no al terminar el hilo: emitir no necesita la
            // placa, y el motor no tiene por qué esperar a que un evento cruce
            // al webview para empezar a cargar.
            drop(exclusiva);

            if let Some(aviso) = discrepancia_de_backend(&carga, residente.as_ref()) {
                eventos::aviso(&app, &aviso, "error");
            }
            if let Err(e) = app.emit(eventos::PERFILADO_LISTO, carga) {
                eprintln!("no pude emitir el perfilado: {e}");
            }
        })
        .map_err(|e| format!("no pude crear el hilo de perfilado: {e}"))?;
    Ok(())
}

/// Mide la máquina, **reusando el motor si ya midió con este mismo modelo**.
///
/// Ese atajo es lo que evita la segunda copia del modelo en memoria; ver la
/// nota de [`perfilar_hardware`]. Cuando no hay nada que reusar —no hay motor,
/// o tiene cargado otro modelo— se mide como siempre.
fn medir(ruta: &std::path::Path, residente: Option<&motor::MotorResidente>) -> PerfilDto {
    if let Some((factor, backend)) = residente.and_then(motor::MotorResidente::medicion_reutilizable)
    {
        println!("perfilado: reuso la medición del motor ({factor:.2}x sobre {backend})");
        return PerfilDto::medido(hardware::perfil_con_factor(factor, backend));
    }
    match hardware::perfilar(ruta) {
        Ok(perfil) => PerfilDto::medido(perfil),
        Err(e) => PerfilDto::fallado(e),
    }
}

/// El aviso cuando la medición y el motor no ligaron al mismo backend.
///
/// Es la caída a CPU en silencio: si el motor anda por Vulkan y la medición
/// midió por procesador (o al revés), el número que decide el modelo **no es el
/// de la máquina que va a dictar**, y la recomendación sale de una medición que
/// no corresponde. Callarlo sería el error caro de este módulo: silencioso, y se
/// lleva puesta la calidad de todos los dictados que vengan después.
///
/// `None` cuando no hay con qué comparar (no hay motor, o el perfil falló) o
/// cuando coinciden, que es lo normal.
fn discrepancia_de_backend(
    perfil: &PerfilDto,
    residente: Option<&motor::MotorResidente>,
) -> Option<String> {
    if perfil.error.is_some() || perfil.backend.is_empty() {
        return None;
    }
    let del_motor = residente.map(|r| r.backend.as_str())?;
    if del_motor.eq_ignore_ascii_case(&perfil.backend) {
        return None;
    }
    Some(format!(
        "Medí tu máquina con «{}» pero el motor está dictando con «{del_motor}». \
         La recomendación puede quedar corta: volvé a medirla desde Ajustes cuando \
         el motor esté cargado.",
        perfil.backend
    ))
}

/// Baja un modelo del catálogo informando por `progreso-descarga`.
///
/// La clave se valida contra el catálogo compilado: la URL y el SHA-256 salen
/// de ahí, nunca de lo que mande el frontend.
///
/// # Qué pasa cuando termina
///
/// Se le avisa al director con [`Mensaje::ModeloDescargado`] y **ahí termina la
/// responsabilidad de este hilo**. No decide nada más: si el motor está ausente
/// —la app recién instalada, que es el caso que este aviso vino a cerrar— lo
/// lanza el director, y si ya hay uno corriendo el cambio de modelo sigue siendo
/// cosa del próximo arranque. La decisión vive allá porque el estado del motor
/// lo conoce el director y nadie más; consultarlo desde acá sería una carrera
/// con dos descargas seguidas.
#[tauri::command]
pub fn descargar_modelo(
    app: AppHandle,
    clave: String,
    al_director: State<'_, AlDirector>,
) -> Result<(), String> {
    let modelo = ajustes::modelo_de_clave(&clave)
        .ok_or_else(|| format!("no conozco el modelo '{clave}'"))?;
    let Some(testigo) = Testigo::tomar(&DESCARGANDO) else {
        return Err("ya hay una descarga en curso".to_string());
    };
    let etiqueta = clave_modelo(modelo);
    // El `State` no se puede mover al hilo; el emisor sí, y clonarlo no cuesta.
    let avisar_al_director = al_director.emisor();

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
                    // Antes que el progreso final: el director es quien cuenta
                    // qué significa la descarga —motor lanzándose o cambio para
                    // el próximo arranque— y ese aviso tiene que llegar sin
                    // depender de que la ventana esté abierta.
                    if avisar_al_director.send(Mensaje::ModeloDescargado).is_err() {
                        eprintln!("el director no escucha: el modelo bajado no arranca el motor");
                    }
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
        let catalogo = leer_catalogo();
        for dto in &catalogo.modelos {
            let modelo = ajustes::modelo_de_clave(dto.clave).expect("clave del catálogo");
            assert_eq!(dto.ram_minima_gb, hardware::ram_minima_gb(modelo));
            // Y el mismo número tiene que ser el que decide si entra: sin esto,
            // la interfaz podría decir "no te entra" de un modelo que el
            // arranque sustituye igual.
            assert_eq!(
                dto.ram_minima_gb <= 8.0,
                hardware::entra_en_memoria(modelo, 8.0, false),
                "{} no coincide con el criterio de memoria en una máquina de 8 GB",
                dto.clave
            );
        }
        assert_eq!(
            hardware::ram_minima_gb(Modelo::Q4KM),
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

    fn motor_en(backend: &str, factor: Option<f32>) -> motor::MotorResidente {
        motor::MotorResidente {
            backend: backend.to_string(),
            factor_tiempo_real: factor,
        }
    }

    /// Con el motor ya cargado y medido, el perfilado **no abre un segundo
    /// modelo**: usa ese número. Es lo que evita tener dos copias de los pesos
    /// residentes a la vez, que en una máquina de 8 GB con gráficos integrados
    /// es la diferencia entre medir bien y medir una máquina más lenta.
    ///
    /// Que no cargue nada es justamente lo que hace que este test pueda correr
    /// sin un `.gguf` de 500 MB en el disco.
    #[test]
    fn con_el_motor_ya_medido_el_perfilado_reusa_su_numero() {
        let perfil = medir(
            std::path::Path::new("no-existe.gguf"),
            Some(&motor_en("vulkan", Some(3.5))),
        );

        assert_eq!(perfil.error, None, "no tenía que intentar cargar nada");
        assert_eq!(perfil.factor_tiempo_real, 3.5);
        assert_eq!(perfil.backend, "vulkan");
        assert!(
            !perfil.modelo_recomendado.is_empty(),
            "la recomendación tiene que salir igual que midiendo de cero"
        );
    }

    /// Y el atajo no se toma cuando no hay nada que reusar: sin motor, o con
    /// uno cargado con otro modelo (cuya medición no es comparable). Ahí se
    /// mide como siempre, abriendo el modelo de perfilado.
    #[test]
    fn sin_medicion_del_motor_no_hay_atajo() {
        let sin_medir = motor_en("vulkan", None);
        assert!(
            sin_medir.medicion_reutilizable().is_none(),
            "un motor cargado con otro modelo no tiene medición que sirva"
        );
        let sin_motor: Option<&motor::MotorResidente> = None;
        assert!(sin_motor
            .and_then(motor::MotorResidente::medicion_reutilizable)
            .is_none());
    }

    /// Dos backends distintos significan que el número que decidió el modelo no
    /// es el de la máquina que va a dictar. No puede pasar en silencio.
    #[test]
    fn un_backend_distinto_al_del_motor_se_avisa() {
        let mut perfil = PerfilDto::medido(hardware::perfil_con_factor(3.5, "cpu".to_string()));
        let aviso = discrepancia_de_backend(&perfil, Some(&motor_en("vulkan", None)))
            .expect("medir por CPU con el motor en Vulkan tiene que avisarse");
        assert!(aviso.contains("cpu") && aviso.contains("vulkan"), "{aviso}");

        // Y lo normal —el mismo backend— no molesta a nadie. Ni siquiera si
        // uno viene en mayúsculas.
        assert_eq!(
            discrepancia_de_backend(&perfil, Some(&motor_en("CPU", None))),
            None
        );
        assert_eq!(discrepancia_de_backend(&perfil, None), None, "no hay motor");

        perfil.error = Some("no pude medir".into());
        assert_eq!(
            discrepancia_de_backend(&perfil, Some(&motor_en("vulkan", None))),
            None,
            "un perfilado que falló ya se cuenta solo; no hay backend que comparar"
        );
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
