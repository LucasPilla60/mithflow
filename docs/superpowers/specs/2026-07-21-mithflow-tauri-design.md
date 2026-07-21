# MithFlow nativo — Diseño (Tauri + Rust)

**Fecha:** 2026-07-21
**Versión:** 3 (tras dos rondas de revisión contra el código real)
**Estado:** listo para implementar, con precondiciones (sección 16)
**Reemplaza a:** `mithflow.py` + `dashboard.py`, que quedan funcionando hasta la validación final

---

## 1. Objetivo

Convertir MithFlow en una aplicación de escritorio instalable para Windows que reemplace a Wispr Flow, con dictado por voz 100% local y un dashboard de analíticas nativo.

Se instalará en **tres máquinas**:

| Máquina | Hardware | Estado |
|---|---|---|
| Escritorio | RTX 3080 10 GB, 32 GB RAM, Python 3.11.9 | Caracterizada |
| Notebook de Jaé | **Sin caracterizar** | Precondición 16.1 |
| Notebook personal | **Sin caracterizar** | Precondición 16.1 |

**Requisito central:** un único instalador que se adapte solo a cada máquina, sin recompilar ni configurar a mano.

## 2. Criterios de aceptación

1. Un instalador `.exe` único funciona en las tres máquinas.
2. En el escritorio, el tiempo **desde que se suelta la tecla hasta que el texto aparece** es menor a **0.9 s** para un dictado de 13 segundos (ver sección 2.1).
3. En cada notebook, ese mismo tiempo es menor a **5 s** para un dictado de 10 segundos.
4. El dictado funciona sin abrir ninguna ventana (bandeja del sistema).
5. El dashboard muestra cada dictado nuevo sin recargar ni esperar, tanto si estaba abierto como si se abre después.
6. Ningún error de micrófono, de transcripción o de red cierra la aplicación. Un fallo de carga de modelo se detecta al reiniciar y hace fallback automático (ver sección 12.2).
7. El historial existente se conserva, incluidas las entradas viejas con campos faltantes.
8. Los 12 casos de `tests/test_cleanup.py` pasan en la implementación Rust.
9. Un WAV de 10 segundos de silencio produce salida vacía y no pega nada.

### 2.1 Por qué el criterio 2 cambió de forma

La versión 1 de este spec pedía "no empeorar los 0.31 s de la versión Python". Eso estaba mal por tres razones, todas verificadas:

**a) El número 0.31 s no representa el uso real.** Se midió con el modelo caliente, sobre el mismo audio, repetidamente. Sobre los 13 dictados reales en modo `fast` del historial, la mediana de transcripción es **0.49 s** y el máximo 0.97 s.

**b) La métrica excluía casi medio segundo de latencia percibida.** `paste_text` espera 0.15 s antes del Ctrl+V y 0.30 s después, y ninguna de las dos entra en `transcribe_s` ni en `cleanup_s`. Lo que el usuario siente es aproximadamente 0.49 + 0.15 = **0.64 s**, no 0.31 s.

**c) Comparaba contra un backend distinto del elegido.** La línea base usa CUDA; este diseño elige Vulkan, que el propio spec reconoce entre 25 y 30% más lento en NVIDIA.

El criterio nuevo mide **latencia percibida** (soltar la tecla → texto visible), que es lo que importa, y se fija en 0.9 s: la mediana observada más el margen de Vulkan. Además hay dos fuentes de mejora identificadas que deberían compensar: eliminar el sobrecosto del intérprete de Python y reemplazar los `sleep` fijos del pegado por una espera basada en el número de secuencia del portapapeles.

**Precondición:** re-medir la línea base con protocolo reproducible antes de escribir código Rust (sección 16.2).

## 3. Arquitectura

Ejecutable único. Núcleo en Rust, interfaz en React + TypeScript, comunicados por comandos y eventos de Tauri. Sin procesos separados, sin navegador, sin servidor HTTP.

```
MithFlow.exe (Tauri v2)
├── Núcleo Rust (crates/core/src/)
│   ├── audio.rs      captura (cpal) → downmix a mono → resampleo a 16 kHz (rubato)
│   ├── stt.rs        carga de modelo, transcripción y filtro anti-alucinación
│   ├── cleanup.rs    limpieza por reglas (función pura, 5 reglas)
│   ├── hotkey.rs     atajo global con consumo de tecla
│   ├── paste.rs      portapapeles y Ctrl+V, sin esperas fijas
│   ├── hardware.rs   perfilado de hardware por benchmark (no por umbrales)
│   ├── models.rs     catálogo, descarga verificada y carga
│   ├── history.rs    lectura tolerante y escritura de history.jsonl
│   ├── config.rs     ajustes persistidos
│   ├── state.rs      máquina de estados del dictado
│   └── tray.rs       ícono y menú de bandeja
└── Interfaz (src/)
    ├── Dashboard     métricas, gráficos, historial paginado
    ├── Ajustes       tecla, modelo, vocabulario, sonidos, autoarranque, privacidad
    └── Asistente     primer arranque: perfila hardware y descarga modelo
```

**Principio de aislamiento:** cada módulo tiene una responsabilidad y una interfaz explícita. `cleanup.rs` es una función pura sobre texto (por eso es trivial de testear); `hardware.rs` devuelve una recomendación pero no decide; `state.rs` es dueño del buffer de audio, de modo que dos dictados simultáneos no puedan pisarse.

### 3.1 Dependencias

| Área | Crate | Nota |
|---|---|---|
| Framework | `tauri` 2.11.x | features: `tray-icon`, `image-png` |
| Transcripción | `transcribe-cpp` 0.1.3 | **`default-features = false`** — ver 3.2. Plan B: `whisper-rs` 0.16 |
| Audio | `cpal` 0.16 | Captura |
| Resampleo | `rubato` 0.16 | Del sample rate nativo a 16 kHz |
| Portapapeles | `arboard` 3 | Leer y escribir |
| Teclado | `enigo` 0.6 | Simular Ctrl+V |
| Atajo global | `rdev` | **Pinnear `rev = "<sha>"`**: Handy usa un fork de git sin versión |
| VRAM (Windows) | `windows` (Win32::Graphics::Dxgi) | `IDXGIAdapter3::QueryVideoMemoryInfo` — ver 3.3 |
| Sistema | `sysinfo` | RAM total y núcleos |
| WAV | `hound` 3.5 | Solo tests |
| Descargas | `reqwest` 0.12 | Con `stream` para progreso |
| Texto | `regex` + `once_cell` | Ver advertencia en sección 9 |

Plugins de Tauri: `autostart`, `single-instance`, `store`, `dialog`, `fs`, `log`, `opener`, `process`.

### 3.2 `default-features = false` no es opcional

`transcribe-cpp` 0.1.3 tiene **`metal` como feature por defecto**. Sin desactivar los defaults, la compilación en Windows arrastra Metal y falla. La declaración correcta, tomada del `Cargo.toml` de Handy:

```toml
transcribe-cpp = { version = "0.1.3", default-features = false, features = ["dynamic-backends", "vulkan"] }
```

Además, `dynamic-backends` implica `shared`: los backends de ggml se distribuyen como **DLLs separadas** (`ggml-vulkan.dll`, `ggml-cpu.dll`, `ggml-base.dll`) que hay que empaquetar y hacer descubribles en tiempo de ejecución. Esto tiene tres consecuencias que la versión 1 de este spec ignoraba, y están tratadas en las secciones 4, 12 y 14.

### 3.3 Detección de VRAM: `wgpu` no sirve

La versión 1 proponía leer la VRAM con `wgpu`. **`wgpu::AdapterInfo` no expone memoria**: sus campos son `name`, `vendor`, `device`, `device_type`, `device_pci_bus_id`, `driver`, `driver_info`, `backend`, `subgroup_min_size`, `subgroup_max_size`, `transient_saves_memory` y `limit_bucket`. Ninguno es capacidad en bytes.

En Windows la VRAM real se obtiene por DXGI (`IDXGIAdapter3::QueryVideoMemoryInfo`), disponible en el crate `windows`. Pero eso no alcanza, por el motivo de la sección 5.

## 4. Aceleración por hardware

`transcribe-cpp` se compila con:

- **`dynamic-backends`**: el backend se carga en tiempo de ejecución. Un mismo binario prueba lo disponible y cae a CPU si falla.
- **`vulkan`**: funciona sobre NVIDIA, AMD, Intel Arc **y gráficos integrados**.

Datos de respaldo (investigación del 21/7/2026): Vulkan sobre gráficos integrados (Radeon 680M, Intel Arc) alcanza **3-4x tiempo real**, unas 12 veces más rápido que solo CPU; sobre NVIDIA moderna queda dentro del **25-30% de CUDA**.

**A evaluar en el spike (16.3):** si `cuda` + `vulkan` + `dynamic-backends` conviven en un mismo binario con selección en tiempo de ejecución. Si conviven —que es precisamente para lo que existe `dynamic-backends`— el escritorio usa CUDA y las notebooks Vulkan, **con un solo instalador**, y desaparece la tensión entre los criterios 1 y 2. Es la opción preferida.

**Modos de falla contemplados:**

- El backend Vulkan no inicializa → fallback a CPU.
- **`vulkan-1.dll` no existe** (Windows recién instalado sin drivers, "Adaptador de pantalla básico de Microsoft") → fallback a CPU. Es un caso distinto del anterior: falla al cargar la DLL, no al inicializar.
- Gráficos integrados Intel antiguos sin soporte de cómputo → fallback a CPU.

## 5. Elección de modelo: por medición, no por umbrales

La versión 1 elegía el modelo con una matriz de umbrales de VRAM. **Ese enfoque no funciona**, por dos razones:

1. `wgpu` no da VRAM (sección 3.3).
2. Aunque se obtenga por DXGI, **en una GPU integrada el número no significa nada**: la memoria es compartida con el sistema y `DedicatedVideoMemory` suele reportar los 128 MB reservados. Una matriz por umbrales mandaría las dos notebooks a la rama "sin GPU utilizable" aunque Vulkan funcione perfecto — exactamente lo contrario de lo que promete la sección 4.

**Diseño adoptado: perfilado por benchmark.** En el primer arranque, `hardware.rs`:

1. Enumera adaptadores y se queda con `device_type` (`DiscreteGpu` / `IntegratedGpu` / `Cpu`).
2. Lee la RAM total con `sysinfo` y, si hay GPU dedicada, la VRAM por DXGI (informativo, no decisorio).
3. Descarga el modelo `base` (142 MB, rápido de bajar) y **mide**: transcribe un audio de referencia de 3 segundos incluido en el binario y calcula el factor de tiempo real alcanzado.
4. Decide con el resultado medido:

| Resultado del benchmark | Modelo elegido | Peso |
|---|---|---|
| ≥ 20x tiempo real | `large-v3-turbo` | ~1.5 GB |
| 8x – 20x | `large-v3-turbo-q5_0` | 547 MB |
| 3x – 8x | `small` | 466 MB |
| < 3x | `base` (ya descargado) | 142 MB |

5. Guarda el resultado en la configuración para no repetir el benchmark en cada arranque. Se puede rehacer desde Ajustes.

**Ventajas:** funciona igual en GPU dedicada, integrada o CPU; se autocorrige si el hardware o los drivers cambian; y la decisión se toma con la métrica que de verdad importa, que es la velocidad, no una cifra de memoria que en la mitad de los casos miente.

**Exhaustividad:** las cuatro ramas cubren todo el dominio de un número real positivo por construcción. Se testea cada rama con valores inyectados, sin depender del hardware de la máquina de desarrollo.

**Restricción de memoria:** antes de elegir un modelo se verifica que entre en la memoria disponible (VRAM si va a GPU, RAM si va a CPU) con 1.5x de margen. Si no entra, se baja al siguiente. **Si el usuario fuerza un modelo que no entra, se bloquea con un aviso** — no se permite, por el motivo de la sección 12.2.

## 6. Captura de audio

No es un puerto directo, y esta es la parte con más diferencias reales.

La versión Python le pide 16 kHz y 1 canal directamente al driver (`sd.InputStream(samplerate=16000, channels=1)`); PortAudio/WASAPI hace la conversión de forma transparente. **Con `cpal` eso no es posible**: se obtiene la configuración nativa del dispositivo y hay que convertir a mano, **en este orden**:

1. **Downmix a mono.** Las muestras llegan intercaladas. El micrófono actual (HyperX QuadCast) reporta **2 canales**, y los arrays de micrófono de notebook también. Pasar un buffer estéreo intercalado como si fuera mono produce audio al doble de velocidad y transcripción basura.
2. **Resampleo a 16 kHz.** El dispositivo entrega típicamente 44.1 o 48 kHz.

Ambas son funciones puras y se testean con vectores conocidos y con un WAV estéreo de 48 kHz, sin micrófono.

**Ciclo de vida del micrófono.** La versión Python abre el stream al arrancar y no lo cierra nunca, así que el indicador de micrófono de Windows queda encendido permanentemente y se consume batería. Dos de las tres máquinas son notebooks. **La versión nativa abre el dispositivo al empezar a grabar y lo cierra al terminar.** (Corregido también en la versión Python durante esta revisión.)

**Guarda del tono de inicio.** El acorde de comienzo suena por los parlantes y el micrófono lo capta, sobre todo en notebooks. Se descartan los primeros 200 ms del buffer. (Corregido también en la versión Python.)

**Detección de voz (VAD).** La versión Python usa el VAD integrado de faster-whisper (`vad_filter=True`), que es Silero con `min_silence_duration_ms=2000` y `speech_pad_ms=400`. No es un componente separado. Antes de incorporar una dependencia nueva:

- Verificar si el motor elegido trae VAD propio — whisper.cpp lo tiene desde la 1.8.x. Si lo trae, se usa ese.
- Si hace falta `vad-rs`, **pinnear el commit**: Handy lo usa como dependencia de git sin versión.
- **Parametrizar el padding y el umbral de silencio en Ajustes.** Un VAD agresivo sin padding corta el ataque de la primera palabra; el síntoma es perder sistemáticamente la primera sílaba.

**Límite de duración.** Configurable, por defecto **3 minutos**, con corte y aviso. La versión Python no tiene límite: el buffer crece a 62 KB/s sin cota y la latencia de transcripción crece más que proporcionalmente (30 s → 0.45 s, pero 187 s → 10.72 s). En una notebook con `small` en CPU, un dictado de 2 minutos podría tardar más de un minuto en transcribirse. Durante la transcripción se muestra progreso.

## 7. Máquina de estados y flujo

```
Idle ──F8──> Recording ──F8──> Processing ──> Idle
                 │                  │
                 └── error ─────────┴──> Idle (con tono de error)
```

`state.rs` es dueño del buffer. Al pasar a `Processing`, el buffer se **mueve** al hilo de trabajo; el estado `Recording` siguiente arranca con un buffer nuevo. En Rust esto sale gratis por ownership.

Esto corrige una condición de carrera real de la versión Python: `process_recording` y `toggle` escribían ambos sobre la lista global `frames`, así que un F8 disparado mientras la transcripción anterior seguía corriendo podía borrar el audio de la grabación en curso. (Corregido también en la versión Python durante esta revisión.)

**Flujo completo:**

```
F8 → abrir micrófono → capturar
F8 → cerrar micrófono → downmix → resampleo → VAD
   → transcribir → filtro anti-alucinación → limpiar por reglas
   → pegar → registrar en historial → emitir evento a la interfaz
```

Se pega antes de registrar: la prioridad es que el texto aparezca cuanto antes.

**Si se dispara F8 durante `Processing`**, se inicia una grabación nueva normalmente. Los dictados se pegan en el orden en que terminan de transcribirse.

## 8. Interfaz y bandeja

**Menú de bandeja** (es la única interfaz en el modo del criterio 4):

| Ítem | Acción |
|---|---|
| Estado (no clickeable) | Cargando modelo / Listo / Grabando / Transcribiendo / Error |
| Pausar dictado | Libera la tecla sin cerrar la app |
| Abrir dashboard | Muestra la ventana |
| Ajustes | Muestra la ventana en Ajustes |
| Salir | Cierra la aplicación |

"Pausar dictado" preserva una capacidad que hoy existe: el dashboard Streamlit tiene botones de iniciar y detener el motor. Sin ese ítem, la única forma de desactivar F8 sería cerrar la app.

**Estado de carga.** El modelo tarda unos 30 segundos en cargar. En la versión Python el atajo se registra recién después, así que durante ese tiempo F8 no hace nada y no hay ninguna señal. En modo bandeja sería peor. La versión nativa registra el atajo desde el principio y responde con un tono de "todavía no estoy listo" más el estado en el ícono.

**Sonidos.** Cuatro, con semántica propia. Son la única realimentación en modo bandeja.

| Sonido | Cuándo |
|---|---|
| Acorde ascendente (523+659 Hz) | Empezó a grabar |
| Tono grave corto (392 Hz) | Dejó de grabar |
| Campanita (659+784 Hz) | Texto pegado |
| Grave largo (220 Hz) | Error, transcripción vacía, o segunda instancia |

**Dashboard.** Carga completa al montarse y **eventos para las novedades**. Solo con eventos, un dictado hecho con la ventana cerrada no aparecería al abrirla (los eventos de Tauri se descartan si no hay webview) — que es justamente la combinación de los criterios 4 y 5.

**Paginación obligatoria** en el historial: se cargan los últimos N y la búsqueda es incremental. El dashboard actual reparsea el archivo entero cada 3 segundos y renderiza todas las filas; `history.jsonl` crece sin cota.

## 9. Limpieza por reglas

`cleanup.rs` replica `fast_cleanup`, verificado por los 12 casos de `tests/test_cleanup.py`. Son **cinco** reglas, no dos:

1. **Muletillas** aisladas por comas o al inicio de frase (lista `FILLERS`, configurable).
2. **Tartamudeos** de palabras funcionales (lista `STUTTER_WORDS`, configurable, 26 palabras).
3. **Espaciado y puntuación**: espacios dobles, espacio antes de signo, comas duplicadas.
4. **Mayúscula inicial**, sin tocar el resto (puede haber siglas: CRM, API).
5. **Red de seguridad**: si el resultado quedó a menos de la mitad del original, se devuelve el original.

La regla 5 es un test obligatorio, no un detalle de implementación.

**Advertencia de migración:** el crate `regex` de Rust **no soporta retrorreferencias** (`\1`), por diseño. El código Python usa una para colapsar tartamudeos. Ese patrón no se porta tal cual: se implementa recorriendo tokens y comparando con el anterior.

## 10. Filtro anti-alucinación

**Riesgo nuevo que la versión Python no tiene.** whisper.cpp es notoriamente propenso a alucinar frases fijas sobre silencio o ruido de fondo — en español, típicamente créditos de subtítulos o "Gracias por ver el video". En esta aplicación el texto alucinado **se pega directamente en el documento del usuario**.

La versión Python está protegida por el VAD de faster-whisper más el chequeo de texto vacío. Al cambiar de motor esa protección hay que reconstruirla:

1. `no_speech_threshold` y `suppress_blank` en los parámetros de transcripción.
2. Lista de bloqueo de frases-alucinación conocidas en español.
3. Si tras el filtro no queda texto, no se pega nada y suena el tono de error.

El criterio de aceptación 9 cubre esto: 10 segundos de silencio deben producir salida vacía.

## 11. Historial y privacidad

**Ubicación:** `%APPDATA%\MithFlow\history.jsonl`. Al primer arranque, si existe `D:\MithFlow\history.jsonl`, el asistente ofrece importarlo.

**Formato:** una línea JSON por dictado. Campos: `ts`, `audio_s`, `transcribe_s`, `cleanup_s`, `words`, `cleaned`, `mode`, `raw`, `final`.

**Lectura tolerante, obligatoria.** Verificado sobre el archivo real: **7 de 20 entradas (35%) no tienen el campo `mode`**, porque se agregó después. Un struct con `mode: String` fallaría en el 35% del historial. Por lo tanto:

- Todos los campos salvo `ts` y `final` van con `#[serde(default)]` y tipo opcional donde corresponda.
- Las líneas inválidas se saltean sin abortar la carga.
- Hay un test que lee el `history.jsonl` real del proyecto.

**Privacidad.** El archivo contiene en texto plano **todo lo que se dictó**, y la aplicación se va a instalar en la notebook de otra persona. Por lo tanto:

- `.gitignore` con `history.jsonl`, `status.json`, `*.log` y `models/` **creado antes de inicializar cualquier repositorio** (hecho durante esta revisión).
- Ajustes incluye **"Borrar historial"** y una opción de **retención** (por ejemplo, conservar 90 días).
- Ajustes incluye **"No guardar el texto dictado"**: registra solo las métricas, sin `raw` ni `final`.
- El historial no se sincroniza ni se envía a ningún lado.

**Rotación:** cuando el archivo supera los 10 MB se rota a `history-YYYYMM.jsonl`.

## 12. Manejo de errores

### 12.1 Tabla

| Situación | Comportamiento |
|---|---|
| Sin micrófono | Aviso en interfaz y bandeja; la app abre igual |
| Micrófono ocupado por otra app | Tono de error y aviso; no se graba |
| Modelo no descargado | Se abre el asistente |
| Descarga interrumpida | Se reintenta; el parcial se descarta |
| Hash del modelo no coincide | Se descarta y se avisa; no se carga (sección 14) |
| Backend GPU no inicializa | Fallback automático a CPU + aviso no bloqueante |
| `vulkan-1.dll` ausente | Fallback a CPU + sugerencia de actualizar drivers |
| Falla al pegar | El texto dictado **queda en el portapapeles**, tono de error y aviso. No se restaura el portapapeles previo: hacerlo perdería la transcripción |
| Transcripción vacía | Tono de error, no se pega nada |
| Solo alucinación tras el filtro | Igual que transcripción vacía |
| Audio más corto que 0.5 s | Se descarta en silencio |
| Audio más largo que el límite | Se corta en el límite y se avisa |
| Segunda instancia | `single-instance` enfoca la ventana existente |
| Excepción en el manejador del atajo | Se captura y se registra; el atajo sigue vivo |
| Falla al guardar el historial | Se registra el error; el dictado se considera exitoso |
| F8 durante la carga del modelo | Tono de "todavía no listo" |

Los dos casos del manejador del atajo y del pegado vienen de fallos reales de la versión Python.

### 12.2 El criterio 6 y `GGML_ASSERT`

`GGML_ASSERT` **no se compila fuera con `-DNDEBUG`**: dispara `abort()` incluso en release. Un fallo de asignación al cargar el modelo, o un modelo corrupto, **mata el proceso**. No es un `panic!` de Rust que se pueda capturar con `catch_unwind`.

Por eso el criterio 6 se redactó como está, y por eso la sección 5 **bloquea** los modelos que no entran en memoria en vez de solo advertir. Además:

- Antes de cargar un modelo se persiste un indicador `cargando_modelo = X`.
- Si al arrancar ese indicador sigue puesto, significa que el intento anterior mató el proceso: se baja automáticamente al modelo siguiente y se avisa.

Es la forma de cumplir el criterio 6 sin cargar el modelo en un proceso hijo, que sería la alternativa completa pero mucho más compleja.

## 13. Configuración

Persistida con `tauri-plugin-store`.

| Ajuste | Default | Origen |
|---|---|---|
| Tecla de dictado | `F8` (`F9` en la transición, ver 16.4) | `HOTKEY` |
| Modelo | Automático por benchmark | `MODEL_SIZE` |
| Idioma | `es` | `LANGUAGE` |
| Vocabulario | Lista actual | `INITIAL_PROMPT` |
| Muletillas | Lista actual | `FILLERS` |
| Tartamudeos | Lista actual | `STUTTER_WORDS` |
| Estrategia de decodificación | Greedy (`best_of=1`) | `BEAM_SIZE = 1` |
| Modo de limpieza | `fast` | `CLEANUP_MODE` (`fast` \| `off`) |
| Sonidos | Activados, volumen 0.15 | `play_tone` |
| Límite de grabación | 3 min | No existía |
| Padding del VAD | 400 ms | Default de faster-whisper |
| Retención del historial | 90 días | No existía |
| Guardar texto dictado | Sí | No existía |
| Arranque con Windows | Desactivado | No existía |

**Greedy no es un detalle.** whisper.cpp no usa greedy por defecto. Si el port toma los defaults, la latencia sube alrededor de un 30% de entrada y el criterio 2 se pierde por esa sola razón. Está medido: greedy fue 28% más rápido que beam search con 5 candidatos, con texto idéntico sobre voz real en español.

**El modo `llm` no se porta.** Costaba 3.5 s sobre 0.31 s totales. Si se quiere más adelante, entra como modo híbrido (LLM solo en textos largos con poca puntuación), no como default.

## 14. Descarga de modelos

- Host único en lista blanca, HTTPS obligatorio.
- **Los hashes SHA-256 esperados van compilados dentro del binario.** Un hash servido por el mismo host que el modelo no verifica nada.
- Se verifica antes de mover el archivo a su ubicación final.
- Descarga con progreso y reanudable.
- Los modelos se guardan en `%APPDATA%\MithFlow\models\` y se pueden copiar entre máquinas a mano.

`cargo-audit` y `cargo-deny` corren en cada build. Las dependencias de git (`rdev`, y `vad-rs` si se usa) van con `rev` fijado.

## 15. Pruebas

**Unitarias:** los 12 casos de limpieza (seis que limpian, seis que no deben tocar nada) más la red de seguridad del 50%; downmix y resampleo con vectores conocidos; las cuatro ramas de elección de modelo con valores inyectados; lectura de historial con entradas incompletas.

**Integración:** transcribir un WAV de voz sintética en español y verificar las palabras clave, incluido el vocabulario propio (MithData, CRM). Transcribir un WAV estéreo de 48 kHz. Transcribir 10 segundos de silencio y verificar salida vacía (criterio 9). Leer el `history.jsonl` real del proyecto.

**Manual, en las tres máquinas:** dictado, pegado en Notepad, VS Code, Chrome y una consola elevada; preservación del portapapeles; bandeja; autoarranque; cambio de modelo; comportamiento sin drivers de video actualizados.

## 16. Precondiciones antes de escribir código

### 16.1 Caracterizar las dos notebooks

Los criterios 2 y 3 no se pueden planificar sobre hardware desconocido. En cada notebook: CPU, RAM, GPU y si `vulkaninfo` responde. Sin esto, el criterio 3 es una esperanza, no un criterio.

### 16.2 Re-medir la línea base

Con protocolo reproducible: el mismo WAV, N=20, reportando mediana y p95, y midiendo **latencia percibida** (soltar tecla → texto visible), no solo transcripción. Es media hora de trabajo que evita discutir contra números fantasma durante todo el proyecto.

### 16.3 Spike técnico

Debe responder con evidencia, no en general:

1. `transcribe-cpp` 0.1.3 expone: `language`, estrategia de sampling / `beam_size`, `n_threads`, `no_context`, `no_speech_threshold`, `suppress_blank`. (`initial_prompt` y `temperature` ya están confirmados vía `WhisperRunOptions`.) Sin `language` ni el control de sampling no hay paridad.
2. Formato de modelo (GGUF o GGML `.bin`), URL canónica y SHA-256 a fijar.
3. `default-features = false` + `["dynamic-backends", "vulkan"]` compila en Windows, y **qué DLLs de ggml quedan en la salida** (tamaño real del bundle, que la versión 1 estimó en 20 MB sin base).
4. **¿`cuda` + `vulkan` + `dynamic-backends` conviven en un solo binario con selección en runtime?** Esto decide si los criterios 1 y 2 se cumplen juntos.
5. Supresión de F8 contra Notepad, VS Code, Chrome y una consola **elevada**.
6. `cpal` sobre el micrófono real: cuántos canales y a qué sample rate, para dimensionar downmix y resampleo.

### 16.4 Convivencia durante el desarrollo

La versión Python sigue en uso mientras se desarrolla la nativa. El plugin `single-instance` de Tauri **solo detecta otras instancias Tauri**, no el `python.exe` que corre `mithflow.py` (que usa su propio mutex). Con ambas activas habría dos hooks de F8, doble grabación y doble pegado sobre el mismo archivo.

Por eso, durante la transición la versión Tauri usa **F9** y **su propio archivo de historial**, con un paso explícito de importación al final.

## 17. Limitaciones conocidas

- **Ventanas elevadas.** Ningún hook de teclado en modo usuario —ni `keyboard`, ni `rdev`, ni `WH_KEYBOARD_LL`— puede suprimir una tecla destinada a un proceso de mayor nivel de integridad. Dictando dentro de una consola de administrador, F8 llega igual a esa aplicación y MithFlow tampoco puede pegar ahí. Ya pasa con la versión Python. Se documenta; opcionalmente se ofrece "ejecutar como administrador" en Ajustes con su advertencia.
- **Portapapeles no textual.** Se lee y restaura solo texto: si había una imagen o archivos copiados, se pierden. Restaurar todos los formatos requiere enumerarlos con `EnumClipboardFormats`. Se implementa si molesta en el uso real.
- **Sin firma digital.** SmartScreen advertirá la primera vez en cada máquina.

## 18. Fuera de alcance

- Push-to-talk y perfiles de tono por aplicación (roadmap).
- Limpieza con LLM (sección 13).
- macOS y Linux: el código será portable, pero solo se compila y prueba en Windows.
- Actualizaciones automáticas.

---

## Apéndice A: mediciones

**Controladas** (21/7/2026, RTX 3080, modelo caliente, mismo WAV de 13.3 s):

| Métrica | Valor |
|---|---|
| Transcripción | 0.30 s |
| Limpieza por reglas | 0.085 ms |
| VRAM ocupada por `large-v3-turbo` float16 | 2.1 GB |
| Buffer de audio | 62 KB/s |

**Uso real** (13 dictados en modo `fast` del historial):

| Métrica | Valor |
|---|---|
| Mediana de transcripción | 0.49 s |
| Media | 0.505 s |
| Mínimo / máximo | 0.21 s / 0.97 s |
| Latencia oculta del pegado (no medida por el dashboard) | 0.45 s |

La diferencia entre 0.30 s y 0.49 s es real y esperable: la medición controlada usa el modelo caliente sobre audio idéntico. **El criterio 2 se ancla en el uso real, no en el laboratorio.**

**Degradación con dictados largos:** 30 s → 0.45 s; 60 s → 0.96 s; 120 s → 3.32 s; 187 s → 10.72 s. La causa es el contexto encadenado entre ventanas de 30 s. Desactivarlo duplica la velocidad en audio largo con salida casi idéntica (120 s baja de 4.92 s a 2.25 s). **Evaluar activarlo desde el inicio.**

**Estrategia de decodificación:** greedy resultó 28% más rápido que beam search con 5 candidatos, con texto idéntico sobre voz real en español.

## Apéndice B: historial de revisiones

**Versión 2** — revisión propia contra el código. Once correcciones: criterio de latencia contradictorio, `STUTTER_WORDS` faltante, downmix no contemplado, VAD sin analizar, umbral de VRAM sin fundamento, `LANGUAGE` y `BEAM_SIZE` ausentes, `status.json` sin destino, retrorreferencias de regex, comportamiento del pegado mal descrito, tests en un directorio temporal, criterio sobre la suite de limpieza faltante.

**Versión 3** — revisión independiente contra el código y las fuentes. Correcciones adicionales:

| Área | Corrección |
|---|---|
| Elección de modelo | `wgpu` no expone VRAM (verificado: 12 campos, ninguno de memoria) y en GPU integrada la cifra miente. Reemplazado por perfilado por benchmark, con ramas exhaustivas |
| Compilación | Falta `default-features = false`: `metal` es feature por defecto y rompe el build en Windows |
| Distribución | `dynamic-backends` implica DLLs de ggml separadas: el bundle no son 20 MB y hay que empaquetarlas y hacerlas descubribles |
| Criterio 2 | Anclado a la mediana real (0.49 s), redefinido como latencia percibida, incluyendo los 0.45 s de espera del pegado que la métrica anterior ocultaba |
| Criterios 1 y 2 | La mitigación anterior (compilar con CUDA aparte) rompía el criterio del instalador único. Se evalúa en el spike que ambos backends convivan en un binario |
| Criterio 6 | `GGML_ASSERT` llama a `abort()` en release: no se puede capturar. Se bloquean los modelos que no entran y se agrega detección de arranque fallido |
| Historial | 35% de las entradas reales no tienen el campo `mode` (verificado). Lectura tolerante obligatoria |
| Privacidad | Ubicación definida, `.gitignore` creado, borrado y retención en Ajustes, opción de no guardar texto |
| Alucinaciones | whisper.cpp alucina sobre silencio; la protección del VAD de faster-whisper hay que reconstruirla. Nuevo criterio de aceptación 9 |
| Bandeja | Contenido del menú especificado, incluida la capacidad de pausar que hoy da el dashboard |
| Estado de carga | Los 30 s de carga del modelo no tenían señal para el usuario |
| Micrófono | Se abría al arrancar y no se cerraba nunca: indicador siempre encendido y batería |
| Concurrencia | Carrera real sobre el buffer de audio entre el hilo del atajo y el de transcripción |
| Convivencia | Dos MithFlow simultáneos durante el desarrollo: la versión nueva usa F9 y su propio historial |
| Descargas | Origen y ancla de confianza especificados; hashes compilados en el binario |
| Dependencias | `rdev` y `vad-rs` son forks de git sin versión: hay que fijar el commit |
| Límites | Tope de duración de grabación y paginación del historial |
| Limitaciones | Ventanas elevadas y portapapeles no textual, documentados como limitaciones y no como riesgos mitigables |

## Apéndice C: correcciones aplicadas a la versión Python

Detectadas durante la revisión y arregladas de inmediato, para no arrastrarlas al port:

| Problema | Estado |
|---|---|
| Al fallar el pegado se restauraba el portapapeles y se perdía la transcripción | Corregido; verificado por `tests/test_paste.py` |
| Carrera sobre `frames` entre el hilo del atajo y el de transcripción | Corregido: el buffer se entrega por parámetro |
| El tono de inicio se grababa a sí mismo | Corregido: se descartan los primeros 200 ms |
| El micrófono quedaba abierto siempre | Corregido: se abre y cierra por dictado |
| El mutex `Global\` fallaba silenciosamente sin privilegios y permitía dos instancias | Corregido: `Local\` y fail closed |
| Transcripción vacía sin ninguna realimentación | Corregido: tono de error |
| `stop_engine` mataba un PID sin verificar identidad | Corregido: se valida la línea de comandos |
| El buscador del dashboard interpretaba la entrada como regex y rompía la página | Corregido: `regex=False` |
| La latencia promedio mezclaba la época del LLM: mostraba 1.76 s contra 0.51 s reales | Corregido: se promedia solo el modo actual |
| Las entradas sin campo `mode` daban `NaN` en la interfaz | Corregido: relleno de campos faltantes |
| `beep()` era código muerto | Eliminado |
| `import os` redundante dentro de una función | Eliminado |
| Comparación `== True` | Corregido |
| Los 12 casos de prueba vivían en un directorio temporal | Guardados en `tests/test_cleanup.py` |
| El spec afirmaba un test de pegado que no existía en el proyecto | Escrito en `tests/test_paste.py`, 2/2 casos pasan |
| `wgpu` para VRAM, y `SincFixedIn` con la API de una versión que no es la fijada | Verificados contra docs.rs; el plan usa la API real de cada versión |
| No existía `.gitignore`: un `git add .` habría commiteado todo el historial dictado | Creado |
