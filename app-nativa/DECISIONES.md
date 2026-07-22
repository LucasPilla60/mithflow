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

### Resultado — FUNCIONA

Ejecutado el 21/7/2026 sobre el fixture de 9.5 s, 5 pasadas tras calentar.

```
ggml_vulkan: Found 1 Vulkan devices:
ggml_vulkan: 0 = NVIDIA GeForce RTX 3080 (NVIDIA) | uma: 0 | fp16: 1 | bf16: 1
             | warp size: 32 | matrix cores: NV_coopmat2
load_backend: loaded Vulkan backend from ...\ggml-vulkan.dll
load_backend: loaded CPU backend from ...\ggml-cpu-haswell.dll
whisper: using vulkan backend: Vulkan0
Modelo cargado en 1.694s
Transcripción: mediana 0.221s | min 0.214s | max 0.225s
```

Texto devuelto, correcto salvo el vocabulario propio:
> "Quería comentarte que el dashboard de **Middata** para el cliente ya está
> listo. Habría que revisar el CRM y los leads pendientes antes de la reunion
> del jueves."

`Middata` en vez de `MithData` porque el spike usa `RunOptions::default()`, sin
`initial_prompt`. Confirma que el vocabulario propio hace falta y funciona como
se esperaba en la versión Python.

### DECISIÓN: `transcribe-cpp` 0.1.3. El plan B (`whisper-rs`) no se usa.

### Hallazgo obligatorio: `init_backends_default()`

Con `dynamic-backends` hay que llamar a `transcribe_cpp::init_backends_default()`
**antes** de `Model::load`, o falla con `backend error (status 8)`. El propio
error lo explica. Las DLLs tienen que estar junto al ejecutable.

---

## Backends (Task 0.6b) — RESUELTO SIN NECESIDAD DE CUDA

**Vulkan resultó MÁS RÁPIDO que la línea base con CUDA**, no un 25-30% más lento
como preveía el spec:

| | Backend | Transcripción (9.5 s de audio) |
|---|---|---|
| Python + faster-whisper | CUDA, float16 | 0.280 s |
| **Rust + transcribe-cpp** | **Vulkan** | **0.221 s** |

Es un **21% más rápido**. La causa probable es que whisper.cpp con Vulkan usa los
matrix cores (`NV_coopmat2`) y el GGUF F16 en lugar de la conversión de
CTranslate2. Sea cual sea la razón, el dato desarma la tensión entre los
criterios 1 y 2 del spec:

**No hace falta compilar con `cuda`.** Un único binario con
`dynamic-backends` + `vulkan` cumple el criterio 1 (un solo instalador) y supera
el criterio 2 (latencia) sin concesiones. La Task 0.6b, que existía para resolver
ese conflicto, queda sin objeto.

Además, `dynamic-backends` eligió **solo** la variante de CPU adecuada para este
procesador (`ggml-cpu-haswell.dll`) entre las nueve disponibles. Es exactamente
la adaptación automática por máquina que pedía el requisito central.

### Tamaño real del bundle — el spec lo subestimó 4x

| Componente | Tamaño |
|---|---|
| `ggml-vulkan.dll` (shaders) | **70.6 MB** |
| 9 variantes de CPU (`sse42`, `haswell`, `skylakex`, `icelake`, `alderlake`...) | ~7.5 MB |
| `ggml-base.dll`, `ggml.dll`, `transcribe.dll` | ~2.2 MB |
| Ejecutable | 0.2 MB |
| **Total sin modelo** | **80.7 MB** |

El spec estimaba "~20 MB". El instalador real va a rondar los **85 MB**, más el
modelo que se descarga aparte. Sigue siendo muy inferior a empaquetar Python con
CUDA (2-3 GB), pero hay que corregir la cifra en el spec y en el README.

**Optimización disponible si molesta el tamaño:** las 9 variantes de CPU son
alternativas del mismo backend. Se podrían recortar a 2 o 3 (`sse42` como piso,
`haswell`, `alderlake`) perdiendo algo de rendimiento en CPUs específicas.
Decisión para el Plan 5 (empaquetado).

## Atajo global (Task 0.6)

- Feature `unstable_grab` requerida: **CONFIRMADO** (ver Hallazgo 3)
- Compila: **SÍ**, binario en `spike-hotkey/target/release/spike-hotkey.exe` (140 KB)
- **rdev grab() SUPRIME la tecla: CONFIRMADO** (prueba manual, 21/7/2026)
- Suprime en ventana elevada: no probado — se asume que NO (limitación de Windows)

### DECISIÓN: `rdev` con la feature `unstable_grab`. `tauri-plugin-global-shortcut` no se incorpora.

**Cómo se verificó, y por qué la primera prueba no servía.** El primer intento fue
con el Bloc de notas, y **no prueba nada**: F9 no tiene ninguna función ahí, así
que "no pasó nada" es idéntico tanto si la tecla se suprimió como si llegó y la
aplicación la ignoró.

La prueba concluyente fue **VS Code**, donde **F9 pone o saca un breakpoint** en
la línea del cursor — efecto visible e inequívoco. Con el cursor en la línea 9 de
un archivo `.sql` y VS Code enfocado, al apretar F9:

- La consola del spike registró `F9 capturada y SUPRIMIDA (#2)`.
- **No apareció ningún breakpoint.** VS Code nunca recibió la tecla.

**Lección de diseño de pruebas:** para verificar supresión de teclas hay que
elegir una aplicación donde esa tecla tenga un efecto observable. Un objetivo que
ignora la tecla da un falso positivo.

---

## Estado de la Fase 1 (núcleo en Rust)

| Módulo | Estado | Tests |
|---|---|---|
| `config` | ✅ | — |
| `cleanup` | ✅ | 7 (12 casos + paridad con puntuación + idempotencia) |
| `history` | ✅ | 6 (incluye BOM y el historial real de 21 entradas) |
| `audio` | ✅ | 6 (incluye el WAV estéreo 48 kHz real) |
| `paste` | ✅ | 4 (2 activos + 2 `#[ignore]` con medición) |
| `stt` | ✅ | 3 + 5 de integración con modelo real |
| pipeline `dictate()` | ✅ | 4 (con doble que paniquea si se toca el modelo) |
| CLI | ✅ | Arranca, calienta el motor y sale limpio |

**28 tests unitarios + 5 de integración pasando. Clippy limpio. Fase 1 COMPLETA.**

### Hallazgo: el calentamiento del motor es obligatorio

La **primera inferencia del sistema cuesta 17.4 s** compilando shaders de Vulkan.
El caché del driver persiste entre procesos, así que se paga una sola vez por
máquina (y de nuevo tras actualizar el driver). Sin calentamiento al arrancar, el
primer dictado del usuario se sentiría roto.

**Sutileza:** el calentamiento **no puede usar silencio**. La compuerta de energía
RMS corta antes de llegar al modelo, así que un buffer de ceros vuelve en
microsegundos sin compilar un solo shader — el calentamiento sería un no-op
silencioso. Se usa medio segundo de senoide a 220 Hz, con RMS 3.5x por encima
del umbral.

### Bugs reales encontrados durante la implementación

1. **Paridad de tartamudeos rota.** `split_whitespace()` deja la puntuación
   pegada al token, así que `"el el."` no se colapsaba mientras Python sí lo
   hace con `\b`. Es el caso más frecuente de tartamudeo al dictar: justo antes
   de la pausa que el modelo transcribe como coma o punto. Arreglado comparando
   el núcleo alfanumérico del token; verificado 13/13 contra Python.

2. **BOM descartaba la primera entrada del historial en silencio.** `load()`
   usa `filter_map(...ok())`: un BOM no da error, simplemente hace que la
   primera línea no parsee y se pierda sin aviso. Detectado y cerrado con test.

3. **El BOM mordió una segunda vez, en la CLI.** PowerShell antepone un BOM
   UTF-8 a la entrada redirigida y `trim()` no lo saca (U+FEFF no tiene la
   propiedad `White_Space`), así que `"q" | mithflow-cli.exe` no salía. Es la
   misma clase de bug que el punto 2: **en Windows, toda entrada de texto
   externa puede traer BOM y hay que sacarlo explícitamente.**

4. **El filtro anti-alucinación por umbral no alcanza.** Se barrieron 6
   combinaciones de `no_speech_thold` (0.1–0.8) × `logprob_thold` (-1.0–-0.3):
   **todas alucinaron** sobre silencio y sobre ruido. Y sobre ruido el modelo
   inventa cadenas distintas cada vez ("911", "Moderna, Civilization…"), así que
   una lista de bloqueo tampoco cierra: es inenumerable. Además, subir
   `no_speech_thold` a 0.8 filtra **menos**, no más — la condición en el C++ es
   `no_speech_prob > thold && avg_logprob < logprob_thold`.

   La solución es una **compuerta de energía RMS antes del modelo**, que es
   determinista y no puede inventar texto. Medido:

   | | RMS |
   |---|---|
   | Silencio | 0.000000 |
   | Ruido de fondo | 0.002891 |
   | Voz | 0.071–0.101 |

   25x de separación. `MIN_SPEECH_RMS = 0.01`, deliberadamente por debajo del
   punto medio geométrico: entre transcribir ruido y perder un dictado flojo, el
   error caro es el segundo.

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

## Caracterización de las tres máquinas (Task 0.8)

Datos aportados por el usuario el 21/7/2026.

| | Escritorio | MSI Katana | Notebook de Jaé |
|---|---|---|---|
| CPU | Ryzen 9 3900X | i7 12ª gen | Ryzen 3 o i3 (sin confirmar) |
| RAM | **64 GB** | 32 GB | ~8 GB (sin confirmar) |
| GPU | RTX 3080 10 GB | **RTX 3050 Ti** (4 GB) | Integrada, probablemente AMD |
| Estado | Medida | Estimada | **Sin confirmar** |

### Qué implica para cada una

**Escritorio** — medido: Vulkan sobre la 3080, `F16`, 0.221 s para 9.5 s de audio.

**MSI Katana** — tiene **GPU dedicada**, no integrada como se asumía en el spec.
Una RTX 3050 Ti con 4 GB de VRAM corre Vulkan sin problema. Con el modelo `F16`
(1.5 GB en disco, ~2.1 GB en uso) entra con margen; si el benchmark muestra
presión de memoria, `Q5_K_M` (590 MB) es la alternativa. Se espera rendimiento
holgadamente dentro del criterio 3, probablemente mejor que el criterio 2.

**Notebook de Jaé** — es la máquina restrictiva y la única con incógnitas reales.
Con gráficos integrados AMD, Vulkan debería funcionar: en integradas equivalentes
(Radeon 680M) se midieron 3-4x tiempo real, unas 12 veces más rápido que CPU
pura. Si la integrada no soporta las operaciones de cómputo necesarias, el
fallback a CPU ya está contemplado. Con ~8 GB de RAM el modelo indicado es
`Q5_K_M` (590 MB) o `small`.

### Esta incertidumbre valida el diseño de la sección 5 del spec

La notebook de Jaé es exactamente el caso que motivó **elegir el modelo por
benchmark medido y no por umbrales de VRAM**: una integrada AMD reporta ~128 MB
de "VRAM dedicada" por DXGI, así que una matriz por umbrales la mandaría a la
rama "sin GPU utilizable" aunque Vulkan funcione bien. El perfilado por medición
resuelve solo las tres máquinas sin necesidad de conocer sus especificaciones de
antemano.

### Hallazgo del Plan 2: el clip de referencia ES la calibración

Al implementar `hardware.rs` se midió el escritorio con el audio de referencia
de **3 s** que pedía el diseño, perfilando con el `Q4_K_M`:

| Clip de referencia | Factor medido | Tiempo de inferencia | Modelo elegido |
|---|---|---|---|
| 3,0 s | **17,25x** | 0,174 s | `Q5_K_M` ❌ |
| 9,5 s | **62,8–65,3x** | 0,146–0,151 s | `F16` ✅ |

Con 3 s, la máquina más rápida del proyecto —la misma que hace 9,5 s de audio en
0,221 s, o sea 43x— **no calificaba para el modelo grande**.

**La causa:** whisper rellena toda entrada hasta 30 s antes del codificador, así
que el tiempo de inferencia es casi independiente de lo que dure el clip (0,174 s
contra 0,151 s, para clips que difieren 3x). Dividir un costo prácticamente fijo
por 3 en vez de por 9,5 desinfla el factor unas tres veces. Los umbrales
(20x / 8x / 3x) salieron de mediciones sobre 9,5 s, así que un clip más corto los
deja descalibrados.

**Decisión: `SEGUNDOS_DE_REFERENCIA = 9.5`**, igual que la línea base de este
documento, con los umbrales intactos. Alargar el clip **no cuesta tiempo de
perfilado** (el trabajo real es el mismo) y devuelve un número directamente
comparable con las mediciones que ya tiene el proyecto.

**Lección general: en un modelo con ventana de entrada fija, un "factor de tiempo
real" no es una propiedad de la máquina sola, sino del par (máquina, duración del
clip).** Elegir la duración del clip de referencia es elegir la calibración de
los umbrales; las dos cosas no se pueden mover por separado.

### Perfilado real del escritorio (21/7/2026)

```
ram_total_gb        : 63.9
gpu_nombre          : Some("NVIDIA GeForce RTX 3080")
backend             : Vulkan0
factor_tiempo_real  : 62.84x
modelo_recomendado  : large-v3-turbo F16
(perfilado con large-v3-turbo Q4_K_M)
```

El factor supera al 43x de la línea base porque se perfila con el `Q4_K_M`, que
es más rápido que el `F16` con el que se midió aquélla. **El modelo con el que se
perfila forma parte de la calibración**: cambiarlo obliga a revisar los umbrales.

### Modelos del catálogo (SHA-256 verificados el 21/7/2026)

| Archivo | Bytes | SHA-256 |
|---|---|---|
| `whisper-large-v3-turbo-F16.gguf` | 1.625.935.520 | `e1d0144e9afc9f479d9e51fc92c7dea9dc36059655eeb3819f16ad2de779046a` |
| `whisper-large-v3-turbo-Q5_K_M.gguf` | 619.628.128 | `977b5db4e004349dffd1ab9caa10ba5aaba3fc3edd3ba72cadb84328a3203e36` |
| `whisper-large-v3-turbo-Q4_K_M.gguf` | 536.069.728 | `ecfe9b6beb4ab18fef49187cc968cc74b5168b94629c8830e2ca6b794c6e25ed` |

Van compilados en el binario (`models::Modelo::sha256`) y son la raíz de
confianza de la descarga: un hash servido por el mismo host que el modelo no
verifica nada. El catálogo **no** incluye `Q8_0` ni `Q6_K` a propósito: una
entrada sin su hash medido sería un agujero, y ninguno de los dos es alcanzable
por la tabla de decisión.

### Estado del Plan 2

| Módulo | Estado | Tests |
|---|---|---|
| `hardware` | ✅ | 17 (cada rama y cada borde, las tres máquinas, `NaN`, barrido de la recta positiva, 1 `#[ignore]` que perfila esta máquina) |
| `models` | ✅ | 15 (vectores SHA-256 del estándar, rechazo con borrado, catálogo, descarga contra un servidor local: limpia, reanudada, `Range` ignorado y cuerpo adulterado; 1 `#[ignore]` contra los archivos reales) |

**58 tests unitarios + 5 de integración pasando. Clippy limpio.**

La descarga se ejercita contra un servidor HTTP mínimo escrito sobre `std::net`
dentro del propio test: sin dependencia nueva y con control exacto de la
respuesta, que es lo único que permite probar el caso "el servidor ignora el
`Range` y manda todo de nuevo" — el que, mal resuelto, concatenaría los bytes de
dos respuestas y daría un archivo corrupto de 1,5 GB.

### Pendiente cuando haya acceso físico

Confirmar en la notebook de Jaé: CPU exacto, RAM real, modelo de la integrada, y
si `vulkaninfo --summary` lista algún dispositivo. **No bloquea el desarrollo**:
el Plan 2 se puede escribir con lo que hay, porque la selección es automática.

---

## Plan 3 — el cascarón de Tauri v2 (21/7/2026)

### Estructura

```
app-nativa/
├── Cargo.toml            workspace: core, cli y app; default-members = core + cli
└── app/
    ├── package.json      React 19 + TypeScript + Vite 7
    ├── src/              frontend PROVISORIO (api.ts + App.tsx)
    └── src-tauri/
        ├── build.rs      junta las DLLs de ggml para el bundler
        ├── tauri.conf.json
        ├── capabilities/default.json   sólo core:default
        └── src/
            ├── main.rs      arranque, plugins, cableado de los hilos
            ├── estado.rs    la máquina de estados y su espejo de lectura
            ├── director.rs  el hilo que decide (único escritor del estado)
            ├── motor.rs     el hilo que carga el modelo y transcribe
            ├── atajo.rs     el hilo de rdev::grab
            ├── sonidos.rs   los cuatro tonos
            ├── bandeja.rs   ícono y menú
            ├── ajustes.rs   persistencia con tauri-plugin-store
            ├── comandos.rs  la API que ve el frontend
            ├── eventos.rs   los eventos que se emiten
            ├── ventana.rs   mostrar / esconder / enfocar
            └── rutas.rs     dónde vive cada archivo
```

**`default-members` es una decisión, no un descuido.** `tauri::generate_context!`
exige que `app/dist/` exista **en tiempo de compilación**, y ese directorio lo
produce `npm run build` y está en `.gitignore`. Si la app fuera miembro por
defecto, un clon recién hecho no podría correr `cargo test` sin instalar Node
primero. Así, los tests del núcleo siguen siendo un comando sin prerrequisitos y
la app se compila con `-p mithflow-app`.

Sí es **miembro** del workspace (y no `exclude`) para compartir `target/`: ahí es
donde el build de `transcribe-cpp-sys` deja las DLLs de ggml, así que la app las
tiene al lado de su ejecutable sin copiar nada, y no hay que recompilar
whisper.cpp con sus shaders de Vulkan una segunda vez.

### Los cinco hilos

| Hilo | Qué hace | Por qué está solo |
|---|---|---|
| principal | ventana, bandeja, comandos | lo exige el sistema operativo |
| `atajo` | `rdev::grab` | bloquea para siempre |
| `director` | la máquina de estados | único escritor del estado |
| `motor` | carga el modelo y transcribe | 20 s de arranque, segundos por dictado |
| `sonidos` | los cuatro tonos | retiene el `Stream` de salida, que no es `Sync` |

Se hablan por canales. La única memoria compartida es un espejo de **sólo
lectura** del estado (`RwLock<EstadoDto>`), que existe para que el comando
`leer_estado` no tenga que esperar a que algo cambie.

### El hilo del atajo, y las tres restricciones que le dan forma

1. **`grab` bloquea**: instala un hook de bajo nivel y se queda en su bucle de
   mensajes. Hilo dedicado, y la única salida es un `Sender`.
2. **El callback es `Fn`, no `FnMut`**: todo el estado configurable (tecla,
   pausa, tecla-abajo) vive en atómicos globales. El `Sender` sí se puede
   capturar por valor, porque `send` toma `&self`.
3. **El hook corre en el camino crítico del teclado**: si tarda más que
   `LowLevelHooksTimeout` (300 ms), Windows lo desengancha sin avisar. El
   callback sólo hace atómicos y un `send` que no bloquea.

**La repetición automática hay que filtrarla.** Windows manda `KeyPress`
repetidos mientras se mantiene una tecla apretada; sin la guarda `ABAJO`, dejar
el dedo en F9 medio segundo dispararía decenas de arranques y paradas. Hay test.

Cambiar la tecla no reinicia el hilo: el callback relee el atómico en cada
evento. La pausa deja el hook instalado y devuelve `Some(evento)`, o sea que la
tecla llega normalmente a la aplicación enfocada.

### La máquina de estados, y la carrera que no puede ocurrir

```
Cargando → Listo ⇄ Grabando → Transcribiendo → Listo
```

Al pasar de `Grabando` a `Transcribiendo` el `Vec<f32>` **se mueve** al motor
por el canal: el director se queda literalmente sin el buffer. Un segundo atajo
durante la transcripción no tiene nada que pisar y recibe "estoy transcribiendo
el anterior". Es el bug que la versión Python resolvió pasando `captured` por
parámetro; acá no hay que resolverlo, no se puede escribir.

Apretar el atajo mientras carga **no se ignora en silencio**: suena el tono de
error y sale un evento `aviso`. Sin realimentación, "no pasó nada" y "todavía no
estoy listo" se ven igual.

**Tope de grabación**: mientras graba, el director usa `recv_timeout(250 ms)` y
vigila `elapsed_secs`. Sin eso, un atajo apretado sin querer deja el micrófono
abierto y el buffer creciendo a 384 KB/s hasta que alguien se dé cuenta.

### Empaquetado: dónde quedan las 13 DLLs — HALLAZGO

**Compilando ya quedan bien, pero de casualidad.** El build de
`transcribe-cpp-sys` copia las DLLs al directorio del perfil de cargo, que es
justo donde queda el `.exe`. Verificado:

```
target/release/mithflow.exe          19,2 MB
target/release/ggml-vulkan.dll       74,0 MB   <- el 91% del peso
target/release/ggml-cpu-*.dll (9)     8,1 MB
target/release/ggml-base.dll, ggml.dll, transcribe.dll
13 DLLs, 81 MB en total
```

Y el arranque real lo confirma:

```
load_backend: loaded Vulkan backend from D:\MithFlow\app-nativa\target\release\ggml-vulkan.dll
load_backend: loaded CPU backend from D:\MithFlow\app-nativa\target\release\ggml-cpu-haswell.dll
whisper: using vulkan backend: Vulkan0
```

**El instalador NO hereda esa casualidad.** El bundler de Tauri empaqueta lo que
se le nombra: sin declararlas, el `.msi`/`.exe` sale sin motor y la app instalada
falla con `backend error (status 8)`. Se resolvió como documenta el README de
`transcribe-cpp`:

- `build.rs` copia las librerías a `src-tauri/transcribe-libs/` (ruta fija, que
  es lo que un archivo de configuración puede nombrar: los directorios de cargo
  llevan un hash).
- `tauri.conf.json` declara `"resources": { "transcribe-libs/*": "." }`. En
  Windows los recursos se instalan al lado del ejecutable, que es exactamente
  donde `init_backends_default()` los busca.
- `transcribe-cpp` figura como dependencia **directa** de la app aunque el
  código no la use: cargo sólo le pasa las variables `DEP_TRANSCRIBE_CPP_*` al
  build script del que depende directamente del crate con `links`, y nuestra
  ruta real pasa por `mithflow-core`, que no las reenvía.

**Sub-hallazgo de orden:** la copia tiene que correr **antes** de
`tauri_build::build()`. Ahí se resuelve el glob de `bundle.resources`, y con
`transcribe-libs/` todavía vacío la compilación aborta con
`glob pattern transcribe-libs/* path not found or didn't match any files`.

*Verificado con un `tauri build` real en el Plan 5: las 13 DLLs están adentro
del instalador y la app instalada las carga desde su propia carpeta. Ver
"Plan 5 — el instalador" más abajo.*

### HALLAZGO: `cargo build --release` a secas da un binario de desarrollo

`tauri-build` decide dev/producción por la feature `custom-protocol` de `tauri`,
que la CLI activa sola en `tauri build`. Con un `cargo build --release` pelado,
`generate_context!` compila en modo desarrollo y el ejecutable busca el frontend
en `http://localhost:1420`: ventana en blanco, sin ningún error visible. Se
agregó la feature al `Cargo.toml` de la app. El comando correcto es:

```powershell
cargo build --release -p mithflow-app --features custom-protocol
```

### Calentamiento del motor: medido

| Corrida | Calentamiento |
|---|---|
| Primera (shaders del `Q4_K_M` sin compilar) | **39,2 s** |
| Segunda (caché del driver caliente) | **0,2 s** |

Confirma el hallazgo de la Fase 1 y su corolario: **el caché de shaders es por
máquina y por juego de shaders**, no por proceso. Los 39 s superan los 17,4 s
medidos con el `F16` porque cada cuantización usa sus propios kernels. Sin
calentamiento, ese costo lo pagaría el primer dictado del usuario.

El calentamiento reusa `hardware::audio_de_referencia()` en vez de silencio, por
la razón de siempre: la compuerta de energía RMS corta antes del modelo y un
buffer de ceros no compilaría un solo shader.

### Decisiones menores, con su costo

- **Historial**: `%APPDATA%\com.mithdata.mithflow\history-nativo.jsonl`.
  Deliberadamente distinto de `history.jsonl`, que sigue siendo de `mithflow.py`.
  Hay un test que lo asegura.
- **Tecla por defecto F9**, con la tabla de teclas admitidas y su índice
  verificados por test, para que nadie mude el default reordenando la tabla.
- **Íconos de bandeja dibujados en memoria** (círculos de 32 px con el borde
  suavizado) en vez de seis PNG empaquetados: el juego de colores vive al lado
  del `match` que lo elige, y hay un test de que ningún estado comparte color.
- **`rodio` arrastra `cpal 0.17`** mientras el núcleo usa `cpal 0.16`: se
  compilan las dos. Es tiempo de compilación, no un conflicto en ejecución
  (entrada y salida son dispositivos distintos). La alternativa era escribir la
  salida de audio a mano sobre el `cpal` del núcleo (~80 líneas con el `match`
  de formatos), y no valía la pena por cuatro tonos.
- **CSP explícita** en `tauri.conf.json` (`default-src 'self'`, sin `connect-src`
  hacia afuera) y `capabilities` con `core:default` solamente. El historial es
  texto dictado en claro: el webview no tiene por dónde sacarlo.
- **El texto dictado nunca va a los logs**, sólo los tiempos.
- **El autoarranque consulta el estado real antes de tocar el registro**:
  `disable()` borra un valor y falla si no existe, así que llamarlo a ciegas en
  cada arranque —el caso normal, porque viene apagado— tiraría un error espurio
  todas las veces.

### Ajustes: qué se aplica y qué queda pendiente

| Ajuste | Persistido | Aplicado |
|---|---|---|
| tecla | ✅ | ✅ en caliente |
| sonidos, volumen | ✅ | ✅ en caliente |
| arranque con Windows | ✅ | ✅ |
| modelo | ✅ | ✅ al reiniciar (se avisa) |
| vocabulario | ✅ | ❌ |
| muletillas | ✅ | ❌ |
| modo de limpieza | ✅ | ❌ |

**Los tres últimos necesitan parametrizar el núcleo**, que hoy los tiene como
constantes: `config::INITIAL_PROMPT` y `config::FILLERS` se leen desde
`stt::opciones_de_dictado` y `cleanup::fast_cleanup`, y `dictate()` decide la
limpieza por su cuenta. Se dejaron sin tocar a propósito —el núcleo está cerrado
y probado, y este plan era el cascarón— pero **es deuda real y no una mejora
opcional**: un ajuste que se guarda y no hace nada es peor que no ofrecerlo. Son
unas 30 líneas en tres archivos del núcleo más sus tests.

### Estado del Plan 3

**39 tests nuevos en la app + los 58 del núcleo = 97 pasando. Clippy limpio con
`-D warnings` en los dos.**

Verificado en ejecución: carga del modelo, calentamiento, bandeja, y el camino
completo de IPC (React llamó a `leer_estado` y `leer_historial` y el backend
respondió, o sea que la CSP no bloquea el webview y los comandos resuelven su
estado manejado).

**No se probó el dictado real** (necesita micrófono y foco): lo hace el usuario.

---

## Plan 5 — el instalador (21/7/2026)

### El riesgo del Plan 3 quedó cerrado: las 13 DLLs SÍ entran

Era la incógnita central del empaquetado. `7z l` sobre
`MithFlow_1.0.0_x64-setup.exe` lista las 13, y las 13 tienen el **mismo SHA-256**
que las de `target/release/`:

```
mithflow.exe               19.337.216
ggml-base.dll                 640.512
ggml-cpu-alderlake.dll        889.344
ggml-cpu-cannonlake.dll       994.304
ggml-cpu-cascadelake.dll      993.280
ggml-cpu-haswell.dll          891.392
ggml-cpu-icelake.dll          993.280
ggml-cpu-sandybridge.dll      834.560
ggml-cpu-skylakex.dll         994.304
ggml-cpu-sse42.dll            754.688
ggml-cpu-x64.dll              756.224
ggml-vulkan.dll            74.008.576
ggml.dll                       66.560
transcribe.dll              1.604.608
```

El `installer.nsi` que genera el bundler las emite como `File` incondicionales
dentro de `Section Install`, después de `SetOutPath $INSTDIR`: quedan al lado del
ejecutable, que es donde `init_backends_default()` las busca.

### La prueba que vale: instalación limpia, lejos de `target/`

Se extrajo el instalador a un directorio temporal aislado y se corrió el `.exe`
desde ahí, sin `target/release` en el `PATH`:

```
load_backend: loaded Vulkan backend from ...\instalacion-limpia\ggml-vulkan.dll
load_backend: loaded CPU backend from ...\instalacion-limpia\ggml-cpu-haswell.dll
whisper: using vulkan backend: Vulkan0
motor listo sobre Vulkan0 (calentado en 0.2 s)
estado: listo
```

Las rutas apuntan a la carpeta de instalación, **no** a `target/release`. Y no
se queda en "cargó las DLLs": `motor listo` significa que corrió una inferencia
real de calentamiento con ellas.

**Cómo se hizo la prueba, y por qué así.** El binario de release lleva
`windows_subsystem = "windows"`, así que no tiene consola: la salida se capturó
con `Start-Process -RedirectStandardOutput/-RedirectStandardError`. El modelo se
apuntó con `MITHFLOW_MODELO` porque en esta máquina `%APPDATA%\MithFlow\models\`
está vacío — eso decide de dónde sale el **modelo**, no de dónde salen las DLLs,
que es lo que se estaba verificando.

### Tamaño real: la descarga es 12 MB, la instalación 99 MB

| | Tamaño |
|---|---|
| `MithFlow_1.0.0_x64-setup.exe` | **12.743.287 bytes (12,15 MiB)** |
| Contenido sin comprimir | 102.265.730 bytes (97,5 MiB) |
| `ESTIMATEDSIZE` que declara el instalador | 101.327 KB (~99 MB) |

**El pronóstico de "instalador de ~85 MB" era pesimista por un factor de 7.** Los
84 MB de DLLs comprimen **8:1** con LZMA sólido, porque `ggml-vulkan.dll` es casi
toda bytecode SPIR-V —muy repetitivo— y no código nativo. O sea que la
optimización que quedaba anotada del Plan 3 (recortar las 9 variantes de CPU)
**no hace falta**: las nueve juntas son 8,1 MB sin comprimir y aportan menos de
1 MB a la descarga. Se dejan las nueve, que es lo que hace que un solo binario
sirva para cualquier procesador.

Hay que distinguir los dos números al comunicarlos: **12 MB es lo que el usuario
baja, 99 MB es lo que ocupa en disco** (más el modelo, que va aparte).

### Metadatos del instalador

| Campo | Valor | Por qué |
|---|---|---|
| `productName` | MithFlow | — |
| `version` | **1.0.0** (era 0.1.0) | primera versión empaquetada; sincronizada en `Cargo.toml` del workspace, `package.json` y el backend simulado, porque `comandos.rs` la publica al frontend con `env!("CARGO_PKG_VERSION")` |
| `identifier` | `com.mithdata.mithflow` | **no se toca**: ya nombra `%APPDATA%\com.mithdata.mithflow\`, donde vive el historial |
| `publisher` | MithData | sin esto, "Editor: desconocido" en el aviso de SmartScreen |
| `copyright` | © 2026 MithData | va a los metadatos del `.exe` |
| `category` | Productivity | — |
| `shortDescription` / `longDescription` | en español | es lo que se ve en el instalador |
| `targets` | **`["nsis"]`** (era `"all"`) | `"all"` intentaba también MSI, que necesita descargar WiX. La app es sólo para Windows (`rdev`, registro de Windows para el autoarranque), así que el resto de los formatos no aplica |
| `nsis.installMode` | `currentUser` | instala en `%LOCALAPPDATA%` sin UAC. `perMachine` pediría administrador para nada: la app no escribe fuera del perfil del usuario |
| `nsis.languages` | `["Spanish", "SpanishInternational", "English"]` | Windows en es-AR (LANGID 11274) no coincide con ninguna de las dos variantes de NSIS, así que cae en **la primera de la lista**: por eso "Spanish" va primera. Verificado: el bundler emitió `Spanish.nsh` y `SpanishInternational.nsh` |
| `nsis.installerIcon` | `icons/icon.ico` | sin esto el instalador sale con el ícono genérico de NSIS |

**Accesos directos** (leídos del `installer.nsi` generado, no asumidos): el del
menú Inicio se crea **siempre**; el del escritorio va como casilla tildada en la
última pantalla (`MUI_FINISHPAGE_SHOWREADME`), y se crea sin preguntar en
instalación silenciosa o `/P`.

### El instalador no va al repo

Queda en `app-nativa/target/release/bundle/nsis/`, que ya está cubierto por las
reglas `target/` y `app-nativa/target/` del `.gitignore`. No hizo falta agregar
nada.

### Deuda que sigue abierta

- **Sin firma digital.** Windows muestra "Windows protegió su PC" en la primera
  ejecución y hay que pasar por "Más información" → "Ejecutar de todas formas".
  Está documentado en el README, que es lo único que se puede hacer sin comprar
  un certificado.
- **Los tres ajustes que se guardan y no se aplican** (vocabulario, muletillas,
  modo de limpieza) siguen igual que en el Plan 3: es deuda del núcleo, no del
  empaquetado.
- **`allow_downgrades` queda en `true`** (el default). Con una sola versión
  publicada no cambia nada, pero conviene revisarlo cuando haya una segunda.

## Arreglo — "Error" en el primer arranque (21/7/2026)

### El síntoma

Instalación nueva. El asistente hace lo correcto —baja el modelo de medición,
con su barra de progreso— y arriba, en la cabecera, una pastilla **roja que dice
"Error"**. No había ningún error: el motor no puede cargar un modelo que todavía
no se descargó, y eso se reportaba como `Estado::Error`. Correcto por dentro,
mentira por fuera, y en el primer minuto de uso.

### El arreglo: un estado propio, no un mensaje más suave

Se agregó `Estado::SinModelo(String)`, con clave `"sin-modelo"` y etiqueta
"Falta el modelo". **Funcionalmente se comporta como `Error`** (no se dicta, y
apretar el atajo contesta), pero se cuenta distinto en los cuatro lugares donde
el estado se ve: clave, etiqueta, color de la bandeja (ámbar `#d9a441`, nunca el
rojo del error) y respuesta al atajo ("Todavía no descargaste el modelo. Abrí
Ajustes y bajá uno para dictar." en vez del motivo con tono de fallo).

`Error` queda para lo que sí se rompió: el modelo está pero no carga, el atajo no
se enganchó, no hay dónde escribir el historial. **La regla que ordena todo el
cambio es que no se convierten errores reales en avisos** — el objetivo era dejar
de mentir en el caso bueno, no esconder los malos.

### Cómo se distingue "no hay archivo" de "no carga" — por tipo, no por texto

`Transcriber::new` devolvía `Result<Self, String>`, y ahí "no encuentro el modelo
en X" y "no pude cargar el modelo X: header inválido" eran la misma cosa: una
cadena. Elegir el estado a partir de ella habría significado buscar subcadenas en
un texto escrito para el usuario, o sea una regresión silenciosa esperando a la
próxima corrección de redacción.

Ahora hay dos tipos, uno por escalón:

- **`mithflow_core::stt::ErrorDeModelo`** — `Faltante(PathBuf)` cuando la ruta no
  apunta a ningún archivo (`stt.rs` ya validaba la existencia por separado, sólo
  había que no perder esa información), `NoCarga(String)` para todo lo demás.
  `falta_el_archivo()` es la única pregunta que se hace hacia afuera.
- **`estado::FalloDelMotor`** — `SinModelo` / `Roto`, con `estado()` como único
  lugar donde se decide qué estado le toca a cada uno. `rutas::modelo` y
  `motor::clasificar` lo producen; `director::motor_no_arranca` lo consume.

**El orden dentro de `Transcriber::new` importa y quedó documentado en el
código**: los backends de ggml se inicializan ANTES de mirar el disco. Si faltan
las DLLs, eso es lo que hay que decir; devolver `Faltante` mandaría al usuario a
descargar 1,5 GB para volver a fallar por lo mismo.

`MITHFLOW_MODELO` apuntando a una ruta que no existe sigue siendo `Roto` y no
`SinModelo`, aunque tampoco haya archivo: la diferencia no es si hay un `.gguf`
sino qué tiene que hacer el usuario, y descargar un modelo no arregla una
variable de entorno mal escrita.

### Dos guardas que impiden esconder una falla

1. `SinModelo` **no pisa** un `Error` ya publicado (`if !self.estado.es_falla()`
   en `motor_no_arranca`). Si el atajo no se enganchó y encima no hay modelo, lo
   que se ve es el problema real. Es la contraparte de la guarda que ya existía
   para que `MotorListo(Ok)` no pisara un error con un "Listo" falso.
2. `es_falla()` es `true` **sólo** para `Error`, con un test que recorre todos los
   estados: si mañana alguien marca otro como falla, se rompe ahí.

### Lo que el usuario ve ahora

| Situación | Antes | Ahora |
|---|---|---|
| Primer arranque, asistente bajando el modelo | pastilla roja "Error" | sin pastilla (el asistente ya lo dice); bandeja ámbar "Falta el modelo" |
| Sin modelo, con el asistente cerrado | roja "Error" | ámbar "Falta el modelo" + el motivo y qué hacer |
| Modelo presente que no carga (corrupto, sin memoria, backend roto) | roja "Error" | **igual**: roja "Error" |
| Atajo que no se pudo enganchar / micrófono que falla | roja "Error" | **igual**: roja "Error" |

Con el asistente en pantalla se esconden la pastilla y el botón de pausa, pero
**sólo** en `sin-modelo`: mostrar "Falta el modelo" arriba mientras la pantalla
entera dice "Bajando el modelo" es redundante, y pausar un dictado que todavía no
puede existir no significa nada. Un error de verdad se sigue mostrando aunque el
asistente esté abierto.

### Al arrancar no suena el tono de error

`SinModelo` publica el aviso con nivel `"info"` y sin tono; `Error` mantiene tono
y nivel `"error"`. Nadie pidió nada todavía —la app se acaba de abrir— así que un
beep de error en el primer arranque es exactamente el ruido que este cambio vino
a sacar. Cuando el usuario SÍ pide algo (aprieta el atajo), `SinModelo` contesta
con la misma realimentación que cualquier otro estado que no dicta: sin eso, "no
pasó nada" y "todavía no se puede" se ven igual.

### El backend simulado también mentía

`src/desarrollo/backendSimulado.ts`, escenario `primer-arranque`, publicaba
`estado: "error"` a mano. Quedó actualizado: si el simulado miente, el próximo
que mire la pantalla de bienvenida en el navegador ve un bug que ya no existe.

### Verificación

- `cargo test --workspace`: 88 núcleo (eran 84) + 59 app (eran 49). Los cuatro
  del núcleo incluyen los dos que valen: `Transcriber::new` sobre una ruta vacía
  da `Faltante`, sobre un archivo de basura da `NoCarga`. Cargan los backends de
  ggml de verdad, así que prueban el orden real y no una maqueta.
- `cargo clippy --workspace --all-targets -- -D warnings`: limpio.
- `npm run build`: sin errores de TypeScript.
