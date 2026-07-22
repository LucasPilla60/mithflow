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

---

## Arreglo — el motor arranca sin reiniciar la app (21/7/2026)

### El síntoma

Instalación nueva, secuencia real de uso:

1. La app arranca. No hay ningún `.gguf` en `%APPDATA%\MithFlow\models\`.
2. `main.rs` resuelve la ruta del modelo, falla, y el hilo del motor **nunca se
   lanza**.
3. El asistente baja el modelo de medición, perfila la máquina y baja el
   recomendado. Los dos quedan bien en el disco.
4. La app **sigue** diciendo "todavía no hay ningún modelo descargado. Bajá uno
   desde Ajustes para poder dictar." El dictado no funciona.
5. Recién cerrando y volviendo a abrir la app, anda.

El usuario hizo exactamente lo que la app le pidió, la app le seguía pidiendo lo
mismo, y el mensaje **ya era falso**. Era contradictorio dentro de la misma
ventana: Ajustes consulta el disco al abrirse y mostraba los modelos como
"Descargado" mientras la cabecera insistía en que no había ninguno. El arreglo
anterior ("Error" en el primer arranque) había corregido el COLOR de ese estado;
esto corrige que el estado no se moviera nunca.

### El arreglo: un solo camino para arrancar el motor, invocable dos veces

El motor tenía un único momento de arranque, enterrado en `main.rs::arrancar_motor`.
Ese cuerpo se movió a **`motor::resolver_y_lanzar(app, al_director, cfg)`**, que es
ahora el camino que usan los dos momentos posibles:

1. el arranque de la aplicación (`main.rs::preparar`);
2. una descarga que termina bien **con el motor ausente**
   (`director::lanzar_el_motor`).

Que sea una sola función y no dos parecidas es lo que garantiza que el motor
lanzado en caliente resuelva el modelo con **las mismas reglas** que el del
arranque: `MITHFLOW_MODELO`, después el modelo elegido en Ajustes (que puede ser
"automático"), después la sustitución por cualquier otro descargado con su aviso.
Un segundo camino habría podido divergir en silencio.

### Quién decide, y por qué ahí y no en el hilo de la descarga

El hilo de la descarga **no decide nada**: manda `Mensaje::ModeloDescargado` al
director y sigue. Quién sabe si hay un motor corriendo es el director y nadie
más, y esa pregunta no se puede contestar leyendo un espejo desde otro hilo sin
abrir una carrera: dos descargas seguidas —justo lo que hace el asistente, el
modelo de medición y después el recomendado— leerían las dos "no hay motor" y
lanzarían dos motores con hasta 1,5 GB de pesos cada uno. En el director hay una
cola y un solo lector: la segunda descarga ve lo que dejó la primera.

La regla vive suelta en `director::tras_una_descarga(&Estado) -> TrasLaDescarga`,
sin tocar nada, para poder probarla sin levantar una aplicación de Tauri:

| Estado al terminar la descarga | Qué se hace | Por qué |
|---|---|---|
| `SinModelo` | **`Lanzar`** | no hay motor y no lo hubo nunca: el caso del primer arranque |
| `Cargando` | `YaEstaCargando` | ya hay uno cargándose; otro serían dos modelos compilando shaders a la vez |
| `Listo` / `Grabando` / `Transcribiendo` | `AlReiniciar` | reemplazar el modelo en caliente son 1,5 GB y ~2 s en medio del trabajo |
| `Error` | `AlReiniciar` | puede ser el atajo o el historial, y ninguno se arregla con un `.gguf` nuevo |

`Error` entra en `AlReiniciar` y no en `Lanzar` a propósito: arrancar un motor
encima de una falla real la taparía con un "Listo" que no sería cierto. Es la
misma familia de guardas que ya protegía a `MotorListo(Ok)`.

### El estado se mueve, y se ve

`SinModelo` → `Cargando` → `Listo`. El paso intermedio se publica **antes** de que
el modelo esté en memoria, y eso es lo que importa: cargar los pesos y compilar
los shaders de Vulkan son **entre veinte segundos y un minuto** la primera vez en
la máquina (17 s de shaders y 1,8 s de carga medidos en el escritorio; en una
integrada, más). Ésa es la cifra única que dicen el código, la interfaz y esta
documentación: antes el director prometía "unos segundos" y el asistente "entre
veinte segundos y un minuto", y quien apretaba la tecla a los diez segundos creía
que la app se había colgado.

Sin publicar el paso intermedio, el usuario que acaba de bajar el modelo se queda
mirando una pantalla que no cambia durante casi un minuto. Publicarlo además es lo
que cierra la puerta al segundo lanzamiento, porque la descarga siguiente ya no ve
`SinModelo`: las dos mitades son la misma línea de código.

`pasa_a_listo_cuando_el_motor_avisa` quedó como función con nombre y no como
`matches!` suelto justamente para poder decir eso en su documentación: es la
puerta por la que sale el arranque en caliente, y si alguien la afloja el motor
terminaría de cargar con la interfaz clavada en "Cargando…" para siempre. Por el
mismo motivo el estado que publica `lanzar_el_motor` sale de
`estado_al_lanzar_en_caliente()` y no de un `Estado::Cargando` escrito adentro:
es el eslabón del que cuelgan las dos mitades, y un test que lo copiara de su
lado seguiría verde aunque el director dejara de publicarlo.

**El director nunca se bloquea.** `motor::lanzar` crea el hilo y vuelve; el minuto
transcurre allá. El director sigue atendiendo la cola mientras tanto. `lanzar` es
además **privada**: el único camino para arrancar el motor es
`resolver_y_lanzar`, y que lo garantice el compilador es más fuerte que pedirlo
en un comentario.

### La carrera con el perfilado — el error más caro que podía quedar

Desde que una descarga arranca el motor, el asistente hace esto: baja el modelo
de medición → el motor empieza a cargar → menos de un segundo después empieza a
**medir la máquina**, que carga su propio `Transcriber`. Dos modelos peleando por
la misma GPU: la medición daría una máquina más lenta de lo que es y la
recomendación saldría para abajo. Sería el peor error posible acá — silencioso, y
se lleva puesta la calidad de todos los dictados que vengan después.

Lo serializa un `Mutex<()>` estático, `motor::CARGA_DE_MODELO`. **Se serializa la
CARGA, no el uso**: el motor suelta el testigo apenas terminó de cargar y
calentar, y después sigue vivo con sus pesos en memoria sin retener nada. Nadie
lo toma dos veces, así que no hay forma de trabarse. Un `Mutex` envenenado no
invalida nada (protege `()`, no un dato con invariantes), así que se sigue igual
en vez de tumbar la carga.

**Serializar no alcanzaba.** Que las dos cargas no sean simultáneas no impide que
las dos copias queden **residentes**: el motor no libera sus pesos ni sus buffers
de Vulkan cuando suelta el testigo, así que el `Transcriber` del perfilado se
abría encima (~1 GB con el `Q4_K_M`, ~3,2 GB si fuera el `F16`). En el escritorio
no se nota; en una notebook con gráficos integrados y 8 GB —la tercera máquina
objetivo del proyecto— ese segundo `Model::load` puede fallar o caer a CPU **en
silencio**, y entonces el perfilado mide una máquina más lenta de la que es y
recomienda un modelo peor del que corresponde. Es exactamente el error que este
mutex vino a evitar, entrando por la otra puerta.

Se cierra por donde correspondía: **el motor mide él mismo**. Cuando el `.gguf`
que cargó es el de perfilado —el caso del primer arranque, y el permanente en las
máquinas chicas— corre `hardware::medir_factor_tiempo_real` sobre su propio
`Transcriber`, ya cargado y caliente, y publica el número en
`motor::MotorResidente`. `perfilar_hardware` espera el testigo, encuentra esa
medición y arma el perfil con `hardware::perfil_con_factor` **sin abrir un
segundo modelo**. El primer arranque termina además más rápido: antes eran los
~40 s del motor más los ~25 s del perfilado; ahora son los del motor más tres
inferencias.

Sólo se reusa con el modelo de perfilado: el número depende del modelo con el que
se mide (uno más chico da un factor más alto) y los umbrales están calibrados
sobre el `Q4_K_M`. Con otro modelo cargado se mide como siempre, y ahí se compara
el backend del perfilado contra el del motor: si no coinciden —uno en Vulkan y el
otro en CPU— se avisa, porque el número que decidió el modelo no sería el de la
máquina que va a dictar.

**Y la espera tiene tope.** `testigo_de_carga` recibe un plazo
(`TOPE_DE_ESPERA`, dos minutos) y devuelve `None` si vence: sin él, un driver de
Vulkan colgado del otro lado dejaba a la aplicación en `Cargando` para siempre,
contestándole al atajo "todavía estoy preparando el motor" hasta que el usuario
la matara. Vencido el plazo se sigue igual, que es estrictamente mejor que no
arrancar nunca. El perfilado además **suelta el testigo apenas termina de medir**,
antes de emitir el evento: el motor no tiene por qué esperar a que algo cruce al
webview. La función lleva `#[must_use]` con motivo, porque
`motor::testigo_de_carga(…);` compilaba y era un no-op silencioso.

El costo visible es que la pantalla "Midiendo tu máquina" puede esperar a que el
motor termine. Se dice en la propia pantalla, en castellano, en vez de dejar al
usuario mirando una barra quieta.

### Lo que el usuario ve ahora

| Momento | Antes | Ahora |
|---|---|---|
| Termina la descarga con el motor ausente | "Modelo descargado. Reiniciá MithFlow para empezar a usarlo." + pastilla clavada en "Falta el modelo" | "Modelo descargado. Estoy cargando el motor; en cuanto diga «Listo» podés dictar." + pastilla "Cargando…" |
| El motor termina de cargar | (no pasaba) | pastilla "Listo": ya se puede dictar, sin reiniciar |
| Termina la descarga con el motor ya cargado | "Reiniciá MithFlow para empezar a usarlo." | "El cambio de modelo aplica cuando reinicies MithFlow." (**igual que antes en lo funcional**: no se reemplaza en caliente) |
| Última pantalla del asistente | "Reiniciá MithFlow para que el motor lo cargue" | lo que el motor esté haciendo **de verdad**, leído del estado en vivo |

Ningún aviso de descarga puede volver a decir que no hay ningún modelo —se acaba
de bajar uno— y hay un test que lo recorre. La simetría también: los dos casos en
los que el modelo recién bajado NO es el que se va a cargar ahora **sí** dicen que
hace falta reiniciar. Callarlo sería la mentira al revés.

La última pantalla del asistente aclara además, cuando corresponde, que se va a
dictar con el modelo de medición y que el elegido se carga al reiniciar: es la
consecuencia honesta de no reemplazar el modelo en caliente.

### Un `.gguf` que nunca se verificó no puede arrancar el motor

`models::descargar` tenía una rama corta: si el modelo "ya está", devuelve `Ok`
sin bajar nada. Y "ya está" lo decidía `esta_descargado`, que **sólo compara el
tamaño**. Ese `Ok` es indistinguible del de una descarga real, así que desde este
cambio dispara el lanzamiento del motor sobre un archivo cuyo hash no miró nadie
nunca: una copia traída a mano de otra máquina, o un archivo del tamaño exacto y
contenido distinto.

Lo peor era que el control ya existía. `models::verificar_instalado` está escrita
justo para esto —recalcula el hash del modelo instalado y lo compara con el
compilado— y **no tenía un solo llamador en todo el repositorio**: un control
documentado y muerto.

Ahora la rama corta pasa por ella, y la invariante del módulo es que **un `Ok` de
`descargar` significa que ese archivo verificó contra el hash compilado**, salga
por la rama que salga. Un archivo que no verifica no se da por bueno y **no se
borra**: se renombra a `<nombre>.gguf.invalido`. Borrarlo sería destruir 1,5 GB
que el usuario puede haber traído a mano (esto no es un `.part` de la propia
descarga); dejarlo con su nombre bueno sería peor todavía, porque con el tamaño
correcto `esta_descargado` volvería a darlo por instalado en el próximo arranque.
Renombrarlo deja al usuario en el único estado honesto —no hay ningún modelo
usable, "Falta el modelo"— con el botón "Descargar" funcionando de nuevo, que es
la salida.

Se decidió **no** publicar `Estado::Error` en ese caso, y es deliberado: `Error`
no ofrece salida y el usuario quedaría trabado, mientras que `SinModelo` más el
motivo en rojo de la descarga dice la verdad y el reintento resuelve. Lo que sí
es innegociable, y es lo que se cerró, es que ese hash que no coincide **nunca**
se cuente como "listo para dictar".

### El backend simulado, otra vez

`src/desarrollo/backendSimulado.ts` dejaba el estado clavado en `sin-modelo`
después de una descarga: o sea, seguía reproduciendo el bug. Ahora publica
`sin-modelo` → `cargando` → `listo` con sus avisos, con la carga acortada a 5 s
para poder iterar pero **no a cero**, porque el paso intermedio es justamente lo
que hay que poder mirar.

Y reproduce los **tres** casos de `TrasLaDescarga`, no dos: colapsar
`YaEstaCargando` con `AlReiniciar` enseñaba en el navegador un comportamiento que
la app no tiene. El orden también estaba al revés —avisaba y después publicaba el
estado, cuando el director hace lo contrario—: un simulado que enseña el orden
equivocado es peor que no tenerlo.

### Los textos que mentían cuando el estado es `Error`

`Estado::Error` es alcanzable de verdad en una instalación nueva: el atajo puede
quedar tomado por otra aplicación mientras el asistente está abierto. Tres textos
lo trataban como si fuera "todavía no hay modelo":

- La última pantalla del asistente mandaba a **reiniciar MithFlow**. Reiniciar no
  devuelve una tecla que tiene otra app. Ahora `error` tiene su propia rama, dice
  que el motivo está arriba y nombra la causa más común y dónde se arregla.
- Ajustes decía "la única excepción es no tener ninguno". La excepción no es "no
  tener ninguno" sino que el estado sea `SinModelo`; con `Error` bajar un modelo
  no arranca nada.
- `App.tsx` escondía el detalle del estado con el asistente abierto, así que se
  veía una pastilla roja "Error" **sin un solo motivo** al lado de un "Todo
  listo". El de `sin-modelo` se sigue escondiendo —el asistente ES la respuesta a
  eso—; el de `error`, no. El título de esa pantalla tampoco dice "Todo listo"
  cuando no lo está.

### Ocultar la pastilla y ocultar "Pausar" son dos cosas distintas

Estaban atadas a la misma condición (`asistente && sin-modelo`). Desde que la
descarga mueve el estado a `cargando` con el asistente todavía abierto, "Pausar"
reaparecía en medio de la bienvenida — y pausar un dictado que todavía no puede
existir no significa nada durante **todo** el asistente. La pastilla, en cambio,
sí tiene que verse: los textos nuevos mandan a mirarla ("en cuanto arriba diga
«Listo»"). Son dos condiciones ahora.

### Verificación

- `cargo test --workspace`: **91 núcleo + 75 app** (eran 88 y 67). Los nuevos
  cubren: un modelo instalado que no verifica no pasa por bueno y pierde su
  nombre; el perfil armado con un factor reusado decide igual que la función
  pura; la carga de modelo es de a una por vez **con dos hilos de verdad**; la
  espera del testigo tiene plazo; sólo el modelo de perfilado sirve para medir;
  el perfilado reusa la medición del motor sin abrir un segundo modelo; un
  backend distinto al del motor se avisa; y la invariante "no puede existir un
  motor vivo con el estado en `SinModelo`".
- `cargo clippy --workspace --all-targets -- -D warnings`: limpio.
- `npm run build` y `tsc --noEmit`: sin errores.
- Instalador NSIS regenerado y copiado a `D:\MithFlow\instalador\`.

---

## Arreglo — tres defectos antes de instalar en las otras dos notebooks (22/7/2026)

Los tres salieron de una auditoría y se cerraron juntos porque los tres muerden
en la **misma máquina**: la notebook de ~8 GB con gráficos integrados, donde el
usuario no va a estar para explicar nada ni para reiniciar la app.

### 1. `"auto"` cargaba el modelo más grande descargado, mirara o no la máquina

`rutas::modelo` resolvía la sustitución con
`Modelo::TODOS.into_iter().find(esta_descargado)`. `TODOS` va **de mayor a
menor**, así que `find` devolvía siempre el más pesado que hubiera en el disco.
Y `"auto"` está documentado en `ajustes.rs` como "el que diga el perfilado de
hardware", que es exactamente lo que esa línea no consultaba.

En una notebook de 8 GB con un `F16` en el disco —bajado a mano desde Ajustes,
copiado de otra máquina— eso cargaba 1,5 GB de pesos que según
`hardware::RAM_MINIMA_F16_GB = 12.0` no entran. El aviso que protege de eso
(`noEntra`) vive **sólo en el asistente**: ni Ajustes ni esta sustitución lo
tenían.

**El arreglo es un criterio de memoria compartido, no una copia de los
umbrales.** `hardware` expone ahora `ram_minima_gb(modelo)` y
`entra_en_memoria(modelo, ram, gpu_dedicada)`, y `limitar_por_memoria` —la
función que ya decidía esto para el perfilado— quedó **escrita sobre
`entra_en_memoria`** en vez de repetir los `if`. Son la misma decisión mirada
desde dos lados ("¿entra?" y "¿cuál pongo en su lugar?") y hay un test que las
recorre juntas: un modelo que `entra_en_memoria` acepta no puede ser bajado por
`limitar_por_memoria`, y uno que rechaza no puede sobrevivir.

La sustitución elige **el más grande que entre**; si no entra ninguno, el más
chico que haya —el que menos aprieta— y lo dice: "pide unos 12 GB de RAM y esta
máquina tiene 8… bajá uno más chico desde Ajustes". No dictar no es mejor que
dictar despacio, pero cargarlo en silencio sí es peor que las dos cosas.

**El paso 2 no se toca**: un modelo elegido a mano y descargado se carga igual.
La interfaz muestra al lado de cada uno cuánta RAM pide (`ram_minima_gb`, que
ahora sale del núcleo y no de una tabla propia en `comandos.rs`) y la decisión
es del usuario. Lo que se arregló es lo que la app decide **por él**.

**Preguntarle a `wgpu` cuesta, así que se pregunta sólo si puede cambiar la
respuesta.** `describir_gpu()` enumera los adaptadores de Vulkan y DX12 —cientos
de milisegundos en el camino de arranque—; cuando todos los modelos descargados
entran por RAM del sistema, tener o no placa dedicada no cambia cuál se elige.
El escritorio y la MSI Katana no llegan a preguntar; la notebook chica, una vez.

### 2. Si fallaba el `spawn` del hilo del motor, la app quedaba en "Cargando…" para siempre

`motor::lanzar` registraba el error en la consola y devolvía **igual** un
`Sender` cuyo receptor se había ido con el closure que nunca corrió. El director
lo guardaba, publicaba `Cargando`, y nadie iba a mandar nunca `MotorListo`.

Lo peor era que no tenía fondo: con el estado en `Cargando`,
`tras_una_descarga` contesta `YaEstaCargando`, así que **ninguna descarga
posterior lo recuperaba**. La única salida era matar el proceso — mientras la
pantalla prometía que en cuanto dijera «Listo» se podía dictar.

Ahora la rama `Err` manda `MotorListo(Err(FalloDelMotor::Roto(…)))` antes de
devolver el canal huérfano: el estado pasa a `Error` en rojo y el motivo dice la
única salida que hay ("Cerrá MithFlow y volvé a abrirlo"). Es `Roto` y no
`SinModelo` porque no falta ningún archivo: el sistema operativo no dio un hilo,
y descargar un modelo no arregla eso.

El aviso se extrajo a `avisar_que_el_hilo_no_arranco` para poder probarlo: forzar
que `Builder::spawn` falle no es razonable, pero que el director se entere sí
tiene que estar cubierto. El clon del emisor es obligatorio, y no cosmético: el
`spawn` consume el closure —y con él el `Sender`— aunque falle.

### 3. Se pedía "reiniciá" justo cuando el motor cargaba ese mismo modelo

`comandos::escribir_ajustes` avisaba "El modelo cambia la próxima vez que abras
MithFlow" mirando **sólo** `guardados.modelo != anteriores.modelo`, sin mirar el
motor. El camino más común del asistente lo disparaba siempre:

1. el ajuste arranca en `"auto"`;
2. se baja el `Q4_K_M` → "Modelo descargado. Estoy cargando el motor; en cuanto
   diga «Listo» podés dictar.";
3. el perfilado recomienda `Q4_K_M` y el asistente escribe `modelo: "Q4_K_M"`;
4. `"auto" != "Q4_K_M"` → **"El modelo cambia la próxima vez que abras
   MithFlow."** Falso: el motor está cargando ese mismo archivo;
5. la pantalla final dice que el modelo está cargado y ya se puede dictar.

Tres mensajes seguidos, dos contradictorios, en el primer minuto de uso.

**La decisión se mudó a donde está la información.** El comando ve que la clave
cambió y nada más; si hay motor y con qué archivo lo sabe el director. Ahora
`Mensaje::Ajustados` la lleva hasta allá y la contesta
`director::aviso_al_cambiar_el_modelo`, que **reusa `tras_una_descarga`** en vez
de duplicar la regla: es literalmente la misma pregunta ("¿esto se usa ahora o
al reiniciar?"). Lo único que agrega es lo que una descarga no necesita: si el
`.gguf` que el motor tiene entre manos ya es el elegido, no cambia nada y no hay
nada que avisar.

Para eso el director guarda `modelo_en_el_motor`, que le llega en el nuevo
`motor::Motor { al_motor, modelo }` — el mismo tipo por los **dos** caminos de
arranque (el de la aplicación y el de la descarga en caliente), que es lo que
impide que diverjan. `Motor::ausente()` reemplaza al canal huérfano que armaba
`main.rs` a mano.

La comparación es **por nombre de archivo** y no por ruta, igual que en
`motor::es_el_modelo_de_perfilado`: `MITHFLOW_MODELO` puede apuntar al mismo
modelo fuera de `%APPDATA%` y sigue siendo el mismo modelo. `"auto"` no nombra
ningún archivo, así que nunca cuenta como igual: conservador en la dirección
correcta —en el peor caso se avisa de más, nunca se niega lo que el usuario
acaba de hacer.

Y las simetrías, que son la mitad que no se puede aflojar y tienen test propio:
elegir **otro** modelo con el motor cargando o cargado sigue avisando; con
`Error` también, porque el cambio tampoco aplica solo; sin motor
(`SinModelo`) no se avisa nada, porque no hay nada que reiniciar.

`backendSimulado.ts` reproduce las dos guardas (lleva su propio
`modeloEnElMotor`): un simulado que enseñe el bug arreglado es peor que no
tenerlo.

### Verificación

- `cargo test --workspace`: **94 núcleo + 86 app** (eran 91 y 75). Los catorce
  nuevos cubren: el criterio de memoria por modelo y su equivalencia con
  `limitar_por_memoria` (barriendo RAM, `NaN` y placa dedicada); la sustitución
  en una máquina grande, en una chica, cuando no entra ninguno y sin nada
  descargado; que el aviso aparezca sólo cuando hay algo que contar; que un hilo
  de motor que no arranca se le informe al director como falla con salida; y los
  cuatro caminos del cambio de modelo en Ajustes, incluido el del asistente que
  disparaba el aviso falso.
- `cargo clippy --workspace --all-targets -- -D warnings`: limpio.
- `npm run build`: sin errores de TypeScript.
- Instalador NSIS regenerado y copiado a `D:\MithFlow\instalador\`.

---

## Plan 6 — la ventanita de grabación (indicador flotante)

### El pedido, con las palabras del usuario

> "Cada vez que aprieto F9 para poder hablar me gustaría que aparezca un iconito
> como tiene Wispr Flow, que muestre el gráfico de que se está hablando, en
> alguna parte de la compu. Porque si no tengo abierta la aplicación no me doy
> cuenta si dice grabando o no grabando, y por ahí estoy hablando al pedo."

Hasta acá la única realimentación con la ventana principal cerrada —que es como
se usa esta app casi siempre— eran los cuatro tonos. **Un tono dice "arrancó", no
dice "te estoy escuchando".** Quien tiene el micrófono silenciado por Windows, o
el auricular equivocado como entrada por defecto, escucha exactamente lo mismo
que quien está dictando bien, y se entera recién cuando no aparece ningún texto.

### La restricción innegociable: no puede tomar el foco

MithFlow pega el texto en la ventana que el usuario tenga enfocada. Una ventanita
que se lleve el foco mueve el cursor de texto y el dictado termina en otro lado —
que es **exactamente** el defecto que el proyecto ya resolvió una vez suprimiendo
la tecla con `rdev::grab`.

Se cierra por cuatro vías, y ninguna sobra:

| Vía | Qué hace | Por qué no alcanza sola |
|---|---|---|
| `focusable(false)` | `WS_EX_NOACTIVATE` en el `CreateWindowEx` | — es **la** garantía |
| `focused(false)` | `SW_SHOWNOACTIVATE` en el primer `show`; y en wry, saltea el `MoveFocus` inicial del WebView2 | `tao` **consume** la marca `MARKER_DONT_FOCUS` al primer `show`: del segundo en adelante usa `SW_SHOW`, que activa |
| `set_ignore_cursor_events(true)` | `WS_EX_TRANSPARENT`: los clics la atraviesan | no es sobre el foco, pero una ventanita que come clics en el medio de la pantalla es peor que no tenerla |
| nunca llamar a `set_focus` | — | la ventana principal tiene `ventana::enfocar`; ésta no tiene equivalente y no debe tenerlo |

**Hallazgo — `focused(false)` sin `focusable(false)` habría robado el foco a
partir de la SEGUNDA grabación.** En `tao 0.35.3`
(`platform_impl/windows/window_state.rs:327-337`), `set_window_flags` usa
`SW_SHOWNOACTIVATE` sólo si `MARKER_DONT_FOCUS` está puesta —y la apaga en el
acto—; después vuelve a `SW_SHOW`. La primera grabación se habría visto bien y de
la segunda en adelante el cursor se iría del campo de texto. Es el defecto de
peor clase posible acá: intermitente, y se lleva puesto el dictado del usuario.

### Cómo se verificó (spike, sin arrancar MithFlow)

No se puede probar con tests unitarios y no se podía arrancar la app —el usuario
la tiene abierta y usándola—, así que se hizo un spike aparte:
`spike-superpuesta/`. Crea una ventana con **exactamente** las mismas opciones,
sobre la misma versión de `tao` que usa Tauri 2.11, y mide con Win32 lo que de
verdad importa. Con el foco en el editor del usuario:

```text
ventana enfocada al arrancar: «Revisar documentación y … - Cosmo-Gestion - Cursor»
foco de teclado / cursor de texto de ese hilo: (329340, 0)

PASA WS_EX_NOACTIVATE en el estilo extendido  (GWL_EXSTYLE = 0x080c0138)
PASA WS_EX_TRANSPARENT (los clics la atraviesan)
PASA el foco no se movió tras «primer show»
PASA el foco no se movió tras «hide»
PASA el foco no se movió tras «SEGUNDO show (el que usa SW_SHOW)»
PASA la ventanita no tiene el foco de teclado

OK: la ventanita no le puede robar el foco al usuario.
```

`GetForegroundWindow` y el `hwndFocus` del hilo de la otra aplicación
(`GetGUIThreadInfo`, que es donde vive el cursor de texto) **no se movieron** en
ninguno de los tres pasos. El paso del segundo `show` es el que justifica todo el
spike: es el que se escapaba.

Del lado del WebView2, la otra puerta posible, `wry 0.55.1` sólo llama a
`MoveFocus` en dos lugares: al crear la vista **si `attributes.focused`**
(`webview2/mod.rs:546`), y al recibir `WM_SETFOCUS` en la ventana padre
(`:1255`). `WebviewWindowBuilder::focused(false)` apaga el primero —pone el flag
en el builder de la ventana **y** en el del webview
(`tauri/src/webview/webview_window.rs:535-539`)— y `WS_EX_NOACTIVATE` impide el
segundo, porque el padre nunca pasa a primer plano.

### De dónde sale el nivel de audio

De `mithflow_core::audio::Medidor`, nuevo: **lo escribe el propio callback de
`cpal` en átomos y lo lee el director sin lock**.

La alternativa era que el director tomara el `Mutex` del buffer veinticinco veces
por segundo para mirarle la cola. Ese mutex es el que toma el callback en cada
chunk, y el callback de audio no puede esperar a nadie: lo que se pierde cuando
se pasa del plazo es audio del usuario. Acá el callback suma una pasada por el
chunk (una multiplicación y dos sumas por muestra, contra la copia al buffer que
ya hacía) y tres operaciones atómicas, y nadie bloquea a nadie.

Tres decisiones que no son obvias:

- **Se mide sobre el audio ya mezclado a mono**, igual que `downmix`, y no sobre
  las muestras intercaladas: es lo que va a recibir el modelo, y medir el
  intercalado daría otro número. Hay test que compara el RMS del medidor contra
  el del audio real.
- **La suma de cuadrados se acumula en `f64`.** En `f32`, una grabación de tres
  minutos pierde los aportes chicos contra un acumulador grande.
- **Se puede apagar.** `Medidor::activar(false)` hace que el callback lea un
  `bool` y vuelva. Es lo que sostiene la promesa de Ajustes: quien desactiva el
  indicador no paga **nada** por él — ni ventana, ni eventos, ni ciclos en el
  camino crítico del audio.

**Cada cuánto se emite:** 25 veces por segundo (`director::LATIDO_CON_MEDIDOR`,
40 ms) y **sólo mientras se graba con el indicador activado**. Sin eso el
director sigue con su latido de siempre (250 ms). En `transcribiendo` la
ventanita se queda a la vista pero el micrófono ya está cerrado: seguir emitiendo
sería mandar ceros. El evento `nivel-audio` es el único del sistema **sin comando
espejo**, y a propósito: no cuenta un hecho que haya que reconstruir al abrir una
ventana, sino el instante que está pasando. Un nivel viejo no significa nada.

### Lo que decide Rust y lo que dibuja el frontend

Las tres reglas viajan calculadas en el evento, por la misma razón que las
métricas del dashboard: se pueden equivocar, así que tienen que poder probarse.

- **`intensidad(rms)`** — la altura de la barra, de 0 a 1, en escala logarítmica.
  En lineal la voz vive pegada al piso: el ruido de fondo mide 0,0029 y la voz
  0,071, y con el eje de 0 a 1 las dos son la misma raya de un píxel. Con la
  escala −60 dBFS → −12 dBFS quedan: silencio 0 %, ruido de fondo ~19 %, umbral
  de voz ~42 %, voz normal ~77 %, voz fuerte 100 %.
- **`hay_voz(rms)`** — **el mismo umbral con el que el motor decide si
  transcribe** (`config::MIN_SPEECH_RMS`, vía `stt::tiene_voz`), no uno propio.
  Un indicador que se pone verde con un audio que después vuelve como "no se
  escuchó nada" es peor que no tener indicador. Hay test que barre amplitudes y
  exige que las dos funciones contesten lo mismo.
- **`cerca_del_tope(pasados, limite)`** — proporcional y acotada (15 % del tope,
  entre 5 s y 30 s), porque el tope es configurable entre 15 s y 10 minutos: un
  aviso fijo de 20 s estaría prendido desde el arranque con el tope mínimo.

### Dónde aparece

`superpuesta::esquina` es una función pura sobre el **área de trabajo** del
monitor —no sobre su resolución: abajo y centrada sobre la resolución quedaría
debajo de la barra de tareas, o sea escondida justo en la posición por defecto—.
Al ser pura se prueba sin Tauri y sin un segundo monitor enchufado, que es
exactamente la parte que se equivoca: un monitor a la izquierda del principal
tiene origen **negativo**, y una pantalla al 150 % informa el área en físicos
mientras la ventana se pide en lógicos.

El monitor se elige **por dónde está el puntero** (`cursor_position` →
`monitor_from_point`), con caída a `current_monitor` y después a
`primary_monitor`: quien tiene dos pantallas dicta en la que está mirando, y ahí
es donde está el mouse. Se recalcula en **cada** aparición y no una vez al
crearla, porque el usuario puede haberse mudado de monitor desde la grabación
anterior.

Las tres posiciones que ofrece Ajustes —abajo centrada (la de fábrica), arriba
centrada y abajo a la derecha— salen del backend
(`superpuesta::POSICIONES` → `Catalogo`), no de una lista escrita a mano en el
frontend que se desincronizaría.

### Por qué se crea al arrancar y no al apretar la tecla

Crear una ventana con su webview cuesta decenas de milisegundos y hay un
presupuesto de latencia de 900 ms (hoy en ~480). Se crea escondida en
`main::preparar` y apretar la tecla no hace más que moverla y mostrarla.

El orden dentro de `Director::empezar_a_grabar` deja el indicador **después** de
que el micrófono ya esté abierto y capturando, y `publicar()` lo pone **último**,
detrás del espejo, el evento y la bandeja: mostrar una ventana despacha al hilo
de la interfaz y espera, y nada de lo anterior tiene por qué quedar atrás de eso.
Del otro lado —al terminar— el hide ocurre en la transición a `Listo`, que llega
**después** de que el motor ya pegó el texto: cero impacto en el camino que el
usuario cronometra.

`reflejar` además contesta rápido cuando no hay nada que cambiar (un átomo
`A_LA_VISTA`, no un `is_visible()` que despacharía al hilo principal y
esperaría), porque el director publica también al pausar y al despausar.

### Qué ve el usuario

| Momento | Ventanita |
|---|---|
| aprieta la tecla | aparece abajo y centrada: punto rojo latiendo, "Grabando", reloj en `0:00` |
| habla | las barras se llenan **en teal** y siguen la cadencia del habla |
| se calla | las barras bajan a ~19 % y se ponen **grises**: sigue grabando, pero no hay voz |
| se acerca al tope | el reloj pasa a ámbar y en negrita |
| suelta la tecla | punto ámbar, "Transcribiendo…", el medidor se aplana y respira, el reloj queda clavado en cuánto duró |
| llega el texto (o falla) | desaparece |

Las capturas de los cuatro estados están en `docs/capturas/`, sacadas con
`superpuesta-mock.html` (backend simulado, sin micrófono y sin arrancar la app).

### Detalles que costaron su comentario

- **La ventanita se rearma en "Grabando" con cualquier estado que no sea
  `transcribiendo`.** Sin eso, el dictado siguiente arrancaría mostrando
  "Transcribiendo…" —lo último que quedó— hasta que llegara el primer nivel 40 ms
  después. Y `nivel-audio` también fuerza el modo, así que un `estado-cambiado`
  perdido no puede dejar el rótulo mintiendo.
- **El medidor no pasa por React.** Veinticinco cuadros por segundo × treinta
  barras serían setecientos elementos por segundo de reconciliación. Las barras
  se montan una vez y después se les toca el `transform`. React sólo se entera
  del modo, de si hay voz y del reloj, que cambia una vez por segundo.
- **Sin `StrictMode` en la entrada de la ventanita**, al revés que la principal:
  en desarrollo monta dos veces cada componente, y acá eso significa suscribirse
  dos veces a `nivel-audio` y correr la historia del medidor de a dos posiciones
  por cuadro.
- **`destroy()` y no `close()`** al desactivar el indicador: `close` pide permiso
  con un `CloseRequested`, y el manejador de `main` lo cancela para que cerrar la
  ventana principal esconda en vez de terminar la app. Ese manejador ahora está
  acotado a la ventana `main` por la misma razón.
- **El indicador se aplica en caliente**, al revés que el tope de grabación:
  apagarlo es una queja ("me molesta esta ventanita") y hacerla esperar a la
  próxima grabación sería no atenderla.
- **Capability propia** (`capabilities/superpuesta.json`) con **sólo**
  `core:event:default`: la ventanita escucha eventos y nada más. No lee ajustes,
  no descarga y no mueve ventanas. Es una ventana que está siempre por encima de
  todo: cuanto menos pueda hacer, mejor.
- **Dos entradas en Vite** (`index.html` y `superpuesta.html`) para que la
  ventanita no arrastre el dashboard: su bundle propio son 1,6 kB. `mock.html` y
  `superpuesta-mock.html` siguen fuera del build, y `dist/` sigue sin contener
  `MITHFLOW_SIMULADO`.

### Lo que NO se hizo, y por qué

- **Quitarla del Alt+Tab.** `skip_taskbar` de `tao` usa `ITaskbarList::DeleteTab`,
  que saca el botón de la barra de tareas pero no necesariamente de la lista de
  Alt+Tab; para eso haría falta `WS_EX_TOOLWINDOW`, que Tauri no expone y
  obligaría a meter Win32 crudo en el crate de la app. Es cosmético y está
  acotado: la ventana existe sólo durante el dictado y, si alguien la eligiera
  desde Alt+Tab, `WS_EX_NOACTIVATE` impide que se active igual.
- **Un medidor con historial largo o con espectro.** Treinta barras de 1,2 s
  alcanzan para ver la cadencia del habla; más sería decoración compitiendo con
  el dato, que es la misma regla del resto de la interfaz.
- **Arrastrar la ventanita con el mouse.** Sería incompatible con
  `WS_EX_TRANSPARENT`, que es lo que hace que los clics lleguen a lo que hay
  debajo. Se elige la esquina desde Ajustes.

### Verificación

- `cargo test --workspace`: **102 núcleo + 104 app** (eran 94 y 86). Los 26
  nuevos cubren: el RMS del medidor contra el del audio que ve el modelo, la
  mezcla de canales, la coincidencia con el umbral del motor, el reinicio de la
  ventana por lectura, el apagado, y una muestra `NaN` que no puede envenenar el
  indicador; la altura de la barra (monotonía, cotas, separación visible entre
  silencio, ruido y voz, y dónde cae el umbral); el aviso del tope con los tres
  topes configurables y con valores imposibles; la visibilidad por estado y por
  ajuste; la posición desconocida que cae en la de fábrica; y la ubicación en las
  tres posiciones, en un segundo monitor a derecha y a izquierda, con la pantalla
  al 150 % y en un monitor más chico que la propia ventanita.
- `spike-superpuesta`: la garantía del foco, medida con Win32 (arriba).
- `cargo clippy --workspace --all-targets -- -D warnings`: limpio.
- `npm run build`: sin errores de TypeScript.
- Instalador NSIS regenerado y copiado a `D:\MithFlow\instalador\`.
