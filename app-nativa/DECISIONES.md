# Decisiones técnicas — MithFlow nativo

Registro de las respuestas del spike. Es el insumo de los Planes 2 a 5.

---

## Entorno (Task 0.1) — 2026-07-21

| Herramienta | Versión |
|---|---|
| rustc / cargo | 1.97.1 |
| CMake | 4.4.0 |
| VS Build Tools | 2022 (17.14.36) |
| Target triple | `x86_64-pc-windows-msvc` |

**Hallazgo 1:** `winget install Rustlang.Rustup` instala el gestor pero **no una
toolchain**, y en esta máquina la primera instalación quedó corrupta
(*"Missing manifest in toolchain"*). Hizo falta
`rustup toolchain uninstall stable` seguido de
`rustup toolchain install stable --profile default`. El plan se corrigió.

**Hallazgo 2 — el Vulkan SDK es dependencia de COMPILACIÓN, no opcional.**
El primer intento de compilar el spike falló con:

```
CMake Error: Could NOT find Vulkan (missing: Vulkan_LIBRARY Vulkan_INCLUDE_DIR glslc)
```

La feature `vulkan` de `transcribe-cpp` necesita cabeceras, librería y el
compilador de shaders `glslc`, no solo el runtime `vulkan-1.dll`. Se instaló
`KhronosGroup.VulkanSDK` (1.4.350.0, shaderc v2026.2) y el plan se corrigió con
un paso propio. Variables necesarias: `VULKAN_SDK` y `%VULKAN_SDK%\Bin` en PATH.

**Hallazgo 3 — `rdev::grab` exige la feature `unstable_grab`.** Confirmado: el
crate no habilita ninguna feature por defecto. Con `rdev = "0.5"` a secas el
import de `grab` no resuelve. Además `grab` toma un callback `Fn`, no `FnMut`:
cualquier estado mutable tiene que ir en un atómico.

---

## Línea base de la versión Python (Task 0.7) — 2026-07-21

Máquina: escritorio, RTX 3080 10 GB, 32 GB RAM.
Protocolo: `tests/bench_baseline.py`, WAV de voz sintética de **9.5 s**, N=20, modelo caliente.
Modelo: `large-v3-turbo`, float16, CUDA, greedy (`BEAM_SIZE = 1`).

| Métrica | Mediana | p95 | min | max |
|---|---|---|---|---|
| Transcripción + limpieza | **0.280 s** | 0.293 s | 0.270 s | 0.293 s |
| **Latencia percibida** | **0.739 s** | 0.751 s | 0.729 s | 0.751 s |

**Sobrecosto del pegado: 0.460 s.** Son las dos esperas fijas de `paste_text`
(0.15 s antes del Ctrl+V y 0.30 s después). Coincide con lo estimado en el spec.

### Lectura

El criterio 2 del spec pide menos de **0.9 s** de latencia percibida. La versión
Python está en 0.739 s, o sea que el margen contra el criterio es de apenas
0.16 s. Pero **el 62% de esa latencia son esperas fijas que la versión Rust
elimina** (spec §2.1: reemplazarlas por la espera del número de secuencia del
portapapeles).

Presupuesto estimado para la versión nativa:

| Componente | Python (CUDA) | Rust estimado (Vulkan) |
|---|---|---|
| Transcripción + limpieza | 0.280 s | ~0.36 s (Vulkan ≈ 30% más lento) |
| Espera del portapapeles | 0.460 s fijos | ~0.05 s medidos |
| **Total** | **0.739 s** | **~0.41 s** |

Es decir: aun perdiendo contra CUDA, la versión nativa debería quedar cómodamente
por debajo del criterio. **Esto valida el razonamiento del spec §2.1.**

### Nota sobre el número que se cita

El spec menciona "0.30 s" en su apéndice de mediciones controladas y "0.49 s" de
mediana en uso real. Este benchmark da 0.280 s con el mismo protocolo controlado,
consistente con el primero. La diferencia con el uso real se explica por audio
variable y modelo frío. **La vara del proyecto es la latencia percibida: 0.739 s.**

---

## Motor de transcripción (Task 0.4 / 0.5)

### Formato de modelo — RESUELTO

Era el bloqueante crítico del spike: `transcribe-cpp` carga **GGUF**, no los
`.bin` en formato GGML de whisper.cpp. Son formatos distintos y el plan original
descargaba el equivocado.

Los GGUF oficiales, probados contra `transcribe.cpp` por sus propios autores,
están en **`huggingface.co/handy-computer/whisper-large-v3-turbo-gguf`**.
Verificado el 21/7/2026 (HTTP 200 y tamaños reales):

| Archivo | Tamaño | Uso previsto |
|---|---|---|
| `whisper-large-v3-turbo-F16.gguf` | 1550 MB | Escritorio con GPU dedicada |
| `whisper-large-v3-turbo-Q8_0.gguf` | 845 MB | — |
| `whisper-large-v3-turbo-Q6_K.gguf` | — | — |
| `whisper-large-v3-turbo-Q5_K_M.gguf` | 590 MB | Notebooks (equivale al q5_0 de GGML) |
| `whisper-large-v3-turbo-Q4_K_M.gguf` | — | Piso de seguridad |

URL base: `https://huggingface.co/handy-computer/whisper-large-v3-turbo-gguf/resolve/main/`

Los `.bin` GGML descargados siguen sirviendo para el plan B (`whisper-rs`).

### API real — RESUELTA

Documentada en docs.rs, contra lo que decía la primera versión del plan:

```rust
pub fn load(path: impl AsRef<Path>) -> Result<Model>
pub fn load_with(path: impl AsRef<Path>, options: &ModelOptions) -> Result<Model>
// uso:
let mut session = Model::load("model.gguf")?.session()?;
let result = session.run(&pcm, &RunOptions::default())?;  // 16 kHz mono f32 en [-1,1]
```

Tipos: `Model`, `Session`, `RunOptions`, `Transcript`, `Segment`, `Token`, `Word`.
**No existe `Context::new`.**

### Pendiente

- ¿Compila y transcribe en esta máquina?: PENDIENTE
- Parámetros disponibles en `RunOptions` (language, sampling, no_speech_threshold): PENDIENTE
- DECISIÓN final: PENDIENTE

## Backends (Task 0.6b)

- cuda + vulkan + dynamic-backends en un binario: PENDIENTE
- DLLs a empaquetar: PENDIENTE
- DECISIÓN sobre criterios 1 y 2: PENDIENTE

## Atajo global (Task 0.6)

- Feature `unstable_grab` requerida: **CONFIRMADO** (ver Hallazgo 3)
- Compila: **SÍ**, binario en `spike-hotkey/target/release/spike-hotkey.exe` (140 KB)
- rdev grab() suprime en apps normales: **PENDIENTE — requiere prueba manual**
- Suprime en ventana elevada: **PENDIENTE — se espera que NO**
- DECISIÓN: PENDIENTE

La prueba requiere apretar F9 con el foco en Bloc de notas, VS Code, Chrome y
una consola elevada, y observar si la aplicación reacciona. No es automatizable:
la tiene que hacer una persona.

---

## Estado de la Fase 1 (núcleo en Rust)

| Módulo | Estado | Tests |
|---|---|---|
| `config` | ✅ | — |
| `cleanup` | ✅ | 7 (12 casos + paridad con puntuación + idempotencia) |
| `history` | ✅ | 6 (incluye BOM y el historial real de 21 entradas) |
| `audio` | ✅ | 6 (incluye el WAV estéreo 48 kHz real) |
| `paste` | ✅ | 4 (2 activos + 2 `#[ignore]` con medición) |
| `stt` | ⏳ | Bloqueado por la decisión del spike |
| pipeline `dictate()` | ⏳ | Depende de `stt` |
| CLI | ⏳ | Depende del pipeline |

**21 tests pasando, 4 ignorados. Clippy limpio.**

### Bugs reales encontrados durante la implementación

1. **Paridad de tartamudeos rota.** `split_whitespace()` deja la puntuación
   pegada al token, así que `"el el."` no se colapsaba mientras Python sí lo
   hace con `\b`. Es el caso más frecuente de tartamudeo al dictar: justo antes
   de la pausa que el modelo transcribe como coma o punto. Arreglado comparando
   el núcleo alfanumérico del token; verificado 13/13 contra Python.

2. **BOM descartaba la primera entrada del historial en silencio.** `load()`
   usa `filter_map(...ok())`: un BOM no da error, simplemente hace que la
   primera línea no parsee y se pierda sin aviso. Detectado y cerrado con test.

### Medición del pegado (Task 1.7)

| | Python | Rust |
|---|---|---|
| `paste()` de punta a punta | 460 ms (esperas fijas) | **122.5 ms** (mediana, N=10) |

De esos 122.5 ms, **120 ms son el margen fijo** que queda antes de restaurar el
portapapeles. Copiar, esperar la confirmación por número de secuencia, enviar
Ctrl+V y restaurar suman ~2.5 ms: el `sleep(150 ms)` de Python se convirtió en
2.5 ms de espera confirmada. El único margen que queda en este módulo es ese
sleep de 120 ms, y reducirlo es empírico: no hay señal del sistema que diga
"la app destino ya procesó el Ctrl+V".

## Caracterización de las notebooks (Task 0.8)

- Notebook de Jaé: PENDIENTE — requiere acceso físico a la máquina
- Notebook personal: PENDIENTE — requiere acceso físico a la máquina
