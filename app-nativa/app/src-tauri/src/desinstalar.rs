//! Desinstalar MithFlow desde adentro de MithFlow.
//!
//! # Por qué existe, si Windows ya tiene su desinstalador
//!
//! Porque el `uninstall.exe` de NSIS **saca el programa y deja los datos**.
//! Quien desinstale por el camino normal se queda con los dos directorios de
//! esta aplicación huérfanos en el disco:
//!
//! | Directorio | Qué hay | Cuánto pesa |
//! |---|---|---|
//! | `%APPDATA%\MithFlow\models\` | los `.gguf` | ~2 GB |
//! | `%APPDATA%\com.mithdata.mithflow\` | `ajustes.json` y el historial | decenas de KB |
//!
//! Los 2 GB son plata en disco que nadie va a encontrar. El historial es peor:
//! es **texto plano con todo lo que el usuario dictó**, y sobrevivir a la
//! desinstalación es exactamente lo que no tiene que hacer. Por eso este módulo
//! no se limita a lanzar el desinstalador: primero decide qué pasa con esos dos
//! directorios, y se lo pregunta al usuario con los tamaños reales a la vista.
//!
//! # El orden, que es lo único que hace segura la operación
//!
//! 1. **verificar que el desinstalador existe**;
//! 2. recién entonces borrar lo que corresponda;
//! 3. y recién entonces lanzarlo.
//!
//! Al revés —borrar y después descubrir que no hay ningún `uninstall.exe`—
//! dejaría al usuario sin sus 2 GB de modelos **y** con la aplicación instalada,
//! que es el peor desenlace posible de los tres. Ver [`ejecutar_desinstalacion`].
//!
//! # Los dos directorios, y ninguno más
//!
//! Las rutas se piden a las mismas funciones que las escribieron
//! ([`models::directorio`] y [`rutas::dir_datos`]), nunca se arman concatenando
//! cadenas, y sólo llegan a `remove_dir_all` a través de [`DirectorioBorrable`],
//! que no se puede construir sin pasar por [`verificar`]. Un `remove_dir_all`
//! sobre una ruta mal armada no tiene vuelta atrás; el tipo está para que ese
//! error no se pueda escribir por descuido.

use crate::estado::{Estado, EstadoCompartido};
use crate::rutas;
use mithflow_core::{history, models};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

/// El desinstalador que deja el instalador NSIS al lado del ejecutable.
const DESINSTALADOR: &str = "uninstall.exe";

/// Hasta dónde baja [`tamano_de`] al medir un directorio. Los dos que mide son
/// planos o de un nivel; el tope está por si alguien deja un árbol raro adentro,
/// para que medirlo no se convierta en una recursión sin fondo.
const MAX_PROFUNDIDAD: u32 = 16;

/// Cuánto se espera antes de cerrar la aplicación, para que la respuesta del
/// comando alcance a cruzar al webview.
const GRACIA_ANTES_DE_CERRAR: Duration = Duration::from_millis(500);

/* ------------------------------------------------------------- las rutas */

/// Un directorio que esta aplicación puede borrar entero.
///
/// Es un tipo y no un `PathBuf` a propósito: `remove_dir_all` se llama **sólo**
/// sobre uno de éstos, y construirlo obliga a pasar por [`verificar`]. Una ruta
/// relativa, la raíz de un disco o algo con `..` en el medio no llega nunca a la
/// llamada.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectorioBorrable(PathBuf);

impl DirectorioBorrable {
    fn nuevo(ruta: PathBuf) -> Result<Self, String> {
        verificar(&ruta)?;
        Ok(Self(ruta))
    }

    fn ruta(&self) -> &Path {
        &self.0
    }

    /// Lo borra entero. Que no exista es el resultado deseado, no un error:
    /// desinstalar dos veces tiene que dar lo mismo que desinstalar una.
    fn borrar(&self) -> Result<(), String> {
        match std::fs::remove_dir_all(&self.0) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("no pude borrar {}: {e}", self.0.display())),
        }
    }
}

/// Las tres cosas que descalifican a una ruta para un `remove_dir_all`.
///
/// No pretende adivinar si la ruta "es de MithFlow" —de eso se encarga que
/// salgan de [`models::directorio`] y [`rutas::dir_datos`]—: es la red que
/// atrapa una ruta rota antes de que borre medio disco. Exigir un abuelo deja
/// afuera `C:\` y `C:\loquesea`; los dos directorios reales cuelgan de
/// `%APPDATA%`, que ya son cinco niveles.
fn verificar(ruta: &Path) -> Result<(), String> {
    if !ruta.is_absolute() {
        return Err(format!("{} no es una ruta absoluta", ruta.display()));
    }
    if ruta.components().any(|c| c == Component::ParentDir) {
        return Err(format!("{} tiene un «..» en el medio", ruta.display()));
    }
    if ruta.parent().and_then(Path::parent).is_none() {
        return Err(format!(
            "{} está demasiado arriba para borrarla entera",
            ruta.display()
        ));
    }
    Ok(())
}

/// Exactamente qué se borra cuando el usuario elige "borrar todo".
///
/// **Son dos directorios y ninguno más**, y los dos salen de las funciones que
/// los escribieron. [`PlanDeBorrado::a_borrar`] los expone para que un test
/// pueda afirmar esa lista completa en vez de confiar en la lectura del código.
#[derive(Debug, Clone)]
struct PlanDeBorrado {
    modelos: DirectorioBorrable,
    datos: DirectorioBorrable,
}

impl PlanDeBorrado {
    fn nuevo(modelos: PathBuf, datos: PathBuf) -> Result<Self, String> {
        Ok(Self {
            modelos: DirectorioBorrable::nuevo(modelos)?,
            datos: DirectorioBorrable::nuevo(datos)?,
        })
    }

    /// Los directorios que se borran recursivamente, en orden. Dos, siempre.
    fn a_borrar(&self) -> [&Path; 2] {
        [self.modelos.ruta(), self.datos.ruta()]
    }

    fn ejecutar(&self) -> Result<(), String> {
        // Se anota qué se borra ANTES de borrarlo, y por la MISMA lista que
        // mira el test: una operación sin vuelta atrás no puede no dejar rastro
        // de qué tocó, y que el rastro salga de otra fuente que la afirmación
        // del test sería el rastro de otra cosa.
        for ruta in self.a_borrar() {
            println!("desinstalación: borro {}", ruta.display());
        }
        self.modelos.borrar()?;
        self.datos.borrar()?;
        // Los modelos viven en `%APPDATA%\MithFlow\models\`; sacarlos deja
        // `MithFlow\` vacío, que no ocupa nada pero queda como un rastro sin
        // dueño. Ver `sacar_si_quedo_vacio`: no puede borrar nada que tenga
        // algo adentro.
        sacar_si_quedo_vacio(self.modelos.ruta().parent());
        Ok(())
    }
}

/// Saca un directorio **sólo si quedó vacío**.
///
/// Es `remove_dir` y no `remove_dir_all` a propósito: si alguien dejó algo más
/// ahí adentro, la llamada falla y el directorio queda intacto. Que falle no es
/// un problema de la desinstalación —una carpeta vacía no ocupa nada— así que el
/// error se ignora en vez de abortar algo que ya está hecho.
fn sacar_si_quedo_vacio(dir: Option<&Path>) {
    let Some(dir) = dir else { return };
    if verificar(dir).is_ok() {
        let _ = std::fs::remove_dir(dir);
    }
}

/// El plan para ESTA instalación: los dos directorios de datos, tal como los
/// nombran los módulos que escriben en ellos.
fn plan_de_borrado(app: &AppHandle) -> Result<PlanDeBorrado, String> {
    PlanDeBorrado::nuevo(models::directorio()?, rutas::dir_datos(app)?)
}

/* ---------------------------------------------------- el desinstalador */

/// ¿Esa carpeta es una instalación de MithFlow y no una copia de desarrollo?
///
/// La señal es el `uninstall.exe` que NSIS deja al lado del ejecutable. Vive acá
/// —y no repetida en cada módulo que la necesite— porque "qué es una
/// instalación" tiene que ser una sola definición: el actualizador la usa para
/// decidir si tiene sentido reemplazar algo (ver `actualizador::esta_instalada`),
/// y si las dos respuestas pudieran diferir, una de las dos estaría mintiendo.
pub fn es_una_instalacion(carpeta_del_ejecutable: &Path) -> bool {
    carpeta_del_ejecutable.join(DESINSTALADOR).is_file()
}

/// Dónde tendría que estar el desinstalador: al lado del ejecutable, que es
/// donde lo deja el instalador NSIS.
///
/// Devuelve la ruta **exista o no**; quien decide es [`ejecutar_desinstalacion`].
fn ruta_del_desinstalador() -> Result<PathBuf, String> {
    let ejecutable = std::env::current_exe()
        .map_err(|e| format!("no pude saber desde dónde estoy corriendo: {e}"))?;
    let carpeta = ejecutable
        .parent()
        .ok_or_else(|| format!("{} no tiene carpeta contenedora", ejecutable.display()))?;
    Ok(carpeta.join(DESINSTALADOR))
}

/// Qué contarle al usuario cuando el desinstalador no está.
///
/// **Es el caso de desarrollo**, y el que ve quien programa esto: corriendo
/// desde `target/release/` no hay ningún `uninstall.exe` al lado del ejecutable
/// porque nadie instaló nada. Decirlo con todas las letras —y aclarar que no se
/// tocó ningún dato— es la diferencia entre un mensaje y un misterio.
fn sin_desinstalador(ruta: &Path) -> String {
    format!(
        "No encontré el desinstalador ({DESINSTALADOR}) en {}. Pasa cuando MithFlow corre desde \
         una copia de desarrollo en vez de la instalación: no hay nada que desinstalar y no \
         toqué ningún dato. Si lo instalaste con el instalador, sacalo desde Configuración → \
         Aplicaciones.",
        ruta.parent().unwrap_or(ruta).display()
    )
}

/// Lanza el desinstalador como un proceso independiente y vuelve enseguida.
///
/// `spawn` y no `status`: el desinstalador tiene que **sobrevivir** a esta
/// aplicación, que está por cerrarse y que además vive en la carpeta que él
/// tiene que borrar. En Windows un proceso lanzado así no muere con su padre.
///
/// El directorio de trabajo se fija en el temporal del sistema y no en el de la
/// instalación: un proceso con el CWD adentro de la carpeta que se está
/// borrando es lo que hace que un desinstalador deje restos.
fn lanzar_desinstalador(desinstalador: &Path) -> Result<(), String> {
    std::process::Command::new(desinstalador)
        .current_dir(std::env::temp_dir())
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            format!(
                "no pude lanzar {}: {e}. No se desinstaló nada.",
                desinstalador.display()
            )
        })
}

/// El orden que hace segura la operación, con el lanzamiento inyectado para
/// poder probarlo sin ejecutar el desinstalador de verdad.
///
/// La primera línea es la que importa: **si el desinstalador no está, no se
/// borra nada**. `plan` en `None` significa "conservar los datos".
fn ejecutar_desinstalacion(
    desinstalador: &Path,
    plan: Option<&PlanDeBorrado>,
    lanzar: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if !desinstalador.is_file() {
        return Err(sin_desinstalador(desinstalador));
    }
    if let Some(plan) = plan {
        plan.ejecutar()?;
    }
    lanzar(desinstalador)
}

/* ------------------------------------------------------------- el resumen */

/// Qué se va a borrar y cuánto pesa, medido sobre este disco.
///
/// Todo sale de mirar los archivos de verdad: un texto fijo con "unos 2 GB"
/// sería mentira en la máquina que sólo bajó el modelo chico, y el número existe
/// justamente para que el usuario sepa qué pierde antes de confirmar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResumenDesinstalacion {
    /// `None` cuando no hay instalación que medir (ver `hay_desinstalador`).
    pub programa_ruta: Option<String>,
    pub programa_bytes: u64,
    pub modelos_ruta: String,
    pub modelos_bytes: u64,
    /// Cuántos `.gguf` hay. Cuenta los que estén, no los del catálogo: un modelo
    /// copiado a mano de otra máquina también se va a borrar.
    pub modelos_cantidad: usize,
    pub datos_ruta: String,
    pub datos_bytes: u64,
    /// Cuántos dictados guardados hay en el historial.
    pub dictados: usize,
    pub hay_desinstalador: bool,
    /// Por qué no lo hay, para poder decirlo sin adivinar. `None` cuando sí está.
    pub motivo_sin_desinstalador: Option<String>,
}

/// Las cuentas del resumen, sin `AppHandle`: es lo que se puede probar contra un
/// árbol de mentira.
///
/// `programa` es `None` cuando no hay instalación —corriendo desde
/// `target/release/` medir esa carpeta daría los gigabytes del build, que no es
/// lo que se va a desinstalar—.
fn medir(
    programa: Option<&Path>,
    modelos: &Path,
    datos: &Path,
    historial: &Path,
    motivo_sin_desinstalador: Option<String>,
) -> ResumenDesinstalacion {
    ResumenDesinstalacion {
        programa_ruta: programa.map(|p| p.display().to_string()),
        programa_bytes: programa.map(tamano_de).unwrap_or(0),
        modelos_ruta: modelos.display().to_string(),
        modelos_bytes: tamano_de(modelos),
        modelos_cantidad: contar_modelos(modelos),
        datos_ruta: datos.display().to_string(),
        datos_bytes: tamano_de(datos),
        // Un historial que no se puede leer cuenta cero: el número es
        // informativo y no decide nada, y no poder abrirlo no puede impedir
        // desinstalar.
        dictados: history::load(historial).map(|e| e.len()).unwrap_or(0),
        hay_desinstalador: motivo_sin_desinstalador.is_none(),
        motivo_sin_desinstalador,
    }
}

/// Cuánto ocupa un directorio, contando lo que haya en sus subdirectorios.
///
/// **No sigue enlaces ni uniones**: `DirEntry::file_type` los informa como
/// enlace y esta función los saltea, así que una unión que apunte a su propio
/// padre no puede colgarla. Un directorio que no existe o que no se puede leer
/// mide cero, que es lo correcto para lo que se usa: si no se puede mirar,
/// tampoco se va a poder borrar.
fn tamano_de(dir: &Path) -> u64 {
    tamano_hasta(dir, MAX_PROFUNDIDAD)
}

fn tamano_hasta(dir: &Path, profundidad: u32) -> u64 {
    let Ok(entradas) = std::fs::read_dir(dir) else {
        return 0;
    };
    entradas
        .flatten()
        .map(|entrada| match entrada.file_type() {
            Ok(t) if t.is_file() => entrada.metadata().map(|m| m.len()).unwrap_or(0),
            Ok(t) if t.is_dir() && profundidad > 0 => {
                tamano_hasta(&entrada.path(), profundidad - 1)
            }
            _ => 0,
        })
        .sum()
}

/// Cuántos `.gguf` hay en el directorio de modelos.
fn contar_modelos(dir: &Path) -> usize {
    let Ok(entradas) = std::fs::read_dir(dir) else {
        return 0;
    };
    entradas
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
        })
        .count()
}

/* ------------------------------------------------------------- comandos */

/// Todo lo que la interfaz necesita para decir qué se va a borrar y cuánto pesa,
/// **antes** de que el usuario confirme.
#[tauri::command]
pub fn resumen_desinstalacion(app: AppHandle) -> Result<ResumenDesinstalacion, String> {
    let desinstalador = ruta_del_desinstalador();
    let motivo = match &desinstalador {
        Ok(ruta) if ruta.is_file() => None,
        Ok(ruta) => Some(sin_desinstalador(ruta)),
        Err(e) => Some(e.clone()),
    };
    // La carpeta del programa se mide sólo si hay instalación de verdad: en
    // desarrollo el ejecutable vive en `target/release/`, y medir esa carpeta
    // informaría los gigabytes del build como si fueran los de MithFlow.
    let programa = match (&desinstalador, &motivo) {
        (Ok(ruta), None) => ruta.parent(),
        _ => None,
    };

    Ok(medir(
        programa,
        &models::directorio()?,
        &rutas::dir_datos(&app)?,
        &rutas::historial(&app)?,
        motivo,
    ))
}

/// Borra lo que corresponda, lanza el desinstalador y cierra la aplicación.
///
/// `conservar` en `true` desinstala el programa y **deja** los modelos y el
/// historial donde están: quien reinstala no tiene por qué volver a bajar 2 GB.
///
/// El orden es el del módulo —verificar, borrar, lanzar— y no es negociable.
#[tauri::command]
pub fn desinstalar(
    app: AppHandle,
    conservar: bool,
    espejo: State<'_, Arc<EstadoCompartido>>,
) -> Result<(), String> {
    if let Some(motivo) = ocupado(espejo.leer().estado) {
        return Err(motivo);
    }

    // Se arma antes de tocar nada —resolver las rutas no borra— para que un
    // error de resolución falle acá y no con medio directorio borrado.
    let plan = if conservar {
        None
    } else {
        Some(plan_de_borrado(&app)?)
    };
    let desinstalador = ruta_del_desinstalador()?;
    ejecutar_desinstalacion(&desinstalador, plan.as_ref(), lanzar_desinstalador)?;
    // Después y no antes: si no hubiera desinstalador, esto habría apagado el
    // arranque con Windows de una aplicación que se queda instalada.
    sacar_el_autoarranque(&app);
    cerrar_en_breve(app);
    Ok(())
}

/// Qué impide desinstalar justo ahora.
///
/// Grabar y transcribir son las dos operaciones con audio en vuelo: cortarlas a
/// la mitad para borrar el modelo que están usando no tiene ninguna ventaja
/// sobre esperar diez segundos, y el dictado que se pierde ya no vuelve. Las
/// claves salen de [`Estado`] y no de cadenas sueltas, que es lo que evita que
/// renombrar un estado deje esta guarda mirando un valor que ya nadie publica.
fn ocupado(estado: &str) -> Option<String> {
    if estado == Estado::Grabando.clave() {
        return Some(
            "Estás grabando. Terminá el dictado y volvé a intentar: no toqué nada.".to_string(),
        );
    }
    if estado == Estado::Transcribiendo.clave() {
        return Some(
            "Estoy transcribiendo un dictado. Esperá a que termine y volvé a intentar: no toqué \
             nada."
                .to_string(),
        );
    }
    None
}

/// Saca el arranque con Windows antes de irse.
///
/// El desinstalador de NSIS borra la carpeta, los accesos directos y su entrada
/// en "Aplicaciones", pero no sabe nada del valor que `tauri-plugin-autostart`
/// escribe en `HKCU\...\Run`: sin esta línea quedaría apuntando a un ejecutable
/// que ya no existe, y cada inicio de sesión intentaría abrirlo. Se consulta
/// antes de tocarlo por lo mismo que en `comandos::aplicar_autoarranque`:
/// `disable()` falla si el valor no está.
///
/// Un fallo acá no puede abortar la desinstalación: es prolijidad, no una
/// condición.
fn sacar_el_autoarranque(app: &AppHandle) {
    let gestor = app.autolaunch();
    if gestor.is_enabled().unwrap_or(false) {
        if let Err(e) = gestor.disable() {
            eprintln!("no pude sacar el arranque con Windows: {e}");
        }
    }
}

/// Cierra la aplicación dándole tiempo a la respuesta del comando a cruzar al
/// webview.
///
/// Sin esa pausa el `invoke` del frontend nunca resuelve: el proceso termina
/// antes de contestar y la ventana se queda congelada en el último cuadro que
/// alcanzó a dibujar. Si el hilo no se puede crear se cierra de una: quedarse
/// abierto después de haber lanzado el desinstalador sería peor.
fn cerrar_en_breve(app: AppHandle) {
    let de_respaldo = app.clone();
    let creado = std::thread::Builder::new()
        .name("mithflow-cierre".into())
        .spawn(move || {
            std::thread::sleep(GRACIA_ANTES_DE_CERRAR);
            app.exit(0);
        });
    if let Err(e) = creado {
        eprintln!("no pude programar el cierre ({e}); cierro de una");
        de_respaldo.exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Un directorio temporal propio de cada test, vacío.
    fn carpeta_temporal(nombre: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mithflow_desinstalar_{}_{nombre}",
            std::process::id()
        ));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).expect("no pude crear la carpeta temporal del test");
        dir
    }

    fn escribir(ruta: &Path, bytes: usize) {
        if let Some(padre) = ruta.parent() {
            fs::create_dir_all(padre).expect("no pude crear el directorio del archivo");
        }
        fs::write(ruta, vec![b'x'; bytes]).expect("no pude escribir el archivo");
    }

    /// Un `%APPDATA%` de mentira con los dos directorios de la aplicación **y
    /// vecinos que no son suyos**. Devuelve la raíz.
    fn appdata_de_mentira(nombre: &str) -> PathBuf {
        let raiz = carpeta_temporal(nombre);
        escribir(&raiz.join("MithFlow/models/whisper-large-v3-turbo-Q4_K_M.gguf"), 40);
        escribir(&raiz.join("MithFlow/models/whisper-large-v3-turbo-F16.gguf"), 60);
        escribir(&raiz.join("com.mithdata.mithflow/ajustes.json"), 10);
        escribir(&raiz.join("com.mithdata.mithflow/history-nativo.jsonl"), 20);
        // Los vecinos: otra aplicación cualquiera y un archivo suelto.
        escribir(&raiz.join("OtraApp/importante.db"), 100);
        escribir(&raiz.join("no-es-de-mithflow.txt"), 5);
        raiz
    }

    fn plan_sobre(raiz: &Path) -> PlanDeBorrado {
        PlanDeBorrado::nuevo(
            raiz.join("MithFlow").join("models"),
            raiz.join("com.mithdata.mithflow"),
        )
        .expect("dos rutas absolutas y profundas tienen que pasar")
    }

    /* ------------------------------------------------ qué se borra y qué no */

    /// **El test que sostiene todo el módulo.** Lo que se borra son exactamente
    /// los dos directorios de datos de la aplicación; todo lo demás que haya al
    /// lado sigue ahí después.
    #[test]
    fn se_borran_los_dos_directorios_de_datos_y_ninguno_mas() {
        let raiz = appdata_de_mentira("solo_los_dos");
        let plan = plan_sobre(&raiz);

        // Primero la lista declarada: dos rutas, y son las que se esperan.
        let a_borrar = plan.a_borrar();
        assert_eq!(a_borrar.len(), 2, "el plan no puede tener una tercera ruta");
        assert_eq!(a_borrar[0], raiz.join("MithFlow").join("models"));
        assert_eq!(a_borrar[1], raiz.join("com.mithdata.mithflow"));

        plan.ejecutar().expect("borrar tenía que funcionar");

        // Lo que tenía que irse, se fue.
        assert!(!raiz.join("MithFlow/models").exists(), "quedaron modelos");
        assert!(
            !raiz.join("com.mithdata.mithflow").exists(),
            "quedó el historial, que es justo lo que no puede sobrevivir"
        );

        // Y lo que NO es de MithFlow sigue intacto, byte por byte.
        assert_eq!(
            fs::read(raiz.join("OtraApp/importante.db")).unwrap().len(),
            100,
            "se tocó el directorio de otra aplicación"
        );
        assert!(
            raiz.join("no-es-de-mithflow.txt").exists(),
            "se borró un archivo vecino"
        );
        assert!(raiz.exists(), "se borró el directorio que los contenía");
    }

    /// El padre de los modelos (`%APPDATA%\MithFlow\`) se saca **sólo si quedó
    /// vacío**. Si alguien dejó algo suyo ahí, se queda donde está: es un
    /// `remove_dir`, no un `remove_dir_all`.
    #[test]
    fn el_padre_de_los_modelos_se_saca_solo_si_no_quedo_nada() {
        let raiz = appdata_de_mentira("padre_vacio");
        plan_sobre(&raiz).ejecutar().expect("borrar");
        assert!(
            !raiz.join("MithFlow").exists(),
            "sin los modelos, MithFlow\\ queda vacío y no tiene por qué sobrevivir"
        );

        let otra = appdata_de_mentira("padre_ocupado");
        escribir(&otra.join("MithFlow/algo-que-alguien-dejo.txt"), 7);
        plan_sobre(&otra).ejecutar().expect("borrar");
        assert!(
            otra.join("MithFlow/algo-que-alguien-dejo.txt").exists(),
            "un directorio con algo adentro no se puede sacar"
        );
        assert!(
            !otra.join("MithFlow/models").exists(),
            "los modelos sí se borran igual"
        );
    }

    /// Borrar dos veces tiene que dar lo mismo que borrar una: el usuario no
    /// tiene por qué saber si quedaba algo.
    #[test]
    fn borrar_lo_que_ya_no_esta_no_es_un_error() {
        let raiz = appdata_de_mentira("dos_veces");
        let plan = plan_sobre(&raiz);
        plan.ejecutar().expect("la primera");
        plan.ejecutar().expect("la segunda no puede fallar");
    }

    /// El plan se arma con las funciones del proyecto, no con cadenas: la ruta
    /// de los modelos es exactamente la que usa quien los descarga.
    #[test]
    fn la_ruta_de_los_modelos_es_la_del_modulo_que_los_escribe() {
        let de_modelos = models::directorio().expect("en Windows %APPDATA% resuelve");
        let plan = PlanDeBorrado::nuevo(de_modelos.clone(), std::env::temp_dir().join("datos"))
            .expect("la ruta real tiene que pasar la verificación");
        assert_eq!(plan.a_borrar()[0], de_modelos);
        assert!(
            de_modelos.ends_with("MithFlow/models") || de_modelos.ends_with("MithFlow\\models"),
            "{} no es el directorio de modelos",
            de_modelos.display()
        );
    }

    /// Una carpeta es una instalación **sólo** si tiene el desinstalador al
    /// lado. Es la misma pregunta que se hace el actualizador antes de bajar
    /// nada, así que las dos respuestas salen de acá.
    #[test]
    fn una_carpeta_es_instalacion_solo_si_tiene_el_desinstalador() {
        let dir = carpeta_temporal("es_instalacion");
        assert!(
            !es_una_instalacion(&dir),
            "una carpeta pelada no es una instalación"
        );
        escribir(&dir.join("mithflow.exe"), 10);
        assert!(
            !es_una_instalacion(&dir),
            "el ejecutable solo tampoco: eso es target/release/"
        );
        escribir(&dir.join(DESINSTALADOR), 3);
        assert!(es_una_instalacion(&dir));
    }

    /// La red que atrapa una ruta rota antes del `remove_dir_all`.
    #[test]
    fn una_ruta_peligrosa_no_se_puede_convertir_en_borrable() {
        for mala in [
            "modelos",                        // relativa
            "C:\\",                           // la raíz de un disco
            "C:\\Users",                       // cuelga directo de la raíz
            "C:\\Users\\alguien\\..\\..\\Windows", // trepa
        ] {
            assert!(
                DirectorioBorrable::nuevo(PathBuf::from(mala)).is_err(),
                "{mala} no puede aceptarse como directorio borrable"
            );
        }
        // Y una de las de verdad sí pasa.
        assert!(DirectorioBorrable::nuevo(
            std::env::temp_dir().join("MithFlow").join("models")
        )
        .is_ok());
    }

    /* --------------------------------------------------- el orden que salva */

    /// **Sin desinstalador no se borra nada.** Es la garantía que impide el peor
    /// desenlace: quedarse sin los 2 GB de modelos Y con la aplicación
    /// instalada.
    #[test]
    fn sin_desinstalador_no_se_borra_nada_y_no_se_lanza_nada() {
        let raiz = appdata_de_mentira("sin_desinstalador");
        let plan = plan_sobre(&raiz);
        let no_existe = raiz.join(DESINSTALADOR);

        let error = ejecutar_desinstalacion(&no_existe, Some(&plan), |_| {
            panic!("no se puede lanzar un desinstalador que no está")
        })
        .expect_err("sin desinstalador tiene que fallar");

        assert!(
            error.contains("no toqué ningún dato"),
            "el mensaje tiene que decir que los datos siguen ahí: {error}"
        );
        assert!(
            raiz.join("MithFlow/models/whisper-large-v3-turbo-F16.gguf").exists(),
            "se borraron los modelos sin poder desinstalar"
        );
        assert!(
            raiz.join("com.mithdata.mithflow/history-nativo.jsonl").exists(),
            "se borró el historial sin poder desinstalar"
        );
    }

    /// Con el desinstalador presente: se borra y se lanza, **en ese orden**.
    #[test]
    fn con_desinstalador_se_borra_primero_y_se_lanza_despues() {
        let raiz = appdata_de_mentira("orden");
        let plan = plan_sobre(&raiz);
        let desinstalador = raiz.join(DESINSTALADOR);
        escribir(&desinstalador, 3);

        let mut habia_datos_al_lanzar = None;
        ejecutar_desinstalacion(&desinstalador, Some(&plan), |ruta| {
            assert_eq!(ruta, desinstalador);
            habia_datos_al_lanzar = Some(raiz.join("MithFlow/models").exists());
            Ok(())
        })
        .expect("tenía que funcionar");

        assert_eq!(
            habia_datos_al_lanzar,
            Some(false),
            "el desinstalador se lanzó antes de borrar los datos"
        );
        assert!(!raiz.join("com.mithdata.mithflow").exists());
    }

    /// "Conservar los datos" es no borrar nada: se desinstala el programa y los
    /// modelos y el historial se quedan donde están.
    #[test]
    fn conservar_los_datos_no_borra_nada() {
        let raiz = appdata_de_mentira("conservar");
        let desinstalador = raiz.join(DESINSTALADOR);
        escribir(&desinstalador, 3);

        let mut lanzado = false;
        ejecutar_desinstalacion(&desinstalador, None, |_| {
            lanzado = true;
            Ok(())
        })
        .expect("tenía que funcionar");

        assert!(lanzado, "el desinstalador tiene que lanzarse igual");
        assert!(raiz.join("MithFlow/models").exists(), "se borraron los modelos");
        assert!(
            raiz.join("com.mithdata.mithflow/history-nativo.jsonl").exists(),
            "se borró el historial"
        );
    }

    /* ------------------------------------------------------------- medición */

    /// El tamaño que se le informa al usuario es la suma real de los archivos,
    /// subdirectorios incluidos.
    #[test]
    fn el_tamano_es_la_suma_de_lo_que_hay_adentro() {
        let dir = carpeta_temporal("tamano");
        escribir(&dir.join("uno.bin"), 1000);
        escribir(&dir.join("dos.bin"), 2500);
        escribir(&dir.join("adentro/tres.bin"), 500);
        escribir(&dir.join("adentro/mas-adentro/cuatro.bin"), 7);

        assert_eq!(tamano_de(&dir), 4007);
        assert_eq!(
            tamano_de(&dir.join("no-existe")),
            0,
            "lo que no está no ocupa"
        );
        assert_eq!(tamano_de(&carpeta_temporal("vacia")), 0);
    }

    /// El resumen completo, contra un árbol conocido: los tres tamaños, la
    /// cantidad de modelos y la de dictados.
    #[test]
    fn el_resumen_cuenta_lo_que_hay_de_verdad() {
        let raiz = carpeta_temporal("resumen");
        let programa = raiz.join("instalacion");
        escribir(&programa.join("mithflow.exe"), 1500);
        escribir(&programa.join("ggml-vulkan.dll"), 2500);

        let modelos = raiz.join("MithFlow").join("models");
        escribir(&modelos.join("whisper-large-v3-turbo-Q4_K_M.gguf"), 800);
        escribir(&modelos.join("whisper-large-v3-turbo-F16.gguf"), 1200);
        // Un `.part` de una descarga cortada: ocupa disco y se va a borrar, pero
        // no es un modelo que el usuario pueda usar.
        escribir(&modelos.join("whisper-large-v3-turbo-Q5_K_M.gguf.part"), 300);

        let datos = raiz.join("com.mithdata.mithflow");
        escribir(&datos.join("ajustes.json"), 388);
        let historial = datos.join("history-nativo.jsonl");
        fs::write(
            &historial,
            "{\"ts\":\"2026-07-21T10:00:00\",\"final\":\"uno\"}\n\
             {\"ts\":\"2026-07-21T11:00:00\",\"final\":\"dos\"}\n\
             {\"ts\":\"2026-07-21T12:00:00\",\"final\":\"tres\"}\n",
        )
        .expect("no pude escribir el historial");

        let resumen = medir(Some(&programa), &modelos, &datos, &historial, None);

        assert_eq!(resumen.programa_bytes, 4000);
        assert_eq!(resumen.modelos_bytes, 2300, "el .part también ocupa");
        assert_eq!(resumen.modelos_cantidad, 2, "el .part no es un modelo");
        assert_eq!(resumen.datos_bytes, 388 + historial_bytes(&historial));
        assert_eq!(resumen.dictados, 3);
        assert!(resumen.hay_desinstalador);
        assert_eq!(resumen.motivo_sin_desinstalador, None);
        let esperada = programa.display().to_string();
        assert_eq!(resumen.programa_ruta.as_deref(), Some(esperada.as_str()));
    }

    fn historial_bytes(ruta: &Path) -> u64 {
        fs::metadata(ruta).expect("el historial tiene que existir").len()
    }

    /// Sin instalación no se mide la carpeta del programa: en desarrollo sería
    /// `target/release/`, y sus gigabytes de artefactos no son lo que se
    /// desinstala.
    #[test]
    fn sin_desinstalador_el_resumen_no_mide_el_programa_y_explica_por_que() {
        let raiz = carpeta_temporal("resumen_sin_instalar");
        let motivo = sin_desinstalador(&raiz.join(DESINSTALADOR));

        let resumen = medir(
            None,
            &raiz.join("modelos"),
            &raiz.join("datos"),
            &raiz.join("historial.jsonl"),
            Some(motivo),
        );

        assert_eq!(resumen.programa_bytes, 0);
        assert_eq!(resumen.programa_ruta, None);
        assert!(!resumen.hay_desinstalador);
        let motivo = resumen
            .motivo_sin_desinstalador
            .expect("sin desinstalador hay que decir por qué");
        assert!(motivo.contains(DESINSTALADOR), "{motivo}");
        assert!(
            motivo.contains("no toqué ningún dato"),
            "el mensaje tiene que aclarar que no se borró nada: {motivo}"
        );
        assert!(
            motivo.contains("desarrollo"),
            "y explicar el caso que lo produce: {motivo}"
        );
    }

    /* --------------------------------------------------------- la ocupación */

    /// Con audio en vuelo no se desinstala: se avisa y no se hace nada.
    #[test]
    fn grabando_o_transcribiendo_no_se_desinstala() {
        for estado in [Estado::Grabando, Estado::Transcribiendo] {
            let motivo = ocupado(estado.clave())
                .unwrap_or_else(|| panic!("{} tiene que frenar la desinstalación", estado.clave()));
            assert!(
                motivo.contains("no toqué nada"),
                "el aviso tiene que decir que no pasó nada: {motivo}"
            );
        }
        for estado in [
            Estado::Listo,
            Estado::Cargando,
            Estado::SinModelo("x".into()),
            Estado::Error("x".into()),
        ] {
            assert_eq!(
                ocupado(estado.clave()),
                None,
                "{} no impide desinstalar",
                estado.clave()
            );
        }
    }
}
