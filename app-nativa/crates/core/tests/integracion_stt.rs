//! Transcripción contra el modelo real.
//!
//! Todos ignorados: necesitan el GGUF de 1.5 GB, que no está en el repo.
//!
//! Correr con:
//!   cargo test -p mithflow-core --test integracion_stt --release -- --ignored --nocapture

use mithflow_core::{audio, stt::Transcriber};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

const MODELO: &str = "models/whisper-large-v3-turbo-F16.gguf";

/// Rutas relativas a `app-nativa/`, dos niveles arriba de este crate.
fn ruta(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Un solo `Transcriber` para toda la suite. Cargar el modelo cuesta ~1.7 s y
/// 1.5 GB de VRAM: cinco tests en paralelo, cada uno con su copia, agotarían
/// la placa. `Transcriber` es `Send` pero no `Sync`, así que va tras un mutex.
static TRANSCRIBER: LazyLock<Mutex<Transcriber>> = LazyLock::new(|| {
    let modelo = ruta(MODELO);
    let t0 = Instant::now();
    let transcriber = Transcriber::new(&modelo)
        .unwrap_or_else(|e| panic!("no pude crear el Transcriber con {}: {e}", modelo.display()));
    eprintln!("[modelo cargado en {:?}]", t0.elapsed());
    Mutex::new(transcriber)
});

/// Transcribe midiendo, e informa el tiempo y el texto exacto devuelto.
fn transcribir(caso: &str, audio: &[f32]) -> String {
    // `into_inner` en vez de `unwrap`: si un test paniquea, el mutex queda
    // envenenado y los demás fallarían por eso y no por su propia causa.
    let mut t = TRANSCRIBER.lock().unwrap_or_else(|e| e.into_inner());
    let t0 = Instant::now();
    let texto = t.transcribe(audio).expect("falló la transcripción");
    eprintln!(
        "[{caso}] {:.1}s de audio -> {:?} en {:.3}s",
        audio.len() as f32 / 16_000.0,
        texto,
        t0.elapsed().as_secs_f64()
    );
    texto
}

/// Devuelve las muestras crudas intercaladas junto con su formato real.
fn cargar_wav(rel: &str) -> (Vec<f32>, u32, u16) {
    let path = ruta(rel);
    let mut reader = hound::WavReader::open(&path)
        .unwrap_or_else(|e| panic!("no pude abrir {}: {e}", path.display()));
    let spec = reader.spec();
    assert_eq!(
        spec.bits_per_sample, 16,
        "el lector asume PCM de 16 bits, {} tiene {}",
        rel, spec.bits_per_sample
    );
    let crudo: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.expect("muestra corrupta en el WAV") as f32 / 32768.0)
        .collect();
    (crudo, spec.sample_rate, spec.channels)
}

/// El fixture pasado por el mismo pipeline que usa la grabadora.
fn cargar_wav_16k_mono(rel: &str) -> Vec<f32> {
    let (crudo, sample_rate, canales) = cargar_wav(rel);
    let mono = audio::downmix(&crudo, canales);
    audio::resample(&mono, sample_rate, 16_000).expect("falló el resampleo del fixture")
}

/// Ruido de baja amplitud reproducible (LCG de Numerical Recipes). Sin `rand`
/// como dependencia y sin que el test cambie de resultado entre corridas.
fn ruido_determinista(muestras: usize, amplitud: f32) -> Vec<f32> {
    let mut estado: u32 = 0x1234_5678;
    (0..muestras)
        .map(|_| {
            estado = estado.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let unitario = (estado >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            unitario * amplitud
        })
        .collect()
}

/// Palabras comunes: si esto falla, el pipeline está roto de punta a punta.
#[test]
#[ignore = "necesita el modelo de 1.5 GB"]
fn transcribe_voz_sintetica_en_espanol() {
    let audio = cargar_wav_16k_mono("tests/fixtures/speech_es.wav");
    let texto = transcribir("mono 22 kHz", &audio).to_lowercase();
    for palabra in ["dashboard", "cliente", "jueves"] {
        assert!(texto.contains(palabra), "falta {palabra:?} en {texto:?}");
    }
}

/// Separado del anterior a propósito: distingue "el pipeline está roto" de
/// "el modelo escribió MithData con espacio". Es la verificación de que el
/// `initial_prompt` llegó al decodificador — sin él, el spike devolvió
/// "Middata".
#[test]
#[ignore = "necesita el modelo de 1.5 GB"]
fn respeta_el_vocabulario_propio() {
    let audio = cargar_wav_16k_mono("tests/fixtures/speech_es.wav");
    let texto = transcribir("vocabulario propio", &audio);
    // Sin espacios: "Mith Data" también cuenta como acierto del vocabulario.
    let compacto: String = texto
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    for termino in ["mithdata", "crm"] {
        assert!(compacto.contains(termino), "falta {termino:?} en {texto:?}");
    }
}

/// Ejercita `downmix` + `resample` sobre el WAV estéreo de 48 kHz real, que es
/// el formato que entrega la placa de sonido.
#[test]
#[ignore = "necesita el modelo de 1.5 GB"]
fn transcribe_wav_estereo_48khz() {
    let (crudo, sample_rate, canales) = cargar_wav("tests/fixtures/speech_es_48k_stereo.wav");
    assert_eq!(canales, 2, "el fixture debe ser estéreo");
    assert_eq!(sample_rate, 48_000, "el fixture debe ser de 48 kHz");

    let mono = audio::downmix(&crudo, canales);
    let audio_16k = audio::resample(&mono, sample_rate, 16_000).expect("falló el resampleo");

    let texto = transcribir("estéreo 48 kHz", &audio_16k).to_lowercase();
    for palabra in ["dashboard", "cliente", "jueves"] {
        assert!(texto.contains(palabra), "falta {palabra:?} en {texto:?}");
    }
}

/// Criterio de aceptación 9 del spec: sobre silencio no se pega nada.
#[test]
#[ignore = "necesita el modelo de 1.5 GB"]
fn silencio_no_produce_texto() {
    let silencio = vec![0.0f32; 16_000 * 10];
    let texto = transcribir("silencio", &silencio);
    assert!(texto.is_empty(), "el silencio produjo texto: {texto:?}");
}

/// El caso real del criterio 9: el micrófono nunca entrega ceros perfectos,
/// entrega el ruido de fondo de la habitación.
#[test]
#[ignore = "necesita el modelo de 1.5 GB"]
fn ruido_de_fondo_no_produce_texto() {
    let ruido = ruido_determinista(16_000 * 10, 0.005);
    let texto = transcribir("ruido de fondo", &ruido);
    assert!(texto.is_empty(), "el ruido produjo texto: {texto:?}");
}
