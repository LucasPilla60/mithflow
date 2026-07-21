//! Ajustes persistidos, sobre `tauri-plugin-store`.
//!
//! # Dos reglas
//!
//! 1. **Nada de lo que viene del archivo se cree sin revisar.** `ajustes.json`
//!    queda en el disco del usuario y se puede editar a mano; además el
//!    frontend puede mandar cualquier cosa por el comando. Todo pasa por
//!    [`Ajustes::normalizar`] antes de aplicarse, que recorta, valida contra
//!    listas conocidas y cae al default cuando algo no cierra. **Falla
//!    cerrado**: ante un valor imposible se usa el default, nunca se propaga.
//! 2. **Un campo faltante no es un error.** Cada campo tiene `#[serde(default)]`
//!    para que agregar un ajuste nuevo no invalide el archivo de la versión
//!    anterior, que es exactamente el problema que ya mordió en el historial
//!    (ver `history::Entry`).

use crate::atajo;
use mithflow_core::models::Modelo;
use mithflow_core::{Limpieza, ModoLimpieza, Preferencias};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

/// Nombre del archivo dentro del directorio de configuración de la app.
pub const ARCHIVO: &str = "ajustes.json";

/// Tope del vocabulario propio. Va al `initial_prompt` del modelo, que compite
/// por la ventana de contexto con lo que se está dictando: un texto enorme
/// degrada la transcripción en vez de mejorarla.
pub const MAX_VOCABULARIO: usize = 1_000;

/// Tope de muletillas. Es una lista curada, no un diccionario.
pub const MAX_MULETILLAS: usize = 100;

/// Cómo se limpia el texto antes de pegarlo.
pub const MODOS_LIMPIEZA: &[&str] = &["rapido", "ninguno"];

/// Tope de grabación por defecto, en segundos.
pub const LIMITE_GRABACION_POR_DEFECTO: u32 = mithflow_core::config::MAX_RECORDING_SECS as u32;

/// Piso del tope de grabación. Menos de esto convertiría cualquier dictado
/// normal en uno cortado por la mitad.
pub const LIMITE_GRABACION_MINIMO: u32 = 15;

/// Techo del tope de grabación. A 384 KB/s de buffer, diez minutos son 230 MB
/// de audio en memoria: es un tope de seguridad, no una preferencia.
pub const LIMITE_GRABACION_MAXIMO: u32 = 600;

fn modo_limpieza_por_defecto() -> String {
    MODOS_LIMPIEZA[0].to_string()
}

fn limite_grabacion_por_defecto() -> u32 {
    LIMITE_GRABACION_POR_DEFECTO
}

fn guardar_texto_por_defecto() -> bool {
    true
}

fn tecla_por_defecto() -> String {
    atajo::TECLA_POR_DEFECTO.to_string()
}

fn modelo_por_defecto() -> String {
    NOMBRE_AUTOMATICO.to_string()
}

fn sonidos_por_defecto() -> bool {
    true
}

fn volumen_por_defecto() -> f32 {
    crate::sonidos::VOLUMEN_POR_DEFECTO
}

fn muletillas_por_defecto() -> Vec<String> {
    mithflow_core::config::FILLERS
        .iter()
        .map(|m| m.to_string())
        .collect()
}

/// Valor de `modelo` que significa "el que diga el perfilado de hardware".
pub const NOMBRE_AUTOMATICO: &str = "auto";

/// Nombre estable de cada modelo para el archivo de ajustes. No se usa el
/// `Display` de `Modelo` ("large-v3-turbo F16") porque ése es texto para el
/// usuario y puede cambiar; esto es una clave persistida.
pub fn clave_modelo(modelo: Modelo) -> &'static str {
    match modelo {
        Modelo::F16 => "F16",
        Modelo::Q5KM => "Q5_K_M",
        Modelo::Q4KM => "Q4_K_M",
    }
}

/// El modelo que nombra esta clave, o `None` si es "auto" o basura.
pub fn modelo_de_clave(clave: &str) -> Option<Modelo> {
    Modelo::TODOS
        .into_iter()
        .find(|m| clave_modelo(*m).eq_ignore_ascii_case(clave))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Ajustes {
    /// Nombre de la tecla del atajo. F9 por defecto: la versión Python usa F8
    /// y las dos conviven durante la transición.
    #[serde(default = "tecla_por_defecto")]
    pub tecla: String,
    /// Clave de modelo, o `"auto"` para dejar decidir al perfilado.
    #[serde(default = "modelo_por_defecto")]
    pub modelo: String,
    #[serde(default = "sonidos_por_defecto")]
    pub sonidos: bool,
    #[serde(default = "volumen_por_defecto")]
    pub volumen: f32,
    #[serde(default)]
    pub arranque_con_windows: bool,
    /// Términos propios que el modelo tiende a errar ("MithData" → "Middata").
    #[serde(default)]
    pub vocabulario: String,
    #[serde(default = "muletillas_por_defecto")]
    pub muletillas: Vec<String>,
    #[serde(default = "modo_limpieza_por_defecto")]
    pub modo_limpieza: String,
    /// Segundos que puede durar una grabación antes de cortarse sola.
    #[serde(default = "limite_grabacion_por_defecto")]
    pub limite_grabacion_s: u32,
    /// Si el historial guarda el texto dictado además de las métricas.
    ///
    /// El default es `true` —es lo que hace la app desde siempre y lo que hace
    /// útil al historial—, pero se puede apagar: este archivo es lo único del
    /// programa que deja en claro, en el disco, todo lo que el usuario dijo.
    #[serde(default = "guardar_texto_por_defecto")]
    pub guardar_texto: bool,
}

impl Default for Ajustes {
    fn default() -> Self {
        Self {
            tecla: tecla_por_defecto(),
            modelo: modelo_por_defecto(),
            sonidos: sonidos_por_defecto(),
            volumen: volumen_por_defecto(),
            // Desactivado por defecto: que una aplicación se meta sola en el
            // arranque de Windows sin que se lo pidan es una falta de respeto.
            arranque_con_windows: false,
            vocabulario: String::new(),
            muletillas: muletillas_por_defecto(),
            modo_limpieza: modo_limpieza_por_defecto(),
            limite_grabacion_s: limite_grabacion_por_defecto(),
            guardar_texto: guardar_texto_por_defecto(),
        }
    }
}

impl Ajustes {
    /// Deja los ajustes en un estado que el resto del programa puede usar sin
    /// volver a chequear nada. Se llama al leer del disco Y al recibir del
    /// frontend: las dos son entradas que no controlamos.
    pub fn normalizar(&mut self) {
        if !atajo::nombre_valido(&self.tecla) {
            self.tecla = tecla_por_defecto();
        }
        if self.modelo != NOMBRE_AUTOMATICO && modelo_de_clave(&self.modelo).is_none() {
            self.modelo = modelo_por_defecto();
        }
        self.volumen = if self.volumen.is_finite() {
            self.volumen.clamp(0.0, 1.0)
        } else {
            volumen_por_defecto()
        };
        // `chars().take()` y no `truncate()`: cortar por bytes en el medio de un
        // carácter multibyte paniquea, y acá hay acentos garantizados.
        self.vocabulario = self.vocabulario.trim().chars().take(MAX_VOCABULARIO).collect();

        let mut vistas = Vec::with_capacity(self.muletillas.len());
        for muletilla in std::mem::take(&mut self.muletillas) {
            let limpia = muletilla.trim().to_lowercase();
            if !limpia.is_empty() && !vistas.contains(&limpia) {
                vistas.push(limpia);
            }
        }
        vistas.truncate(MAX_MULETILLAS);
        self.muletillas = vistas;

        if !MODOS_LIMPIEZA.contains(&self.modo_limpieza.as_str()) {
            self.modo_limpieza = modo_limpieza_por_defecto();
        }
        self.limite_grabacion_s = self
            .limite_grabacion_s
            .clamp(LIMITE_GRABACION_MINIMO, LIMITE_GRABACION_MAXIMO);
    }

    /// El modo de limpieza como lo entiende el núcleo.
    ///
    /// Las claves del archivo están en español (`"rapido"`) y las del historial
    /// en inglés (`"fast"`, que es lo que escribe `mithflow.py`): esta función
    /// es el único lugar donde los dos vocabularios se tocan.
    pub fn modo_limpieza(&self) -> ModoLimpieza {
        if self.modo_limpieza == "ninguno" {
            ModoLimpieza::Ninguno
        } else {
            ModoLimpieza::Rapido
        }
    }

    /// Lo que el pipeline de dictado necesita saber. Compila los patrones de
    /// las muletillas, así que se arma una vez por cambio de ajustes y no una
    /// vez por dictado.
    pub fn preferencias(&self) -> Preferencias {
        Preferencias {
            limpieza: Limpieza::nueva(self.modo_limpieza(), &self.muletillas),
            guardar_texto: self.guardar_texto,
        }
    }
}

/// Lee los ajustes del disco. **Nunca falla**: un archivo corrupto o ilegible
/// da los valores por defecto, porque quedarse sin arrancar por un JSON roto
/// sería peor que perder la configuración.
pub fn cargar<R: Runtime>(app: &AppHandle<R>) -> Ajustes {
    let mut ajustes = match app.store(ARCHIVO) {
        Ok(store) => {
            let mapa: Map<String, Value> = store.entries().into_iter().collect();
            serde_json::from_value(Value::Object(mapa)).unwrap_or_else(|e| {
                eprintln!("{ARCHIVO} no se pudo interpretar ({e}); uso los valores por defecto.");
                Ajustes::default()
            })
        }
        Err(e) => {
            eprintln!("no pude abrir {ARCHIVO} ({e}); uso los valores por defecto.");
            Ajustes::default()
        }
    };
    ajustes.normalizar();
    ajustes
}

/// Escribe los ajustes ya normalizados. Devuelve lo que efectivamente se
/// guardó, que puede diferir de lo que se pidió si algo se recortó.
pub fn guardar<R: Runtime>(app: &AppHandle<R>, mut ajustes: Ajustes) -> Result<Ajustes, String> {
    ajustes.normalizar();
    let store = app
        .store(ARCHIVO)
        .map_err(|e| format!("no pude abrir {ARCHIVO}: {e}"))?;

    let Value::Object(campos) = serde_json::to_value(&ajustes)
        .map_err(|e| format!("no pude serializar los ajustes: {e}"))?
    else {
        // `Ajustes` es una struct: `to_value` no puede dar otra cosa.
        return Err("los ajustes no serializaron como objeto".to_string());
    };
    for (clave, valor) in campos {
        store.set(clave, valor);
    }
    store
        .save()
        .map_err(|e| format!("no pude guardar {ARCHIVO}: {e}"))?;
    Ok(ajustes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_defaults_son_los_prometidos() {
        let a = Ajustes::default();
        assert_eq!(a.tecla, "F9", "F8 es de la version Python");
        assert!(!a.arranque_con_windows, "el autoarranque va apagado");
        assert!(a.sonidos);
        assert_eq!(a.volumen, 0.15);
        assert_eq!(a.modelo, "auto");
        assert_eq!(a.modo_limpieza, "rapido");
        assert_eq!(a.limite_grabacion_s, 180);
        assert!(a.guardar_texto, "el historial guarda el texto salvo que se apague");
    }

    /// El modo del archivo se traduce al del núcleo, y una basura cae en el
    /// comportamiento normal en vez de dejar al usuario sin limpieza.
    #[test]
    fn el_modo_de_limpieza_se_traduce_al_del_nucleo() {
        let rapido = Ajustes { modo_limpieza: "rapido".into(), ..Default::default() };
        assert_eq!(rapido.modo_limpieza(), ModoLimpieza::Rapido);

        let ninguno = Ajustes { modo_limpieza: "ninguno".into(), ..Default::default() };
        assert_eq!(ninguno.modo_limpieza(), ModoLimpieza::Ninguno);

        let raro = Ajustes { modo_limpieza: "vaya a saber".into(), ..Default::default() };
        assert_eq!(raro.modo_limpieza(), ModoLimpieza::Rapido);
    }

    /// Los ajustes tienen que CONVERTIRSE en preferencias que el núcleo aplica.
    /// Es el enganche que hacía falta para que vocabulario, muletillas y modo
    /// dejaran de ser decorativos.
    #[test]
    fn los_ajustes_se_convierten_en_preferencias_que_el_nucleo_aplica() {
        let a = Ajustes {
            muletillas: vec!["ponele".into()],
            modo_limpieza: "rapido".into(),
            guardar_texto: false,
            ..Default::default()
        };
        let p = a.preferencias();

        assert!(!p.guardar_texto);
        assert_eq!(
            p.limpieza.aplicar("El informe, ponele, ya está."),
            "El informe, ya está.",
            "la muletilla configurada no se aplicó"
        );

        let sin_limpieza = Ajustes { modo_limpieza: "ninguno".into(), ..Default::default() };
        assert_eq!(
            sin_limpieza.preferencias().limpieza.aplicar("Eh, el el informe."),
            "Eh, el el informe.",
            "el modo ninguno tiene que dejar el crudo"
        );
    }

    /// Un tope de grabación disparatado —o un archivo editado a mano— no puede
    /// dejar el micrófono abierto diez minutos ni cortar a los dos segundos.
    #[test]
    fn normalizar_acota_el_limite_de_grabacion() {
        let mut a = Ajustes { limite_grabacion_s: 0, ..Default::default() };
        a.normalizar();
        assert_eq!(a.limite_grabacion_s, LIMITE_GRABACION_MINIMO);

        let mut a = Ajustes { limite_grabacion_s: u32::MAX, ..Default::default() };
        a.normalizar();
        assert_eq!(a.limite_grabacion_s, LIMITE_GRABACION_MAXIMO);

        let mut a = Ajustes { limite_grabacion_s: 90, ..Default::default() };
        a.normalizar();
        assert_eq!(a.limite_grabacion_s, 90, "un valor razonable no se toca");
    }

    #[test]
    fn normalizar_rechaza_una_tecla_desconocida() {
        let mut a = Ajustes {
            tecla: "Ctrl+Alt+Supr".into(),
            ..Default::default()
        };
        a.normalizar();
        assert_eq!(a.tecla, "F9");
    }

    #[test]
    fn normalizar_recorta_el_volumen_y_atrapa_el_nan() {
        let mut a = Ajustes { volumen: 4.2, ..Default::default() };
        a.normalizar();
        assert_eq!(a.volumen, 1.0);

        let mut a = Ajustes { volumen: -1.0, ..Default::default() };
        a.normalizar();
        assert_eq!(a.volumen, 0.0);

        let mut a = Ajustes { volumen: f32::NAN, ..Default::default() };
        a.normalizar();
        assert_eq!(a.volumen, 0.15);
    }

    /// El vocabulario se corta por CARACTERES: cortar por bytes en el medio de
    /// una "ó" paniquea, y el vocabulario de esta app está en español.
    #[test]
    fn normalizar_recorta_el_vocabulario_sin_romper_los_acentos() {
        let mut a = Ajustes {
            vocabulario: "ó".repeat(MAX_VOCABULARIO + 500),
            ..Default::default()
        };
        a.normalizar();
        assert_eq!(a.vocabulario.chars().count(), MAX_VOCABULARIO);
    }

    #[test]
    fn normalizar_limpia_las_muletillas() {
        let mut a = Ajustes {
            muletillas: vec![
                "  Eh ".into(),
                "eh".into(),
                "EH".into(),
                "   ".into(),
                "o sea".into(),
            ],
            ..Default::default()
        };
        a.normalizar();
        assert_eq!(a.muletillas, vec!["eh", "o sea"]);
    }

    #[test]
    fn normalizar_rechaza_un_modo_de_limpieza_inventado() {
        let mut a = Ajustes { modo_limpieza: "llm".into(), ..Default::default() };
        a.normalizar();
        assert_eq!(a.modo_limpieza, "rapido");
    }

    #[test]
    fn normalizar_acepta_auto_y_las_claves_del_catalogo() {
        for clave in ["auto", "F16", "Q5_K_M", "q4_k_m"] {
            let mut a = Ajustes { modelo: clave.into(), ..Default::default() };
            a.normalizar();
            assert_eq!(a.modelo, clave, "{clave} es una clave válida");
        }
        let mut a = Ajustes { modelo: "gpt-5".into(), ..Default::default() };
        a.normalizar();
        assert_eq!(a.modelo, "auto");
    }

    #[test]
    fn las_claves_de_modelo_van_y_vuelven() {
        for modelo in Modelo::TODOS {
            assert_eq!(modelo_de_clave(clave_modelo(modelo)), Some(modelo));
        }
        assert_eq!(modelo_de_clave("auto"), None);
    }

    /// Un archivo de una versión anterior (sin los campos nuevos) tiene que
    /// leerse igual, completando con los defaults.
    #[test]
    fn un_archivo_incompleto_se_completa_con_defaults() {
        let json = serde_json::json!({ "tecla": "F10", "sonidos": false });
        let mut a: Ajustes = serde_json::from_value(json).expect("faltar campos no es un error");
        a.normalizar();
        assert_eq!(a.tecla, "F10");
        assert!(!a.sonidos);
        assert_eq!(a.volumen, 0.15, "el resto sale del default");
        assert_eq!(a.modo_limpieza, "rapido");
    }
}
