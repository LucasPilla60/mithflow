# MithFlow nativo — Plan 1: spike de validación y núcleo de dictado

> **Para quien ejecute esto:** SUB-SKILL REQUERIDA: usar `superpowers:subagent-driven-development` (recomendado) o `superpowers:executing-plans` para implementar tarea por tarea. Los pasos usan casillas (`- [ ]`) para seguimiento.

**Objetivo:** Validar los riesgos técnicos del spec y construir un motor de dictado funcional en Rust, ejecutable por línea de comandos, con la misma calidad que la versión Python.

**Arquitectura:** Binario Rust independiente (todavía sin Tauri) con módulos aislados: captura de audio, transcripción, limpieza por reglas, pegado e historial. El módulo de limpieza es una función pura y se construye con TDD usando los 12 casos ya validados en Python. Tauri se suma en el Plan 3, sobre este núcleo probado.

**Stack:** Rust ≥ 1.84, `transcribe-cpp` 0.1.3 (con `whisper-rs` 0.16 como plan B), `cpal` 0.16, `rubato` ~0.16, `enigo` 0.6, `arboard` 3, `hound` 3.5, `serde_json`, `chrono`.

**Alcance:** Fases 0 y 1 del spec. Al terminar hay un `mithflow-cli.exe` que dicta y pega. La detección de hardware, la interfaz y el instalador son los Planes 2, 3 y 4.

**Versión 2 de este plan** — corregida tras una revisión que encontró 37 desvíos, incluidos cuatro que impedían compilar. Las correcciones están anotadas en su contexto.

---

## Estructura de archivos

Todo bajo `D:\MithFlow\app-nativa\`. La versión Python en `D:\MithFlow\` no se toca.

| Archivo | Responsabilidad |
|---|---|
| `crates/core/src/config.rs` | Constantes: vocabulario, muletillas, tartamudeos, límites |
| `crates/core/src/cleanup.rs` | Limpieza por reglas. Función pura, sin estado ni E/S |
| `crates/core/src/history.rs` | Lectura tolerante y escritura de `history.jsonl` |
| `crates/core/src/audio.rs` | Captura con cpal, downmix, resampleo, guardas |
| `crates/core/src/stt.rs` | Transcripción y filtro anti-alucinación |
| `crates/core/src/paste.rs` | Portapapeles y Ctrl+V |
| `crates/core/src/lib.rs` | Reexporta módulos y define el pipeline `dictate()` |
| `crates/cli/src/main.rs` | Binario de prueba: graba con Enter, transcribe y pega |
| `tests/fixtures/speech_es.wav` | Voz sintética mono 16 kHz |
| `tests/fixtures/speech_es_48k_stereo.wav` | La misma voz en estéreo 48 kHz, para ejercitar downmix y resampleo |

---

# Fase 0 — Spike de validación

Responde las preguntas que pueden cambiar el resto del plan. Si alguna falla, el plan cambia.

### Task 0.1: Cadena de herramientas

**Archivos:** ninguno (setup de máquina).

> **Orden corregido:** las herramientas de C++ van **antes** que Rust. `rustup-init` detecta la ausencia del linker MSVC y abre un diálogo interactivo que bajo winget puede colgarse.

- [ ] **Paso 1: Herramientas de compilación de C++ (whisper.cpp se compila desde fuente)**

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-source-agreements --accept-package-agreements --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

- [ ] **Paso 2: CMake**

```powershell
winget install --id Kitware.CMake -e --accept-source-agreements --accept-package-agreements
```

- [ ] **Paso 3: Rust**

`winget` instala **rustup**, el gestor, pero deja la instalación **sin toolchain por defecto**: `rustc` falla con *"could not choose a version of rustc to run"*. Hay que instalarla explícitamente.

```powershell
winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements
# consola nueva, para que tome el PATH
rustup toolchain install stable --profile default
rustup default stable
```

Si `rustc` responde *"Missing manifest in toolchain"*, la instalación quedó a medias (pasó al ejecutar este plan): reinstalarla limpia con
`rustup toolchain uninstall stable; rustup toolchain install stable --profile default`.

- [ ] **Paso 4: Vulkan SDK — obligatorio, no opcional**

Compilar `transcribe-cpp` con la feature `vulkan` requiere el **SDK**, no solo el runtime: CMake busca `Vulkan_LIBRARY`, `Vulkan_INCLUDE_DIR` y el compilador de shaders `glslc`. Sin él la compilación falla con *"Could NOT find Vulkan"* (pasó al ejecutar este plan).

```powershell
winget install --id KhronosGroup.VulkanSDK -e --accept-source-agreements --accept-package-agreements
```

Verificar en una consola nueva: `vulkaninfo --summary` debe listar al menos un dispositivo, y `glslc --version` debe responder. El SDK trae además `vulkaninfo`, que la Task 0.8 usa para caracterizar las notebooks.

- [ ] **Paso 5: CUDA Toolkit — solo en la máquina con GPU NVIDIA**

Necesario para la Task 0.6b. Sin `nvcc`, agregar la feature `cuda` falla al compilar y el resultado se confundiría con "los backends no conviven", que es la respuesta equivocada a la pregunta que decide los criterios 1 y 2 del spec.

```powershell
if (Get-Command nvidia-smi -ErrorAction SilentlyContinue) {
    winget install --id Nvidia.CUDA -e --accept-source-agreements --accept-package-agreements
} else {
    "Sin GPU NVIDIA: saltear la Task 0.6b en esta máquina."
}
```

- [ ] **Paso 6: Verificar en una consola NUEVA (el PATH cambió)**

```powershell
rustc --version; cargo --version; cmake --version | Select-Object -First 1
```

Esperado: Rust **1.84 o superior** (el paso 6 usa un comando agregado en esa versión). Si es menor, correr `rustup update`.

- [ ] **Paso 7: Anotar el target triple**

```powershell
rustc --print host-tuple
```

Esperado: `x86_64-pc-windows-msvc`. Tauri lo necesita en el Plan 4.

> Si Rust es anterior a 1.84, `--print host-tuple` no existe. Alternativa: `rustc -vV | Select-String "^host:"`.

- [ ] **Paso 8: Inicializar el repositorio**

El plan hace commit en cada tarea y hoy `D:\MithFlow` **no es un repositorio git**. El `.gitignore` ya existe y excluye `history.jsonl`, así que el historial de dictados no se versiona.

```powershell
cd D:\MithFlow
git init
git add .gitignore README.md mithflow.py dashboard.py requirements.txt tests/ docs/ instalar.ps1 MithFlow.bat MithFlow-App.vbs MithFlow-invisible.vbs Detener-MithFlow.bat
git status --short
```

Revisar que **`history.jsonl` NO aparezca** en la lista antes de commitear:

```powershell
git commit -m "chore: estado inicial de MithFlow (version Python) + spec y plan de la version nativa"
```

### Task 0.2: Resolver formato de modelo y descargar

**Archivos:**
- Crear: `D:\MithFlow\app-nativa\models\`

> **Corrección importante:** `transcribe-cpp` 0.1.3 carga **GGUF** (`Model::load` está documentado como *"Load a GGUF model from disk"*), no los `.bin` en formato GGML de whisper.cpp. Son formatos distintos. `whisper-rs` sí usa los `.bin` GGML. Descargar el formato equivocado haría fallar el spike por el motivo equivocado.

- [ ] **Paso 1: Crear la carpeta y acelerar las descargas**

```powershell
New-Item -ItemType Directory -Force -Path D:\MithFlow\app-nativa\models
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest es 10-50x más lento con la barra de progreso
```

- [ ] **Paso 2: Descargar los modelos GGML (para whisper-rs, el plan B)**

URLs verificadas el 21/7/2026: las cuatro responden HTTP 200 con estos tamaños.

```powershell
$base = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main"
$dst  = "D:\MithFlow\app-nativa\models"
curl.exe -L -o "$dst\ggml-large-v3-turbo.bin"      "$base/ggml-large-v3-turbo.bin"       # 1549 MB
curl.exe -L -o "$dst\ggml-large-v3-turbo-q5_0.bin" "$base/ggml-large-v3-turbo-q5_0.bin"  #  547 MB
curl.exe -L -o "$dst\ggml-base.bin"                "$base/ggml-base.bin"                 #  141 MB
```

- [ ] **Paso 3: Ubicar el GGUF equivalente (para transcribe-cpp, el camino principal)**

Buscar en el repositorio de Hugging Face de la organización de Handy (`handy-computer`) o en la documentación de `transcribe-cpp` el GGUF de `large-v3-turbo`. **Anotar la URL exacta en `DECISIONES.md`.** Si no existe un GGUF publicado de Whisper, eso por sí solo inclina la decisión hacia `whisper-rs` y hay que anotarlo como tal.

- [ ] **Paso 4: Registrar tamaños y hashes**

El spec §14 exige hashes SHA-256 compilados dentro del binario. El Plan 2 los va a necesitar y este es el momento de obtenerlos.

```powershell
Get-ChildItem D:\MithFlow\app-nativa\models\*.bin | ForEach-Object {
    $h = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
    "{0}  {1} MB  {2}" -f $_.Name, [math]::Round($_.Length/1MB,0), $h
}
```

Verificar que los tamaños coincidan con los de arriba. Un archivo de pocos KB es una página de error HTML, no un modelo. **Copiar la salida completa a `DECISIONES.md`.**

### Task 0.3: Fixtures de audio

**Archivos:**
- Crear: `D:\MithFlow\app-nativa\tests\fixtures\speech_es.wav`
- Crear: `D:\MithFlow\app-nativa\tests\fixtures\speech_es_48k_stereo.wav`

Permiten testear el pipeline sin micrófono ni intervención humana.

- [ ] **Paso 1: Generar el WAV mono — ejecutar como UN SOLO bloque**

La variable `$s` tiene que persistir entre líneas; corriendo línea por línea no funciona.

```powershell
New-Item -ItemType Directory -Force -Path D:\MithFlow\app-nativa\tests\fixtures
Add-Type -AssemblyName System.Speech
$voces = (New-Object System.Speech.Synthesis.SpeechSynthesizer).GetInstalledVoices() |
         ForEach-Object { $_.VoiceInfo } | Where-Object { $_.Culture.Name -like 'es-*' }
if (-not $voces) { throw "No hay ninguna voz en español instalada. Configuración -> Hora e idioma -> Voz." }
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
$s.SelectVoice($voces[0].Name)
$s.SetOutputToWaveFile("D:\MithFlow\app-nativa\tests\fixtures\speech_es.wav")
$s.Speak("Quería comentarte que el dashboard de MithData para el cliente ya está listo. Habría que revisar el CRM y los leads pendientes antes de la reunión del jueves.")
$s.Dispose()
"Voz usada: $($voces[0].Name)"
```

- [ ] **Paso 2: Verificar el mono**

```powershell
$f = Get-Item D:\MithFlow\app-nativa\tests\fixtures\speech_es.wav
"{0} KB" -f [math]::Round($f.Length/1KB,0)
```

Esperado: más de 100 KB.

- [ ] **Paso 3: Generar la versión estéreo a 48 kHz**

El spec §15 la pide explícitamente: es la única forma de ejercitar `downmix` y `resample` sobre un archivo real, y son las dos funciones que el spec marca como "la parte con más diferencias reales" respecto de Python. Las voces *Desktop* de SAPI solo producen mono 16 kHz, así que se convierte con ffmpeg.

```powershell
winget install --id Gyan.FFmpeg -e --accept-source-agreements --accept-package-agreements
# consola nueva para que tome el PATH
ffmpeg -y -i D:\MithFlow\app-nativa\tests\fixtures\speech_es.wav -ar 48000 -ac 2 D:\MithFlow\app-nativa\tests\fixtures\speech_es_48k_stereo.wav
ffprobe -v error -show_entries stream=sample_rate,channels -of csv=p=0 D:\MithFlow\app-nativa\tests\fixtures\speech_es_48k_stereo.wav
```

Esperado: `48000,2`.

### Task 0.4: Spike — ¿funciona `transcribe-cpp`?

**Archivos:**
- Crear: `D:\MithFlow\app-nativa\spike\Cargo.toml`
- Crear: `D:\MithFlow\app-nativa\spike\src\main.rs`

- [ ] **Paso 1: Leer la API real — está publicada**

Abrir **https://docs.rs/transcribe-cpp/0.1.3/transcribe_cpp/** y anotar en `DECISIONES.md` las firmas reales.

> **Corrección:** la versión anterior de este plan decía que la API "no está documentada" y mandaba a correr `cargo doc`, que exige compilar todo whisper.cpp desde C++ (varios minutos) para obtener algo que ya está online. La API verificada es:
>
> ```rust
> let mut session = Model::load("model.gguf")?.session()?;
> let result = session.run(&pcm, &RunOptions::default())?;  // pcm: 16 kHz mono f32 en [-1,1]
> ```
>
> Los tipos son `Model`, `Session`, `RunOptions`, `Transcript`, `Segment`, `Token`, `Word`. También hay una función suelta `transcribe()` de conveniencia. **No existe ningún `Context::new`.**

Confirmar en `RunOptions` / `WhisperRunOptions` qué parámetros están disponibles. Sin los dos primeros no hay paridad con la versión Python:

| Parámetro | Para qué |
|---|---|
| `language` | Fijar español. Sin esto autodetecta y confunde rioplatense con portugués en clips cortos |
| Estrategia de sampling / `beam_size` | Greedy es 28% más rápido con texto idéntico. El default de whisper.cpp **no** es greedy |
| `initial_prompt` | Vocabulario propio (MithData, CRM). Confirmado que existe |
| `temperature` | Reducir alucinación. Confirmado que existe |
| `no_speech_threshold` / `suppress_blank` | Filtro anti-alucinación (criterio 9) |
| VAD | La versión Python usa el VAD de faster-whisper. Ver si el crate lo expone y si necesita un modelo Silero aparte |
| `n_threads` | Rendimiento en CPU (notebooks) |
| `no_context` | Evita la degradación en audio largo |

- [ ] **Paso 2: Crear el proyecto**

```powershell
cd D:\MithFlow\app-nativa
cargo new spike
```

- [ ] **Paso 3: Escribir el `Cargo.toml`**

```toml
[package]
name = "spike"
version = "0.1.0"
edition = "2021"

[dependencies]
hound = "3.5"

[target.'cfg(windows)'.dependencies]
# default-features = false NO es opcional: `metal` es feature por defecto de
# transcribe-cpp y arrastrarla en Windows rompe la compilación.
transcribe-cpp = { version = "0.1.3", default-features = false, features = ["dynamic-backends", "vulkan"] }
```

- [ ] **Paso 4: Escribir el spike**

Las rutas usan `env!("CARGO_MANIFEST_DIR")` para no depender del directorio desde el que se invoque.

```rust
use std::path::{Path, PathBuf};

fn ruta(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// Carga un WAV y lo deja como el modelo lo espera: mono, 16 kHz, f32 en [-1,1].
fn load_wav_16k_mono(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("no pude abrir el WAV");
    let spec = reader.spec();
    println!("WAV: {} Hz, {} canales, {:?}", spec.sample_rate, spec.channels, spec.sample_format);

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
    // AJUSTAR al GGUF ubicado en la Task 0.2 paso 3.
    let model = ruta("models/large-v3-turbo.gguf");

    assert!(wav.exists(), "falta el WAV de prueba en {}", wav.display());
    assert!(model.exists(), "falta el modelo en {}", model.display());

    let audio = load_wav_16k_mono(&wav);
    println!("Audio: {} muestras ({:.1}s)", audio.len(), audio.len() as f32 / 16000.0);

    // AJUSTAR los nombres a lo anotado en el Paso 1. Esta es la forma
    // documentada en docs.rs para la 0.1.3.
    let t0 = std::time::Instant::now();
    let mut session = transcribe_cpp::Model::load(&model)
        .expect("no pude cargar el modelo")
        .session()
        .expect("no pude crear la sesión");
    println!("Modelo cargado en {:?}", t0.elapsed());

    let t1 = std::time::Instant::now();
    let result = session
        .run(&audio, &transcribe_cpp::RunOptions::default())
        .expect("falló la transcripción");
    println!("Transcripción en {:?}", t1.elapsed());
    println!("TEXTO: {result:?}");
}
```

- [ ] **Paso 5: Compilar y ejecutar**

```powershell
cd D:\MithFlow\app-nativa\spike
cargo run --release
```

Esperado: imprime un texto parecido a "Quería comentarte que el dashboard de MithData para el cliente ya está listo…". La primera compilación tarda varios minutos porque compila whisper.cpp desde C++.

- [ ] **Paso 6: Anotar en `DECISIONES.md`**

Crear `D:\MithFlow\app-nativa\DECISIONES.md`:

```markdown
# Decisiones técnicas

## Motor de transcripción
- transcribe-cpp 0.1.3: [FUNCIONA / NO FUNCIONA]
- Formato de modelo: [GGUF / GGML] — URL: [...] — SHA-256: [...]
- API real observada: [firmas de Model, Session, RunOptions]
- Parámetros disponibles: language [sí/no], sampling [...], no_speech_threshold [...], VAD [...]
- Carga del modelo: [X]s | Transcripción de 13s de audio: [X]s
- Backend usado según el log: [Vulkan / CPU]
- DECISIÓN: [transcribe-cpp / whisper-rs]
```

### Task 0.5: Plan B — `whisper-rs` (solo si la Task 0.4 falló)

**Archivos:**
- Modificar: `D:\MithFlow\app-nativa\spike\Cargo.toml` y `src\main.rs`

- [ ] **Paso 1: Cambiar la dependencia**

```toml
[dependencies]
hound = "3.5"
whisper-rs = { version = "0.16", features = ["vulkan"] }
```

Y usar el `.bin` GGML ya descargado: `let model = ruta("models/ggml-large-v3-turbo.bin");`

- [ ] **Paso 2: Reemplazar el bloque de transcripción**

Mantener `ruta` y `load_wav_16k_mono`. Reemplazar desde `let t0 = ...` hasta el final por:

> **Corrección:** `full_get_segment_text` **no existe** en whisper-rs 0.16, y `full_n_segments` devuelve `c_int` directo, no un `Result` (llamarle `.map_err()` es error de compilación). La API correcta es `as_iter()` sobre `WhisperSegment`, cuyo `to_str_lossy()` devuelve `Result<Cow<str>, WhisperError>`.

```rust
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    let t0 = std::time::Instant::now();
    let ctx = WhisperContext::new_with_params(
        model.to_str().unwrap(),
        WhisperContextParameters::default(),
    )
    .expect("no pude cargar el modelo");
    println!("Modelo cargado en {:?}", t0.elapsed());

    let mut state = ctx.create_state().expect("no pude crear el estado");
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("es"));
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);

    let t1 = std::time::Instant::now();
    state.full(params, &audio).expect("falló la transcripción");
    let mut texto = String::new();
    for seg in state.as_iter() {
        if let Ok(t) = seg.to_str_lossy() {
            texto.push_str(t.trim());
            texto.push(' ');
        }
    }
    println!("Transcripción en {:?}", t1.elapsed());
    println!("TEXTO: {}", texto.trim());
```

- [ ] **Paso 3: Ejecutar y anotar**

`cargo run --release` — mismo texto esperado. Actualizar `DECISIONES.md` con el crate adoptado.

### Task 0.6: Spike — ¿el atajo global consume la tecla?

**Archivos:**
- Crear: `D:\MithFlow\app-nativa\spike-hotkey\Cargo.toml` y `src\main.rs`

Riesgo del spec §12: si la tecla llega también a la aplicación enfocada, el cursor se mueve y el texto se pega en otro lado.

- [ ] **Paso 1: Cerrar la versión Python antes de probar**

```powershell
D:\MithFlow\Detener-MithFlow.bat
```

> El spec §16.4 asigna **F9** a la versión nueva justamente porque no hay detección cruzada de instancias. Este spike usa F9 para que, si el motor Python quedara corriendo, no se dispare una grabación real que contamine la conclusión.

- [ ] **Paso 2: Crear el proyecto**

```powershell
cd D:\MithFlow\app-nativa
cargo new spike-hotkey
```

- [ ] **Paso 3: Escribir el `Cargo.toml`**

> **Corrección:** `rdev::grab` está detrás de la feature `unstable_grab`, y el crate no habilita **ninguna** feature por defecto. Con `rdev = "0.5"` a secas, el spike falla con `unresolved import rdev::grab`.

```toml
[package]
name = "spike-hotkey"
version = "0.1.0"
edition = "2021"

[dependencies]
rdev = { version = "0.5", features = ["unstable_grab"] }
```

- [ ] **Paso 4: Escribir el test manual**

```rust
use rdev::{grab, Event, EventType, Key};

fn main() {
    println!("Escuchando F9. Abrí el Bloc de notas, escribí algo, dejá el cursor en medio y apretá F9.");
    println!("Si el cursor NO se mueve y el Bloc de notas no reacciona, la supresión funciona.");
    println!("Ctrl+C para salir.");

    // Devolver None descarta el evento: no llega a la app enfocada.
    if let Err(e) = grab(|event: Event| -> Option<Event> {
        match event.event_type {
            EventType::KeyPress(Key::F9) => {
                println!("F9 capturada y SUPRIMIDA");
                None
            }
            EventType::KeyRelease(Key::F9) => None,
            _ => Some(event),
        }
    }) {
        eprintln!("Error al enganchar el teclado: {e:?}");
    }
}
```

- [ ] **Paso 5: Probar en aplicaciones normales**

`cargo run --release`, y con el cursor en cada una verificar que **no se mueve** y la app no reacciona:

- [ ] Bloc de notas
- [ ] VS Code
- [ ] Chrome, en un campo de texto

- [ ] **Paso 6: Probar en una ventana ELEVADA (se espera que falle)**

Abrir PowerShell **como administrador** y apretar F9 con el foco ahí.

Esperado: **la supresión NO funciona.** Ningún hook de teclado en modo usuario puede suprimir una tecla destinada a un proceso de mayor nivel de integridad. Ya pasa con la versión Python. El objetivo es confirmar la limitación (spec §17), no arreglarla.

- [ ] **Paso 7: Anotar en `DECISIONES.md`**

```markdown
## Atajo global
- rdev grab() suprime en apps normales: [SÍ / NO]
- Suprime en ventana elevada: [se espera NO]
- DECISIÓN: [rdev / tauri-plugin-global-shortcut]
```

Si `rdev` funciona se usa `rdev` y `tauri-plugin-global-shortcut` no se incorpora, igual que Handy. **Fijar el commit con `rev`** en el Plan 3: rdev 0.5.3 es de junio de 2023 y el crate está sin mantenimiento.

### Task 0.6b: Spike — ¿conviven CUDA y Vulkan en un binario?

**Archivos:**
- Modificar: `D:\MithFlow\app-nativa\spike\Cargo.toml`

Decide si se pueden cumplir a la vez el criterio 1 (un solo instalador) y el criterio 2 (latencia en el escritorio). Requiere el CUDA Toolkit de la Task 0.1 paso 4.

- [ ] **Paso 1: Agregar la feature `cuda`**

```toml
transcribe-cpp = { version = "0.1.3", default-features = false, features = ["dynamic-backends", "vulkan", "cuda"] }
```

- [ ] **Paso 2: Compilar y ejecutar**

```powershell
cargo build --release
cargo run --release
```

Esperado: compila y transcribe. Anotar qué backend reporta el log.

**Si falla al compilar, distinguir la causa antes de concluir:** un error que mencione `nvcc`, `CUDA_PATH` o `cudart` significa que falta el toolkit (Task 0.1 paso 4), **no** que los backends sean incompatibles.

- [ ] **Paso 3: Inventariar las DLLs generadas**

```powershell
Get-ChildItem D:\MithFlow\app-nativa\spike\target\release\*.dll |
    Select-Object Name, @{N='MB';E={[math]::Round($_.Length/1MB,1)}} | Format-Table -AutoSize
(Get-ChildItem D:\MithFlow\app-nativa\spike\target\release\*.dll | Measure-Object Length -Sum).Sum /1MB
```

`dynamic-backends` distribuye los backends de ggml como DLLs separadas que hay que empaquetar. El spec estimó 20 MB de instalador sin contarlas: acá sale el número real.

- [ ] **Paso 4: Verificar el fallback sin Vulkan**

> **Corrección:** la versión anterior mandaba a renombrar `C:\Windows\System32\vulkan-1.dll`. Eso es propiedad de TrustedInstaller, falla con acceso denegado aun elevado, y si se fuerza deja sin aceleración a toda la máquina. Se reemplaza por una variable de entorno, que es reversible y no toca el sistema.

```powershell
$env:VK_ICD_FILENAMES = "C:\ruta\inexistente\nada.json"
cargo run --release      # debe funcionar, más lento, reportando CPU
Remove-Item Env:\VK_ICD_FILENAMES
```

Esperado: transcribe igual, usando CPU. Es el caso de una notebook recién formateada sin drivers de video.

- [ ] **Paso 5: Anotar en `DECISIONES.md`**

```markdown
## Backends
- cuda + vulkan + dynamic-backends en un binario: [SÍ / NO / falta nvcc]
- Backend elegido en runtime acá: [...]
- DLLs a empaquetar: [lista] — total [X] MB
- Fallback sin ICD de Vulkan: [OK / rompe]
- DECISIÓN sobre criterios 1 y 2: [un solo instalador / dos builds]
```

### Task 0.7: Línea base reproducible

**Archivos:**
- Crear: `D:\MithFlow\tests\bench_baseline.py`

El spec §16.2 exige re-medir la línea base **antes** de escribir código Rust, midiendo *latencia percibida* y no solo transcripción. La versión Python no tiene forma de transcribir un WAV: su única entrada es el micrófono por F8. Este script provee el harness.

- [ ] **Paso 1: Escribir el script**

```python
"""
Línea base reproducible de la versión Python (spec §16.2).

Mide el pipeline COMPLETO —transcripción, limpieza y pegado— sobre un WAV
fijo, N veces, reportando mediana y p95. La latencia percibida incluye las
esperas de `paste_text`, que las métricas de `history.jsonl` no cuentan.

Uso: .venv\\Scripts\\python.exe tests\\bench_baseline.py <ruta.wav> [N]
"""
import statistics
import sys
import time
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import numpy as np
import mithflow


def cargar_wav_16k(ruta):
    with wave.open(str(ruta)) as w:
        sr, n, ch = w.getframerate(), w.getnframes(), w.getnchannels()
        pcm = np.frombuffer(w.readframes(n), dtype=np.int16).astype("float32") / 32768.0
    if ch == 2:
        pcm = pcm.reshape(-1, 2).mean(axis=1)
    if sr != 16000:
        idx = np.linspace(0, len(pcm), int(len(pcm) * 16000 / sr), endpoint=False)
        pcm = np.interp(idx, np.arange(len(pcm)), pcm).astype("float32")
    return pcm


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    wav = Path(sys.argv[1])
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 20

    audio = cargar_wav_16k(wav)
    dur = len(audio) / 16000
    print(f"Audio: {dur:.1f}s | N={n}")

    modelo = mithflow.load_model()
    mithflow.transcribe(modelo, audio)  # calentar

    solo_stt, percibida = [], []
    for i in range(n):
        t0 = time.perf_counter()
        crudo = mithflow.transcribe(modelo, audio)
        limpio = mithflow.cleanup(crudo)
        t_stt = time.perf_counter() - t0
        mithflow.paste_text(limpio)      # incluye las esperas reales
        t_total = time.perf_counter() - t0
        solo_stt.append(t_stt)
        percibida.append(t_total)
        print(f"  {i+1}/{n}: stt+limpieza {t_stt:.3f}s | percibida {t_total:.3f}s")

    def resumen(nombre, xs):
        xs = sorted(xs)
        p95 = xs[min(len(xs) - 1, int(len(xs) * 0.95))]
        print(f"{nombre}: mediana {statistics.median(xs):.3f}s | p95 {p95:.3f}s | "
              f"min {xs[0]:.3f}s | max {xs[-1]:.3f}s")

    print()
    resumen("Transcripción + limpieza", solo_stt)
    resumen("LATENCIA PERCIBIDA (la vara del criterio 2)", percibida)
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Paso 2: Ejecutar**

```powershell
D:\MithFlow\Detener-MithFlow.bat   # que no compita por la GPU
D:\MithFlow\.venv\Scripts\python.exe D:\MithFlow\tests\bench_baseline.py D:\MithFlow\app-nativa\tests\fixtures\speech_es.wav 20
```

**Ojo:** el script llama a `paste_text`, así que va a pegar el texto 20 veces donde esté el cursor. Poner el foco en un Bloc de notas descartable antes de arrancar.

- [ ] **Paso 3: Anotar en `DECISIONES.md`**

```markdown
## Línea base (versión Python, mismo WAV, N=20)
- Transcripción + limpieza: mediana [X]s, p95 [X]s
- LATENCIA PERCIBIDA: mediana [X]s, p95 [X]s
- El criterio 2 del spec (< 0.9s percibida) se compara contra ESTE número.
```

### Task 0.8: Caracterizar las notebooks

Sin esto, el criterio 3 del spec es una esperanza. En **cada** notebook:

- [ ] **Paso 1: Datos del sistema**

```powershell
Get-CimInstance Win32_ComputerSystem | Select-Object Model, @{N='RAM_GB';E={[math]::Round($_.TotalPhysicalMemory/1GB,1)}}
Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors
Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion
```

> No se usa `AdapterRAM`: es un `UInt32` que satura en 4 GB y en gráficos integrados reporta los ~128 MB reservados. Es exactamente el dato engañoso que llevó al spec §5 a elegir perfilado por benchmark.

- [ ] **Paso 2: ¿Hay Vulkan utilizable?**

```powershell
winget install --id LunarG.VulkanSDK -e --accept-source-agreements --accept-package-agreements
vulkaninfo --summary
```

Esperado: lista al menos un dispositivo. Si dice que no encuentra ICD, esa máquina va a correr en CPU y el criterio 3 hay que medirlo en ese escenario.

- [ ] **Paso 3: Anotar ambas máquinas en `DECISIONES.md` y commitear**

```powershell
cd D:\MithFlow
git add app-nativa/ tests/
git commit -m "spike: validar motor, backends, atajo global y linea base"
```

---

# Fase 1 — Núcleo de dictado

Se arranca por `cleanup`, que es una función pura sin dependencias, para tener el ciclo de TDD andando antes de tocar audio o modelos.

### Task 1.1: Estructura del workspace

**Archivos:**
- Crear: `D:\MithFlow\app-nativa\Cargo.toml`, `crates\core\Cargo.toml`, `crates\core\src\lib.rs`, `.gitignore`

- [ ] **Paso 1: Crear el workspace**

> **Corrección:** la versión anterior listaba `crates/cli` como miembro desde el principio, pero ese crate recién se crea en la Task 1.9. Cargo aborta con *"failed to load manifest for workspace member"* **antes de resolver nada**, así que ningún `cargo check` ni `cargo test` de las Tasks 1.1 a 1.8 habría corrido. Los spikes se excluyen porque quedan dentro del directorio del workspace y si no fallan con *"current package believes it's in a workspace when it's not"*.

`D:\MithFlow\app-nativa\Cargo.toml`:

```toml
[workspace]
members = ["crates/core"]
exclude = ["spike", "spike-hotkey"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
```

- [ ] **Paso 2: Crear el crate `core`**

`crates\core\Cargo.toml`:

```toml
[package]
name = "mithflow-core"
version.workspace = true
edition.workspace = true

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = "0.4"
regex = "1"
once_cell = "1"

[dev-dependencies]
hound = "3.5"
```

- [ ] **Paso 3: Crear el `lib.rs`**

```rust
pub mod cleanup;
pub mod config;
```

- [ ] **Paso 4: Crear el `.gitignore` del subproyecto**

```
target/
models/
```

(`history.jsonl`, `*.log` y `models/` ya están cubiertos por el `.gitignore` raíz.)

- [ ] **Paso 5: Verificar**

```powershell
cd D:\MithFlow\app-nativa
cargo check
```

Esperado: falla con `file not found for module 'cleanup'` y `'config'` — se crean en las tareas siguientes.

### Task 1.2: Configuración

**Archivos:**
- Crear: `crates\core\src\config.rs`

Migrado del bloque CONFIGURACIÓN de `mithflow.py` (constantes `INITIAL_PROMPT`, `FILLERS`, `STUTTER_WORDS`, `LANGUAGE`, `SAMPLE_RATE`, `TONE_GUARD_S`).

- [ ] **Paso 1: Escribir el módulo**

```rust
/// Vocabulario propio: el modelo lo usa como contexto para no inventar palabras.
pub const INITIAL_PROMPT: &str = concat!(
    "Transcripción de dictado en español rioplatense sobre negocios y tecnología. ",
    "Términos frecuentes: MithData, PyME, dashboard, frontend, backend, UX, UI, ",
    "lead, CRM, IA, ciencia de datos, machine learning, API, Canva."
);

/// Idioma del dictado. Fijo a propósito: la autodetección confunde el español
/// rioplatense con portugués en clips cortos y cuesta tiempo en cada dictado.
pub const LANGUAGE: &str = "es";

/// Muletillas que se eliminan cuando quedaron aisladas por comas.
/// Deliberadamente NO incluidas: "bueno", "nada", "a ver" — son muletillas
/// frecuentes pero también arranques legítimos, y sacarlas cambia el tono.
pub const FILLERS: &[&str] = &[
    "eh", "ehh", "em", "mmm", "este", "esto", "o sea", "osea", "digamos", "viste", "tipo",
];

/// Palabras que se tartamudean al dictar ("el el informe"). Lista explícita
/// para no tocar repeticiones intencionales ("muy muy bueno", "no no").
pub const STUTTER_WORDS: &[&str] = &[
    "el", "la", "los", "las", "un", "una", "unos", "unas", "de", "del", "que", "y", "a", "en",
    "con", "por", "para", "se", "lo", "le", "les", "es", "al", "su", "mi", "te", "me",
];

/// Frecuencia de muestreo que espera el modelo.
pub const SAMPLE_RATE: u32 = 16_000;

/// Audio más corto que esto se descarta (ver `process_recording` en mithflow.py).
pub const MIN_AUDIO_SECS: f32 = 0.5;

/// El tono de inicio suena por los parlantes y el micrófono lo capta, sobre
/// todo en notebooks. Se descarta ese tramo del buffer.
pub const TONE_GUARD_SECS: f32 = 0.2;

/// Tope de duración de una grabación (spec §6). El buffer a tasa nativa
/// consume ~384 KB/s (48 kHz, 2 canales, f32): un dictado olvidado de 20
/// minutos serían 460 MB en RAM. Además la transcripción crece más que
/// proporcionalmente con la duración.
pub const MAX_RECORDING_SECS: f32 = 180.0;

/// Frases que el modelo inventa sobre silencio o ruido: residuos de los
/// subtítulos con los que se entrenó. Acá el texto se pega directo en el
/// documento del usuario, así que hay que filtrarlas.
pub const HALLUCINATION_PHRASES: &[&str] = &[
    "gracias por ver el video",
    "gracias por ver el vídeo",
    "suscribete al canal",
    "suscríbete al canal",
    "subtitulos realizados por la comunidad de amara org",
    "subtítulos realizados por la comunidad de amara org",
    "mas informacion en www",
    "más información en www",
    "gracias",
    "muchas gracias",
    "adios",
    "adiós",
];
```

- [ ] **Paso 2: Verificar**

`cargo check -p mithflow-core` — sigue fallando solo por `cleanup`.

### Task 1.3: Limpieza por reglas

**Archivos:**
- Crear: `crates\core\src\cleanup.rs`

Los 12 casos vienen de `D:\MithFlow\tests\test_cleanup.py`, que pasa hoy. Los seis primeros verifican que limpia; los seis últimos, que **no** toca texto legítimo. Dos de estos últimos —`"Bueno el resultado, malo el proceso."` y `"Muy muy bueno el resultado."`— vienen de fallos reales de la primera implementación, que borraba "Bueno," al inicio de frase y colapsaba "muy muy".

- [ ] **Paso 1: Escribir los tests que fallan**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// (entrada, esperado). Si esperado es None, el texto NO debe cambiar.
    const CASES: &[(&str, Option<&str>)] = &[
        // --- Debe limpiar ---
        ("Eh, quería decirte que el dashboard está listo.",
         Some("Quería decirte que el dashboard está listo.")),
        ("Bueno, este, quería mostrarte el CRM.",
         Some("Bueno, quería mostrarte el CRM.")),
        ("El cliente, o sea, pidió el reporte.",
         Some("El cliente, pidió el reporte.")),
        ("Vamos a ver el el dashboard de MithData.",
         Some("Vamos a ver el dashboard de MithData.")),
        ("Hola  ,  qué tal .", Some("Hola, qué tal.")),
        ("el reporte ya está.", Some("El reporte ya está.")),
        // --- NO debe tocar ---
        ("Este dashboard está listo para el cliente.", None),
        ("No quedó nada pendiente.", None),
        ("Bueno el resultado, malo el proceso.", None),
        ("Vamos a ver qué dice la API del CRM.", None),
        ("Ahora me parece que sí está funcionando, aunque en el frontend local el motor no se inicia.", None),
        ("Muy muy bueno el resultado.", None),
    ];

    #[test]
    fn los_doce_casos() {
        let mut fallos = Vec::new();
        for (entrada, esperado) in CASES {
            let objetivo = esperado.unwrap_or(entrada);
            let salida = fast_cleanup(entrada);
            if salida != objetivo {
                fallos.push(format!(
                    "\n  entrada:  {entrada:?}\n  esperado: {objetivo:?}\n  obtenido: {salida:?}"
                ));
            }
        }
        assert!(fallos.is_empty(), "{} caso(s) fallaron:{}", fallos.len(), fallos.join(""));
    }

    #[test]
    fn texto_vacio_no_rompe() {
        assert_eq!(fast_cleanup(""), "");
    }

    #[test]
    fn red_de_seguridad_devuelve_original_si_borra_demasiado() {
        // Puras muletillas: limpiarlo dejaría casi nada, así que se devuelve tal cual.
        let entrada = "Eh, o sea, digamos, viste,";
        assert_eq!(fast_cleanup(entrada), entrada);
    }

    #[test]
    fn tartamudeos_solo_en_palabras_funcionales() {
        assert_eq!(colapsar_tartamudeos("el el informe"), "el informe");
        assert_eq!(colapsar_tartamudeos("de de la casa"), "de la casa");
        // "muy" y "no" no están en STUTTER_WORDS: se respetan
        assert_eq!(colapsar_tartamudeos("muy muy bueno"), "muy muy bueno");
        assert_eq!(colapsar_tartamudeos("no no gracias"), "no no gracias");
    }
}
```

- [ ] **Paso 2: Ejecutar para ver que falla DE VERDAD**

```powershell
cargo test -p mithflow-core cleanup
```

Esperado: **error de compilación** `cannot find function 'fast_cleanup'`.

> Ojo con el TDD acá: `cleanup` ya está declarado en `lib.rs` desde la Task 1.1, así que el módulo se compila y el rojo es real. En las tareas que crean módulos nuevos hay que declararlos en `lib.rs` **antes** de escribir los tests, o cargo reporta "0 tests, ok" con código de salida 0 y el rojo del TDD es un falso verde.

- [ ] **Paso 3: Implementar**

Escribir **arriba** del bloque `#[cfg(test)]`:

```rust
use crate::config::{FILLERS, STUTTER_WORDS};
use once_cell::sync::Lazy;
use regex::Regex;

static RE_ESPACIOS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static RE_ESPACIO_ANTES_PUNTUACION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s+([,.;:!?])").unwrap());
static RE_COMAS_REPETIDAS: Lazy<Regex> = Lazy::new(|| Regex::new(r",\s*(?:,\s*)+").unwrap());
static RE_INICIO_SUCIO: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[\s,]+").unwrap());

/// Muletillas aisladas por comas: ", este," -> ","
static RE_FILLERS_MEDIO: Lazy<Vec<Regex>> = Lazy::new(|| {
    FILLERS
        .iter()
        .map(|f| Regex::new(&format!(r"(?i),\s*{}\s*,", regex::escape(f))).unwrap())
        .collect()
});

/// Muletillas al inicio o después de punto: "Eh, quería" -> "quería"
static RE_FILLERS_INICIO: Lazy<Vec<Regex>> = Lazy::new(|| {
    FILLERS
        .iter()
        .map(|f| Regex::new(&format!(r"(?i)(^|[.!?]\s){}\s*,\s*", regex::escape(f))).unwrap())
        .collect()
});

/// Colapsa repeticiones consecutivas de palabras funcionales: "el el informe"
/// se vuelve "el informe".
///
/// No se puede resolver con una expresión regular como en Python: el crate
/// `regex` de Rust **no soporta retrorreferencias** (`\1`) por diseño, para
/// garantizar tiempo lineal. Recorrer tokens además es más rápido y claro.
fn colapsar_tartamudeos(text: &str) -> String {
    let mut salida: Vec<&str> = Vec::new();
    for token in text.split_whitespace() {
        let repetido = salida
            .last()
            .map(|previo: &&str| {
                previo.eq_ignore_ascii_case(token)
                    && STUTTER_WORDS.iter().any(|w| w.eq_ignore_ascii_case(token))
            })
            .unwrap_or(false);
        if !repetido {
            salida.push(token);
        }
    }
    salida.join(" ")
}

/// Limpieza por reglas: instantánea, sin LLM. Conservadora por diseño — ante
/// la duda deja el texto como está, porque el modelo ya puntúa y capitaliza
/// bien por su cuenta.
pub fn fast_cleanup(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = text.to_string();

    // 1. Muletillas
    for re in RE_FILLERS_MEDIO.iter() {
        out = re.replace_all(&out, ",").into_owned();
    }
    for re in RE_FILLERS_INICIO.iter() {
        out = re.replace_all(&out, "$1").into_owned();
    }

    // 2. Tartamudeos
    out = colapsar_tartamudeos(&out);

    // 3. Espaciado y puntuación
    out = RE_ESPACIOS.replace_all(&out, " ").into_owned();
    out = RE_ESPACIO_ANTES_PUNTUACION.replace_all(&out, "$1").into_owned();
    out = RE_COMAS_REPETIDAS.replace_all(&out, ", ").into_owned();
    out = RE_INICIO_SUCIO.replace_all(&out, "").into_owned();
    out = out.trim().to_string();

    // 4. Mayúscula inicial, sin tocar el resto (puede haber siglas: CRM, API)
    if let Some(primera) = out.chars().next() {
        if primera.is_lowercase() {
            out = primera.to_uppercase().collect::<String>() + &out[primera.len_utf8()..];
        }
    }

    // 5. Red de seguridad: si se comió más de la mitad, algo salió mal.
    //    Comparación en punto flotante para replicar `len(x) * 0.5` de Python;
    //    con división entera el umbral quedaría medio carácter más permisivo.
    if (out.chars().count() as f32) < (text.chars().count() as f32) * 0.5 {
        return text.to_string();
    }
    out
}
```

- [ ] **Paso 4: Ejecutar los tests**

```powershell
cargo test -p mithflow-core cleanup
```

Esperado: PASA los 4. Si `los_doce_casos` falla, el mensaje muestra el caso, lo esperado y lo obtenido.

- [ ] **Paso 5: Commit**

```powershell
cd D:\MithFlow
git add app-nativa/crates/core/
git commit -m "feat(core): limpieza por reglas con los 12 casos migrados de Python"
```

### Task 1.4: Historial compatible

**Archivos:**
- Modificar: `crates\core\src\lib.rs`
- Crear: `crates\core\src\history.rs`

El formato debe ser idéntico al de `save_history` en `mithflow.py`.

> **Verificado sobre el archivo real:** `history.jsonl` tiene **20 entradas**, de las cuales **7 (35%) no tienen el campo `mode`**, porque se agregó después. Un struct con `mode: String` fallaría en más de un tercio del historial.

- [ ] **Paso 1: Declarar el módulo PRIMERO**

En `lib.rs`, para que el rojo del TDD sea real:

```rust
pub mod cleanup;
pub mod config;
pub mod history;
```

- [ ] **Paso 2: Escribir los tests que fallan**

Crear `history.rs` con solo esto:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializa_igual_que_la_version_python() {
        let e = Entry {
            ts: "2026-07-21T12:00:00".into(),
            audio_s: 5.0,
            transcribe_s: 0.3,
            cleanup_s: 0.001,
            words: 7,
            cleaned: true,
            mode: "fast".into(),
            raw: "eh hola".into(),
            final_text: "Hola".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        // El campo se llama "final" en el archivo, aunque en Rust sea reservado
        assert!(json.contains(r#""final":"Hola""#), "json real: {json}");
        assert!(json.contains(r#""ts":"2026-07-21T12:00:00""#));
        assert!(json.contains(r#""mode":"fast""#));
    }

    #[test]
    fn lee_el_formato_actual() {
        let linea = r#"{"ts": "2026-07-21T11:49:40", "audio_s": 6.58, "transcribe_s": 0.51, "cleanup_s": 2.69, "words": 22, "cleaned": true, "mode": "fast", "raw": "crudo", "final": "limpio"}"#;
        let e: Entry = serde_json::from_str(linea).unwrap();
        assert_eq!(e.words, 22);
        assert_eq!(e.final_text, "limpio");
        assert_eq!(e.mode, "fast");
    }

    /// 7 de las 20 entradas reales NO tienen `mode`. Sin `#[serde(default)]`
    /// esto falla y se pierde el 35% del historial.
    #[test]
    fn lee_entradas_viejas_sin_campo_mode() {
        let linea = r#"{"ts": "2026-07-21T10:15:00", "audio_s": 9.5, "transcribe_s": 0.5, "cleanup_s": 1.2, "words": 18, "cleaned": true, "raw": "crudo", "final": "limpio"}"#;
        let e: Entry = serde_json::from_str(linea).expect("debe leer entradas sin `mode`");
        assert_eq!(e.words, 18);
        assert_eq!(e.mode, "llm", "las entradas viejas son de la época del LLM");
    }

    #[test]
    fn saltea_lineas_invalidas_sin_abortar() {
        let dir = std::env::temp_dir().join(format!("mithflow_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("h.jsonl");
        std::fs::write(
            &path,
            "{\"ts\":\"2026-07-21T10:00:00\",\"final\":\"uno\"}\n\
             esto no es json\n\
             \n\
             {\"ts\":\"2026-07-21T10:01:00\",\"final\":\"dos\"}\n",
        )
        .unwrap();
        let entries = load(&path).unwrap();
        assert_eq!(entries.len(), 2, "debe leer las 2 válidas e ignorar la basura");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// El historial real del proyecto debe leerse completo (spec, criterio 7).
    /// Se ignora por defecto porque depende de un archivo fuera del crate.
    #[test]
    #[ignore]
    fn lee_el_historial_real_del_proyecto() {
        let path = std::path::Path::new(r"D:\MithFlow\history.jsonl");
        if !path.exists() {
            eprintln!("no está el historial real, se saltea");
            return;
        }
        let entries = load(path).expect("debe poder leerse");
        let lineas = std::fs::read_to_string(path).unwrap().lines()
            .filter(|l| !l.trim().is_empty()).count();
        assert_eq!(entries.len(), lineas, "se perdieron entradas al parsear");
    }
}
```

- [ ] **Paso 3: Ejecutar para ver que falla**

```powershell
cargo test -p mithflow-core history
```

Esperado: **error de compilación** `cannot find type 'Entry'`.

- [ ] **Paso 4: Implementar**

Escribir arriba del bloque de tests:

```rust
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// Valor por defecto de `mode` para las entradas anteriores al 21/7/2026,
/// que son de la época en que la limpieza la hacía un LLM.
fn modo_historico() -> String {
    "llm".to_string()
}

/// Una entrada del historial, con el mismo formato que `save_history` de la
/// versión Python.
///
/// Todos los campos salvo `ts` y `final` llevan `#[serde(default)]`: el
/// historial real tiene entradas de varias épocas del programa.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub ts: String,
    #[serde(default)]
    pub audio_s: f32,
    #[serde(default)]
    pub transcribe_s: f32,
    #[serde(default)]
    pub cleanup_s: f32,
    #[serde(default)]
    pub words: usize,
    #[serde(default)]
    pub cleaned: bool,
    #[serde(default = "modo_historico")]
    pub mode: String,
    #[serde(default)]
    pub raw: String,
    /// `final` es palabra reservada en Rust; en el archivo se llama "final".
    #[serde(rename = "final")]
    pub final_text: String,
}

/// Agrega una entrada. Un fallo acá no debe invalidar el dictado: quien llame
/// registra el error, pero el texto ya fue pegado.
pub fn append(path: &Path, entry: &Entry) -> std::io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(entry)?)
}

/// Lee todas las entradas, salteando líneas corruptas en vez de fallar.
pub fn load(path: &Path) -> std::io::Result<Vec<Entry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = std::fs::File::open(path)?;
    Ok(BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect())
}
```

- [ ] **Paso 5: Ejecutar**

```powershell
cargo test -p mithflow-core history
cargo test -p mithflow-core history -- --ignored --nocapture
```

Esperado: los 4 primeros pasan; el de historial real también, leyendo las 20 entradas.

- [ ] **Paso 6: Commit**

```powershell
cd D:\MithFlow
git add app-nativa/crates/core/
git commit -m "feat(core): historial tolerante a entradas de versiones anteriores"
```

### Task 1.5: Captura de audio

**Archivos:**
- Modificar: `crates\core\src\lib.rs` y `crates\core\Cargo.toml`
- Crear: `crates\core\src\audio.rs`

Diferencia central con Python: allá se le pide 16 kHz y 1 canal al driver y WASAPI convierte solo. Con `cpal` se obtiene la configuración **nativa** del dispositivo y hay que convertir a mano, en este orden: downmix y después resampleo.

- [ ] **Paso 1: Declarar el módulo y agregar dependencias**

En `lib.rs` agregar `pub mod audio;`. En `Cargo.toml`, bajo `[dependencies]`:

```toml
cpal = "0.16"
# Fijar la serie 0.16: en versiones posteriores la API cambia por completo
# (`SincFixedIn` pasa a ser `Async::new_sinc` y `f_cutoff` a `Option<f32>`).
rubato = "~0.16"
```

- [ ] **Paso 2: Escribir los tests de las transformaciones puras**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_estereo_promedia_canales() {
        let estereo = vec![1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(downmix(&estereo, 2), vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn downmix_mono_no_cambia_nada() {
        let mono = vec![0.1, 0.2, 0.3];
        assert_eq!(downmix(&mono, 1), mono);
    }

    #[test]
    fn resample_ajusta_la_cantidad_de_muestras() {
        // 48 kHz -> 16 kHz da aproximadamente un tercio. La tolerancia no es
        // de ±1: un resampler sinc tiene retardo de filtro (sinc_len/2 * ratio
        // ≈ 42 muestras acá), así que la primera pasada devuelve algo menos.
        // Lo que este test detecta es un error de FACTOR, no de bordes.
        let entrada = vec![0.0f32; 48_000];
        let salida = resample(&entrada, 48_000, 16_000).unwrap();
        let error_relativo = (salida.len() as f32 - 16_000.0).abs() / 16_000.0;
        assert!(
            error_relativo < 0.01,
            "esperaba ~16000 muestras (±1%), obtuve {}",
            salida.len()
        );
    }

    #[test]
    fn resample_a_la_misma_frecuencia_es_identidad() {
        let entrada = vec![0.1, 0.2, 0.3];
        assert_eq!(resample(&entrada, 16_000, 16_000).unwrap(), entrada);
    }

    /// Un tono puro debe seguir siendo un tono puro después del resampleo:
    /// verifica que no haya aliasing ni pérdida de energía.
    #[test]
    fn resample_preserva_la_energia_de_una_senoide() {
        let f = 440.0;
        let entrada: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / 48_000.0).sin())
            .collect();
        let salida = resample(&entrada, 48_000, 16_000).unwrap();
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let (a, b) = (rms(&entrada), rms(&salida));
        assert!((a - b).abs() / a < 0.1, "RMS cambió demasiado: {a} -> {b}");
    }
}
```

- [ ] **Paso 3: Ejecutar para ver que falla**

```powershell
cargo test -p mithflow-core audio
```

Esperado: **error de compilación** `cannot find function 'downmix'`.

- [ ] **Paso 4: Implementar las transformaciones puras**

```rust
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// Promedia los canales para obtener mono. `cpal` entrega las muestras
/// intercaladas; pasar un buffer estéreo como si fuera mono produce audio al
/// doble de velocidad y transcripción basura. El HyperX QuadCast y los arrays
/// de micrófono de notebook reportan 2 canales.
pub fn downmix(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples
        .chunks(channels as usize)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect()
}

/// Resampleo a la frecuencia que espera el modelo.
///
/// Devuelve `Result` en vez de paniquear: el criterio de aceptación 6 del spec
/// dice que ningún error de audio puede cerrar la aplicación.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Result<Vec<f32>, String> {
    if from == to || samples.is_empty() {
        return Ok(samples.to_vec());
    }
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler =
        SincFixedIn::<f32>::new(to as f64 / from as f64, 2.0, params, samples.len(), 1)
            .map_err(|e| format!("no pude crear el resampler: {e}"))?;
    let out = resampler
        .process(&[samples.to_vec()], None)
        .map_err(|e| format!("falló el resampleo: {e}"))?;
    Ok(out.into_iter().next().unwrap_or_default())
}
```

- [ ] **Paso 5: Ejecutar los tests**

```powershell
cargo test -p mithflow-core audio
```

Esperado: PASA los 5.

- [ ] **Paso 6: Implementar la grabadora**

Agregar arriba del bloque de tests:

```rust
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::sync::{Arc, Mutex};

/// Grabadora que acumula audio en memoria mientras está activa.
///
/// El dispositivo se abre al empezar a grabar y se cierra al terminar. La
/// versión Python lo dejaba abierto siempre, lo que mantiene encendido el
/// indicador de micrófono de Windows y consume batería en las notebooks.
pub struct Recorder {
    buffer: Arc<Mutex<Vec<f32>>>,
    stream: Option<cpal::Stream>,
    sample_rate: u32,
    channels: u16,
}

impl Recorder {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            sample_rate: crate::config::SAMPLE_RATE,
            channels: 1,
        })
    }

    pub fn start(&mut self) -> Result<(), String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or("no hay dispositivo de entrada (¿micrófono conectado?)")?;
        let cfg = device
            .default_input_config()
            .map_err(|e| format!("no pude leer la configuración del micrófono: {e}"))?;
        self.sample_rate = cfg.sample_rate().0;
        self.channels = cfg.channels();
        self.buffer.lock().map_err(|_| "buffer envenenado")?.clear();

        let buf = Arc::clone(&self.buffer);
        let err_cb = |e| eprintln!("error de captura: {e}");
        let stream_cfg = cfg.config();

        // El formato nativo no siempre es f32: construir un stream f32 sobre un
        // dispositivo que entrega i16 falla con StreamConfigNotSupported, o
        // paniquea al interpretar el buffer.
        let stream = match cfg.sample_format() {
            SampleFormat::F32 => device.build_input_stream(
                &stream_cfg,
                move |data: &[f32], _: &_| {
                    if let Ok(mut b) = buf.lock() {
                        b.extend_from_slice(data);
                    }
                },
                err_cb,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                &stream_cfg,
                move |data: &[i16], _: &_| {
                    if let Ok(mut b) = buf.lock() {
                        b.extend(data.iter().map(|s| *s as f32 / 32768.0));
                    }
                },
                err_cb,
                None,
            ),
            SampleFormat::U16 => device.build_input_stream(
                &stream_cfg,
                move |data: &[u16], _: &_| {
                    if let Ok(mut b) = buf.lock() {
                        b.extend(data.iter().map(|s| (*s as f32 - 32768.0) / 32768.0));
                    }
                },
                err_cb,
                None,
            ),
            otro => return Err(format!("formato de audio no soportado: {otro:?}")),
        }
        .map_err(|e| format!("no pude abrir el stream: {e}"))?;

        stream.play().map_err(|e| e.to_string())?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Detiene la captura, cierra el micrófono y devuelve el audio listo para
    /// el modelo: mono, 16 kHz, f32.
    pub fn stop(&mut self) -> Result<Vec<f32>, String> {
        self.stream.take(); // al soltarlo se cierra el dispositivo
        let raw = self.buffer.lock().map_err(|_| "buffer envenenado")?.clone();
        let mono = downmix(&raw, self.channels);
        let mut audio = resample(&mono, self.sample_rate, crate::config::SAMPLE_RATE)?;

        // Descartar el tramo donde suena el tono de inicio: el micrófono lo
        // capta por los parlantes y el modelo alucina más sobre ese ruido.
        let guarda = (crate::config::TONE_GUARD_SECS * crate::config::SAMPLE_RATE as f32) as usize;
        if audio.len() > guarda {
            audio.drain(..guarda);
        }

        // Tope de duración: recortar en vez de rechazar, para no perder lo ya dicho.
        let maximo =
            (crate::config::MAX_RECORDING_SECS * crate::config::SAMPLE_RATE as f32) as usize;
        if audio.len() > maximo {
            audio.truncate(maximo);
        }
        Ok(audio)
    }

    /// Duración capturada hasta el momento, para avisar cuando se acerca al tope.
    pub fn elapsed_secs(&self) -> f32 {
        let n = self.buffer.lock().map(|b| b.len()).unwrap_or(0);
        if self.channels == 0 || self.sample_rate == 0 {
            return 0.0;
        }
        n as f32 / self.channels as f32 / self.sample_rate as f32
    }
}
```

- [ ] **Paso 7: Verificar que `Stream` se puede mover entre hilos**

`cpal::Stream` es `Send` en Windows, pero cpal lo rompió y lo re-arregló entre versiones. Agregar al final de `audio.rs`:

```rust
#[cfg(test)]
const _: fn() = || {
    fn assert_send<T: Send>() {}
    let _ = assert_send::<cpal::Stream>;
};
```

- [ ] **Paso 8: Ejecutar todo y commitear**

```powershell
cargo test -p mithflow-core
cd D:\MithFlow
git add app-nativa/crates/core/
git commit -m "feat(core): captura con downmix, resampleo, guarda de tono y tope de duracion"
```

### Task 1.6: Transcripción y filtro anti-alucinación

**Archivos:**
- Modificar: `crates\core\src\lib.rs` y `crates\core\Cargo.toml`
- Crear: `crates\core\src\stt.rs`
- Crear: `crates\core\tests\integracion_stt.rs`

**Usar el crate que haya quedado en `DECISIONES.md`.** El código de abajo es para `whisper-rs`; si ganó `transcribe-cpp`, adaptar los nombres a `Model::load` / `session.run` manteniendo **los mismos parámetros**, que son los que sostienen los criterios 2 y 9.

- [ ] **Paso 1: Declarar el módulo y agregar la dependencia**

En `lib.rs` agregar `pub mod stt;`. En `Cargo.toml`, bajo `[dependencies]`:

```toml
whisper-rs = { version = "0.16", features = ["vulkan"] }
```

(`hound` ya está en `[dev-dependencies]` desde la Task 1.1: **no agregar un segundo bloque `[dev-dependencies]`**, es un error de TOML por clave duplicada.)

- [ ] **Paso 2: Escribir los tests unitarios del filtro**

En `stt.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtra_alucinaciones_conocidas() {
        assert_eq!(filtrar_alucinacion("Gracias por ver el video."), "");
        assert_eq!(filtrar_alucinacion("¡Muchas gracias!"), "");
        assert_eq!(filtrar_alucinacion("   "), "");
    }

    #[test]
    fn no_filtra_texto_legitimo_que_contiene_esas_palabras() {
        let real = "Gracias por el reporte, lo reviso mañana.";
        assert_eq!(filtrar_alucinacion(real), real);
        let otro = "Muchas gracias por mandarme el dashboard.";
        assert_eq!(filtrar_alucinacion(otro), otro);
    }
}
```

- [ ] **Paso 3: Ejecutar para ver que falla**

```powershell
cargo test -p mithflow-core stt
```

Esperado: **error de compilación** `cannot find function 'filtrar_alucinacion'`.

- [ ] **Paso 4: Implementar**

```rust
use crate::config::{HALLUCINATION_PHRASES, INITIAL_PROMPT, LANGUAGE};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn normalizar(texto: &str) -> String {
    texto
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Devuelve cadena vacía si la transcripción ENTERA es una alucinación
/// conocida. Compara el texto completo, no por contención, para no borrar un
/// "gracias" legítimo en medio de un dictado.
pub fn filtrar_alucinacion(texto: &str) -> String {
    let norm = normalizar(texto);
    if norm.is_empty() || HALLUCINATION_PHRASES.contains(&norm.as_str()) {
        return String::new();
    }
    texto.trim().to_string()
}

pub struct Transcriber {
    ctx: WhisperContext,
}

impl Transcriber {
    pub fn new(model_path: &Path) -> Result<Self, String> {
        if !model_path.exists() {
            return Err(format!("no encuentro el modelo en {}", model_path.display()));
        }
        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or("ruta de modelo inválida")?,
            WhisperContextParameters::default(),
        )
        .map_err(|e| format!("no pude cargar el modelo: {e}"))?;
        Ok(Self { ctx })
    }

    pub fn transcribe(&self, audio: &[f32]) -> Result<String, String> {
        let mut state = self.ctx.create_state().map_err(|e| e.to_string())?;

        // Greedy con best_of 1 equivale a beam_size=1 en Python, medido 28% más
        // rápido con transcripción idéntica. El default de whisper.cpp NO es
        // greedy: sin esta línea la latencia sube ~30% y se pierde el criterio 2.
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        params.set_language(Some(LANGUAGE));
        params.set_initial_prompt(INITIAL_PROMPT);

        // Anti-alucinación (criterio 9). La versión Python está cubierta por el
        // VAD de faster-whisper; al cambiar de motor hay que reconstruirlo.
        params.set_no_speech_thold(0.6);
        params.set_suppress_blank(true);

        // Sin encadenar contexto entre ventanas de 30 s: medido en Python, 120 s
        // de audio bajan de 4.92 s a 2.25 s con salida casi idéntica.
        params.set_no_context(true);

        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state.full(params, audio).map_err(|e| format!("falló la transcripción: {e}"))?;

        // `full_get_segment_text` no existe en whisper-rs 0.16 y
        // `full_n_segments` devuelve c_int, no Result. La API es `as_iter()`.
        let mut texto = String::new();
        for seg in state.as_iter() {
            if let Ok(t) = seg.to_str_lossy() {
                texto.push_str(t.trim());
                texto.push(' ');
            }
        }
        Ok(filtrar_alucinacion(&texto))
    }
}
```

> **Nota sobre memoria:** `set_initial_prompt` y `set_language` hacen `CString::into_raw()` internamente, o sea que filtran unos cientos de bytes por llamada. Con un dictado por minuto son ~0.4 MB por día. Aceptable para el Plan 1; si molesta, en el Plan 3 se construye el `FullParams` una vez.

- [ ] **Paso 5: Ejecutar los tests unitarios**

```powershell
cargo test -p mithflow-core stt
```

Esperado: PASA los 2.

- [ ] **Paso 6: Escribir los tests de integración**

`crates\core\tests\integracion_stt.rs`:

```rust
use mithflow_core::{audio, stt::Transcriber};
use std::path::{Path, PathBuf};

fn raiz() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn modelo() -> PathBuf {
    raiz().join("models/ggml-large-v3-turbo.bin")
}

fn cargar_wav(nombre: &str) -> Vec<f32> {
    let path = raiz().join("tests/fixtures").join(nombre);
    let mut r = hound::WavReader::open(&path)
        .unwrap_or_else(|e| panic!("no pude abrir {}: {e}", path.display()));
    let spec = r.spec();
    let raw: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
    let mono = audio::downmix(&raw, spec.channels);
    audio::resample(&mono, spec.sample_rate, 16_000).unwrap()
}

/// Pipeline completo sobre voz sintética. Palabras comunes: si esto falla,
/// el pipeline está roto.
#[test]
#[ignore]
fn transcribe_voz_sintetica_en_espanol() {
    let audio = cargar_wav("speech_es.wav");
    let t = Transcriber::new(&modelo()).expect("no pude cargar el modelo");
    let texto = t.transcribe(&audio).expect("falló la transcripción");
    let bajo = texto.to_lowercase();
    for esperada in ["dashboard", "cliente", "jueves"] {
        assert!(bajo.contains(esperada), "falta {esperada:?} en: {texto}");
    }
}

/// El vocabulario propio va aparte: que el modelo escriba "MithData" pegado a
/// partir de voz sintética es más frágil que el pipeline en sí, y conviene
/// distinguir "está roto" de "escribió Mith Data".
#[test]
#[ignore]
fn respeta_el_vocabulario_propio() {
    let audio = cargar_wav("speech_es.wav");
    let t = Transcriber::new(&modelo()).expect("no pude cargar el modelo");
    let texto = t.transcribe(&audio).expect("falló la transcripción");
    let sin_espacios = texto.to_lowercase().replace(' ', "");
    assert!(sin_espacios.contains("mithdata"), "no reconoció MithData en: {texto}");
    assert!(sin_espacios.contains("crm"), "no reconoció CRM en: {texto}");
}

/// Ejercita downmix y resampleo sobre un archivo real (spec §15).
#[test]
#[ignore]
fn transcribe_wav_estereo_48khz() {
    let audio = cargar_wav("speech_es_48k_stereo.wav");
    let t = Transcriber::new(&modelo()).expect("no pude cargar el modelo");
    let texto = t.transcribe(&audio).expect("falló la transcripción");
    assert!(
        texto.to_lowercase().contains("dashboard"),
        "el downmix o el resampleo rompieron el audio: {texto}"
    );
}

/// Criterio de aceptación 9: silencio no produce texto.
#[test]
#[ignore]
fn silencio_no_produce_texto() {
    let silencio = vec![0.0f32; 16_000 * 10];
    let t = Transcriber::new(&modelo()).expect("no pude cargar el modelo");
    let texto = t.transcribe(&silencio).expect("falló la transcripción");
    assert!(texto.trim().is_empty(), "ALUCINACIÓN sobre silencio: {texto:?}");
}

/// Ruido de fondo suave tampoco.
#[test]
#[ignore]
fn ruido_de_fondo_no_produce_texto() {
    let ruido: Vec<f32> = (0..16_000 * 8).map(|i| (i as f32 * 0.7).sin() * 0.002).collect();
    let t = Transcriber::new(&modelo()).expect("no pude cargar el modelo");
    let texto = t.transcribe(&ruido).expect("falló la transcripción");
    assert!(texto.trim().is_empty(), "ALUCINACIÓN sobre ruido: {texto:?}");
}
```

- [ ] **Paso 7: Ejecutar los tests de integración**

```powershell
cargo test -p mithflow-core --test integracion_stt --release -- --ignored --nocapture
```

Esperado: pasan los 5. Anotar el tiempo de `transcribe_voz_sintetica_en_espanol`.

**Si fallan los de silencio o ruido**, el filtro no alcanza. En ese orden: subir `no_speech_thold` a 0.8, agregar la frase que devolvió a `HALLUCINATION_PHRASES`, y si el modelo devuelve el propio `initial_prompt`, agregar una comparación contra él en `filtrar_alucinacion`. **No borrar el test.**

- [ ] **Paso 8: Commit**

```powershell
cd D:\MithFlow
git add app-nativa/crates/core/
git commit -m "feat(core): transcripcion con filtro anti-alucinacion y tests de integracion"
```

### Task 1.7: Pegado

**Archivos:**
- Modificar: `crates\core\src\lib.rs` y `crates\core\Cargo.toml`
- Crear: `crates\core\src\paste.rs`

Mejora deliberada sobre Python, ya corregida allá y verificada por `D:\MithFlow\tests\test_paste.py`: si el pegado falla, el texto dictado **queda en el portapapeles** en vez de perderse.

- [ ] **Paso 1: Declarar el módulo y agregar dependencias**

En `lib.rs` agregar `pub mod paste;`.

En `Cargo.toml`, agregar estas dos líneas **dentro del `[dependencies]` que ya existe**:

```toml
arboard = "3"
enigo = "0.6"
```

Y agregar esta tabla nueva **al final del archivo**, después de `[dev-dependencies]`:

```toml
[target.'cfg(windows)'.dependencies]
windows = { version = "0.58", features = ["Win32_System_DataExchange"] }
```

`GetClipboardSequenceNumber` vive en ese módulo y es `unsafe`, por eso la llamada va envuelta en un bloque `unsafe`.

- [ ] **Paso 2: Implementar**

> **Por qué no hay `sleep` fijos:** el spec §2.1 identifica reemplazarlos como una de las dos únicas fuentes de mejora para compensar lo que Vulkan pierde contra CUDA. Python gasta 450 ms fijos, sobre un presupuesto total de 900 ms. Acá se espera a que el portapapeles confirme el cambio.

```rust
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::{thread::sleep, time::{Duration, Instant}};

#[cfg(windows)]
fn secuencia_portapapeles() -> u32 {
    unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}
#[cfg(not(windows))]
fn secuencia_portapapeles() -> u32 { 0 }

/// Espera a que el portapapeles refleje el cambio, con tope. Sustituye al
/// `sleep(150 ms)` fijo de la versión Python.
fn esperar_portapapeles(anterior: u32, tope: Duration) {
    let t0 = Instant::now();
    while t0.elapsed() < tope {
        if secuencia_portapapeles() != anterior {
            return;
        }
        sleep(Duration::from_millis(5));
    }
}

/// Pega el texto donde esté el cursor, preservando el portapapeles previo.
///
/// Si el pegado falla, el texto queda en el portapapeles a propósito: es
/// preferible que el usuario pegue a mano antes que perder lo que dictó.
pub fn paste(text: &str) -> Result<(), String> {
    let mut clip = arboard::Clipboard::new().map_err(|e| format!("sin portapapeles: {e}"))?;
    let anterior_texto = clip.get_text().ok();
    let seq = secuencia_portapapeles();

    clip.set_text(text).map_err(|e| format!("no pude copiar: {e}"))?;
    esperar_portapapeles(seq, Duration::from_millis(300));

    match enviar_ctrl_v() {
        Ok(()) => {
            // Dar tiempo a que la app destino procese el Ctrl+V antes de
            // restaurar. 120 ms alcanza en la práctica; restaurar es
            // best-effort y su fallo no invalida el dictado.
            sleep(Duration::from_millis(120));
            if let Some(prev) = anterior_texto {
                let _ = clip.set_text(prev);
            }
            Ok(())
        }
        Err(e) => Err(format!("{e} — el texto quedó en el portapapeles")),
    }
}

fn enviar_ctrl_v() -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.key(Key::Control, Direction::Press).map_err(|e| e.to_string())?;
    enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string())?;
    enigo.key(Key::Control, Direction::Release).map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Paso 3: Compilar**

```powershell
cargo build -p mithflow-core
```

No hay test automatizado: pegar requiere una ventana con foco. Se valida a mano en la Task 1.9.

- [ ] **Paso 4: Commit**

```powershell
cd D:\MithFlow
git add app-nativa/crates/core/
git commit -m "feat(core): pegado sin esperas fijas que preserva el texto si falla"
```

### Task 1.8: Pipeline completo

**Archivos:**
- Modificar: `crates\core\src\lib.rs`

- [ ] **Paso 1: Escribir el pipeline**

```rust
pub mod audio;
pub mod cleanup;
pub mod config;
pub mod history;
pub mod paste;
pub mod stt;

use std::path::Path;
use std::time::Instant;

/// Resultado de un dictado, con los tiempos medidos.
#[derive(Debug)]
pub struct DictationResult {
    pub raw: String,
    pub final_text: String,
    pub audio_s: f32,
    pub transcribe_s: f32,
    pub cleanup_s: f32,
}

/// Toma el audio ya capturado (mono, 16 kHz), lo transcribe, lo limpia, lo
/// pega y lo registra.
///
/// Devuelve `Ok(None)` cuando no hay nada que pegar: audio demasiado corto,
/// silencio, o solo alucinación. Quien llame decide qué realimentación dar.
pub fn dictate(
    transcriber: &stt::Transcriber,
    audio: &[f32],
    history_path: &Path,
) -> Result<Option<DictationResult>, String> {
    let audio_s = audio.len() as f32 / config::SAMPLE_RATE as f32;
    if audio_s < config::MIN_AUDIO_SECS {
        return Ok(None);
    }

    let t0 = Instant::now();
    let raw = transcriber.transcribe(audio)?; // ya filtra alucinaciones
    let transcribe_s = t0.elapsed().as_secs_f32();
    if raw.trim().is_empty() {
        return Ok(None);
    }

    let t1 = Instant::now();
    let final_text = cleanup::fast_cleanup(&raw);
    let cleanup_s = t1.elapsed().as_secs_f32();

    // Se pega primero: es lo que el usuario está esperando.
    let paste_result = paste::paste(&final_text);

    let entry = history::Entry {
        ts: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        audio_s: (audio_s * 100.0).round() / 100.0,
        transcribe_s: (transcribe_s * 100.0).round() / 100.0,
        cleanup_s: (cleanup_s * 100.0).round() / 100.0,
        words: final_text.split_whitespace().count(),
        cleaned: final_text != raw,
        mode: "fast".to_string(),
        raw: raw.clone(),
        final_text: final_text.clone(),
    };
    // Un fallo al guardar el historial no invalida el dictado.
    if let Err(e) = history::append(history_path, &entry) {
        eprintln!("no pude guardar el historial: {e}");
    }

    paste_result?;
    Ok(Some(DictationResult { raw, final_text, audio_s, transcribe_s, cleanup_s }))
}
```

- [ ] **Paso 2: Compilar, testear y commitear**

```powershell
cargo test -p mithflow-core
cd D:\MithFlow
git add app-nativa/crates/core/src/lib.rs
git commit -m "feat(core): pipeline de dictado punta a punta"
```

### Task 1.9: Binario de validación manual

**Archivos:**
- Modificar: `D:\MithFlow\app-nativa\Cargo.toml`
- Crear: `crates\cli\Cargo.toml` y `crates\cli\src\main.rs`

Valida el núcleo con voz real antes de sumar Tauri. Usa Enter en vez de una tecla global: el atajo llega en el Plan 3.

- [ ] **Paso 1: Agregar el crate al workspace**

En `D:\MithFlow\app-nativa\Cargo.toml`:

```toml
members = ["crates/core", "crates/cli"]
```

- [ ] **Paso 2: Crear el crate**

`crates\cli\Cargo.toml`:

```toml
[package]
name = "mithflow-cli"
version.workspace = true
edition.workspace = true

[dependencies]
mithflow-core = { path = "../core" }
```

- [ ] **Paso 3: Escribir el binario**

Sin `.expect()` sobre errores recuperables: el criterio 6 del spec dice que ningún fallo de micrófono cierra la aplicación.

```rust
use mithflow_core::{audio::Recorder, dictate, stt::Transcriber};
use std::io::{stdin, stdout, Write};
use std::path::PathBuf;

fn main() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let modelo = base.join("models/ggml-large-v3-turbo.bin");
    // Historial propio durante la transición (spec §16.4): no compartir
    // archivo con la versión Python mientras las dos estén en uso.
    let historial = base.join("history-nativo.jsonl");

    println!("Cargando el modelo...");
    let t = match Transcriber::new(&modelo) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    };
    let mut rec = match Recorder::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    };
    println!("Listo.\n");

    loop {
        print!("Enter para grabar (o 'q' para salir): ");
        stdout().flush().ok();
        let mut linea = String::new();
        if stdin().read_line(&mut linea).is_err() || linea.trim() == "q" {
            break;
        }

        // Un fallo de micrófono NO cierra el programa: se avisa y se sigue.
        if let Err(e) = rec.start() {
            println!("  No pude grabar: {e}\n");
            continue;
        }
        print!("GRABANDO — Enter para detener: ");
        stdout().flush().ok();
        stdin().read_line(&mut String::new()).ok();

        let audio = match rec.stop() {
            Ok(a) => a,
            Err(e) => {
                println!("  Error al cerrar la grabación: {e}\n");
                continue;
            }
        };
        println!("Procesando {:.1}s de audio...", audio.len() as f32 / 16000.0);

        match dictate(&t, &audio, &historial) {
            Ok(Some(r)) => {
                println!("  Crudo:  {}", r.raw);
                println!("  Limpio: {}", r.final_text);
                println!("  Tiempos: transcripción {:.2}s, limpieza {:.4}s\n",
                         r.transcribe_s, r.cleanup_s);
            }
            Ok(None) => println!("  Audio demasiado corto, silencio, o solo ruido.\n"),
            Err(e) => println!("  ERROR: {e}\n"),
        }
    }
}
```

- [ ] **Paso 4: Ejecutar**

```powershell
cd D:\MithFlow\app-nativa
cargo run -p mithflow-cli --release
```

- [ ] **Paso 5: Validación manual**

Con el binario corriendo y un Bloc de notas al lado:

**Transcripción y limpieza**
- [ ] Grabar 5 segundos hablando normal → el texto se pega en el Bloc de notas
- [ ] El texto tiene acentos y puntuación correctos
- [ ] Decir "eh, quería probar esto" → la muletilla "eh" desaparece
- [ ] Decir "muy muy bueno" → **no** se convierte en "muy bueno"
- [ ] Decir "el el informe" → sí se convierte en "el informe"

**Portapapeles**
- [ ] Copiar un texto antes de dictar → después del dictado ese contenido sigue ahí
- [ ] Copiar una **imagen** antes de dictar → anotar qué pasa (limitación conocida del spec §17: solo se preserva texto)

**Casos límite**
- [ ] Grabar menos de medio segundo → se descarta sin error
- [ ] Grabar 10 segundos de silencio → **no pega nada** (criterio 9)
- [ ] Grabar con música o TV de fondo, sin hablar → no pega nada
- [ ] Grabar más de 3 minutos → se corta en el tope, sin romperse
- [ ] Desconectar el micrófono y apretar Enter → avisa y **no cierra el programa**

**Micrófono y ruido**
- [ ] En reposo, el indicador de micrófono de Windows está **apagado** (Configuración → Privacidad → Micrófono)
- [ ] Al grabar se enciende y al terminar se apaga
- [ ] El acorde de inicio no aparece transcripto ni afecta la primera palabra

**Rendimiento e historial**
- [ ] Comparar el tiempo contra la **latencia percibida** anotada en la Task 0.7, no contra los 0.30 s de la medición controlada
- [ ] `history-nativo.jsonl` tiene una línea por dictado con el mismo formato
- [ ] `cargo test -p mithflow-core history -- --ignored` lee el `history.jsonl` real de Python sin perder entradas

- [ ] **Paso 6: Anotar el resultado y commitear**

```powershell
cd D:\MithFlow
git add app-nativa/
git commit -m "feat(cli): binario de validacion manual del nucleo"
```

---

## Cobertura del spec

| Requisito | Dónde se cubre |
|---|---|
| §16.1 Caracterizar notebooks | Task 0.8 |
| §16.2 Re-medir línea base | Task 0.7 |
| §16.3 Spike técnico | Tasks 0.2, 0.4, 0.5, 0.6, 0.6b |
| §3.2 `default-features = false` | Task 0.4 paso 3 |
| §3.2 Inventario de DLLs | Task 0.6b paso 3 |
| §4 Convivencia CUDA + Vulkan | Task 0.6b paso 2 |
| §4 Fallback sin ICD de Vulkan | Task 0.6b paso 4 |
| §6 Downmix y resampleo | Task 1.5 |
| §6 Ciclo de vida del micrófono | Task 1.5 (`stop` suelta el stream) |
| §6 Guarda del tono | Task 1.5 (`TONE_GUARD_SECS`) |
| §6 Límite de duración | Task 1.5 (`MAX_RECORDING_SECS`) |
| §9 Las 5 reglas de limpieza | Task 1.3 |
| §10 Filtro anti-alucinación | Task 1.6 |
| §11 Lectura tolerante | Task 1.4 |
| §13 Greedy e idioma fijo | Task 1.6 |
| §14 SHA-256 de modelos | Task 0.2 paso 4 |
| §15 WAV estéreo 48 kHz | Tasks 0.3 y 1.6 |
| §16.4 Historial separado | Task 1.9 (`history-nativo.jsonl`) |
| Criterio 8 (12 casos) | Task 1.3 |
| Criterio 9 (silencio) | Task 1.6 |

**Fuera de alcance por diseño:** la máquina de estados de §7 (el CLI es secuencial y bloqueante, así que no puede haber dos dictados a la vez; se implementa en el **Plan 3** junto con el atajo global, que es lo que la hace necesaria), el perfilado de hardware de §5 y la descarga verificada de §14 (Plan 2), la interfaz y la bandeja de §8 (Planes 3 y 4), y `CLEANUP_MODE = "off"` de §13 (Plan 3, con los ajustes).

## Planes siguientes

| Plan | Contenido |
|---|---|
| **2** | Perfilado de hardware por benchmark, catálogo y descarga verificada de modelos |
| **3** | Cascarón Tauri: ventana, bandeja, atajo global (F9), máquina de estados, instancia única, autoarranque, ajustes |
| **4** | Interfaz React + TypeScript: dashboard, ajustes, asistente de primer arranque |
| **5** | Empaquetado NSIS con las DLLs de ggml, instalación y prueba en las tres máquinas |

Cada uno se escribe cuando el anterior esté validado, para que las decisiones de `DECISIONES.md` queden reflejadas y no adivinadas.
