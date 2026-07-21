//! Los cuatro tonos de realimentación.
//!
//! Con la ventana cerrada —que es como se usa esto el 99% del tiempo— el sonido
//! es la ÚNICA señal de lo que está pasando. Por eso son cuatro y no uno, y por
//! eso cada uno significa algo distinto:
//!
//! | Tono | Cuándo | Forma |
//! |---|---|---|
//! | [`Tono::Inicio`] | empezó a grabar | acorde ascendente (do–mi) |
//! | [`Tono::Fin`] | dejó de grabar | grave corto (sol) |
//! | [`Tono::Pegado`] | el texto llegó al documento | campanita (mi–sol) |
//! | [`Tono::Error`] | algo falló o no se escuchó nada | grave largo (la) |
//!
//! Las frecuencias, la duración, el volumen y el fade de 20 ms son los mismos
//! que `play_tone` en `mithflow.py`: la persona que use las dos versiones
//! durante la transición no tiene que aprender un vocabulario nuevo.
//!
//! # Por qué hay un hilo
//!
//! `MixerDeviceSink` retiene el `cpal::Stream` de salida, que no es `Sync`, así
//! que no se puede compartir por `Arc` entre el director y el motor. Un hilo
//! propio con un canal resuelve las dos cosas de una vez: el handle vive en un
//! solo lugar y `Sonidos` queda siendo un `Sender`, que sí es `Send + Sync`.
//! De paso, abrir el dispositivo (decenas de ms) nunca ocurre en el hilo que
//! está por empezar a grabar.

use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink};
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

/// Frecuencia de generación. La del `play_tone` de Python; el mezclador de
/// rodio la remuestrea a lo que pida la placa.
const FRECUENCIA_MUESTREO: u32 = 44_100;

/// Fade de entrada y de salida. Sin él, arrancar y cortar una senoide en un
/// valor distinto de cero produce un chasquido bien audible.
const FADE_SEGS: f32 = 0.020;

/// Volumen por defecto, igual que en Python. Es de realimentación, no de
/// música: tiene que oírse sin tapar lo que el usuario esté escuchando.
pub const VOLUMEN_POR_DEFECTO: f32 = 0.15;

/// Los cuatro significados. Deliberadamente NO hay un tono genérico: si algo
/// nuevo necesita sonar, tiene que decidir a cuál de estos cuatro se parece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tono {
    Inicio,
    Fin,
    Pegado,
    Error,
}

impl Tono {
    /// Frecuencias en Hz. Varias suenan como acorde.
    fn frecuencias(self) -> &'static [f32] {
        match self {
            Tono::Inicio => &[523.0, 659.0],
            Tono::Fin => &[392.0],
            Tono::Pegado => &[659.0, 784.0],
            Tono::Error => &[220.0],
        }
    }

    fn duracion_segs(self) -> f32 {
        match self {
            Tono::Inicio | Tono::Fin => 0.14,
            Tono::Pegado => 0.18,
            Tono::Error => 0.30,
        }
    }
}

/// Genera las muestras del tono: suma de senoides normalizada, con fade lineal
/// en los dos extremos, escalada por el volumen.
///
/// Es una función pura para poder verificar la envolvente sin placa de sonido.
fn muestras(tono: Tono, volumen: f32) -> Vec<f32> {
    let frecuencias = tono.frecuencias();
    let total = (FRECUENCIA_MUESTREO as f32 * tono.duracion_segs()) as usize;
    let fade = ((FRECUENCIA_MUESTREO as f32 * FADE_SEGS) as usize).clamp(1, total.max(1) / 2);

    (0..total)
        .map(|i| {
            let t = i as f32 / FRECUENCIA_MUESTREO as f32;
            let onda: f32 = frecuencias
                .iter()
                .map(|f| (2.0 * std::f32::consts::PI * f * t).sin())
                .sum::<f32>()
                / frecuencias.len() as f32;
            let envolvente = if i < fade {
                i as f32 / fade as f32
            } else if i >= total - fade {
                (total - i) as f32 / fade as f32
            } else {
                1.0
            };
            onda * envolvente * volumen
        })
        .collect()
}

/// Handle para pedir tonos desde cualquier hilo.
///
/// Clonable y barato. Si la placa de sonido no existe o falla, todo esto
/// degrada a no hacer nada: un dictado no se pierde porque no haya parlantes.
#[derive(Clone)]
pub struct Sonidos {
    peticiones: Sender<Tono>,
    config: Arc<Config>,
}

struct Config {
    activos: AtomicBool,
    /// El volumen en `f32` no cabe en un atómico; se guardan sus bits.
    volumen: AtomicU32,
}

impl Sonidos {
    /// Arranca el hilo de audio. El dispositivo se abre en la primera
    /// reproducción, no acá: una máquina sin salida de audio no debe pagar el
    /// error al arrancar la aplicación.
    pub fn lanzar() -> Self {
        let (peticiones, cola) = mpsc::channel::<Tono>();
        let config = Arc::new(Config {
            activos: AtomicBool::new(true),
            volumen: AtomicU32::new(VOLUMEN_POR_DEFECTO.to_bits()),
        });

        let config_hilo = Arc::clone(&config);
        let creado = std::thread::Builder::new()
            .name("mithflow-sonidos".into())
            .spawn(move || reproducir_hasta_que_cierren(cola, config_hilo));
        if let Err(e) = creado {
            eprintln!("no pude crear el hilo de sonidos: {e}; la app sigue muda.");
        }

        Self { peticiones, config }
    }

    /// Aplica los ajustes del usuario. Un volumen fuera de `[0, 1]` se recorta
    /// en vez de rechazarse: viene de un archivo editable a mano.
    pub fn configurar(&self, activos: bool, volumen: f32) {
        self.config.activos.store(activos, Ordering::Relaxed);
        let seguro = if volumen.is_finite() {
            volumen.clamp(0.0, 1.0)
        } else {
            VOLUMEN_POR_DEFECTO
        };
        self.config.volumen.store(seguro.to_bits(), Ordering::Relaxed);
    }

    /// Pide un tono. No bloquea ni falla: si el hilo de audio murió, se ignora.
    pub fn tocar(&self, tono: Tono) {
        if !self.config.activos.load(Ordering::Relaxed) {
            return;
        }
        let _ = self.peticiones.send(tono);
    }
}

/// El bucle del hilo de audio. Termina cuando se sueltan todos los `Sender`,
/// o sea al cerrar la aplicación.
fn reproducir_hasta_que_cierren(cola: mpsc::Receiver<Tono>, config: Arc<Config>) {
    let mut salida: Option<MixerDeviceSink> = None;
    let mut ya_avise = false;

    for tono in cola {
        if salida.is_none() {
            match DeviceSinkBuilder::open_default_sink() {
                Ok(s) => salida = Some(s),
                Err(e) => {
                    // Una sola vez: esto se llamaría en cada dictado.
                    if !ya_avise {
                        eprintln!("sin salida de audio ({e}); los tonos quedan desactivados.");
                        ya_avise = true;
                    }
                    continue;
                }
            }
        }

        let volumen = f32::from_bits(config.volumen.load(Ordering::Relaxed));
        let datos = muestras(tono, volumen);
        // `NonZero` con constantes conocidas: los `expect` documentan que no
        // dependen de nada de tiempo de ejecución.
        let canales = NonZero::new(1u16).expect("1 no es cero");
        let frecuencia = NonZero::new(FRECUENCIA_MUESTREO).expect("44100 no es cero");
        if let Some(s) = salida.as_ref() {
            s.mixer().add(SamplesBuffer::new(canales, frecuencia, datos));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pico(v: &[f32]) -> f32 {
        v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    #[test]
    fn cada_tono_dura_lo_que_dice() {
        for tono in [Tono::Inicio, Tono::Fin, Tono::Pegado, Tono::Error] {
            let v = muestras(tono, VOLUMEN_POR_DEFECTO);
            let segundos = v.len() as f32 / FRECUENCIA_MUESTREO as f32;
            assert!(
                (segundos - tono.duracion_segs()).abs() < 0.001,
                "{tono:?}: esperaba {} s, obtuve {segundos} s",
                tono.duracion_segs()
            );
        }
    }

    /// El chasquido que el fade viene a evitar: la señal tiene que empezar y
    /// terminar prácticamente en cero.
    #[test]
    fn la_envolvente_arranca_y_termina_en_silencio() {
        let v = muestras(Tono::Pegado, VOLUMEN_POR_DEFECTO);
        assert_eq!(v[0], 0.0);
        assert!(
            v.last().unwrap().abs() < 0.001,
            "el final tiene que caer a cero, quedó en {}",
            v.last().unwrap()
        );
        // Y en el medio sí tiene que sonar.
        assert!(pico(&v[v.len() / 3..v.len() * 2 / 3]) > 0.05);
    }

    #[test]
    fn el_volumen_acota_la_amplitud() {
        assert!(pico(&muestras(Tono::Inicio, 0.15)) <= 0.15 + 1e-6);
        assert_eq!(pico(&muestras(Tono::Inicio, 0.0)), 0.0);
    }

    /// Un volumen imposible (viene de un JSON editable a mano) no puede
    /// terminar en un `NaN` yendo a la placa de sonido.
    #[test]
    fn un_volumen_invalido_cae_al_default() {
        let sonidos = Sonidos::lanzar();
        sonidos.configurar(true, f32::NAN);
        let v = f32::from_bits(sonidos.config.volumen.load(Ordering::Relaxed));
        assert_eq!(v, VOLUMEN_POR_DEFECTO);

        sonidos.configurar(true, 9.0);
        let v = f32::from_bits(sonidos.config.volumen.load(Ordering::Relaxed));
        assert_eq!(v, 1.0);
    }

    #[test]
    fn desactivados_no_encolan_nada() {
        let (peticiones, cola) = mpsc::channel::<Tono>();
        let sonidos = Sonidos {
            peticiones,
            config: Arc::new(Config {
                activos: AtomicBool::new(false),
                volumen: AtomicU32::new(VOLUMEN_POR_DEFECTO.to_bits()),
            }),
        };
        sonidos.tocar(Tono::Error);
        assert!(cola.try_recv().is_err());
    }
}
