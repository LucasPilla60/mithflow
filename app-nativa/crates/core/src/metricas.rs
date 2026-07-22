//! Las cuentas del dashboard, hechas del lado del programa.
//!
//! # Por qué acá y no en la interfaz
//!
//! El dashboard de Streamlit levantaba el archivo entero, lo volvía un
//! `DataFrame` y sacaba los totales en el navegador. Con miles de dictados eso
//! es mandar todo el texto dictado del usuario a la vista para terminar
//! mostrando cinco números. Acá se recorre el historial una vez y **viajan sólo
//! los agregados**: menos trabajo, y el texto en claro no sale del proceso.
//!
//! # La latencia promedio, que el dashboard actual calcula mal
//!
//! `dashboard.py` promediaba `transcribe_s + cleanup_s` sobre TODO el historial,
//! mezclando la época en que la limpieza la hacía un LLM (2-5 s por dictado) con
//! la actual (menos de 1 ms). Mostraba 1,76 s cuando la latencia real era
//! 0,51 s: un 3,5x de error, y siempre en contra.
//!
//! La corrección es agrupar **por modo de limpieza** ([`ResumenModo`]), que es
//! justo el campo que distingue las dos épocas, y mostrar el del modo vigente.
//! Los otros no se tiran: se informan aparte, porque "antes tardaba 1,76 s y
//! ahora 0,51 s" es información útil, y esconderla sería el error opuesto.

use crate::history::Entry;
use chrono::{Duration, NaiveDate, NaiveDateTime, Timelike};
use serde::Serialize;

/// Cuántos días entran en el gráfico de palabras por día.
pub const DIAS_DEL_GRAFICO: usize = 30;

/// Velocidad de tipeo con la que se compara, en palabras por minuto. Es la
/// misma cifra que usa `dashboard.py`, para que los dos números sean
/// comparables mientras las dos versiones convivan.
pub const PALABRAS_POR_MINUTO_TIPEANDO: f32 = 40.0;

/// Formato de `Entry::ts`: hora local sin zona, el mismo que escribe
/// `time.strftime` en la versión Python.
const FORMATO_TS: &str = "%Y-%m-%dT%H:%M:%S";

/// Palabras dictadas en un día.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiaConPalabras {
    /// `YYYY-MM-DD`, para que la interfaz no tenga que interpretar zonas.
    pub dia: String,
    pub palabras: usize,
    pub dictados: usize,
}

/// Cuánto tardó, en promedio, cada época de la limpieza.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResumenModo {
    pub modo: String,
    pub dictados: usize,
    /// Promedio de `transcribe_s + cleanup_s` en segundos.
    pub latencia_s: f32,
}

/// Todo lo que el dashboard muestra, ya calculado.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Metricas {
    pub total_dictados: usize,
    pub total_palabras: usize,
    pub total_audio_s: f32,
    pub dictados_hoy: usize,
    pub palabras_hoy: usize,
    /// Palabras por minuto hablando. `0` sin audio registrado.
    pub palabras_por_minuto: f32,
    /// Minutos ahorrados contra tipear a [`PALABRAS_POR_MINUTO_TIPEANDO`].
    pub minutos_ahorrados: f32,
    /// Últimos [`DIAS_DEL_GRAFICO`] días, con los días sin dictados en cero: un
    /// gráfico que saltea los días vacíos miente sobre la regularidad de uso.
    pub por_dia: Vec<DiaConPalabras>,
    /// Palabras por hora del día, siempre 24 posiciones.
    pub por_hora: Vec<usize>,
    /// Un resumen por modo, del más usado al menos usado.
    pub por_modo: Vec<ResumenModo>,
    /// El modo del dictado más reciente: el que está vigente.
    pub modo_actual: Option<String>,
    /// Latencia promedio del modo vigente. `None` sin dictados.
    pub latencia_s: Option<f32>,
    /// Cuántos dictados quedaron FUERA de esa latencia por ser de otro modo.
    /// Se informa para que el número no parezca calculado sobre todo.
    pub dictados_de_otros_modos: usize,
}

/// Recorre el historial y saca todas las cuentas.
///
/// `hoy` se recibe en vez de leerse del reloj para que los tests puedan fijar
/// el día: una función que consulta la hora del sistema sólo se puede probar
/// el día que se escribió.
///
/// Las entradas con `ts` ilegible se cuentan en los totales pero no en las
/// series temporales: perder un dictado del total sería peor que no poder
/// ubicarlo en el calendario.
pub fn calcular(entradas: &[Entry], hoy: NaiveDate) -> Metricas {
    let mut m = Metricas {
        por_hora: vec![0; 24],
        ..Default::default()
    };
    let desde = hoy - Duration::days(DIAS_DEL_GRAFICO as i64 - 1);
    let mut dias: Vec<DiaConPalabras> = (0..DIAS_DEL_GRAFICO)
        .map(|i| DiaConPalabras {
            dia: (desde + Duration::days(i as i64)).format("%Y-%m-%d").to_string(),
            palabras: 0,
            dictados: 0,
        })
        .collect();
    // Acumuladores por modo, en orden de aparición.
    let mut modos: Vec<(String, usize, f64)> = Vec::new();

    for entrada in entradas {
        m.total_dictados += 1;
        m.total_palabras += entrada.words;
        m.total_audio_s += entrada.audio_s;

        let latencia = (entrada.transcribe_s + entrada.cleanup_s) as f64;
        match modos.iter_mut().find(|(modo, _, _)| *modo == entrada.mode) {
            Some((_, cantidad, suma)) => {
                *cantidad += 1;
                *suma += latencia;
            }
            None => modos.push((entrada.mode.clone(), 1, latencia)),
        }

        let Some(momento) = momento_de(&entrada.ts) else {
            continue;
        };
        // `hour()` es 0..=23 por construcción, así que el índice es seguro.
        m.por_hora[momento.hour() as usize] += entrada.words;
        let dia = momento.date();
        if dia == hoy {
            m.dictados_hoy += 1;
            m.palabras_hoy += entrada.words;
        }
        if dia >= desde && dia <= hoy {
            let indice = (dia - desde).num_days() as usize;
            if let Some(punto) = dias.get_mut(indice) {
                punto.palabras += entrada.words;
                punto.dictados += 1;
            }
        }
    }

    let minutos_hablando = m.total_audio_s / 60.0;
    if minutos_hablando > 0.0 {
        m.palabras_por_minuto = m.total_palabras as f32 / minutos_hablando;
    }
    m.minutos_ahorrados =
        (m.total_palabras as f32 / PALABRAS_POR_MINUTO_TIPEANDO - minutos_hablando).max(0.0);

    // El modo vigente es el del dictado más nuevo, no el más frecuente: la
    // pregunta que contesta la métrica es "¿cuánto tarda HOY?".
    m.modo_actual = entradas.last().map(|e| e.mode.clone());
    if let Some(actual) = &m.modo_actual {
        if let Some((_, cantidad, suma)) = modos.iter().find(|(modo, _, _)| modo == actual) {
            m.latencia_s = Some((suma / *cantidad as f64) as f32);
            m.dictados_de_otros_modos = m.total_dictados - cantidad;
        }
    }

    m.por_modo = modos
        .into_iter()
        .map(|(modo, dictados, suma)| ResumenModo {
            modo,
            dictados,
            latencia_s: (suma / dictados as f64) as f32,
        })
        .collect();
    // De más usado a menos. `Reverse` sobre la clave en vez de dar vuelta el
    // comparador: es el mismo orden y sigue siendo estable, pero no hay forma de
    // equivocarse escribiendo `a` donde va `b`.
    m.por_modo
        .sort_by_key(|resumen| std::cmp::Reverse(resumen.dictados));
    m.por_dia = dias;
    m
}

/// Interpreta el `ts` del historial. `None` si no tiene el formato esperado.
///
/// Se aceptan también entradas con fracciones de segundo o zona pegada
/// (`...T10:00:00.123`, `...T10:00:00-03:00`) quedándose con los 19 primeros
/// caracteres: son formatos que ningún escritor del proyecto produce hoy, pero
/// un historial es un archivo de años y descartar una fila por un sufijo sería
/// perder un dictado del gráfico sin decirlo.
fn momento_de(ts: &str) -> Option<NaiveDateTime> {
    let recortado = if ts.len() > 19 { &ts[..19] } else { ts };
    NaiveDateTime::parse_from_str(recortado, FORMATO_TS).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entrada(ts: &str, palabras: usize, audio_s: f32, modo: &str, latencia: f32) -> Entry {
        Entry {
            ts: ts.to_string(),
            audio_s,
            transcribe_s: latencia,
            cleanup_s: 0.0,
            words: palabras,
            cleaned: true,
            mode: modo.to_string(),
            raw: String::new(),
            final_text: "texto".to_string(),
        }
    }

    fn dia(fecha: &str) -> NaiveDate {
        NaiveDate::parse_from_str(fecha, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn sin_dictados_no_hay_division_por_cero() {
        let m = calcular(&[], dia("2026-07-21"));
        assert_eq!(m.total_dictados, 0);
        assert_eq!(m.palabras_por_minuto, 0.0);
        assert_eq!(m.minutos_ahorrados, 0.0);
        assert_eq!(m.latencia_s, None);
        assert_eq!(m.modo_actual, None);
        assert_eq!(m.por_hora.len(), 24);
        assert_eq!(m.por_dia.len(), DIAS_DEL_GRAFICO);
    }

    #[test]
    fn los_totales_y_las_derivadas() {
        // 120 palabras en 60 s de audio: 120 ppm hablando; tipear a 40 ppm
        // costaría 3 min, hablando costó 1 → 2 min ahorrados.
        let entradas = vec![
            entrada("2026-07-21T10:00:00", 60, 30.0, "fast", 0.5),
            entrada("2026-07-21T11:00:00", 60, 30.0, "fast", 0.5),
        ];
        let m = calcular(&entradas, dia("2026-07-21"));

        assert_eq!(m.total_dictados, 2);
        assert_eq!(m.total_palabras, 120);
        assert_eq!(m.total_audio_s, 60.0);
        assert!((m.palabras_por_minuto - 120.0).abs() < 0.01);
        assert!((m.minutos_ahorrados - 2.0).abs() < 0.01);
        assert_eq!(m.dictados_hoy, 2);
        assert_eq!(m.palabras_hoy, 120);
    }

    /// Lo de ayer no cuenta como de hoy. El delta "+N hoy" es una de las cosas
    /// que más se miran, y equivocarlo es peor que no mostrarlo.
    #[test]
    fn el_delta_de_hoy_es_solo_de_hoy() {
        let entradas = vec![
            entrada("2026-07-20T23:59:59", 10, 5.0, "fast", 0.5),
            entrada("2026-07-21T00:00:00", 7, 5.0, "fast", 0.5),
        ];
        let m = calcular(&entradas, dia("2026-07-21"));
        assert_eq!(m.dictados_hoy, 1);
        assert_eq!(m.palabras_hoy, 7);
        assert_eq!(m.total_dictados, 2, "los de ayer siguen en el total");
    }

    /// EL bug del dashboard actual: la latencia se calcula sobre el modo
    /// vigente, no sobre la mezcla de épocas.
    #[test]
    fn la_latencia_no_mezcla_la_epoca_del_llm_con_la_actual() {
        let entradas = vec![
            entrada("2026-07-19T10:00:00", 10, 5.0, "llm", 5.0),
            entrada("2026-07-19T10:01:00", 10, 5.0, "llm", 5.0),
            entrada("2026-07-21T10:00:00", 10, 5.0, "fast", 0.5),
            entrada("2026-07-21T10:01:00", 10, 5.0, "fast", 0.3),
        ];
        let m = calcular(&entradas, dia("2026-07-21"));

        assert_eq!(m.modo_actual.as_deref(), Some("fast"));
        let latencia = m.latencia_s.expect("con dictados hay latencia");
        assert!(
            (latencia - 0.4).abs() < 0.001,
            "promedió los del LLM: {latencia}"
        );
        assert_eq!(m.dictados_de_otros_modos, 2);

        // Y los otros modos no se pierden: se informan aparte.
        let llm = m.por_modo.iter().find(|r| r.modo == "llm").expect("falta el modo llm");
        assert_eq!(llm.dictados, 2);
        assert!((llm.latencia_s - 5.0).abs() < 0.001);
    }

    /// La serie diaria tiene que incluir los días sin dictados: un gráfico que
    /// los saltea muestra un uso más parejo del real.
    #[test]
    fn la_serie_diaria_rellena_los_dias_vacios() {
        let entradas = vec![
            entrada("2026-07-19T10:00:00", 5, 5.0, "fast", 0.5),
            entrada("2026-07-21T10:00:00", 9, 5.0, "fast", 0.5),
        ];
        let m = calcular(&entradas, dia("2026-07-21"));

        assert_eq!(m.por_dia.len(), DIAS_DEL_GRAFICO);
        assert_eq!(m.por_dia.last().unwrap().dia, "2026-07-21");
        assert_eq!(m.por_dia.last().unwrap().palabras, 9);
        let ayer = &m.por_dia[DIAS_DEL_GRAFICO - 2];
        assert_eq!(ayer.dia, "2026-07-20");
        assert_eq!(ayer.palabras, 0, "el día sin dictados tiene que estar, en cero");
        assert_eq!(m.por_dia[DIAS_DEL_GRAFICO - 3].palabras, 5);
    }

    /// Un dictado más viejo que la ventana del gráfico cuenta en los totales
    /// pero no puede desbordar el vector de días.
    #[test]
    fn un_dictado_fuera_de_la_ventana_no_desborda() {
        let entradas = vec![entrada("2020-01-01T10:00:00", 42, 5.0, "fast", 0.5)];
        let m = calcular(&entradas, dia("2026-07-21"));
        assert_eq!(m.total_palabras, 42);
        assert!(m.por_dia.iter().all(|d| d.palabras == 0));
        assert_eq!(m.por_hora[10], 42, "en el reloj sí entra: la hora no caduca");
    }

    #[test]
    fn las_palabras_se_reparten_por_hora_del_dia() {
        let entradas = vec![
            entrada("2026-07-21T09:30:00", 5, 5.0, "fast", 0.5),
            entrada("2026-07-21T09:45:00", 3, 5.0, "fast", 0.5),
            entrada("2026-07-21T23:00:00", 2, 5.0, "fast", 0.5),
        ];
        let m = calcular(&entradas, dia("2026-07-21"));
        assert_eq!(m.por_hora[9], 8);
        assert_eq!(m.por_hora[23], 2);
        assert_eq!(m.por_hora.iter().sum::<usize>(), 10);
    }

    /// Un `ts` roto no puede tirar abajo el dashboard ni desaparecer del total.
    #[test]
    fn una_marca_de_tiempo_ilegible_no_rompe_nada() {
        let entradas = vec![
            entrada("no es una fecha", 11, 5.0, "fast", 0.5),
            entrada("2026-07-21T10:00:00", 4, 5.0, "fast", 0.5),
        ];
        let m = calcular(&entradas, dia("2026-07-21"));
        assert_eq!(m.total_dictados, 2, "la entrada rota sigue contando");
        assert_eq!(m.total_palabras, 15);
        assert_eq!(m.palabras_hoy, 4, "pero no se le puede poner fecha");
    }

    #[test]
    fn acepta_marcas_de_tiempo_con_sufijo() {
        assert!(momento_de("2026-07-21T10:00:00.123456").is_some());
        assert!(momento_de("2026-07-21T10:00:00-03:00").is_some());
        assert!(momento_de("2026-07-21").is_none());
        assert!(momento_de("").is_none());
    }
}
