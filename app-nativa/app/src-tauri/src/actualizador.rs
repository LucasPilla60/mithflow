//! Actualizaciones automáticas contra GitHub Releases.
//!
//! # La firma es lo único que hace esto seguro
//!
//! Este módulo baja un `.exe` de internet y lo ejecuta. Sin verificar nada, eso
//! es una puerta trasera con forma de comodidad: quien pueda contestar en lugar
//! del endpoint —un DNS envenenado, una release ajena, un proxy corporativo—
//! consigue ejecución de código en las tres máquinas.
//!
//! Lo que lo cierra es la firma **minisign**: `tauri.conf.json` lleva la clave
//! pública, el manifiesto lleva la firma del instalador, y
//! [`tauri_plugin_updater::Update::download`] la verifica **antes** de devolver
//! los bytes. Un artefacto sin firma, con otra firma o alterado no llega nunca a
//! [`tauri_plugin_updater::Update::install`]. La clave privada vive fuera del
//! árbol del proyecto (ver `README.md`) y no se commitea jamás.
//!
//! Esto **no** es firma de código de Windows: SmartScreen va a seguir avisando
//! al instalar a mano. Son cosas distintas y sólo la segunda cuesta plata.
//!
//! # Las tres reglas de convivencia
//!
//! 1. **Un fallo de red no molesta.** Sin internet, con GitHub caído o con el
//!    endpoint todavía sin configurar, la consulta falla, se anota y la
//!    aplicación sigue exactamente igual. Nada de este módulo corre antes que el
//!    motor, y nada de acá puede impedir dictar.
//! 2. **No se interrumpe un dictado.** Grabando o transcribiendo no se avisa
//!    (ver [`publicar`]) y no se instala (ver [`instalar_actualizacion`]). La
//!    verificación se repite **después** de bajar el paquete, porque bajar 12 MB
//!    tarda y el estado pudo cambiar en el medio.
//! 3. **El aviso sobrevive a la ventana cerrada.** Esta app arranca en la
//!    bandeja: un evento emitido a los quince segundos no lo escucha nadie. Por
//!    eso la consulta deja el resultado en [`UltimaConsulta`] y
//!    [`leer_actualizacion`] es su comando espejo, igual que
//!    `estado-cambiado`/`leer_estado`.

use crate::estado::{Estado, EstadoCompartido, EstadoDto};
use crate::{desinstalar, eventos};
use serde::Serialize;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

/// Cuánto se espera antes de la consulta del arranque.
///
/// No es prolijidad: al abrir, el motor está cargando el modelo y compilando
/// shaders de Vulkan —entre veinte segundos y un minuto— y ése es el trabajo que
/// decide cuándo se puede dictar. La consulta de actualizaciones es lo último
/// que puede pelearle la máquina, así que espera a que lo peor haya pasado.
const ESPERA_AL_ARRANCAR: Duration = Duration::from_secs(15);

/// Cada cuánto se vuelve a consultar con la app abierta. Cuatro consultas por
/// día a un archivo de GitHub no le cuestan nada a nadie.
const ENTRE_CONSULTAS: Duration = Duration::from_secs(6 * 60 * 60);

/// Tope de cada petición al endpoint. Sin esto, un servidor que acepta la
/// conexión y no contesta deja un hilo colgado para siempre.
const TOPE_DE_CONSULTA: Duration = Duration::from_secs(15);

/// La versión que corre ahora mismo. Sale del `Cargo.toml`, que es el mismo
/// número que `Generar-Instalador.ps1` sincroniza con `package.json` y
/// `tauri.conf.json`; si se desincronizaran, el actualizador compararía contra
/// una versión que no es la que está instalada.
fn version_instalada() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/* ------------------------------------------------------- el resultado */

pub const SIN_CONSULTAR: &str = "sin-consultar";
pub const AL_DIA: &str = "al-dia";
pub const DISPONIBLE: &str = "disponible";
pub const SIN_RED: &str = "sin-red";
pub const NO_INSTALADA: &str = "no-instalada";

/// En qué quedó la última consulta. Es lo que ve la interfaz, y lo mismo que
/// viaja por el evento y por el comando espejo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EstadoActualizacion {
    /// Una de las constantes de arriba. Es lo que decide qué dibuja la interfaz:
    /// se elige acá y no se deriva de nada, así que cambiarla rompe el frontend.
    pub clave: &'static str,
    pub instalada: String,
    /// La versión que se instalaría. Sólo con [`DISPONIBLE`].
    pub disponible: Option<String>,
    /// Las notas de la release, si el manifiesto las trae.
    pub notas: Option<String>,
    /// El texto que se le muestra al usuario, ya armado acá para que la interfaz
    /// no tenga que repetir la lógica de qué significa cada clave.
    pub mensaje: String,
}

impl EstadoActualizacion {
    fn nueva(clave: &'static str, mensaje: String) -> Self {
        Self {
            clave,
            instalada: version_instalada().to_string(),
            disponible: None,
            notas: None,
            mensaje,
        }
    }

    pub fn sin_consultar() -> Self {
        Self::nueva(
            SIN_CONSULTAR,
            "Todavía no me fijé si hay una versión nueva.".to_string(),
        )
    }

    fn al_dia() -> Self {
        Self::nueva(
            AL_DIA,
            format!("Estás en la última versión (v{}).", version_instalada()),
        )
    }

    fn disponible(nueva: &str, notas: Option<String>) -> Self {
        let instalada = version_instalada();
        Self {
            disponible: Some(nueva.to_string()),
            notas,
            ..Self::nueva(
                DISPONIBLE,
                format!("Hay una versión nueva: v{nueva}. Tenés la v{instalada}."),
            )
        }
    }

    /// El endpoint no contestó. **Deliberadamente sin el detalle técnico**: el
    /// motivo exacto va a la salida de diagnóstico, no a una tarjeta que el
    /// usuario no puede accionar, y así tampoco se le muestra una URL interna.
    fn sin_red() -> Self {
        Self::nueva(
            SIN_RED,
            "No pude fijarme si hay una versión nueva (sin conexión, o el servidor no contestó). \
             MithFlow funciona igual: esto no toca el dictado."
                .to_string(),
        )
    }

    /// Corriendo desde `target/release/` no hay instalación que reemplazar. Es
    /// el caso que ve quien programa esto, y decirlo con todas las letras es la
    /// diferencia entre un mensaje y un misterio.
    fn no_instalada() -> Self {
        Self::nueva(
            NO_INSTALADA,
            "Estás usando una copia de desarrollo, no la instalación: no hay nada que actualizar. \
             Las actualizaciones automáticas sólo funcionan sobre lo que dejó el instalador."
                .to_string(),
        )
    }
}

/// El espejo de la última consulta.
///
/// Existe por lo mismo que [`EstadoCompartido`]: un evento cuenta una novedad y
/// sólo le llega a quien esté escuchando. La consulta del arranque ocurre a los
/// quince segundos, cuando lo más probable es que no haya ninguna ventana
/// abierta —esta app vive en la bandeja—, así que sin este espejo el aviso se
/// perdería hasta el próximo arranque.
pub struct UltimaConsulta(RwLock<EstadoActualizacion>);

impl UltimaConsulta {
    pub fn nueva() -> Self {
        Self(RwLock::new(EstadoActualizacion::sin_consultar()))
    }

    /// Si el lock quedó envenenado se devuelve el último valor igual: no hay
    /// invariante entre campos que un panic pueda romper, y dejar la interfaz
    /// sin respuesta sería peor que informar algo viejo.
    pub fn leer(&self) -> EstadoActualizacion {
        match self.0.read() {
            Ok(g) => g.clone(),
            Err(envenenado) => envenenado.into_inner().clone(),
        }
    }

    fn escribir(&self, nuevo: EstadoActualizacion) {
        match self.0.write() {
            Ok(mut g) => *g = nuevo,
            Err(envenenado) => *envenenado.into_inner() = nuevo,
        }
    }
}

impl Default for UltimaConsulta {
    fn default() -> Self {
        Self::nueva()
    }
}

/* ------------------------------------------------- comparar versiones */

/// Las tres partes de `mayor.menor.parche`, o `None` si no se puede leer.
///
/// Tolera la `v` de las etiquetas de git (`v1.2.3`) y descarta el sufijo de
/// pre-release o de build (`1.2.3-beta.1`, `1.2.3+abc`), que este proyecto no
/// usa. Todo lo demás —cuatro números, letras, vacío— devuelve `None`, y quien
/// llama lo trata como "no actualizar": ante una versión que no se entiende, el
/// error barato es no ofrecer nada.
fn partes(version: &str) -> Option<[u64; 3]> {
    let nucleo = version.trim().trim_start_matches('v');
    let nucleo = nucleo.split(['-', '+']).next()?;
    let mut leidas = [0u64; 3];
    for (i, trozo) in nucleo.split('.').enumerate() {
        *leidas.get_mut(i)? = trozo.parse().ok()?;
    }
    Some(leidas)
}

/// ¿`ofrecida` es estrictamente más nueva que `instalada`?
///
/// # Por qué existe si el plugin ya compara
///
/// Porque compara **otra cosa**: `tauri-plugin-updater` decide con `semver` y
/// con lo que diga el manifiesto, y ese manifiesto es un archivo que se sube a
/// mano a una release. Un `latest.json` viejo, uno de otra release o uno
/// generado con la versión equivocada haría que la aplicación se "actualice"
/// hacia atrás, borrando la instalación buena con una vieja. Esta función es el
/// segundo cerrojo: nada se ofrece ni se instala si el número no sube.
pub fn es_mas_nueva(instalada: &str, ofrecida: &str) -> bool {
    match (partes(instalada), partes(ofrecida)) {
        (Some(actual), Some(nueva)) => nueva > actual,
        _ => false,
    }
}

/* ----------------------------------------------------- las precondiciones */

/// ¿Esta copia salió del instalador?
///
/// La señal es la misma que usa la desinstalación —el `uninstall.exe` que NSIS
/// deja al lado del ejecutable— y sale del mismo módulo a propósito: "qué es una
/// instalación" tiene que ser una sola definición, o un día la app se creería
/// instalable y la otra no.
fn esta_instalada() -> bool {
    match std::env::current_exe() {
        Ok(ejecutable) => ejecutable
            .parent()
            .is_some_and(desinstalar::es_una_instalacion),
        Err(e) => {
            eprintln!("no pude saber desde dónde estoy corriendo: {e}");
            false
        }
    }
}

/// Qué impide avisar o actualizar justo ahora.
///
/// Mismo criterio que `desinstalar::ocupado` y por el mismo motivo: grabar y
/// transcribir son las dos operaciones con audio en vuelo, y acá el final del
/// camino es matar el proceso para que corra el instalador. Un dictado perdido a
/// la mitad no vuelve; la actualización espera dos minutos sin problema.
///
/// Las claves salen de [`Estado`] y no de cadenas sueltas: renombrar un estado
/// tiene que romper la compilación, no dejar esta guarda mirando un valor que ya
/// nadie publica.
fn ocupado(estado: &EstadoDto) -> Option<String> {
    if estado.estado == Estado::Grabando.clave() {
        return Some("Estás grabando. Terminá el dictado y volvé a intentar.".to_string());
    }
    if estado.estado == Estado::Transcribiendo.clave() {
        return Some("Estoy transcribiendo un dictado. Esperá a que termine.".to_string());
    }
    None
}

fn estado_actual(app: &AppHandle) -> EstadoDto {
    app.state::<Arc<EstadoCompartido>>().leer()
}

/* ------------------------------------------------------------ la consulta */

/// Lo que anunció el endpoint, sin nada de `tauri-plugin-updater` adentro.
///
/// Existe para que [`interpretar`] se pueda probar sin red y sin una aplicación
/// de Tauri levantada, que es justo lo que hace falta para cubrir el caso más
/// frecuente de todos: el endpoint que no contesta.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ofrecida {
    version: String,
    notas: Option<String>,
}

/// Qué se informa según lo que haya contestado el endpoint.
///
/// `Err(())` es "no contestó", y agrupa a propósito todo lo que puede salir mal
/// afuera —sin internet, DNS caído, GitHub con un 500, el endpoint todavía sin
/// configurar, un `latest.json` ilegible, el tope de espera agotado—: desde acá
/// **todos significan lo mismo**, que es "no sé, y no pasa nada". El motivo
/// exacto va a la salida de diagnóstico, no a la interfaz.
fn interpretar(respuesta: Result<Option<Ofrecida>, ()>) -> EstadoActualizacion {
    let Ok(anunciada) = respuesta else {
        return EstadoActualizacion::sin_red();
    };
    match anunciada {
        // La comparación propia manda: ver `es_mas_nueva`.
        Some(o) if es_mas_nueva(version_instalada(), &o.version) => {
            EstadoActualizacion::disponible(&o.version, o.notas)
        }
        Some(o) => {
            eprintln!(
                "el servidor ofrece la v{} y acá corre la v{}: no es una actualización",
                o.version,
                version_instalada()
            );
            EstadoActualizacion::al_dia()
        }
        None => EstadoActualizacion::al_dia(),
    }
}

/// Le pregunta al endpoint y arma el resultado. **Nunca devuelve `Err`**: no hay
/// forma de fallar en la que la respuesta correcta no sea "seguí usando la app".
async fn consultar(app: &AppHandle) -> EstadoActualizacion {
    if !esta_instalada() {
        return EstadoActualizacion::no_instalada();
    }
    let actualizador = match app.updater_builder().timeout(TOPE_DE_CONSULTA).build() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("no pude armar el actualizador: {e}");
            return EstadoActualizacion::sin_red();
        }
    };
    let respuesta = actualizador.check().await.map_err(|e| {
        eprintln!("no pude consultar actualizaciones: {e}");
    });
    interpretar(respuesta.map(|anunciada| {
        anunciada.map(|u| Ofrecida {
            version: u.version.clone(),
            notas: u.body.clone(),
        })
    }))
}

/// Deja el resultado en el espejo y avisa **sólo si corresponde**.
///
/// El espejo se escribe siempre, incluso callando el aviso: es lo que hace que
/// abrir Ajustes cinco minutos después muestre lo que ya se sabía en vez de
/// volver a salir a la red.
fn publicar(app: &AppHandle, resultado: EstadoActualizacion) {
    app.state::<Arc<UltimaConsulta>>()
        .escribir(resultado.clone());
    println!("actualizaciones: {}", resultado.mensaje);

    if resultado.clave != DISPONIBLE {
        return;
    }
    if let Some(motivo) = ocupado(&estado_actual(app)) {
        println!("hay versión nueva, pero no aviso ahora: {motivo}");
        return;
    }
    eventos::actualizacion_disponible(app, resultado);
}

/// Programa las consultas automáticas en un hilo aparte: una al arrancar y
/// después una cada [`ENTRE_CONSULTAS`].
///
/// La repetición existe porque esta app pasa días abierta en la bandeja: con
/// una sola consulta al arrancar, una versión publicada después no se veía
/// hasta reiniciar Windows. Las consultas van en fila en el mismo hilo, así que
/// nunca se pisan dos.
///
/// Que falle crear el hilo no puede tumbar nada: se anota y la aplicación
/// arranca sin consulta automática. El botón de Ajustes sigue estando.
pub fn consultar_periodicamente(app: &AppHandle) {
    let app = app.clone();
    let creado = std::thread::Builder::new()
        .name("mithflow-actualizacion".into())
        .spawn(move || {
            std::thread::sleep(ESPERA_AL_ARRANCAR);
            loop {
                let resultado = tauri::async_runtime::block_on(consultar(&app));
                publicar(&app, resultado);
                std::thread::sleep(ENTRE_CONSULTAS);
            }
        });
    if let Err(e) = creado {
        eprintln!("no pude programar la consulta de actualizaciones: {e}");
    }
}

/* ------------------------------------------------------------- comandos */

/// El presente, sin salir a la red: lo que dejó la última consulta.
///
/// Es el comando espejo de `actualizacion-disponible`. Sin él, abrir la ventana
/// después de que la consulta del arranque ya pasó mostraría "todavía no me
/// fijé" para siempre.
#[tauri::command]
pub fn leer_actualizacion(app: AppHandle) -> EstadoActualizacion {
    app.state::<Arc<UltimaConsulta>>().leer()
}

/// La consulta forzada, la del botón de Ajustes.
///
/// A diferencia del arranque, ésta corre **aunque se esté dictando**: es una
/// petición HTTP que el usuario pidió, no interrumpe nada y no toca el audio.
/// Lo que sí sigue prohibido mientras hay audio en vuelo es instalar.
#[tauri::command]
pub async fn buscar_actualizacion(app: AppHandle) -> EstadoActualizacion {
    let resultado = consultar(&app).await;
    app.state::<Arc<UltimaConsulta>>()
        .escribir(resultado.clone());
    resultado
}

/// Baja el paquete, verifica la firma, instala y reinicia.
///
/// # El orden, que es lo que lo hace seguro y no molesto
///
/// 1. **No se instala con audio en vuelo.** Se verifica al empezar.
/// 2. **No se instala sin instalación**: desde una copia de desarrollo no hay
///    nada que reemplazar y el instalador dejaría dos MithFlow distintos.
/// 3. Se vuelve a consultar en vez de confiar en lo que dijo el arranque: entre
///    una cosa y la otra pueden haber pasado horas.
/// 4. `download` **verifica la firma minisign** y recién ahí devuelve los bytes.
/// 5. Se vuelve a mirar el estado: bajar 12 MB tarda, y arrancar un dictado
///    mientras tanto es lo más normal del mundo.
/// 6. `install` lanza el instalador NSIS con `/UPDATE` y termina este proceso;
///    el instalador vuelve a abrir MithFlow ya actualizado. Por eso en el camino
///    feliz **esta función no retorna**.
#[tauri::command]
pub async fn instalar_actualizacion(app: AppHandle) -> Result<(), String> {
    if let Some(motivo) = ocupado(&estado_actual(&app)) {
        return Err(format!("{motivo} No actualicé nada."));
    }
    if !esta_instalada() {
        return Err(EstadoActualizacion::no_instalada().mensaje);
    }

    let actualizador = app
        .updater_builder()
        .timeout(TOPE_DE_CONSULTA)
        .build()
        .map_err(|e| {
            eprintln!("no pude armar el actualizador: {e}");
            "No pude preparar la actualización.".to_string()
        })?;

    let disponible = actualizador.check().await.map_err(|e| {
        eprintln!("no pude consultar actualizaciones: {e}");
        EstadoActualizacion::sin_red().mensaje
    })?;
    let Some(actualizacion) = disponible else {
        return Err(EstadoActualizacion::al_dia().mensaje);
    };
    if !es_mas_nueva(version_instalada(), &actualizacion.version) {
        return Err(format!(
            "El servidor ofrece la v{} y acá corre la v{}: no instalo algo que no es más nuevo.",
            actualizacion.version,
            version_instalada()
        ));
    }

    eventos::aviso(
        &app,
        &format!(
            "Bajando MithFlow {}. Cuando termine, la aplicación se cierra y vuelve sola.",
            actualizacion.version
        ),
        "info",
    );

    let paquete = actualizacion
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| {
            eprintln!("no pude bajar la actualización: {e}");
            // El caso interesante acá es la firma: si el `.exe` no está firmado
            // con la clave privada que le corresponde a la pública de
            // `tauri.conf.json`, esto falla y no se ejecuta nada.
            "No pude bajar la actualización, o la firma no verificó. No se instaló nada."
                .to_string()
        })?;

    // Segunda verificación, y la que importa: la descarga pudo tardar un minuto.
    if let Some(motivo) = ocupado(&estado_actual(&app)) {
        return Err(format!(
            "{motivo} Bajé la actualización pero no la instalo mientras dictás; \
             volvé a apretar cuando termines."
        ));
    }

    actualizacion.install(paquete).map_err(|e| {
        eprintln!("no pude instalar la actualización: {e}");
        "No pude instalar la actualización. La versión que tenías sigue funcionando.".to_string()
    })?;
    // En Windows no se llega acá: `install` lanza el instalador y termina el
    // proceso. Se devuelve `Ok` igual porque el tipo lo pide.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /* --------------------------------------------- comparación de versiones */

    /// El caso que una comparación de cadenas erraría: `"1.10.0" < "1.9.0"`
    /// alfabéticamente, y sería la actualización que nunca se ofrece.
    #[test]
    fn diez_es_mayor_que_nueve_aunque_alfabeticamente_no_lo_sea() {
        assert!(es_mas_nueva("1.9.0", "1.10.0"));
        assert!(!es_mas_nueva("1.10.0", "1.9.0"));
        assert!(es_mas_nueva("1.0.9", "1.0.10"));
    }

    #[test]
    fn sube_en_cualquiera_de_las_tres_partes() {
        assert!(es_mas_nueva("1.0.0", "1.0.1"));
        assert!(es_mas_nueva("1.0.0", "1.1.0"));
        assert!(es_mas_nueva("1.0.0", "2.0.0"));
        assert!(es_mas_nueva("1.2.3", "2.0.0"));
    }

    /// La misma versión no es una actualización: sin esto, cada consulta
    /// ofrecería reinstalar lo que ya está.
    #[test]
    fn la_misma_version_no_es_una_actualizacion() {
        assert!(!es_mas_nueva("1.0.0", "1.0.0"));
        assert!(!es_mas_nueva("1.0.0", "v1.0.0"));
    }

    /// **Nunca hacia atrás.** Un `latest.json` viejo pegado en la release
    /// equivocada reemplazaría una instalación buena por una vieja.
    #[test]
    fn nunca_se_ofrece_una_version_anterior() {
        assert!(!es_mas_nueva("2.0.0", "1.9.9"));
        assert!(!es_mas_nueva("1.1.0", "1.0.9"));
        assert!(!es_mas_nueva("1.0.1", "1.0.0"));
    }

    /// Ante una versión que no se entiende, no se ofrece nada: el error barato
    /// es quedarse en la que anda.
    #[test]
    fn una_version_ilegible_no_actualiza() {
        for basura in ["", "   ", "ultima", "1.0.0.1", "1.a.0", "1..0", "-1.0.0"] {
            assert!(
                !es_mas_nueva("1.0.0", basura),
                "{basura:?} no puede pasar por una versión más nueva"
            );
            assert!(
                !es_mas_nueva(basura, "1.0.0"),
                "{basura:?} como versión instalada tampoco puede decidir nada"
            );
        }
    }

    /// La `v` de las etiquetas de git y los sufijos de pre-release no rompen la
    /// lectura: `v1.2.3` y `1.2.3-beta.1` valen 1.2.3.
    #[test]
    fn se_toleran_la_v_de_git_y_el_sufijo_de_prerelease() {
        assert_eq!(partes("v1.2.3"), Some([1, 2, 3]));
        assert_eq!(partes("1.2.3-beta.1"), Some([1, 2, 3]));
        assert_eq!(partes("1.2.3+build7"), Some([1, 2, 3]));
        assert_eq!(partes("1.2"), Some([1, 2, 0]), "faltar el parche es 0");
        assert!(es_mas_nueva("1.0.0", "v1.0.1"));
    }

    /// Y la versión que compila esta app tiene que ser legible: si el
    /// `Cargo.toml` quedara con algo raro, **ninguna** actualización se
    /// ofrecería nunca y no se notaría hasta el día que hiciera falta.
    #[test]
    fn la_version_de_este_ejecutable_es_legible() {
        let instalada = version_instalada();
        assert!(
            partes(instalada).is_some(),
            "la versión compilada ({instalada}) no se puede comparar"
        );
    }

    /* ------------------------------------------------- la guarda del dictado */

    fn dto(estado: &Estado) -> EstadoDto {
        EstadoDto::nuevo(estado, false)
    }

    /// Con audio en vuelo no se avisa y no se instala. Es la regla que evita que
    /// una actualización se lleve puesto un dictado.
    #[test]
    fn grabando_o_transcribiendo_no_se_actualiza() {
        for estado in [Estado::Grabando, Estado::Transcribiendo] {
            assert!(
                ocupado(&dto(&estado)).is_some(),
                "{} tiene que frenar la actualización",
                estado.clave()
            );
        }
    }

    /// Y ningún otro estado la frena: pausado, cargando o con un error, una
    /// actualización es justamente lo que puede arreglar el problema.
    #[test]
    fn el_resto_de_los_estados_deja_actualizar() {
        for estado in [
            Estado::Listo,
            Estado::Cargando,
            Estado::SinModelo("falta bajarlo".into()),
            Estado::Error("el modelo no carga".into()),
        ] {
            assert_eq!(
                ocupado(&dto(&estado)),
                None,
                "{} no tiene por qué impedir actualizar",
                estado.clave()
            );
        }
    }

    /* ---------------------------------------------------- el estado que se ve */

    /// Cada clave llega con lo que la interfaz necesita para dibujarla, y sólo
    /// `disponible` trae una versión: mostrar "actualizar a v" sin número sería
    /// el peor de los avisos.
    #[test]
    fn solo_disponible_trae_la_version_que_se_instalaria() {
        let hay = EstadoActualizacion::disponible("9.9.9", Some("arreglos".into()));
        assert_eq!(hay.clave, DISPONIBLE);
        assert_eq!(hay.disponible.as_deref(), Some("9.9.9"));
        assert!(hay.mensaje.contains("9.9.9"), "{}", hay.mensaje);
        assert!(
            hay.mensaje.contains(version_instalada()),
            "el aviso tiene que decir también cuál está instalada: {}",
            hay.mensaje
        );

        for otra in [
            EstadoActualizacion::al_dia(),
            EstadoActualizacion::sin_red(),
            EstadoActualizacion::no_instalada(),
            EstadoActualizacion::sin_consultar(),
        ] {
            assert_eq!(otra.disponible, None, "{} no instala nada", otra.clave);
            assert!(!otra.mensaje.is_empty(), "{} sin mensaje", otra.clave);
            assert_eq!(otra.instalada, version_instalada());
        }
    }

    /* ------------------------------------------ qué se hace con la respuesta */

    fn ofrece(version: &str) -> Result<Option<Ofrecida>, ()> {
        Ok(Some(Ofrecida {
            version: version.to_string(),
            notas: None,
        }))
    }

    /// **El endpoint inalcanzable.** Es el caso más frecuente de todos —sin
    /// internet, GitHub caído, el repositorio todavía sin configurar— y tiene
    /// que terminar en `sin-red` y en nada más: ni un error que interrumpa, ni
    /// una actualización fantasma.
    #[test]
    fn un_endpoint_que_no_contesta_termina_en_sin_red() {
        let resultado = interpretar(Err(()));
        assert_eq!(resultado.clave, SIN_RED);
        assert_eq!(resultado.disponible, None, "no hay nada que instalar");
        assert_eq!(resultado.instalada, version_instalada());
    }

    /// Y las otras dos respuestas posibles: no hay novedades, o hay una versión
    /// que **sube**. Una que no sube se informa como "al día", no como
    /// disponible: es el manifiesto viejo pegado en la release equivocada.
    #[test]
    fn solo_una_version_que_sube_se_ofrece() {
        assert_eq!(interpretar(Ok(None)).clave, AL_DIA);

        let nueva = interpretar(ofrece("999.0.0"));
        assert_eq!(nueva.clave, DISPONIBLE);
        assert_eq!(nueva.disponible.as_deref(), Some("999.0.0"));

        assert_eq!(
            interpretar(ofrece(version_instalada())).clave,
            AL_DIA,
            "la misma versión no es una actualización"
        );
        assert_eq!(
            interpretar(ofrece("0.0.1")).clave,
            AL_DIA,
            "una versión anterior no puede ofrecerse"
        );
        assert_eq!(
            interpretar(ofrece("no-es-una-version")).clave,
            AL_DIA,
            "una versión ilegible tampoco"
        );
    }

    /// Un endpoint inalcanzable no puede parecer un problema del usuario ni
    /// sugerir que la app dejó de andar: es el mensaje que más se va a ver
    /// mientras no haya red.
    #[test]
    fn el_fallo_de_red_dice_que_la_app_sigue_funcionando() {
        let sin_red = EstadoActualizacion::sin_red();
        assert_eq!(sin_red.clave, SIN_RED);
        assert!(
            sin_red.mensaje.contains("funciona igual"),
            "el mensaje tiene que aclarar que no se rompió nada: {}",
            sin_red.mensaje
        );
        assert!(
            !sin_red.mensaje.contains("http"),
            "no se le muestra la URL al usuario: {}",
            sin_red.mensaje
        );
    }

    /// El caso de desarrollo se explica, no se disfraza de error de red: es el
    /// que ve quien programa esto todos los días.
    #[test]
    fn la_copia_de_desarrollo_se_explica_aparte() {
        let dev = EstadoActualizacion::no_instalada();
        assert_eq!(dev.clave, NO_INSTALADA);
        assert_ne!(dev.clave, SIN_RED);
        assert!(dev.mensaje.contains("desarrollo"), "{}", dev.mensaje);
        assert!(dev.mensaje.contains("instalador"), "{}", dev.mensaje);
    }

    /// Las cinco claves son distintas entre sí: si dos coincidieran, la interfaz
    /// dibujaría un estado por otro.
    #[test]
    fn las_claves_son_distintas() {
        let claves = [SIN_CONSULTAR, AL_DIA, DISPONIBLE, SIN_RED, NO_INSTALADA];
        for (i, una) in claves.iter().enumerate() {
            for otra in &claves[i + 1..] {
                assert_ne!(una, otra);
            }
        }
    }

    /// El espejo arranca sin consultar y refleja lo último que se escribió, que
    /// es lo que hace que el aviso sobreviva a la ventana cerrada.
    #[test]
    fn el_espejo_arranca_sin_consultar_y_guarda_lo_ultimo() {
        let espejo = UltimaConsulta::nueva();
        assert_eq!(espejo.leer().clave, SIN_CONSULTAR);

        espejo.escribir(EstadoActualizacion::disponible("9.9.9", None));
        assert_eq!(espejo.leer().clave, DISPONIBLE);
        assert_eq!(espejo.leer().disponible.as_deref(), Some("9.9.9"));
    }
}
