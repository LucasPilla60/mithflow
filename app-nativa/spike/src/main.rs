//! Spike de validación: ¿funciona `transcribe-cpp` en esta máquina?
//!
//! Responde tres preguntas del plan:
//!   1. ¿La API real (`Model::load` / `session.run`) compila y transcribe?
//!   2. ¿Qué backend elige en tiempo de ejecución (Vulkan o CPU)?
//!   3. ¿Cuánto tarda comparado con la línea base de Python (0.280 s)?

use std::path::{Path, PathBuf};

fn ruta(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// Carga un WAV y lo deja como el modelo lo espera: mono, 16 kHz, f32 en [-1,1].
fn load_wav_16k_mono(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("no pude abrir el WAV");
    let spec = reader.spec();
    println!(
        "WAV: {} Hz, {} canales, {:?}",
        spec.sample_rate, spec.channels, spec.sample_format
    );

    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
    };
    let mono: Vec<f32> = if spec.channels == 2 {
        raw.chunks(2).map(|c| (c[0] + c[1]) / 2.0).collect()
    } else {
        raw
    };
    // Resampleo lineal: alcanza para el spike; en producción va rubato.
    let ratio = 16000.0 / spec.sample_rate as f32;
    let out_len = (mono.len() as f32 * ratio) as usize;
    (0..out_len)
        .map(|i| mono.get((i as f32 / ratio) as usize).copied().unwrap_or(0.0))
        .collect()
}

fn main() {
    let wav = ruta("tests/fixtures/speech_es.wav");
    let model = ruta("models/whisper-large-v3-turbo-F16.gguf");

    assert!(wav.exists(), "falta el WAV en {}", wav.display());
    assert!(model.exists(), "falta el modelo en {}", model.display());

    let audio = load_wav_16k_mono(&wav);
    println!(
        "Audio: {} muestras ({:.1}s)\n",
        audio.len(),
        audio.len() as f32 / 16000.0
    );

    // Con `dynamic-backends` los backends de ggml son DLLs separadas que hay
    // que cargar ANTES del modelo. Sin esto: "backend error (status 8)".
    // Es la contrapartida de tener un binario que se adapta a cada máquina.
    transcribe_cpp::init_backends_default().expect("no pude inicializar los backends");
    println!("Backends inicializados.");

    let t0 = std::time::Instant::now();
    let modelo = transcribe_cpp::Model::load(&model).expect("no pude cargar el modelo");
    let mut session = modelo.session().expect("no pude crear la sesión");
    println!("Modelo cargado en {:?}", t0.elapsed());

    // Primera pasada para calentar, después se mide.
    let opts = transcribe_cpp::RunOptions::default();
    let _ = session.run(&audio, &opts);

    let mut tiempos = Vec::new();
    let mut texto = String::new();
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let r = session.run(&audio, &opts).expect("falló la transcripción");
        tiempos.push(t.elapsed().as_secs_f64());
        texto = format!("{r:?}");
    }
    tiempos.sort_by(|a, b| a.partial_cmp(b).unwrap());

    println!("\nTranscripción (5 pasadas):");
    println!("  mediana {:.3}s | min {:.3}s | max {:.3}s", tiempos[2], tiempos[0], tiempos[4]);
    println!("  Linea base Python (CUDA): 0.280s");
    println!("\nTEXTO: {texto}");
}
