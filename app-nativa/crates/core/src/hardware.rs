//! Perfilado de la máquina: el modelo se elige MIDIENDO, no por umbrales.
//!
//! # Por qué no se decide por VRAM
//!
//! La primera versión del diseño elegía el modelo con una matriz de umbrales de
//! memoria de video. No funciona, por dos motivos independientes:
//!
//! 1. **`wgpu` no expone la VRAM.** `AdapterInfo` tiene doce campos y ninguno es
//!    capacidad en bytes.
//! 2. **Aunque se obtenga por DXGI, en una integrada el número miente.** La
//!    memoria es compartida con el sistema y `DedicatedVideoMemory` reporta los
//!    ~128 MB reservados. Una matriz por umbrales mandaría a toda notebook con
//!    gráficos integrados a la rama "sin GPU utilizable" **aunque Vulkan
//!    funcione perfecto ahí**, que es justo lo contrario de lo que se quiere.
//!
//! El caso que fuerza este diseño es real y está en el proyecto: la tercera
//! máquina objetivo es una notebook con integrada AMD y ~8 GB de RAM. Medir la
//! resuelve sin conocer una sola especificación de antemano.
//!
//! # Qué se mide y qué es informativo
//!
//! Lo único **decisorio** es el factor de tiempo real: cuántos segundos de audio
//! procesa la máquina por cada segundo de reloj, medido con el modelo más
//! liviano sobre un audio de referencia. El nombre de la GPU y el backend son
//! informativos: van al perfil para que el usuario y el soporte sepan qué pasó,
//! no para decidir.
//!
//! La única excepción es el **tipo** de dispositivo: si no hay GPU dedicada, los
//! pesos viven en la RAM del sistema, y eso sí acota qué modelo entra
//! ([`elegir_modelo`]).
//!
//! # Cómo se testea sin hardware
//!
//! [`elegir_modelo`] es pura y no toca ni el modelo ni la placa: recibe los tres
//! números y devuelve el modelo. Medirlos es trabajo de [`perfilar`]. Esa
//! separación es la razón por la que las ramas se pueden verificar con valores
//! inyectados en una máquina cualquiera.

use crate::models::Modelo;

/// A partir de acá la máquina procesa audio veinte veces más rápido de lo que
/// dura: el modelo grande entra con margen de sobra.
pub const FACTOR_HOLGADO: f32 = 20.0;

/// Piso de la franja cómoda.
pub const FACTOR_COMODO: f32 = 8.0;

/// Piso de la franja justa. Por debajo de esto un dictado de diez segundos
/// tarda más de tres, y hay que bajar al modelo más chico.
pub const FACTOR_JUSTO: f32 = 3.0;

/// Sin GPU dedicada, los pesos del modelo viven en la RAM del sistema junto con
/// todo lo demás. Por debajo de esta cifra el `F16` (1,5 GB en disco, más el
/// contexto) deja a la máquina sin aire.
pub const RAM_MINIMA_F16_GB: f32 = 12.0;

/// Piso absoluto: por debajo de esto sólo entra el modelo más chico.
pub const RAM_MINIMA_Q5_GB: f32 = 6.0;

/// Duración del audio con el que se perfila. **Es la calibración de los
/// umbrales de arriba, no un detalle**: ver la nota que sigue.
///
/// # Por qué 9,5 s y no 3
///
/// El diseño pedía "un audio de referencia de ~3 segundos". Se implementó así y
/// se midió en el escritorio (RTX 3080, Vulkan, perfilando con el `Q4_K_M`):
/// **17,25x**, que cae en la franja cómoda y elige el modelo intermedio. La
/// máquina más rápida del proyecto —la misma que hace 9,5 s de audio en 0,221 s,
/// o sea 43x— quedaba sin el modelo grande.
///
/// La causa es que **whisper rellena toda entrada hasta 30 s antes del
/// codificador**, así que el tiempo de inferencia es casi independiente de lo
/// que dure el clip. Dividir un costo prácticamente fijo por 3 en vez de por 9,5
/// desinfla el factor unas tres veces. Los umbrales de este módulo vienen de
/// mediciones sobre **9,5 s** (la línea base de `DECISIONES.md`), así que un
/// clip más corto los deja descalibrados.
///
/// Se iguala la referencia a esa línea base. Alargarla **no cuesta tiempo de
/// perfilado** —el trabajo real es el mismo— y devuelve un número directamente
/// comparable con las mediciones que ya tiene el proyecto.
pub const SEGUNDOS_DE_REFERENCIA: f32 = 9.5;

/// Cuántas pasadas medidas se promedian por mediana. Impar para que la mediana
/// sea un valor medido y no un promedio de dos.
const PASADAS_MEDIDAS: usize = 3;

/// Frecuencia del tono de referencia, en Hz.
const FRECUENCIA_REFERENCIA: f32 = 220.0;

/// Amplitud del tono de referencia. Da un RMS de 0,035: 3,5x el umbral de la
/// compuerta de energía ([`crate::config::MIN_SPEECH_RMS`]).
const AMPLITUD_REFERENCIA: f32 = 0.05;

/// Lo que el perfilado averiguó de esta máquina.
#[derive(Debug, Clone)]
pub struct PerfilHardware {
    pub ram_total_gb: f32,
    /// Nombre del adaptador según `wgpu`, o `None` si no se enumeró ninguno.
    pub gpu_nombre: Option<String>,
    /// Si la placa tiene memoria propia. Es el bit que decide si la RAM del
    /// sistema acota el modelo ([`limitar_por_memoria`]), y por eso el asistente
    /// de primer arranque lo necesita: es lo que le permite avisar "ese modelo
    /// no te entra" ANTES de que el usuario espere una descarga de 1,5 GB.
    pub gpu_dedicada: bool,
    /// Cómo clasificó `wgpu` al adaptador ("dedicada", "integrada", "cpu"...),
    /// o `None` si no se enumeró ninguno. Informativo.
    pub gpu_clase: Option<&'static str>,
    /// El backend al que el motor ligó realmente el modelo ("vulkan", "cpu"...).
    /// Es lo que reporta el motor, no lo que se le pidió: la diferencia entre
    /// los dos ES la caída a CPU.
    pub backend: String,
    /// Segundos de audio procesados por segundo de reloj. El único número que
    /// decide.
    pub factor_tiempo_real: f32,
    pub modelo_recomendado: Modelo,
}

/// La franja de velocidad en la que cayó la medición.
///
/// Existe para que la decisión sea un `match` que el compilador verifica
/// exhaustivo. Con un `if/else` sobre `f32` no hay nada que garantice que
/// alguien no borre una rama; con esto, agregar una variante rompe la
/// compilación hasta que se decida qué modelo le toca.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Franja {
    /// `>= FACTOR_HOLGADO`
    Holgada,
    /// `[FACTOR_COMODO, FACTOR_HOLGADO)`
    Comoda,
    /// `[FACTOR_JUSTO, FACTOR_COMODO)`
    Justa,
    /// Todo lo demás: por debajo de `FACTOR_JUSTO`, cero, negativo o `NaN`.
    Ajustada,
}

/// Clasifica el factor medido. Cubre todo el dominio de un `f32` por
/// construcción: las tres comparaciones son falsas para `NaN`, así que un
/// número imposible cae en la franja más conservadora en vez de propagarse.
fn franja(factor_tiempo_real: f32) -> Franja {
    if factor_tiempo_real >= FACTOR_HOLGADO {
        Franja::Holgada
    } else if factor_tiempo_real >= FACTOR_COMODO {
        Franja::Comoda
    } else if factor_tiempo_real >= FACTOR_JUSTO {
        Franja::Justa
    } else {
        Franja::Ajustada
    }
}

/// El modelo que le corresponde a esta máquina.
///
/// `factor_tiempo_real` es lo medido por [`perfilar`]; `gpu_dedicada` distingue
/// una placa con memoria propia de una integrada o de no tener nada.
///
/// Función pura: mismos tres números, mismo modelo, en cualquier máquina. Es lo
/// que permite testear cada rama sin hardware.
pub fn elegir_modelo(factor_tiempo_real: f32, ram_total_gb: f32, gpu_dedicada: bool) -> Modelo {
    let por_velocidad = match franja(factor_tiempo_real) {
        Franja::Holgada => Modelo::F16,
        // Las franjas cómoda y justa comparten modelo hoy. Se mantienen
        // separadas porque son decisiones distintas: si mañana entra un modelo
        // intermedio, va acá y no hay que volver a partir el rango.
        Franja::Comoda => Modelo::Q5KM,
        Franja::Justa => Modelo::Q5KM,
        Franja::Ajustada => Modelo::Q4KM,
    };
    limitar_por_memoria(por_velocidad, ram_total_gb, gpu_dedicada)
}

/// La RAM del sistema que pide este modelo cuando los pesos NO viven en una
/// placa con memoria propia.
///
/// Es la tabla de la que salen [`entra_en_memoria`] y, por lo tanto,
/// [`limitar_por_memoria`]: un solo lugar decide qué modelo entra en qué
/// máquina, y es también el que la interfaz muestra al lado de cada modelo. Si
/// se copiara a mano en otro módulo, el aviso "este modelo no te entra" podría
/// contradecir a la recomendación del perfilado.
///
/// El más chico devuelve `0.0` a propósito: es el piso del catálogo y **nunca**
/// se descarta por memoria — si no entrara, no habría con qué dictar.
pub fn ram_minima_gb(modelo: Modelo) -> f32 {
    match modelo {
        Modelo::F16 => RAM_MINIMA_F16_GB,
        Modelo::Q5KM => RAM_MINIMA_Q5_GB,
        Modelo::Q4KM => 0.0,
    }
}

/// ¿Este modelo entra en esta máquina?
///
/// Con GPU dedicada entra cualquiera: los pesos viven en la VRAM y no compiten
/// con el resto del sistema. Sin ella hay que llegar al piso de
/// [`ram_minima_gb`].
///
/// Es pública porque la decisión de memoria hace falta en dos lugares más allá
/// del perfilado: la sustitución de modelo del arranque —"uso el más grande que
/// entre", no el más grande a secas— y el aviso de la interfaz. Que sea la misma
/// función es lo que impide que se contradigan.
pub fn entra_en_memoria(modelo: Modelo, ram_total_gb: f32, gpu_dedicada: bool) -> bool {
    if gpu_dedicada {
        return true;
    }
    let piso = ram_minima_gb(modelo);
    // El piso del catálogo entra siempre, incluso con una RAM que no se pudo
    // leer: descartarlo dejaría a la máquina sin ningún modelo posible.
    piso <= 0.0 || !no_llega_a(ram_total_gb, piso)
}

/// Baja el modelo elegido si la memoria del sistema no lo banca.
///
/// Sólo se aplica **sin GPU dedicada**: con una placa con VRAM propia los pesos
/// no compiten con el resto del sistema, y es el caso de las dos máquinas del
/// proyecto que tienen 32 y 64 GB igual.
///
/// La regla se expresa sobre [`entra_en_memoria`] y no con umbrales escritos
/// otra vez acá: son la misma decisión mirada desde dos lados —"¿entra?" y
/// "¿cuál pongo en su lugar?"— y separarlas sería dejarlas divergir.
fn limitar_por_memoria(elegido: Modelo, ram_total_gb: f32, gpu_dedicada: bool) -> Modelo {
    if entra_en_memoria(elegido, ram_total_gb, gpu_dedicada) {
        return elegido;
    }
    // El más grande de los que entran, sin subir por encima del elegido.
    // `TODOS` va de mayor a menor y el más chico entra siempre, así que esta
    // búsqueda no puede quedar vacía; el `unwrap_or` es por si el catálogo
    // cambiara.
    Modelo::TODOS
        .into_iter()
        .find(|m| {
            m.bytes() < elegido.bytes() && entra_en_memoria(*m, ram_total_gb, gpu_dedicada)
        })
        .unwrap_or(elegido)
}

/// ¿`valor` se queda corto contra `piso`?
///
/// Existe en vez de un `<` pelado para tratar el `NaN` de forma explícita: una
/// RAM que no se pudo leer tiene que contar como "no llega", nunca como
/// "alcanza". Con `<` a secas, `NaN < piso` es falso y el caso roto habilitaría
/// el modelo más pesado.
fn no_llega_a(valor: f32, piso: f32) -> bool {
    use std::cmp::Ordering;
    match valor.partial_cmp(&piso) {
        Some(Ordering::Less) => true,
        Some(Ordering::Equal | Ordering::Greater) => false,
        // `None` es exactamente el caso `NaN`.
        None => true,
    }
}

/// RAM total de la máquina en GB (base 1024).
///
/// `sysinfo` se instancia pidiendo sólo memoria: enumerar procesos y discos
/// cuesta y no se usa.
pub fn ram_total_gb() -> f32 {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};
    let sistema = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    sistema.total_memory() as f32 / (1024.0 * 1024.0 * 1024.0)
}

/// Lo que se sabe del adaptador de video, sin una sola cifra de memoria.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub nombre: String,
    /// `true` sólo para `DiscreteGpu`. Es el único bit de acá que decide algo
    /// (ver [`limitar_por_memoria`]).
    pub dedicada: bool,
    /// Cómo la clasificó `wgpu`, para mostrar y para diagnosticar.
    pub clase: &'static str,
}

/// Backends que se consultan. Explícitos y no `Backends::all()` para no incluir
/// el backend `NOOP` de `wgpu`, que enumeraría un adaptador falso de tipo `Cpu`
/// y ensuciaría la elección.
const BACKENDS_CONSULTADOS: wgpu::Backends = wgpu::Backends::VULKAN.union(wgpu::Backends::DX12);

/// El mejor adaptador que reporta `wgpu`, o `None` si no hay ninguno.
///
/// "Mejor" es por tipo de dispositivo, no por rendimiento: una misma placa
/// aparece una vez por backend (Vulkan y DX12 la enumeran las dos), así que
/// alcanza con quedarse con la de mayor jerarquía.
///
/// Nunca falla: en una máquina sin drivers de video la lista viene vacía y eso
/// es una respuesta, no un error.
pub fn describir_gpu() -> Option<Gpu> {
    // `Instance::new` paniquea si el binario se compiló sin ningún backend.
    // Es una propiedad del build, no del hardware, pero un panic en el
    // perfilado sería un arranque roto: se contesta `None`.
    if wgpu::Instance::enabled_backend_features().is_empty() {
        return None;
    }
    let instancia = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adaptadores = pollster::block_on(instancia.enumerate_adapters(BACKENDS_CONSULTADOS));
    adaptadores
        .iter()
        .map(|adaptador| adaptador.get_info())
        .max_by_key(|info| jerarquia(info.device_type))
        .map(|info| Gpu {
            nombre: info.name,
            dedicada: info.device_type == wgpu::DeviceType::DiscreteGpu,
            clase: clase(info.device_type),
        })
}

/// Cuánto vale cada tipo de dispositivo a la hora de elegir cuál reportar.
fn jerarquia(tipo: wgpu::DeviceType) -> u8 {
    match tipo {
        wgpu::DeviceType::DiscreteGpu => 4,
        wgpu::DeviceType::IntegratedGpu => 3,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Cpu => 1,
        wgpu::DeviceType::Other => 0,
    }
}

fn clase(tipo: wgpu::DeviceType) -> &'static str {
    match tipo {
        wgpu::DeviceType::DiscreteGpu => "dedicada",
        wgpu::DeviceType::IntegratedGpu => "integrada",
        wgpu::DeviceType::VirtualGpu => "virtual",
        wgpu::DeviceType::Cpu => "cpu",
        wgpu::DeviceType::Other => "desconocida",
    }
}

/// Arma el perfil completo a partir del único número que hay que medir.
///
/// Está separada de [`perfilar`] porque el factor puede venir de dos lados y el
/// resto del perfil —RAM, adaptador, decisión— es el mismo en los dos:
///
/// 1. medido acá, abriendo un `Transcriber` propio ([`perfilar`]);
/// 2. **reusado del motor**, que ya tiene el modelo cargado y lo midió con él.
///
/// Lo segundo es lo que evita tener dos copias del modelo en memoria a la vez.
/// En el escritorio daría igual; en una notebook con gráficos integrados y 8 GB,
/// el segundo `Transcriber` puede no entrar y caer a CPU **en silencio**: el
/// perfilado mediría una máquina más lenta de la que es y recomendaría un
/// modelo peor del que corresponde, que es exactamente el error que este módulo
/// existe para no cometer.
///
/// `backend` es el que reporta quien midió, no el que se pidió: la diferencia
/// entre los dos ES la caída a CPU.
pub fn perfil_con_factor(factor_tiempo_real: f32, backend: String) -> PerfilHardware {
    let ram_total_gb = ram_total_gb();
    let gpu = describir_gpu();
    let gpu_dedicada = gpu.as_ref().is_some_and(|g| g.dedicada);
    PerfilHardware {
        ram_total_gb,
        gpu_clase: gpu.as_ref().map(|g| g.clase),
        gpu_nombre: gpu.map(|g| g.nombre),
        gpu_dedicada,
        backend,
        factor_tiempo_real,
        modelo_recomendado: elegir_modelo(factor_tiempo_real, ram_total_gb, gpu_dedicada),
    }
}

/// Los [`SEGUNDOS_DE_REFERENCIA`] de audio con los que se mide.
///
/// Es un tono generado acá y no un WAV empaquetado: la señal es idéntica en las
/// tres máquinas, bit por bit, y no hay fixture que instalar.
///
/// **No puede ser silencio.** `Transcriber::transcribe` descarta el audio por
/// debajo de [`crate::config::MIN_SPEECH_RMS`] ANTES de llamar al modelo, así
/// que un buffer de ceros volvería en microsegundos y el perfilado mediría el
/// costo de un `if` en vez del de la máquina.
pub fn audio_de_referencia() -> Vec<f32> {
    let frecuencia_muestreo = crate::config::SAMPLE_RATE as f32;
    let muestras = (frecuencia_muestreo * SEGUNDOS_DE_REFERENCIA) as usize;
    (0..muestras)
        .map(|i| {
            let fase =
                2.0 * std::f32::consts::PI * FRECUENCIA_REFERENCIA * i as f32 / frecuencia_muestreo;
            fase.sin() * AMPLITUD_REFERENCIA
        })
        .collect()
}

#[cfg(windows)]
pub use medicion::{medir_factor_tiempo_real, perfilar};

#[cfg(windows)]
mod medicion {
    use super::{
        audio_de_referencia, perfil_con_factor, PerfilHardware, PASADAS_MEDIDAS,
        SEGUNDOS_DE_REFERENCIA,
    };
    use crate::stt::Transcriber;
    use std::path::Path;
    use std::time::Instant;

    /// Mide esta máquina y devuelve el perfil completo.
    ///
    /// `modelo_de_prueba` tiene que ser un GGUF ya presente en disco — el más
    /// liviano ([`crate::models::Modelo::DE_PERFILADO`]), porque esto corre
    /// antes de saber cuál conviene bajar. **El modelo con el que se mide
    /// influye en el número**: uno más chico da un factor más alto, así que
    /// cambiarlo obliga a recalibrar los umbrales.
    ///
    /// Cuesta lo que cuesten cuatro inferencias más la carga del modelo, y la
    /// primera vez de todas también la compilación de los shaders de Vulkan
    /// (17 s medidos). Va en un hilo aparte, no en el que dibuja la interfaz.
    /// **Abre un `Transcriber` propio**, así que quien ya tenga uno cargado con
    /// este mismo modelo no debería llamar acá: le alcanza con medir sobre el
    /// suyo y armar el perfil con [`perfil_con_factor`]. Dos copias del modelo
    /// residentes a la vez es lo que rompe una máquina chica.
    pub fn perfilar(modelo_de_prueba: &Path) -> Result<PerfilHardware, String> {
        // Acá no hace falta distinguir por qué no abrió: perfilar SIEMPRE se
        // llama con un modelo que el asistente acaba de descargar, así que
        // cualquier fallo es igual de excepcional y el texto alcanza.
        let mut transcriber = Transcriber::new(modelo_de_prueba).map_err(|e| e.to_string())?;
        let backend = transcriber.backend();
        let factor_tiempo_real = medir_factor_tiempo_real(&mut transcriber)?;
        Ok(perfil_con_factor(factor_tiempo_real, backend))
    }

    /// Segundos de audio procesados por segundo de reloj.
    ///
    /// # Por qué hay una pasada que se tira
    ///
    /// La **primera** inferencia del proceso compila los shaders de Vulkan:
    /// 17,4 s medidos en el escritorio con una RTX 3080. Medirla daría un factor
    /// de 0,17x en la máquina más rápida del proyecto y mandaría a todo el mundo
    /// al modelo más chico. Se descarta y se miden las siguientes.
    ///
    /// Se toma la **mediana** y no el promedio: un pico del planificador del
    /// sistema operativo en una de las tres pasadas no debe mover la decisión.
    pub fn medir_factor_tiempo_real(transcriber: &mut Transcriber) -> Result<f32, String> {
        let audio = audio_de_referencia();

        transcriber.transcribe(&audio)?;

        let mut tiempos = Vec::with_capacity(PASADAS_MEDIDAS);
        for _ in 0..PASADAS_MEDIDAS {
            let t0 = Instant::now();
            transcriber.transcribe(&audio)?;
            tiempos.push(t0.elapsed().as_secs_f32());
        }
        tiempos.sort_by(f32::total_cmp);
        let mediana = tiempos[PASADAS_MEDIDAS / 2];

        // Un cero o un `NaN` acá daría un factor infinito, y el infinito cae en
        // la franja holgada: se elegiría el modelo más grande porque el reloj
        // falló. Es exactamente el error que no se puede cometer.
        // El orden importa: `is_finite` descarta el `NaN` primero, así que la
        // comparación de abajo nunca lo ve.
        if !mediana.is_finite() || mediana <= 0.0 {
            return Err(format!(
                "la medición dio un tiempo imposible ({mediana} s); no puedo perfilar"
            ));
        }
        Ok(SEGUNDOS_DE_REFERENCIA / mediana)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Una máquina que no limita nada por memoria, para aislar la velocidad.
    const RAM_DE_SOBRA: f32 = 64.0;

    #[test]
    fn la_franja_holgada_elige_el_modelo_grande() {
        assert_eq!(elegir_modelo(43.0, RAM_DE_SOBRA, true), Modelo::F16);
        // El borde exacto pertenece a la franja de arriba.
        assert_eq!(
            elegir_modelo(FACTOR_HOLGADO, RAM_DE_SOBRA, true),
            Modelo::F16
        );
    }

    #[test]
    fn la_franja_comoda_elige_el_intermedio() {
        assert_eq!(
            elegir_modelo(FACTOR_HOLGADO - 0.1, RAM_DE_SOBRA, true),
            Modelo::Q5KM
        );
        assert_eq!(elegir_modelo(12.0, RAM_DE_SOBRA, true), Modelo::Q5KM);
        assert_eq!(
            elegir_modelo(FACTOR_COMODO, RAM_DE_SOBRA, true),
            Modelo::Q5KM
        );
    }

    #[test]
    fn la_franja_justa_tambien_elige_el_intermedio() {
        assert_eq!(
            elegir_modelo(FACTOR_COMODO - 0.1, RAM_DE_SOBRA, true),
            Modelo::Q5KM
        );
        assert_eq!(elegir_modelo(5.0, RAM_DE_SOBRA, true), Modelo::Q5KM);
        assert_eq!(
            elegir_modelo(FACTOR_JUSTO, RAM_DE_SOBRA, true),
            Modelo::Q5KM
        );
    }

    #[test]
    fn la_franja_ajustada_elige_el_chico() {
        assert_eq!(
            elegir_modelo(FACTOR_JUSTO - 0.1, RAM_DE_SOBRA, true),
            Modelo::Q4KM
        );
        assert_eq!(elegir_modelo(1.0, RAM_DE_SOBRA, true), Modelo::Q4KM);
        assert_eq!(elegir_modelo(0.2, RAM_DE_SOBRA, true), Modelo::Q4KM);
    }

    /// Un factor imposible (reloj roto, medición corrupta) no puede terminar
    /// eligiendo el modelo más pesado. Cae en la franja más conservadora.
    #[test]
    fn un_factor_imposible_cae_en_la_franja_mas_conservadora() {
        assert_eq!(elegir_modelo(f32::NAN, RAM_DE_SOBRA, true), Modelo::Q4KM);
        assert_eq!(elegir_modelo(0.0, RAM_DE_SOBRA, true), Modelo::Q4KM);
        assert_eq!(elegir_modelo(-5.0, RAM_DE_SOBRA, true), Modelo::Q4KM);
        assert_eq!(
            elegir_modelo(f32::NEG_INFINITY, RAM_DE_SOBRA, true),
            Modelo::Q4KM
        );
    }

    /// El caso que motivó todo el diseño: la notebook secundaria, con gráficos
    /// integrados AMD y ~8 GB. Vulkan anda, así que el factor es decente, pero
    /// el modelo tiene que vivir en la RAM del sistema.
    #[test]
    fn la_notebook_secundaria_recibe_el_intermedio() {
        const RAM: f32 = 8.0;
        // 3-4x tiempo real es lo medido en integradas equivalentes (Radeon 680M).
        assert_eq!(elegir_modelo(3.5, RAM, false), Modelo::Q5KM);
        assert_eq!(elegir_modelo(4.0, RAM, false), Modelo::Q5KM);
    }

    /// La restricción de memoria explícita: 8 GB y sin GPU dedicada NUNCA da
    /// `F16`, por rápida que haya salido la medición.
    #[test]
    fn con_8gb_y_sin_gpu_dedicada_nunca_elige_f16() {
        for factor in [0.5, 3.0, 8.0, 20.0, 43.0, 200.0, f32::INFINITY] {
            let elegido = elegir_modelo(factor, 8.0, false);
            assert_ne!(
                elegido,
                Modelo::F16,
                "con 8 GB y sin GPU dedicada, un factor de {factor} eligió F16"
            );
        }
    }

    /// El borde de la restricción: 12 GB justos ya habilitan el `F16`.
    #[test]
    fn el_umbral_de_ram_para_el_f16_es_inclusivo() {
        assert_eq!(
            elegir_modelo(43.0, RAM_MINIMA_F16_GB, false),
            Modelo::F16
        );
        assert_eq!(
            elegir_modelo(43.0, RAM_MINIMA_F16_GB - 0.1, false),
            Modelo::Q5KM
        );
    }

    /// Por debajo de 6 GB se fuerza el más chico, aunque la medición diga que
    /// la máquina vuela.
    #[test]
    fn con_menos_de_6gb_se_fuerza_el_chico() {
        for factor in [0.5, 5.0, 43.0, 200.0] {
            assert_eq!(
                elegir_modelo(factor, 4.0, false),
                Modelo::Q4KM,
                "con 4 GB, un factor de {factor} no eligió el modelo chico"
            );
        }
        assert_eq!(
            elegir_modelo(43.0, RAM_MINIMA_Q5_GB, false),
            Modelo::Q5KM,
            "6 GB justos ya habilitan el intermedio"
        );
    }

    /// Una RAM que no se pudo leer no puede habilitar el modelo más grande.
    #[test]
    fn una_ram_imposible_cae_del_lado_conservador() {
        assert_eq!(elegir_modelo(43.0, f32::NAN, false), Modelo::Q4KM);
        assert_eq!(elegir_modelo(43.0, 0.0, false), Modelo::Q4KM);
    }

    /// La pregunta "¿entra?" y la respuesta "¿cuál pongo?" tienen que contar lo
    /// mismo: un modelo que [`entra_en_memoria`] acepta no puede ser bajado por
    /// [`limitar_por_memoria`], y uno que rechaza no puede sobrevivir.
    ///
    /// Es la invariante que permite que la sustitución del arranque
    /// (`rutas::modelo`) use `entra_en_memoria` sin repetir umbrales.
    #[test]
    fn entrar_en_memoria_y_limitar_por_memoria_dicen_lo_mismo() {
        for ram in [0.0, 4.0, 5.9, 6.0, 8.0, 11.9, 12.0, 64.0, f32::NAN] {
            for dedicada in [true, false] {
                for modelo in Modelo::TODOS {
                    let entra = entra_en_memoria(modelo, ram, dedicada);
                    let limitado = limitar_por_memoria(modelo, ram, dedicada);
                    assert_eq!(
                        entra,
                        limitado == modelo,
                        "{modelo} con {ram} GB (dedicada: {dedicada}): entra={entra} pero \
                         limitar devolvió {limitado}"
                    );
                    assert!(
                        entra_en_memoria(limitado, ram, dedicada),
                        "limitar devolvió {limitado}, que tampoco entra"
                    );
                }
            }
        }
    }

    /// El caso de la notebook de 8 GB, que es el que motiva todo esto: el `F16`
    /// no entra, los otros dos sí. El piso del catálogo entra SIEMPRE, incluso
    /// con una RAM que no se pudo leer.
    #[test]
    fn el_criterio_de_memoria_por_modelo() {
        assert!(!entra_en_memoria(Modelo::F16, 8.0, false));
        assert!(entra_en_memoria(Modelo::Q5KM, 8.0, false));
        assert!(entra_en_memoria(Modelo::Q4KM, 8.0, false));

        // Con placa propia entran todos.
        for modelo in Modelo::TODOS {
            assert!(entra_en_memoria(modelo, 8.0, true), "{modelo} con GPU dedicada");
        }

        // Una RAM ilegible es conservadora, pero nunca deja a la máquina sin
        // ningún modelo posible.
        assert!(!entra_en_memoria(Modelo::F16, f32::NAN, false));
        assert!(!entra_en_memoria(Modelo::Q5KM, f32::NAN, false));
        assert!(entra_en_memoria(Modelo::Q4KM, f32::NAN, false));
    }

    /// Los pisos que se publican son los mismos que decide el perfilado, y el
    /// más chico no pide nada.
    #[test]
    fn la_ram_minima_sale_de_los_umbrales_del_perfilado() {
        assert_eq!(ram_minima_gb(Modelo::F16), RAM_MINIMA_F16_GB);
        assert_eq!(ram_minima_gb(Modelo::Q5KM), RAM_MINIMA_Q5_GB);
        assert_eq!(ram_minima_gb(Modelo::Q4KM), 0.0);
    }

    /// Con VRAM propia los pesos no compiten con el sistema: la RAM total deja
    /// de acotar. Es lo que separa a la MSI Katana (RTX 3050 Ti) de una
    /// integrada con la misma memoria.
    #[test]
    fn con_gpu_dedicada_la_ram_del_sistema_no_limita() {
        assert_eq!(elegir_modelo(43.0, 8.0, true), Modelo::F16);
        assert_eq!(elegir_modelo(43.0, 4.0, true), Modelo::F16);
    }

    /// Las tres máquinas del proyecto, de punta a punta.
    #[test]
    fn las_tres_maquinas_objetivo() {
        // Escritorio: RTX 3080, 64 GB. Medido: 9,5 s de audio en 0,221 s.
        assert_eq!(elegir_modelo(43.0, 64.0, true), Modelo::F16);
        // MSI Katana: RTX 3050 Ti (dedicada), 32 GB.
        assert_eq!(elegir_modelo(25.0, 32.0, true), Modelo::F16);
        // Notebook secundaria: integrada AMD, ~8 GB.
        assert_eq!(elegir_modelo(3.5, 8.0, false), Modelo::Q5KM);
    }

    /// Si la clasificación de franjas dejara un hueco, existiría un `f32` que
    /// no cae en ninguna. Se barre el dominio para que no sea una promesa del
    /// comentario sino una verificación.
    #[test]
    fn toda_la_recta_positiva_tiene_franja() {
        let mut factor = 0.0f32;
        while factor < 100.0 {
            let elegido = elegir_modelo(factor, RAM_DE_SOBRA, true);
            assert!(Modelo::TODOS.contains(&elegido));
            factor += 0.05;
        }
        assert_eq!(elegir_modelo(f32::INFINITY, RAM_DE_SOBRA, true), Modelo::F16);
        assert_eq!(elegir_modelo(f32::MIN_POSITIVE, RAM_DE_SOBRA, true), Modelo::Q4KM);
    }

    /// El audio con el que se mide tiene que llegar al modelo: si no pasa la
    /// compuerta de energía, el perfilado mediría el costo de un `if` y todas
    /// las máquinas darían el mismo número absurdo.
    #[test]
    fn el_audio_de_referencia_pasa_la_compuerta_de_energia() {
        let audio = audio_de_referencia();
        assert_eq!(
            audio.len(),
            (crate::config::SAMPLE_RATE as f32 * SEGUNDOS_DE_REFERENCIA) as usize
        );
        assert!(
            crate::stt::tiene_voz(&audio),
            "el audio de referencia no pasa la compuerta RMS: el perfilado no mediría nada"
        );
    }

    #[test]
    fn la_ram_total_es_un_numero_creible() {
        let ram = ram_total_gb();
        assert!(
            ram > 0.5 && ram < 4096.0,
            "la RAM total dio {ram} GB, que no es creíble"
        );
    }

    /// El perfil armado con un factor ya medido tiene que decidir **lo mismo**
    /// que la función pura. Es el camino por el que sale el perfilado cuando
    /// reusa la medición del motor en vez de abrir un segundo `Transcriber`: si
    /// divergiera, la recomendación dependería de quién midió.
    #[test]
    fn el_perfil_armado_con_un_factor_decide_igual_que_la_funcion_pura() {
        let perfil = perfil_con_factor(43.0, "vulkan".to_string());

        assert_eq!(perfil.factor_tiempo_real, 43.0);
        assert_eq!(perfil.backend, "vulkan");
        assert_eq!(
            perfil.modelo_recomendado,
            elegir_modelo(43.0, perfil.ram_total_gb, perfil.gpu_dedicada),
            "el perfil reusado no coincide con la función pura de decisión"
        );
        assert_eq!(
            perfil.gpu_dedicada,
            perfil.gpu_clase == Some("dedicada"),
            "el bit que decide y la clase informativa tienen que contar lo mismo"
        );
    }

    /// No se puede afirmar qué GPU hay en la máquina que corra esto, pero sí
    /// que enumerar no explota y que lo que devuelve es coherente consigo mismo.
    ///
    /// `None` —una máquina sin drivers de video— es una respuesta válida y no
    /// un fallo: por eso no se asevera nada en ese caso.
    #[test]
    fn describir_la_gpu_no_falla_y_es_coherente() {
        if let Some(gpu) = describir_gpu() {
            assert!(!gpu.nombre.trim().is_empty(), "una GPU sin nombre");
            assert_eq!(gpu.dedicada, gpu.clase == "dedicada");
        }
    }

    /// El perfilado real de ESTA máquina. Ignorado porque necesita el GGUF.
    ///
    /// Correr con:
    ///   cargo test -p mithflow-core hardware -- --ignored --nocapture
    #[cfg(windows)]
    #[test]
    #[ignore = "necesita el GGUF de perfilado y mide sobre el hardware real"]
    fn perfilado_real_de_esta_maquina() {
        let modelo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../models")
            .join(Modelo::DE_PERFILADO.nombre_archivo());

        let perfil = perfilar(&modelo).expect("falló el perfilado");

        eprintln!("\n=== PerfilHardware ===");
        eprintln!("  ram_total_gb        : {:.1}", perfil.ram_total_gb);
        eprintln!("  gpu_nombre          : {:?}", perfil.gpu_nombre);
        eprintln!("  backend             : {}", perfil.backend);
        eprintln!("  factor_tiempo_real  : {:.2}x", perfil.factor_tiempo_real);
        eprintln!("  modelo_recomendado  : {}", perfil.modelo_recomendado);
        eprintln!("  (perfilado con {})\n", Modelo::DE_PERFILADO);

        assert!(
            perfil.factor_tiempo_real > 0.0,
            "el factor medido tiene que ser positivo"
        );
        assert!(!perfil.backend.is_empty(), "el motor no reportó backend");
        assert_eq!(
            perfil.modelo_recomendado,
            elegir_modelo(
                perfil.factor_tiempo_real,
                perfil.ram_total_gb,
                describir_gpu().is_some_and(|g| g.dedicada)
            ),
            "el perfil no es consistente con la función pura de decisión"
        );
    }
}
