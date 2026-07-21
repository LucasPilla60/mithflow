//! Catálogo de modelos y descarga verificada.
//!
//! # Por qué el hash va compilado adentro del binario
//!
//! Un hash que se descarga del mismo host que el modelo no verifica nada: quien
//! pueda cambiar el `.gguf` puede cambiar el `.sha256` que lo acompaña. Los
//! valores de [`Modelo::sha256`] son constantes del programa, medidas una vez
//! sobre los archivos reales, y son lo único que decide si un archivo bajado se
//! instala o se tira.
//!
//! # Por qué el catálogo tiene tres modelos y no cinco
//!
//! El repositorio de origen publica también `Q8_0` y `Q6_K`. No están acá a
//! propósito: una entrada de catálogo sin su hash compilado sería un agujero, y
//! el hash sólo se puede obtener bajando el archivo y midiéndolo. Los tres que
//! están son los que [`crate::hardware::elegir_modelo`] puede llegar a devolver.
//!
//! # El flujo de la descarga
//!
//! Se baja a un `.part`, se verifica el hash del `.part` **completo**, y recién
//! entonces se renombra al nombre final. Nunca existe un archivo con el nombre
//! definitivo que no haya pasado la verificación.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Único origen aceptado. Las URLs del catálogo son constantes, así que esto no
/// filtra entrada del usuario: es la red que hace fallar la compilación de un
/// futuro agregado que apunte a otro lado.
pub const HOST_PERMITIDO: &str = "https://huggingface.co/";

/// Carpeta del repositorio de GGUF probados contra `transcribe.cpp` por sus
/// propios autores (ver `DECISIONES.md`).
const BASE_URL: &str =
    "https://huggingface.co/handy-computer/whisper-large-v3-turbo-gguf/resolve/main/";

/// Cuánto se lee del socket por vuelta. 256 KiB amortiza el syscall sin inflar
/// la memoria residente durante una descarga de 1,5 GB.
const TAMANO_BLOQUE: usize = 256 * 1024;

/// Cada cuántos bytes se avisa del progreso. Sin esto serían ~6.000 llamadas
/// para el `F16`, y del otro lado del callback puede haber un cruce de proceso.
const PASO_PROGRESO: u64 = 4 * 1024 * 1024;

/// Cuánto se espera a que el host conteste.
const ESPERA_CONEXION: Duration = Duration::from_secs(30);

/// Cuánto se tolera sin recibir un solo byte antes de dar la conexión por
/// colgada.
///
/// En la serie 0.13 el `timeout` del cliente bloqueante se aplica a **cada
/// operación** —`Read for Response` envuelve cada `read` con él—, no al total de
/// la descarga. Eso es exactamente lo que hace falta acá: 1,5 GB por una
/// conexión hogareña son minutos legítimos, pero un socket mudo no puede
/// esperar para siempre.
const ESPERA_LECTURA: Duration = Duration::from_secs(60);

/// Tope de saltos. HuggingFace redirige a su CDN, así que seguir redirecciones
/// es obligatorio; encadenarlas sin límite no.
const MAX_REDIRECCIONES: usize = 5;

/// Los modelos que el perfilado puede elegir.
///
/// Todos son `large-v3-turbo`: cambia la cuantización, no la arquitectura, así
/// que la calidad de transcripción baja de forma gradual y el vocabulario
/// propio sigue funcionando igual en los tres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modelo {
    /// Sin cuantizar. El de mejor calidad y el más pesado.
    F16,
    /// Cuantización de compromiso: un tercio del peso del `F16`.
    Q5KM,
    /// El piso. Es también el que se usa para perfilar la máquina, porque es el
    /// más rápido de bajar.
    Q4KM,
}

impl Modelo {
    /// El catálogo completo, en orden de mayor a menor peso.
    pub const TODOS: [Modelo; 3] = [Modelo::F16, Modelo::Q5KM, Modelo::Q4KM];

    /// El modelo con el que se perfila la máquina: el más liviano, porque el
    /// perfilado ocurre antes de saber cuál conviene bajar.
    pub const DE_PERFILADO: Modelo = Modelo::Q4KM;

    pub fn nombre_archivo(self) -> &'static str {
        match self {
            Modelo::F16 => "whisper-large-v3-turbo-F16.gguf",
            Modelo::Q5KM => "whisper-large-v3-turbo-Q5_K_M.gguf",
            Modelo::Q4KM => "whisper-large-v3-turbo-Q4_K_M.gguf",
        }
    }

    pub fn url(self) -> String {
        format!("{BASE_URL}{}", self.nombre_archivo())
    }

    /// Tamaño exacto del archivo. Medido sobre la descarga real, no estimado:
    /// se usa para el porcentaje de progreso y para detectar una descarga
    /// cortada antes de gastar el hash.
    pub fn bytes(self) -> u64 {
        match self {
            Modelo::F16 => 1_625_935_520,
            Modelo::Q5KM => 619_628_128,
            Modelo::Q4KM => 536_069_728,
        }
    }

    /// SHA-256 en hexadecimal, medido el 21/7/2026 sobre el archivo bajado
    /// (`Get-FileHash -Algorithm SHA256`). **Es la raíz de confianza de la
    /// descarga**: ver la nota del módulo.
    pub fn sha256(self) -> &'static str {
        match self {
            Modelo::F16 => "e1d0144e9afc9f479d9e51fc92c7dea9dc36059655eeb3819f16ad2de779046a",
            Modelo::Q5KM => "977b5db4e004349dffd1ab9caa10ba5aaba3fc3edd3ba72cadb84328a3203e36",
            Modelo::Q4KM => "ecfe9b6beb4ab18fef49187cc968cc74b5168b94629c8830e2ca6b794c6e25ed",
        }
    }

    /// El peso redondeado, para mostrarle al usuario antes de que confirme la
    /// descarga.
    pub fn megabytes(self) -> u64 {
        self.bytes() / (1024 * 1024)
    }
}

impl std::fmt::Display for Modelo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let etiqueta = match self {
            Modelo::F16 => "large-v3-turbo F16",
            Modelo::Q5KM => "large-v3-turbo Q5_K_M",
            Modelo::Q4KM => "large-v3-turbo Q4_K_M",
        };
        write!(f, "{etiqueta}")
    }
}

/// Dónde viven los modelos: `%APPDATA%\MithFlow\models\`.
///
/// Es la ubicación que fija el spec (§14) para que se puedan copiar a mano
/// entre máquinas. En Windows `dirs::config_dir()` devuelve `%APPDATA%`, que es
/// el perfil *itinerante*; si alguna vez se despliega en un dominio con perfiles
/// móviles habría que mudarlo a `%LOCALAPPDATA%`, porque 1,5 GB no deberían
/// sincronizarse contra un servidor en cada inicio de sesión.
pub fn directorio() -> Result<PathBuf, String> {
    let base = dirs::config_dir().ok_or_else(|| {
        "no pude resolver el directorio de configuración del usuario (%APPDATA%)".to_string()
    })?;
    Ok(base.join("MithFlow").join("models"))
}

/// Dónde debería estar este modelo, exista o no.
pub fn ruta(modelo: Modelo) -> Result<PathBuf, String> {
    Ok(directorio()?.join(modelo.nombre_archivo()))
}

/// ¿Está el modelo instalado y con el tamaño que corresponde?
///
/// **No vuelve a calcular el hash**: son segundos por cada 1,5 GB y esto se
/// consulta en cada arranque. El hash se verifica una vez, antes de instalar
/// (ver [`descargar`]). El tamaño exacto es el centinela barato contra un
/// archivo truncado. Para revalidar de verdad una copia traída a mano está
/// [`verificar_instalado`].
pub fn esta_descargado(modelo: Modelo) -> bool {
    ruta(modelo)
        .ok()
        .and_then(|p| fs::metadata(p).ok())
        .is_some_and(|m| m.is_file() && m.len() == modelo.bytes())
}

/// Recalcula el hash del modelo ya instalado y lo compara con el compilado.
///
/// Es la verificación que [`esta_descargado`] deliberadamente no hace: cuesta
/// leer el archivo entero. Existe porque el spec permite copiar los modelos
/// entre máquinas a mano, y una copia a mano nunca pasó por [`descargar`].
pub fn verificar_instalado(modelo: Modelo) -> Result<(), String> {
    let path = ruta(modelo)?;
    let real = sha256_de_archivo(&path)?;
    if real.eq_ignore_ascii_case(modelo.sha256()) {
        Ok(())
    } else {
        Err(format!(
            "{} no coincide con el hash esperado (esperaba {}, obtuve {real})",
            path.display(),
            modelo.sha256()
        ))
    }
}

/// SHA-256 en hexadecimal minúsculo de un archivo, leído por bloques.
///
/// Por bloques y no de una: el `F16` son 1,5 GB y cargarlo entero en memoria
/// para hashearlo sería gratis de escribir y caro de correr.
pub fn sha256_de_archivo(path: &Path) -> Result<String, String> {
    let mut archivo =
        File::open(path).map_err(|e| format!("no pude abrir {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut bloque = vec![0u8; TAMANO_BLOQUE];
    loop {
        let leidos = archivo
            .read(&mut bloque)
            .map_err(|e| format!("no pude leer {}: {e}", path.display()))?;
        if leidos == 0 {
            break;
        }
        hasher.update(&bloque[..leidos]);
    }
    let resumen = hasher.finalize();
    let mut hex = String::with_capacity(resumen.len() * 2);
    for byte in resumen.iter() {
        // `write!` sobre un `String` no puede fallar; el `Result` se descarta
        // adrede en vez de propagar un error imposible.
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// Verifica el `.part` contra el hash esperado y, sólo si coincide, lo renombra
/// a su nombre definitivo.
///
/// Un archivo que no verifica **se borra**. Conservarlo sería peor que inútil:
/// el próximo intento lo encontraría con el tamaño correcto y lo retomaría como
/// si fuera bueno, o peor, alguien lo renombraría a mano.
pub fn verificar_y_promover(
    parcial: &Path,
    destino: &Path,
    sha256_esperado: &str,
) -> Result<(), String> {
    let real = sha256_de_archivo(parcial)?;
    if !real.eq_ignore_ascii_case(sha256_esperado) {
        let borrado = fs::remove_file(parcial);
        return Err(format!(
            "el archivo descargado no coincide con el hash esperado \
             (esperaba {sha256_esperado}, obtuve {real}){}",
            match borrado {
                Ok(()) => "; se descartó",
                Err(_) => "; ADEMÁS no pude borrarlo, sacalo a mano",
            }
        ));
    }
    fs::rename(parcial, destino).map_err(|e| {
        format!(
            "verificó bien pero no pude moverlo a {}: {e}",
            destino.display()
        )
    })
}

/// Rechaza cualquier URL que no venga del único host en la lista blanca.
fn validar_url(url: &str) -> Result<(), String> {
    if url.starts_with(HOST_PERMITIDO) {
        Ok(())
    } else {
        Err(format!(
            "la URL {url} no pertenece al único origen aceptado ({HOST_PERMITIDO})"
        ))
    }
}

/// Cliente con esperas acotadas y redirecciones vigiladas.
///
/// La lista blanca se aplica a la URL inicial, no a las redirecciones: el CDN
/// de HuggingFace vive en otro host y bloquearlo rompería toda descarga. Lo que
/// sí se exige de cada salto es **https**, y el que garantiza que el archivo es
/// el correcto —venga del host que venga— es el SHA-256 compilado.
fn cliente() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(ESPERA_CONEXION)
        .timeout(ESPERA_LECTURA)
        .redirect(reqwest::redirect::Policy::custom(|intento| {
            if intento.url().scheme() != "https" {
                intento.error("redirección a un esquema que no es https".to_string())
            } else if intento.previous().len() >= MAX_REDIRECCIONES {
                intento.error(format!("más de {MAX_REDIRECCIONES} redirecciones"))
            } else {
                intento.follow()
            }
        }))
        .build()
        .map_err(|e| format!("no pude construir el cliente HTTP: {e}"))
}

/// Baja el modelo y lo deja instalado, o falla sin dejar nada a medio instalar.
///
/// `progreso` recibe `(bytes_descargados, bytes_totales)` cada pocos megabytes y
/// una vez más al terminar. El total sale del catálogo, no del `Content-Length`
/// que manda el servidor: si el archivo remoto cambió de tamaño, la descarga
/// tiene que fallar, no reajustar la barra.
///
/// Si el modelo ya está instalado no baja nada y devuelve su ruta.
///
/// # Reanudable
///
/// Un `.part` de una descarga cortada se retoma con `Range`. Si el servidor
/// ignora el pedido y contesta `200`, se empieza de cero: mezclar los bytes de
/// dos respuestas distintas daría un archivo que no verifica y una hora perdida.
pub fn descargar<F>(modelo: Modelo, mut progreso: F) -> Result<PathBuf, String>
where
    F: FnMut(u64, u64),
{
    let total = modelo.bytes();
    let destino = ruta(modelo)?;
    if esta_descargado(modelo) {
        progreso(total, total);
        return Ok(destino);
    }

    let url = modelo.url();
    // La lista blanca se aplica acá, en la única puerta pública del módulo.
    validar_url(&url)?;
    let carpeta = directorio()?;
    fs::create_dir_all(&carpeta)
        .map_err(|e| format!("no pude crear {}: {e}", carpeta.display()))?;
    let parcial = carpeta.join(format!("{}.part", modelo.nombre_archivo()));

    descargar_verificado(
        &url,
        &parcial,
        &destino,
        total,
        modelo.sha256(),
        progreso,
    )
}

/// El mecanismo de la descarga, sin el catálogo.
///
/// Separado de [`descargar`] para poder ejercitar contra un servidor local la
/// parte que de verdad tiene aristas: la negociación del `Range`, el reinicio
/// cuando el servidor la ignora, y el rechazo de un cuerpo que no verifica.
/// Es privada a propósito — la lista blanca de origen vive en [`descargar`],
/// que es la única forma pública de llegar hasta acá.
fn descargar_verificado<F>(
    url: &str,
    parcial: &Path,
    destino: &Path,
    total: u64,
    sha256_esperado: &str,
    mut progreso: F,
) -> Result<PathBuf, String>
where
    F: FnMut(u64, u64),
{
    let Continuacion {
        mut escritos,
        archivo,
        mut respuesta,
    } = abrir_para_continuar(parcial, total, url)?;
    progreso(escritos, total);

    let mut destino_parcial = BufWriter::with_capacity(TAMANO_BLOQUE, archivo);
    let mut bloque = vec![0u8; TAMANO_BLOQUE];
    let mut ultimo_aviso = escritos;
    loop {
        let leidos = respuesta
            .read(&mut bloque)
            .map_err(|e| format!("se cortó la descarga de {url}: {e}"))?;
        if leidos == 0 {
            break;
        }
        destino_parcial
            .write_all(&bloque[..leidos])
            .map_err(|e| format!("no pude escribir en {}: {e}", parcial.display()))?;
        escritos += leidos as u64;
        if escritos - ultimo_aviso >= PASO_PROGRESO {
            ultimo_aviso = escritos;
            progreso(escritos.min(total), total);
        }
    }
    destino_parcial
        .flush()
        .map_err(|e| format!("no pude terminar de escribir {}: {e}", parcial.display()))?;
    drop(destino_parcial);
    progreso(escritos.min(total), total);

    // Se compara contra el disco y no contra el contador: si el archivo quedó
    // más corto de lo que se creyó escribir, el hash lo detectaría igual, pero
    // este mensaje explica qué pasó y el `.part` sobrevive para reanudarse.
    let en_disco = fs::metadata(parcial)
        .map_err(|e| format!("no pude medir {}: {e}", parcial.display()))?
        .len();
    if en_disco != total {
        return Err(format!(
            "descarga incompleta: {en_disco} de {total} bytes. Volvé a intentar y se retoma."
        ));
    }

    verificar_y_promover(parcial, destino, sha256_esperado)?;
    Ok(destino.to_path_buf())
}

/// Dónde seguir escribiendo y de dónde seguir leyendo.
struct Continuacion {
    /// Bytes que ya están en el `.part` y que la respuesta NO va a repetir.
    escritos: u64,
    /// El `.part` abierto en el modo que corresponde (agregar o truncar).
    archivo: File,
    respuesta: reqwest::blocking::Response,
}

/// Abre el `.part` en el punto donde corresponda seguir y pide ese tramo.
///
/// Es una función aparte porque la negociación del `Range` tiene cuatro
/// desenlaces y mezclarla con el bucle de copia haría ilegibles a los dos.
fn abrir_para_continuar(
    parcial: &Path,
    total: u64,
    url: &str,
) -> Result<Continuacion, String> {
    // Un `.part` igual o más grande que el total es basura de otra versión del
    // archivo: retomarlo produciría un pegote que nunca va a verificar.
    let en_disco = fs::metadata(parcial).map(|m| m.len()).unwrap_or(0);
    let desde = if en_disco >= total { 0 } else { en_disco };

    let cliente = cliente()?;
    let mut pedido = cliente.get(url);
    if desde > 0 {
        pedido = pedido.header(reqwest::header::RANGE, format!("bytes={desde}-"));
    }
    let respuesta = pedido
        .send()
        .map_err(|e| format!("no pude iniciar la descarga de {url}: {e}"))?;

    let estado = respuesta.status();
    match estado {
        // El servidor aceptó retomar: se agrega al final de lo que ya hay.
        reqwest::StatusCode::PARTIAL_CONTENT if desde > 0 => {
            let archivo = OpenOptions::new()
                .append(true)
                .open(parcial)
                .map_err(|e| format!("no pude reabrir {}: {e}", parcial.display()))?;
            Ok(Continuacion {
                escritos: desde,
                archivo,
                respuesta,
            })
        }
        // Manda el archivo entero (haya pedido `Range` o no): se empieza de
        // cero. `File::create` trunca, que es exactamente lo que hace falta.
        reqwest::StatusCode::OK => {
            let archivo = File::create(parcial)
                .map_err(|e| format!("no pude crear {}: {e}", parcial.display()))?;
            Ok(Continuacion {
                escritos: 0,
                archivo,
                respuesta,
            })
        }
        // El `.part` quedó fuera de rango: se tira y el próximo intento arranca
        // limpio. No se reintenta acá adentro para no esconder el problema.
        reqwest::StatusCode::RANGE_NOT_SATISFIABLE => {
            let _ = fs::remove_file(parcial);
            Err(format!(
                "el servidor rechazó retomar la descarga de {url}; se descartó lo parcial, \
                 volvé a intentar"
            ))
        }
        otro => Err(format!("{url} respondió {otro}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vectores públicos del estándar: si `sha256_de_archivo` no los reproduce,
    /// no es SHA-256 y todo lo demás de este módulo es decorativo.
    const SHA_VACIO: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn carpeta_temporal(nombre: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mithflow_models_{}_{nombre}",
            std::process::id()
        ));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).expect("no pude crear la carpeta temporal del test");
        dir
    }

    #[test]
    fn el_hash_reproduce_los_vectores_del_estandar() {
        let dir = carpeta_temporal("vectores");
        let vacio = dir.join("vacio.bin");
        fs::write(&vacio, b"").unwrap();
        assert_eq!(sha256_de_archivo(&vacio).unwrap(), SHA_VACIO);

        let abc = dir.join("abc.bin");
        fs::write(&abc, b"abc").unwrap();
        assert_eq!(sha256_de_archivo(&abc).unwrap(), SHA_ABC);
    }

    /// El hash se calcula por bloques de 256 KiB: un archivo de varios bloques
    /// prueba que el acumulador se alimenta bien y no sólo el primer `read`.
    #[test]
    fn el_hash_no_depende_del_tamano_del_bloque() {
        let dir = carpeta_temporal("bloques");
        let grande = dir.join("grande.bin");
        let contenido = vec![0xABu8; TAMANO_BLOQUE * 3 + 17];
        fs::write(&grande, &contenido).unwrap();

        let mut esperado = Sha256::new();
        esperado.update(&contenido);
        let esperado_hex: String = esperado.finalize().iter().map(|b| format!("{b:02x}")).collect();

        assert_eq!(sha256_de_archivo(&grande).unwrap(), esperado_hex);
    }

    /// El requisito central del módulo: lo que no verifica no se instala, y
    /// además no queda tirado invitando a un reintento a darlo por bueno.
    #[test]
    fn un_archivo_con_hash_incorrecto_se_rechaza_y_se_borra() {
        let dir = carpeta_temporal("hash_malo");
        let parcial = dir.join("modelo.gguf.part");
        let destino = dir.join("modelo.gguf");
        fs::write(&parcial, b"esto no es el modelo").unwrap();

        let error = verificar_y_promover(&parcial, &destino, SHA_ABC)
            .expect_err("un hash que no coincide TIENE que fallar");

        assert!(
            !parcial.exists(),
            "el .part que no verifica tiene que borrarse, no sobrevivir"
        );
        assert!(
            !destino.exists(),
            "nunca puede existir el archivo final sin haber verificado"
        );
        assert!(
            error.contains(SHA_ABC),
            "el error debe decir qué esperaba: {error}"
        );
    }

    #[test]
    fn un_archivo_con_el_hash_correcto_se_promueve() {
        let dir = carpeta_temporal("hash_bueno");
        let parcial = dir.join("modelo.gguf.part");
        let destino = dir.join("modelo.gguf");
        fs::write(&parcial, b"abc").unwrap();

        verificar_y_promover(&parcial, &destino, SHA_ABC).expect("el hash coincide");

        assert!(!parcial.exists(), "el .part se renombra, no se copia");
        assert_eq!(fs::read(&destino).unwrap(), b"abc");
    }

    /// El hash se compara sin distinguir mayúsculas porque `Get-FileHash` lo
    /// imprime en mayúsculas y el catálogo lo guarda en minúsculas.
    #[test]
    fn la_comparacion_de_hash_ignora_mayusculas() {
        let dir = carpeta_temporal("mayusculas");
        let parcial = dir.join("modelo.gguf.part");
        let destino = dir.join("modelo.gguf");
        fs::write(&parcial, b"abc").unwrap();

        verificar_y_promover(&parcial, &destino, &SHA_ABC.to_uppercase())
            .expect("el mismo hash en mayúsculas es el mismo hash");
        assert!(destino.exists());
    }

    /// Un catálogo con un hash de 63 caracteres o una URL a otro host es un
    /// error que no se ve hasta que alguien intenta bajar 1,5 GB.
    #[test]
    fn el_catalogo_es_coherente() {
        for modelo in Modelo::TODOS {
            let url = modelo.url();
            assert!(
                validar_url(&url).is_ok(),
                "{modelo} apunta fuera del origen permitido: {url}"
            );
            assert!(url.starts_with("https://"), "{modelo} no usa https: {url}");
            assert!(
                url.ends_with(modelo.nombre_archivo()),
                "la URL de {modelo} no termina en su nombre de archivo"
            );

            let sha = modelo.sha256();
            assert_eq!(sha.len(), 64, "el SHA-256 de {modelo} no mide 64 dígitos");
            assert!(
                sha.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "el SHA-256 de {modelo} no es hexadecimal en minúsculas"
            );
            assert!(modelo.bytes() > 0, "{modelo} declara tamaño cero");
            assert!(modelo.megabytes() > 0);
        }
    }

    #[test]
    fn cada_modelo_tiene_nombre_hash_y_tamano_propios() {
        for (i, a) in Modelo::TODOS.iter().enumerate() {
            for b in &Modelo::TODOS[i + 1..] {
                assert_ne!(a.nombre_archivo(), b.nombre_archivo());
                assert_ne!(a.sha256(), b.sha256());
                assert_ne!(a.bytes(), b.bytes());
            }
        }
    }

    #[test]
    fn el_modelo_de_perfilado_es_el_mas_liviano() {
        let mas_liviano = Modelo::TODOS
            .iter()
            .min_by_key(|m| m.bytes())
            .copied()
            .unwrap();
        assert_eq!(Modelo::DE_PERFILADO, mas_liviano);
    }

    #[test]
    fn rechaza_una_url_de_otro_host() {
        assert!(validar_url("https://ejemplo.invalido/modelo.gguf").is_err());
        assert!(validar_url("http://huggingface.co/modelo.gguf").is_err());
        assert!(validar_url("https://huggingface.co.malo.invalido/x").is_err());
    }

    #[test]
    fn el_directorio_cuelga_de_mithflow_models() {
        let dir = directorio().expect("en Windows %APPDATA% siempre resuelve");
        assert!(dir.is_absolute(), "{} no es absoluta", dir.display());
        assert!(dir.ends_with("MithFlow/models") || dir.ends_with("MithFlow\\models"));

        let ruta_f16 = ruta(Modelo::F16).unwrap();
        assert_eq!(
            ruta_f16.file_name().unwrap(),
            Modelo::F16.nombre_archivo()
        );
        assert_eq!(ruta_f16.parent().unwrap(), dir);
    }

    /// Servidor HTTP mínimo de un solo pedido, sobre `std::net`.
    ///
    /// Sin dependencia nueva a propósito: lo que estos tests necesitan es
    /// controlar exactamente el estado y el cuerpo de la respuesta —incluido el
    /// caso "el servidor ignora el `Range`"—, y eso ningún servidor de
    /// juguete de terceros lo hace mejor que treinta líneas acá.
    ///
    /// Devuelve la URL y el hilo, que al unirse entrega **el byte desde el que
    /// el cliente pidió retomar**, o `None` si no mandó `Range`. Es un `u64` y
    /// no la cabecera cruda porque el nombre de la cabecera lo normaliza el
    /// cliente (reqwest la manda en minúsculas, que es HTTP válido) y lo que el
    /// test tiene que afirmar es el desplazamiento, no las mayúsculas.
    fn servidor_de_prueba(
        cuerpo: Vec<u8>,
        honrar_range: bool,
    ) -> (String, std::thread::JoinHandle<Option<u64>>) {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let escucha = TcpListener::bind("127.0.0.1:0").expect("no pude abrir un puerto local");
        let puerto = escucha.local_addr().unwrap().port();

        let hilo = std::thread::spawn(move || {
            let (mut socket, _) = escucha.accept().expect("nadie se conectó");
            let mut lector = BufReader::new(socket.try_clone().unwrap());

            let mut pedido_desde: Option<u64> = None;
            loop {
                let mut linea = String::new();
                if lector.read_line(&mut linea).unwrap_or(0) == 0 || linea == "\r\n" {
                    break;
                }
                if linea.to_ascii_lowercase().starts_with("range:") {
                    pedido_desde = linea
                        .split("bytes=")
                        .nth(1)
                        .and_then(|r| r.trim().trim_end_matches('-').parse().ok());
                }
            }

            let desde = pedido_desde.unwrap_or(0) as usize;
            let respuesta = if honrar_range && desde > 0 && desde < cuerpo.len() {
                let resto = &cuerpo[desde..];
                let mut bytes = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\n\
                     Content-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                    resto.len(),
                    desde,
                    cuerpo.len() - 1,
                    cuerpo.len()
                )
                .into_bytes();
                bytes.extend_from_slice(resto);
                bytes
            } else {
                // También es la rama del servidor que IGNORA el `Range`.
                let mut bytes = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    cuerpo.len()
                )
                .into_bytes();
                bytes.extend_from_slice(&cuerpo);
                bytes
            };

            socket.write_all(&respuesta).ok();
            socket.flush().ok();
            pedido_desde
        });

        (format!("http://127.0.0.1:{puerto}/modelo.gguf"), hilo)
    }

    /// Cuerpo reproducible y más grande que un bloque de lectura, para que el
    /// bucle de copia dé varias vueltas.
    fn cuerpo_de_prueba() -> Vec<u8> {
        (0..(TAMANO_BLOQUE * 2 + 1234))
            .map(|i| (i % 251) as u8)
            .collect()
    }

    fn sha256_de(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn una_descarga_limpia_verifica_y_deja_el_archivo_final() {
        let dir = carpeta_temporal("descarga_limpia");
        let cuerpo = cuerpo_de_prueba();
        let (url, hilo) = servidor_de_prueba(cuerpo.clone(), true);
        let parcial = dir.join("modelo.gguf.part");
        let destino = dir.join("modelo.gguf");

        let mut avisos = Vec::new();
        let devuelto = descargar_verificado(
            &url,
            &parcial,
            &destino,
            cuerpo.len() as u64,
            &sha256_de(&cuerpo),
            |hechos, total| avisos.push((hechos, total)),
        )
        .expect("la descarga tenía que verificar");

        assert_eq!(devuelto, destino);
        assert_eq!(fs::read(&destino).unwrap(), cuerpo);
        assert!(!parcial.exists(), "el .part tiene que desaparecer");
        assert_eq!(hilo.join().unwrap(), None, "sin .part no se pide Range");
        assert_eq!(
            avisos.last().copied(),
            Some((cuerpo.len() as u64, cuerpo.len() as u64)),
            "el último aviso de progreso tiene que ser el 100%"
        );
    }

    /// La reanudación: con medio archivo en el `.part`, el cliente pide el
    /// resto y el resultado tiene que verificar igual.
    #[test]
    fn una_descarga_cortada_se_retoma_desde_donde_quedo() {
        let dir = carpeta_temporal("reanuda");
        let cuerpo = cuerpo_de_prueba();
        let corte = cuerpo.len() / 3;
        let parcial = dir.join("modelo.gguf.part");
        let destino = dir.join("modelo.gguf");
        fs::write(&parcial, &cuerpo[..corte]).unwrap();

        let (url, hilo) = servidor_de_prueba(cuerpo.clone(), true);
        descargar_verificado(
            &url,
            &parcial,
            &destino,
            cuerpo.len() as u64,
            &sha256_de(&cuerpo),
            |_, _| {},
        )
        .expect("retomar tenía que funcionar");

        assert_eq!(
            fs::read(&destino).unwrap(),
            cuerpo,
            "el archivo retomado no reconstruye el original"
        );
        assert_eq!(
            hilo.join().unwrap(),
            Some(corte as u64),
            "no se pidió el tramo que faltaba"
        );
    }

    /// Un servidor que ignora el `Range` y manda todo de nuevo NO puede
    /// terminar en un archivo con los primeros bytes duplicados.
    #[test]
    fn si_el_servidor_ignora_el_range_se_empieza_de_cero() {
        let dir = carpeta_temporal("ignora_range");
        let cuerpo = cuerpo_de_prueba();
        let parcial = dir.join("modelo.gguf.part");
        let destino = dir.join("modelo.gguf");
        fs::write(&parcial, &cuerpo[..cuerpo.len() / 2]).unwrap();

        let (url, hilo) = servidor_de_prueba(cuerpo.clone(), false);
        descargar_verificado(
            &url,
            &parcial,
            &destino,
            cuerpo.len() as u64,
            &sha256_de(&cuerpo),
            |_, _| {},
        )
        .expect("tenía que rehacer la descarga entera");

        assert_eq!(
            fs::read(&destino).unwrap(),
            cuerpo,
            "los bytes de las dos respuestas se concatenaron"
        );
        assert!(hilo.join().unwrap().is_some(), "se pidió Range igual");
    }

    /// El extremo del módulo: un cuerpo que no es el modelo esperado no se
    /// instala, aunque mida exactamente lo que debía medir.
    #[test]
    fn un_cuerpo_que_no_verifica_no_se_instala() {
        let dir = carpeta_temporal("cuerpo_malo");
        let esperado = cuerpo_de_prueba();
        let mut adulterado = esperado.clone();
        // Un solo byte distinto, y el mismo tamaño: el centinela de tamaño no
        // lo detecta, el hash sí. Es exactamente el ataque que importa.
        adulterado[42] ^= 0xFF;

        let (url, hilo) = servidor_de_prueba(adulterado, true);
        let parcial = dir.join("modelo.gguf.part");
        let destino = dir.join("modelo.gguf");

        let error = descargar_verificado(
            &url,
            &parcial,
            &destino,
            esperado.len() as u64,
            &sha256_de(&esperado),
            |_, _| {},
        )
        .expect_err("un cuerpo adulterado TIENE que fallar");

        assert!(!destino.exists(), "se instaló un archivo que no verifica");
        assert!(!parcial.exists(), "el .part adulterado tiene que borrarse");
        assert!(error.contains("hash"), "el error no explica la causa: {error}");
        hilo.join().unwrap();
    }

    /// Los hashes compilados son la raíz de confianza de la descarga: este test
    /// los contrasta contra los archivos reales del repositorio.
    ///
    /// Ignorado porque lee 2,7 GB. Correr con:
    ///   cargo test -p mithflow-core models -- --ignored --nocapture
    #[test]
    #[ignore = "lee los 2,7 GB de modelos de app-nativa/models/"]
    fn los_hashes_compilados_son_los_de_los_archivos_reales() {
        let carpeta = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models");
        for modelo in Modelo::TODOS {
            let path = carpeta.join(modelo.nombre_archivo());
            let metadatos = fs::metadata(&path)
                .unwrap_or_else(|e| panic!("falta {}: {e}", path.display()));
            assert_eq!(
                metadatos.len(),
                modelo.bytes(),
                "el tamaño de {modelo} no coincide"
            );
            let real = sha256_de_archivo(&path).unwrap();
            eprintln!("{modelo}: {} bytes, sha256 {real}", metadatos.len());
            assert_eq!(real, modelo.sha256(), "el hash de {modelo} no coincide");
        }
    }
}
