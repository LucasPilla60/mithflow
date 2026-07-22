//! El hilo que carga el modelo y transcribe.
//!
//! Está separado del director por dos motivos que valen igual:
//!
//! 1. **Cargar el modelo y calentarlo tarda entre veinte segundos y un minuto**
//!    la primera vez en cada máquina (1,8 s de carga y 17 s compilando shaders
//!    de Vulkan medidos en el escritorio; en una integrada, más). Ésa es la
//!    cifra que se le dice al usuario en todos lados. Si eso pasara en el hilo
//!    de la interfaz, la app arrancaría congelada.
//! 2. **El buffer de audio se MUEVE hasta acá.** El director le manda el `Vec`
//!    y se queda sin él: no hay estado compartido que un segundo atajo pueda
//!    pisar mientras esto transcribe. Es el bug real que tuvo la versión
//!    Python, donde `process_recording` y `toggle` escribían los dos sobre el
//!    mismo buffer global; en Rust lo resuelve el sistema de tipos, no un lock.
//!
//! # Cuándo arranca
//!
//! Dos momentos posibles, los dos por [`resolver_y_lanzar`]: al abrir la
//! aplicación y —sólo si ahí no había ningún modelo— apenas una descarga termina
//! bien. El segundo existe para que instalar MithFlow y bajar el modelo no
//! obligue a cerrarla y volver a abrirla, que era exactamente lo que pasaba: la
//! app pedía descargar un modelo y después seguía diciendo que no había ninguno.
//!
//! # Por qué los ajustes viajan por el mismo canal que el audio
//!
//! Vocabulario, muletillas y modo de limpieza los aplica este hilo, que está
//! bloqueado esperando audio. Mandarlos por otra vía obligaría a que alguien
//! los escriba mientras acá se leen —o sea un lock en el camino del dictado—;
//! por la misma cola llegan **ordenados** respecto de los dictados y sin
//! sincronización de por medio. Un cambio guardado a mitad de una
//! transcripción se aplica a la siguiente, que es exactamente lo que se espera.

use crate::ajustes::Ajustes;
use crate::director::Mensaje;
use crate::estado::FalloDelMotor;
use crate::{eventos, rutas};
use mithflow_core::models::Modelo;
use mithflow_core::stt::ErrorDeModelo;
use mithflow_core::{dictate_con, hardware, stt::Transcriber, Preferencias};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard, RwLock, TryLockError};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Lo que se le puede mandar al motor.
pub enum AlMotor {
    /// Audio mono de 16 kHz listo para transcribir.
    Audio(Vec<f32>),
    /// El usuario guardó Ajustes: hay preferencias nuevas que aplicar.
    ///
    /// Va en `Box` porque `Ajustes` es varias veces más grande que un `Vec`, y
    /// el enum entero mide lo que su variante mayor: sin el `Box`, cada buffer
    /// de audio que pasa por la cola arrastraría ese tamaño.
    Ajustes(Box<Ajustes>),
}

/// Un modelo cargándose por vez en todo el proceso.
///
/// El motor y el perfilado de hardware abren cada uno su propio `Transcriber`
/// —hasta 1,5 GB de pesos y ~17 s compilando shaders de Vulkan la primera vez en
/// la máquina— y desde que una descarga puede arrancar el motor **sin
/// reiniciar**, los dos pueden querer hacerlo a la vez: el asistente baja el
/// modelo de medición, eso lanza el motor, y menos de un segundo después empieza
/// a medir. Midiendo con el motor peleando por la misma GPU, la máquina parece
/// más lenta de lo que es y la recomendación sale para abajo. Sería el peor
/// error posible acá: silencioso, y se lleva puesta la calidad de todos los
/// dictados que vengan después.
///
/// **Se serializa la CARGA, no el uso.** El testigo se suelta apenas el modelo
/// está cargado y calentado; el motor sigue vivo con sus pesos en memoria sin
/// retener nada. Nadie lo toma dos veces, así que no hay forma de trabarse.
static CARGA_DE_MODELO: Mutex<()> = Mutex::new(());

/// Cuánto se espera el testigo antes de seguir igual.
///
/// **Sin tope no habría salida.** El que espera acá puede ser el motor, y si un
/// driver de Vulkan se cuelga del otro lado la aplicación se quedaría en
/// `Cargando` para siempre, contestándole al atajo "todavía estoy preparando el
/// motor" hasta que el usuario la mate. Vencido el plazo se sigue sin el
/// testigo: en el peor caso hay dos cargas a la vez —lento, y con la medición
/// avisada— que es estrictamente mejor que no arrancar nunca.
///
/// Dos minutos son holgados contra lo que de verdad tarda: cargar y calentar
/// son entre veinte segundos y un minuto, y medir agrega unos pocos segundos.
pub const TOPE_DE_ESPERA: Duration = Duration::from_secs(120);

/// Cada cuánto se reintenta mientras se espera el testigo. El hilo duerme, así
/// que 50 ms no cuestan nada y acotan la latencia de arranque.
const SONDEO: Duration = Duration::from_millis(50);

/// Toma el testigo de [`CARGA_DE_MODELO`], esperando como mucho `tope`.
///
/// `None` significa que se venció el plazo y **quien llama tiene que seguir
/// igual**: ver [`TOPE_DE_ESPERA`].
///
/// Un `Mutex` envenenado no invalida nada acá —lo que protege es `()`, no un
/// dato con invariantes que un panic pueda dejar a medias—, así que se sigue
/// igual en vez de tumbar la carga.
#[must_use = "el testigo serializa la carga sólo mientras esté vivo: llamarla y \
              descartar lo que devuelve lo suelta en el acto y no protege nada"]
pub fn testigo_de_carga(tope: Duration) -> Option<MutexGuard<'static, ()>> {
    let limite = Instant::now() + tope;
    loop {
        match CARGA_DE_MODELO.try_lock() {
            Ok(testigo) => return Some(testigo),
            Err(TryLockError::Poisoned(envenenado)) => return Some(envenenado.into_inner()),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= limite {
                    return None;
                }
                std::thread::sleep(SONDEO);
            }
        }
    }
}

/// Lo que el motor tiene cargado en memoria ahora mismo.
///
/// Es de sólo lectura para todos menos el propio motor, que lo escribe una vez
/// al terminar de cargar. **No decide nada**: para eso está el director, que es
/// el único que sabe si hay un motor (ver `director::tras_una_descarga`). Esto
/// contesta otra pregunta, la del perfilado: "¿ya hay una medición hecha con
/// este mismo modelo, para no cargarlo de nuevo?".
///
/// No lleva la ruta del `.gguf` a propósito: quien lea esto no tiene que
/// decidir nada mirándola. La medición se publica **sólo** si se hizo con el
/// modelo de perfilado ([`medir_si_es_el_de_perfilado`]), así que un
/// `factor_tiempo_real` presente ya significa "comparable con los umbrales".
#[derive(Debug, Clone)]
pub struct MotorResidente {
    /// El backend al que ligó de verdad ("vulkan", "cpu"…).
    pub backend: String,
    /// El factor de tiempo real medido con el modelo cargado, o `None` si ese
    /// modelo no es el de perfilado y por lo tanto su número no sirve para
    /// decidir.
    pub factor_tiempo_real: Option<f32>,
}

impl MotorResidente {
    /// La medición que el perfilado puede reusar tal cual, si la hay.
    pub fn medicion_reutilizable(&self) -> Option<(f32, String)> {
        self.factor_tiempo_real.map(|f| (f, self.backend.clone()))
    }
}

static RESIDENTE: RwLock<Option<MotorResidente>> = RwLock::new(None);

/// Qué modelo tiene cargado el motor, si es que hay uno.
pub fn residente() -> Option<MotorResidente> {
    match RESIDENTE.read() {
        Ok(g) => g.clone(),
        // Envenenado no corrompe nada: adentro hay un dato inmutable que se
        // reemplaza entero, no una invariante entre campos.
        Err(envenenado) => envenenado.into_inner().clone(),
    }
}

fn fijar_residente(nuevo: MotorResidente) {
    match RESIDENTE.write() {
        Ok(mut g) => *g = Some(nuevo),
        Err(envenenado) => *envenenado.into_inner() = Some(nuevo),
    }
}

/// Resuelve dónde está el `.gguf` y arranca el motor sobre él.
///
/// Es el camino que usan los **dos** momentos en que el motor puede arrancar: el
/// arranque de la aplicación y una descarga que termina bien con el motor
/// ausente. Que sea uno solo es lo que garantiza que el motor lanzado en
/// caliente resuelva el modelo con las mismas reglas —incluida la sustitución de
/// [`rutas::modelo`] y su aviso— que el del arranque.
///
/// Un `Err` significa que no se va a poder dictar, y de qué clase es; quien
/// llama decide cómo contarlo.
pub fn resolver_y_lanzar(
    app: &AppHandle,
    al_director: &Sender<Mensaje>,
    cfg: &Ajustes,
) -> Result<Sender<AlMotor>, FalloDelMotor> {
    // Sin dónde escribir el historial no hay dictado, y no es algo que se
    // arregle descargando un modelo: es una falla.
    let historial = rutas::historial(app).map_err(FalloDelMotor::Roto)?;
    println!("historial: {}", historial.display());

    let (ruta, aviso) = rutas::modelo(cfg)?;
    if let Some(aviso) = aviso {
        println!("{aviso}");
        eventos::aviso(app, &aviso, "info");
    }
    println!("cargando el modelo {}…", ruta.display());
    Ok(lanzar(al_director.clone(), ruta, historial, cfg.clone()))
}

/// Arranca el motor y devuelve el extremo por donde mandarle trabajo.
///
/// Nunca falla acá: todo lo que pueda salir mal (el modelo no carga) viaja por
/// `al_director` como [`Mensaje::MotorListo`], que es quien sabe convertirlo en
/// un estado visible. Devolver un `Result` obligaría a quien llama a decidir
/// dos veces lo mismo.
///
/// **Privada a propósito**: el único camino para arrancar el motor es
/// [`resolver_y_lanzar`], y que el compilador lo garantice es más fuerte que un
/// comentario pidiéndolo. Un segundo camino podría resolver el `.gguf` con
/// otras reglas y divergir en silencio del arranque de la aplicación.
fn lanzar(
    al_director: Sender<Mensaje>,
    modelo: PathBuf,
    historial: PathBuf,
    ajustes: Ajustes,
) -> Sender<AlMotor> {
    let (al_motor, cola) = mpsc::channel::<AlMotor>();

    let creado = std::thread::Builder::new()
        .name("mithflow-motor".into())
        .spawn(move || trabajar(cola, al_director, modelo, historial, ajustes));
    if let Err(e) = creado {
        eprintln!("no pude crear el hilo del motor: {e}");
    }
    al_motor
}

fn trabajar(
    cola: mpsc::Receiver<AlMotor>,
    al_director: Sender<Mensaje>,
    modelo: PathBuf,
    historial: PathBuf,
    ajustes: Ajustes,
) {
    // Cargar y calentar es lo que hay que serializar contra el perfilado; ver
    // `CARGA_DE_MODELO`. El testigo se suelta enseguida, antes de atender la
    // cola: lo que se protege es la carga, no el motor ya cargado.
    let carga = testigo_de_carga(TOPE_DE_ESPERA);
    if carga.is_none() {
        eprintln!(
            "el testigo de carga no se liberó en {} s; cargo el modelo igual",
            TOPE_DE_ESPERA.as_secs()
        );
    }
    let mut transcriber = match Transcriber::new(&modelo) {
        Ok(t) => t,
        Err(e) => {
            // Fin del hilo. El director publica el estado que corresponda y la
            // app sigue viva: el usuario todavía puede abrir Ajustes y
            // descargar un modelo, que es justo la forma de arreglar esto.
            let _ = al_director.send(Mensaje::MotorListo(Err(clasificar(e))));
            return;
        }
    };
    // El vocabulario propio se aplica ANTES del calentamiento: así la primera
    // inferencia real ya usa el prompt definitivo.
    transcriber.fijar_vocabulario(&ajustes.vocabulario);

    let backend = transcriber.backend();
    let calentamiento = calentar(&mut transcriber);
    // Con el modelo de perfilado cargado, medir acá cuesta tres inferencias
    // sobre un `Transcriber` que YA está en memoria y le ahorra al perfilado
    // abrir un segundo. Ver `medir_si_es_el_de_perfilado`.
    let factor_tiempo_real = medir_si_es_el_de_perfilado(&mut transcriber, &modelo);
    fijar_residente(MotorResidente {
        backend: backend.clone(),
        factor_tiempo_real,
    });
    drop(carga);

    let mut preferencias = ajustes.preferencias();
    let _ = al_director.send(Mensaje::MotorListo(Ok(format!(
        "motor listo sobre {backend} ({calentamiento})"
    ))));

    // El `for` termina cuando el director suelta su extremo, o sea al cerrar.
    for trabajo in cola {
        match trabajo {
            AlMotor::Ajustes(nuevos) => {
                transcriber.fijar_vocabulario(&nuevos.vocabulario);
                preferencias = nuevos.preferencias();
            }
            AlMotor::Audio(audio) => {
                if dictar(&mut transcriber, &audio, &historial, &preferencias, &al_director)
                    .is_err()
                {
                    return; // el director se fue; no hay a quién contarle
                }
            }
        }
    }
}

/// Mide el factor de tiempo real **sobre el modelo que este motor ya tiene
/// cargado**, y sólo si es el de perfilado.
///
/// # Por qué acá y no en el perfilado
///
/// El perfilado abría su propio `Transcriber` sobre el mismo `.gguf`. Desde que
/// una descarga arranca el motor sin reiniciar, eso pasa con el motor **vivo**:
/// dos copias de los pesos residentes al mismo tiempo (~1 GB con el `Q4_K_M`,
/// ~3,2 GB si fuera el `F16`). En el escritorio no se nota; en una notebook con
/// gráficos integrados y 8 GB, el segundo `Model::load` puede fallar o caer a
/// CPU en silencio, y entonces el perfilado mide una máquina más lenta de la que
/// es y recomienda un modelo peor del que corresponde. Midiendo acá esa segunda
/// copia no existe nunca.
///
/// # Por qué sólo con el modelo de perfilado
///
/// El número depende del modelo con el que se mide —uno más chico da un factor
/// más alto— y los umbrales de `hardware` están calibrados sobre el `Q4_K_M`.
/// Medir con el `F16` cargado daría un factor mucho más bajo y mandaría a todo
/// el mundo al modelo más chico. Con cualquier otro modelo se devuelve `None` y
/// el perfilado hace lo de siempre.
///
/// El costo es de tres inferencias sobre un modelo ya cargado y caliente
/// (~1 s), y se paga sólo en las máquinas cuyo modelo ES el de perfilado, que
/// son justamente las chicas: las mismas a las que la segunda copia les hace
/// daño.
fn medir_si_es_el_de_perfilado(transcriber: &mut Transcriber, modelo: &Path) -> Option<f32> {
    if !es_el_modelo_de_perfilado(modelo) {
        return None;
    }
    match hardware::medir_factor_tiempo_real(transcriber) {
        Ok(factor) => {
            println!("medido con el modelo ya cargado: {factor:.2}x tiempo real");
            Some(factor)
        }
        // No es fatal: lo único que se pierde es el atajo, y el perfilado
        // vuelve a medir por su cuenta.
        Err(e) => {
            eprintln!("no pude medir con el modelo ya cargado: {e}");
            None
        }
    }
}

/// ¿El `.gguf` cargado es el modelo de perfilado del catálogo?
///
/// Se compara por **nombre de archivo** y no por ruta completa: `MITHFLOW_MODELO`
/// puede apuntar al mismo modelo fuera de `%APPDATA%` —es lo que hace el
/// desarrollo, con los `.gguf` en `app-nativa/models/`— y ahí la medición vale
/// igual, porque lo que decide el número es el modelo, no dónde está guardado.
fn es_el_modelo_de_perfilado(modelo: &Path) -> bool {
    modelo
        .file_name()
        .is_some_and(|nombre| nombre == Modelo::DE_PERFILADO.nombre_archivo())
}

/// Traduce el fallo del núcleo al que entiende el director.
///
/// Es el punto exacto donde se decide si el usuario ve un aviso o un error, y
/// **la decisión no mira el texto del mensaje**: la toma
/// [`ErrorDeModelo::falta_el_archivo`], que sabe si el `.gguf` estaba en el
/// disco porque es quien lo miró. Un archivo ausente es el primer arranque —o
/// alguien que borró el modelo— y se arregla descargando; un archivo presente
/// que no carga (GGUF cortado, sin memoria, backends de ggml rotos) es una
/// falla y tiene que seguir viéndose como tal.
fn clasificar(error: ErrorDeModelo) -> FalloDelMotor {
    if error.falta_el_archivo() {
        FalloDelMotor::SinModelo(format!(
            "{error}. Bajá un modelo desde Ajustes para poder dictar."
        ))
    } else {
        FalloDelMotor::Roto(error.to_string())
    }
}

/// Un dictado y su informe al director. `Err(())` significa que el director ya
/// no está escuchando.
fn dictar(
    transcriber: &mut Transcriber,
    audio: &[f32],
    historial: &std::path::Path,
    preferencias: &Preferencias,
    al_director: &Sender<Mensaje>,
) -> Result<(), ()> {
    let salida = dictate_con(transcriber, audio, historial, preferencias);
    // La entrada viene del propio pipeline, que es quien la escribió: releer el
    // archivo para buscarla sería O(n) por dictado y, con el texto sin guardar,
    // no habría por dónde reconocerla.
    let entrada = match &salida {
        Ok(Some(resultado)) => resultado.entrada.clone(),
        _ => None,
    };
    al_director
        .send(Mensaje::Transcripcion {
            salida: Box::new(salida),
            entrada,
        })
        .map_err(|_| ())
}

/// Paga por adelantado la compilación de los shaders de Vulkan.
///
/// **No puede usar silencio.** `Transcriber::transcribe` descarta el audio por
/// debajo de `MIN_SPEECH_RMS` ANTES de llamar al modelo, así que un buffer de
/// ceros volvería en microsegundos sin compilar un solo shader: el
/// calentamiento sería un no-op silencioso y el primer dictado del usuario se
/// colgaría 17 s. Por eso se reusa el clip de referencia del perfilado, que
/// está construido justo para estar por encima de esa compuerta.
///
/// Que falle no es fatal: lo único que se pierde es el adelanto.
fn calentar(transcriber: &mut Transcriber) -> String {
    let t0 = Instant::now();
    match transcriber.transcribe(&hardware::audio_de_referencia()) {
        Ok(_) => format!("calentado en {:.1} s", t0.elapsed().as_secs_f32()),
        Err(e) => format!("sin calentar: {e}; el primer dictado va a tardar más"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// La regla del testigo, que hasta acá era sólo un comentario: **de a una
    /// carga por vez**. Sin ella, el motor y el perfilado abren cada uno su
    /// `Transcriber` a la vez y en una máquina de 8 GB la segunda no entra.
    ///
    /// Se comprueba desde afuera, con dos hilos, porque es lo único que
    /// distingue un mutex de verdad de una variable que nadie mira.
    #[test]
    fn la_carga_de_modelo_es_de_a_una_por_vez() {
        let (aviso, escucha) = mpsc::channel::<()>();
        let primero = testigo_de_carga(TOPE_DE_ESPERA).expect("el testigo estaba libre");

        let segundo = std::thread::spawn(move || {
            let _testigo =
                testigo_de_carga(TOPE_DE_ESPERA).expect("entra apenas el primero suelta");
            let _ = aviso.send(());
        });

        assert!(
            escucha
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "el segundo entró con el primero adentro: no se está serializando nada"
        );

        drop(primero);
        escucha
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("soltado el testigo, el segundo tiene que entrar");
        segundo.join().expect("el segundo hilo paniqueó");
    }

    /// Y el tope, que es la otra mitad: esperar sin plazo deja al motor clavado
    /// en `Cargando` para siempre si el de enfrente se cuelga. Vencido el
    /// plazo se contesta `None` y quien llama sigue igual.
    #[test]
    fn esperar_el_testigo_tiene_plazo() {
        let ocupado = testigo_de_carga(TOPE_DE_ESPERA).expect("el testigo estaba libre");
        let t0 = Instant::now();

        let tope = std::time::Duration::from_millis(150);
        assert!(
            testigo_de_carga(tope).is_none(),
            "con el testigo tomado, la espera acotada no puede devolver otro"
        );
        assert!(
            t0.elapsed() >= tope,
            "volvió antes de agotar el plazo: no esperó nada"
        );
        drop(ocupado);
    }

    /// De qué modelo se puede reusar la medición. El número depende del modelo
    /// con el que se mide, así que confundirse acá es recomendar mal.
    #[test]
    fn solo_el_modelo_de_perfilado_sirve_para_medir() {
        let carpeta = Path::new("D:\\lo\\que\\sea");
        assert!(es_el_modelo_de_perfilado(
            &carpeta.join(Modelo::DE_PERFILADO.nombre_archivo())
        ));
        for otro in Modelo::TODOS.into_iter().filter(|m| *m != Modelo::DE_PERFILADO) {
            assert!(
                !es_el_modelo_de_perfilado(&carpeta.join(otro.nombre_archivo())),
                "{otro} no está calibrado para medir: su factor no es comparable"
            );
        }
        assert!(!es_el_modelo_de_perfilado(Path::new("")));
    }

    /// Un motor cargado con otro modelo no ofrece medición, pero sí su backend:
    /// es lo que permite avisar cuando el perfilado ligó a otro y el número no
    /// es el de la máquina que va a dictar.
    #[test]
    fn solo_se_reusa_la_medicion_que_existe() {
        let sin_medir = MotorResidente {
            backend: "vulkan".into(),
            factor_tiempo_real: None,
        };
        assert!(sin_medir.medicion_reutilizable().is_none());

        let medido = MotorResidente {
            factor_tiempo_real: Some(3.5),
            ..sin_medir
        };
        assert_eq!(
            medido.medicion_reutilizable(),
            Some((3.5, "vulkan".to_string()))
        );
    }

    /// Un modelo que todavía no está en el disco NO es una falla: es el primer
    /// arranque. Este test y el siguiente son los dos lados de la misma
    /// regla, y el par tiene que moverse junto.
    #[test]
    fn un_modelo_que_falta_lleva_al_estado_sin_modelo() {
        let fallo = clasificar(ErrorDeModelo::Faltante(
            Path::new("D:\\modelos\\turbo.gguf").to_path_buf(),
        ));

        assert!(matches!(fallo, FalloDelMotor::SinModelo(_)));
        let estado = fallo.estado();
        assert_eq!(estado.clave(), "sin-modelo");
        assert!(!estado.es_falla(), "faltar el archivo no es una falla");
        assert!(
            estado.detalle().is_some_and(|d| d.contains("Ajustes")),
            "el motivo tiene que decir dónde se arregla: {estado:?}"
        );
    }

    /// Y el reverso, que es la mitad que no se puede aflojar: un modelo que
    /// está pero no carga sigue siendo `Error` en rojo. Bajarlo de tono
    /// escondería un problema real.
    #[test]
    fn un_modelo_que_no_carga_sigue_siendo_error() {
        let fallo = clasificar(ErrorDeModelo::NoCarga(
            "no pude cargar el modelo D:\\modelos\\turbo.gguf: header inválido".into(),
        ));

        assert!(matches!(fallo, FalloDelMotor::Roto(_)));
        let estado = fallo.estado();
        assert_eq!(estado.clave(), "error");
        assert!(estado.es_falla());
        assert!(
            !estado.detalle().unwrap_or_default().contains("Bajá"),
            "no se le pide descargar a quien ya tiene el archivo: {estado:?}"
        );
    }
}
