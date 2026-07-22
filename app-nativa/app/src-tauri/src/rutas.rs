//! Dónde vive cada cosa en el disco.
//!
//! El historial **no** es el de la versión Python. Mientras las dos convivan
//! escriben archivos distintos: `history.jsonl` es de `mithflow.py` y lo lee su
//! dashboard; esta app usa `history-nativo.jsonl` en su propio directorio de
//! datos. Mezclarlos haría que un bug de la versión nueva corrompa el registro
//! de la que el usuario usa todos los días.

use crate::ajustes::{modelo_de_clave, Ajustes};
use crate::estado::FalloDelMotor;
use mithflow_core::hardware;
use mithflow_core::models::{self, Modelo};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

/// Deliberadamente distinto de `history.jsonl`.
pub const ARCHIVO_HISTORIAL: &str = "history-nativo.jsonl";

/// Variable de entorno para apuntar a un `.gguf` concreto.
///
/// Existe para desarrollo (los modelos del spike viven en `app-nativa/models/`,
/// no en `%APPDATA%`) y para una instalación portable. Se valida que sea un
/// archivo existente antes de usarla.
pub const VAR_MODELO: &str = "MITHFLOW_MODELO";

/// `%APPDATA%\com.mithdata.mithflow\`, creado si no existe.
pub fn dir_datos<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no pude resolver el directorio de datos: {e}"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("no pude crear {}: {e}", dir.display()))?;
    Ok(dir)
}

pub fn historial<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(dir_datos(app)?.join(ARCHIVO_HISTORIAL))
}

/// El `.gguf` que hay que cargar, en orden de preferencia:
///
/// 1. lo que diga [`VAR_MODELO`], si apunta a un archivo que existe;
/// 2. el modelo elegido en Ajustes, si está descargado;
/// 3. el más grande de los descargados **que entre en esta máquina**.
///
/// El paso 3 no es un capricho: si el usuario eligió el `F16` pero todavía sólo
/// bajó el `Q4_K_M`, dictar con el chico es mucho mejor que no dictar. Devuelve
/// también si hubo que sustituir, para poder avisarlo.
///
/// # Por qué el paso 3 mide la memoria
///
/// Es la rama por la que entra `"auto"`, que significa "el que diga el perfilado
/// de hardware" — y elegir el más grande a secas es justo lo que el perfilado NO
/// hace. En una notebook de 8 GB con gráficos integrados, un `F16` que llegó al
/// disco por cualquier vía (lo bajó el usuario desde Ajustes, lo copió a mano de
/// otra máquina) se cargaría igual, aunque `hardware::RAM_MINIMA_F16_GB` diga
/// que no entra: 1,5 GB de pesos que no tienen dónde vivir. El criterio es el
/// mismo de [`hardware::entra_en_memoria`], no una copia de los umbrales.
///
/// **El paso 2 no se toca**: si el usuario eligió un modelo a mano y lo tiene
/// descargado, se carga ése. Elegir por él sería otro defecto — la interfaz le
/// avisa cuánta RAM pide cada uno y la decisión es suya.
///
/// El error es un [`FalloDelMotor`] y no un `String` porque acá se decide algo
/// que después se ve: **no haber descargado nada todavía no es una falla**
/// (`SinModelo`), pero no poder resolver el directorio de modelos o tener la
/// variable de entorno apuntando a cualquier lado sí lo es (`Roto`).
pub fn modelo(ajustes: &Ajustes) -> Result<(PathBuf, Option<String>), FalloDelMotor> {
    if let Some(desde_entorno) = desde_variable(std::env::var_os(VAR_MODELO).map(PathBuf::from)) {
        return desde_entorno;
    }

    let elegido = modelo_de_clave(&ajustes.modelo);
    if let Some(m) = elegido {
        if models::esta_descargado(m) {
            return Ok((models::ruta(m).map_err(FalloDelMotor::Roto)?, None));
        }
    }

    let descargados: Vec<Modelo> = Modelo::TODOS
        .into_iter()
        .filter(|m| models::esta_descargado(*m))
        .collect();
    let ram_total_gb = hardware::ram_total_gb();
    let gpu_dedicada = hay_gpu_dedicada_si_hace_falta(&descargados, ram_total_gb);
    let entra = |m: Modelo| hardware::entra_en_memoria(m, ram_total_gb, gpu_dedicada);

    let Some(disponible) = sustituto(&descargados, entra) else {
        // El primer arranque entra por acá: no hay nada descargado porque la
        // app se acaba de instalar. Se cuenta como lo que es.
        return Err(FalloDelMotor::SinModelo(
            "todavía no hay ningún modelo descargado. Bajá uno desde Ajustes para poder dictar."
                .to_string(),
        ));
    };
    Ok((
        models::ruta(disponible).map_err(FalloDelMotor::Roto)?,
        aviso_de_sustitucion(elegido, disponible, entra(disponible), ram_total_gb),
    ))
}

/// El modelo con el que se va a dictar cuando el elegido no está descargado.
///
/// **El más grande que entre**, y si ninguno entra, el más chico que haya: es el
/// que menos aprieta, y no dictar no es una opción mejor que dictar despacio.
/// Recibe el criterio de memoria como función para poder probar cada máquina sin
/// depender de la que corre los tests.
///
/// `descargados` viene ordenado de mayor a menor (es el orden de
/// [`Modelo::TODOS`]), así que el primero que entra es el más grande que entra.
fn sustituto(descargados: &[Modelo], entra: impl Fn(Modelo) -> bool) -> Option<Modelo> {
    descargados
        .iter()
        .copied()
        .find(|m| entra(*m))
        .or_else(|| descargados.last().copied())
}

/// ¿Hay una placa con memoria propia? Se pregunta **sólo si puede cambiar la
/// respuesta**.
///
/// `hardware::describir_gpu` enumera los adaptadores de Vulkan y DX12, que son
/// cientos de milisegundos en el camino de arranque de la aplicación. Cuando
/// todos los modelos descargados entran por RAM del sistema, tener o no tener
/// placa dedicada no cambia cuál se elige: no se pregunta.
fn hay_gpu_dedicada_si_hace_falta(descargados: &[Modelo], ram_total_gb: f32) -> bool {
    let entran_todos = descargados
        .iter()
        .all(|m| hardware::entra_en_memoria(*m, ram_total_gb, false));
    if entran_todos {
        return false;
    }
    hardware::describir_gpu().is_some_and(|g| g.dedicada)
}

/// Lo que hay que contarle al usuario sobre el modelo con el que se va a dictar.
///
/// Dos cosas distintas, que pueden pasar juntas:
///
/// 1. **sustitución**: eligió uno que todavía no bajó (con `"auto"` no hay nada
///    que avisar: sustituir ES lo que pidió);
/// 2. **memoria**: ni el más chico de los descargados entra en esta máquina. Se
///    carga igual —es eso o no dictar— pero callarlo dejaría al usuario sin
///    explicación si el motor tarda una eternidad o no llega a cargar.
fn aviso_de_sustitucion(
    elegido: Option<Modelo>,
    disponible: Modelo,
    entra: bool,
    ram_total_gb: f32,
) -> Option<String> {
    let mut partes = Vec::new();
    if let Some(m) = elegido {
        partes.push(format!(
            "{m} todavía no está descargado; uso {disponible} mientras tanto."
        ));
    }
    if !entra {
        partes.push(format!(
            "Ojo: {disponible} pide unos {:.0} GB de RAM y esta máquina tiene {ram_total_gb:.1}. \
             Lo cargo igual, pero puede ir lento o no llegar a cargar; bajá uno más chico desde \
             Ajustes.",
            hardware::ram_minima_gb(disponible)
        ));
    }
    if partes.is_empty() {
        None
    } else {
        Some(partes.join(" "))
    }
}

/// La rama de [`VAR_MODELO`], separada de la lectura del entorno para poder
/// probarla sin tocar variables globales del proceso.
///
/// `None` significa "la variable no está, seguí con el catálogo". Si está pero
/// apunta a cualquier cosa, es un ERROR y no un aviso: quien la fijó lo hizo a
/// propósito, y caerse en silencio a otro modelo escondería el problema.
///
/// Es `Roto` y no `SinModelo` aunque el archivo tampoco esté: la diferencia no
/// es si hay un `.gguf` en esa ruta sino qué tiene que hacer el usuario.
/// Descargar un modelo no arregla una variable de entorno mal escrita, así que
/// mandarlo a Ajustes a bajar 1,5 GB sería peor que decirle la verdad.
fn desde_variable(
    valor: Option<PathBuf>,
) -> Option<Result<(PathBuf, Option<String>), FalloDelMotor>> {
    let ruta = valor?;
    if ruta.is_file() {
        let aviso = format!("modelo tomado de {VAR_MODELO}: {}", ruta.display());
        return Some(Ok((ruta, Some(aviso))));
    }
    Some(Err(FalloDelMotor::Roto(format!(
        "{VAR_MODELO} apunta a {}, que no es un archivo",
        ruta.display()
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La regla que sostiene la convivencia con la versión Python.
    #[test]
    fn el_historial_no_es_el_de_python() {
        assert_ne!(ARCHIVO_HISTORIAL, "history.jsonl");
        assert_eq!(ARCHIVO_HISTORIAL, "history-nativo.jsonl");
    }

    /// Una variable mal puesta tiene que dar un error claro, no caerse a otro
    /// modelo por su cuenta: si alguien la fijó, es a propósito. Y es una falla
    /// de verdad, no "todavía no descargaste": bajar un modelo no la arregla.
    #[test]
    fn la_variable_apuntando_a_la_nada_es_un_error() {
        let salida = desde_variable(Some(PathBuf::from("D:\\no\\existe\\modelo.gguf")))
            .expect("con la variable puesta, la rama decide");
        let error = salida.expect_err("una ruta inexistente no puede pasar");
        assert!(
            error.motivo().contains("no es un archivo"),
            "mensaje: {}",
            error.motivo()
        );
        assert!(
            error.estado().es_falla(),
            "una variable mal escrita es una falla, no un modelo por descargar"
        );
    }

    #[test]
    fn sin_variable_la_decision_sigue_por_el_catalogo() {
        assert!(desde_variable(None).is_none());
    }

    /// Una máquina cualquiera, expresada como el criterio de memoria que aplica
    /// la sustitución. Es exactamente lo que usa `modelo()`, sin el disco.
    fn maquina(ram_total_gb: f32, gpu_dedicada: bool) -> impl Fn(Modelo) -> bool {
        move |m| hardware::entra_en_memoria(m, ram_total_gb, gpu_dedicada)
    }

    /// El escritorio: 64 GB y una RTX 3080. Con `"auto"` y los tres modelos en
    /// el disco se dicta con el más pesado, que es lo que ya hacía.
    #[test]
    fn en_una_maquina_grande_se_sustituye_por_el_mas_pesado() {
        let descargados = Modelo::TODOS;
        assert_eq!(
            sustituto(&descargados, maquina(64.0, true)),
            Some(Modelo::F16)
        );
        // Y con placa dedicada la RAM del sistema no acota: los pesos viven en
        // la VRAM. Es el caso de la MSI Katana (RTX 3050 Ti, 32 GB).
        assert_eq!(
            sustituto(&descargados, maquina(32.0, true)),
            Some(Modelo::F16)
        );
    }

    /// El defecto que cierra este arreglo: la notebook de 8 GB con gráficos
    /// integrados. Con el `F16` en el disco —bajado a mano o copiado de otra
    /// máquina— la sustitución elegía 1,5 GB de pesos que no tienen dónde vivir.
    /// Ahora salta al más grande **que entre**.
    #[test]
    fn en_una_maquina_chica_se_salta_al_que_entra() {
        let integrada = maquina(8.0, false);
        assert_eq!(
            sustituto(&Modelo::TODOS, &integrada),
            Some(Modelo::Q5KM),
            "con 8 GB y sin placa dedicada el F16 no entra"
        );
        assert_eq!(
            sustituto(&[Modelo::F16, Modelo::Q4KM], &integrada),
            Some(Modelo::Q4KM),
            "si el intermedio no está, se sigue bajando hasta el que entre"
        );
        // Y no se baja de más: lo que entra se usa.
        assert_eq!(
            sustituto(&[Modelo::Q5KM, Modelo::Q4KM], &integrada),
            Some(Modelo::Q5KM)
        );
    }

    /// Cuando NINGUNO de los descargados entra no se rompe nada ni se deja al
    /// usuario sin dictar: se usa el más chico que haya —el que menos aprieta—
    /// y el aviso dice por qué puede ir mal.
    #[test]
    fn si_no_entra_ninguno_se_usa_el_mas_chico_y_se_avisa() {
        let apretada = maquina(2.0, false);
        // Con 2 GB ni el intermedio entra; queda el más chico, que entra
        // siempre por catálogo.
        assert_eq!(
            sustituto(&[Modelo::F16, Modelo::Q5KM, Modelo::Q4KM], &apretada),
            Some(Modelo::Q4KM)
        );
        // Y el caso duro: lo único descargado es el que no entra.
        let solo_el_grande = [Modelo::F16, Modelo::Q5KM];
        let elegido = sustituto(&solo_el_grande, &apretada).expect("hay modelos descargados");
        assert_eq!(elegido, Modelo::Q5KM, "el más chico de los que hay");

        let aviso = aviso_de_sustitucion(None, elegido, apretada(elegido), 2.0)
            .expect("un modelo que no entra no puede cargarse en silencio");
        assert!(
            aviso.contains("RAM") && aviso.contains("Ajustes"),
            "el aviso tiene que decir qué pasa y qué hacer: {aviso}"
        );
    }

    /// Sin nada descargado no hay sustituto, que es lo que `modelo()` convierte
    /// en `SinModelo` — el primer arranque, no una falla.
    #[test]
    fn sin_nada_descargado_no_hay_sustituto() {
        assert_eq!(sustituto(&[], maquina(64.0, true)), None);
    }

    /// Con `"auto"` y un modelo que entra no se avisa nada: sustituir ES lo que
    /// el usuario pidió. Con un modelo elegido a mano que todavía no bajó, sí.
    #[test]
    fn el_aviso_solo_aparece_cuando_hay_algo_que_contar() {
        assert_eq!(
            aviso_de_sustitucion(None, Modelo::Q5KM, true, 8.0),
            None,
            "con «auto» y un modelo que entra no hay nada que avisar"
        );

        let aviso = aviso_de_sustitucion(Some(Modelo::F16), Modelo::Q4KM, true, 8.0)
            .expect("el modelo elegido no está: hay que decirlo");
        assert!(aviso.contains("todavía no está descargado"), "{aviso}");
        assert!(
            !aviso.contains("RAM"),
            "el que entra no puede llevar un aviso de memoria: {aviso}"
        );
    }

    /// Un archivo que sí existe se acepta y se avisa de dónde salió.
    #[test]
    fn la_variable_apuntando_a_un_archivo_real_manda() {
        let archivo = std::env::temp_dir().join(format!("mithflow_ruta_{}.gguf", std::process::id()));
        std::fs::write(&archivo, b"no es un modelo de verdad").expect("no pude crear el archivo");

        let (ruta, aviso) = desde_variable(Some(archivo.clone()))
            .expect("con la variable puesta, la rama decide")
            .expect("un archivo existente se acepta");
        assert_eq!(ruta, archivo);
        assert!(aviso.unwrap_or_default().contains(VAR_MODELO));

        std::fs::remove_file(&archivo).ok();
    }
}
