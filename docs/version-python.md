# MithFlow — la versión Python (el prototipo original)

> Esta es la **primera** versión de MithFlow: un script de Python con un
> dashboard de Streamlit. Sigue funcionando y sigue mantenida, pero la versión
> que se recomienda instalar es la **app nativa**, que no necesita Python ni
> CUDA y viene en un instalador. Ver el [README principal](../README.md).

Las dos versiones **conviven a propósito**: usan teclas distintas (F8 la de
Python, F9 la nativa) y archivos de historial distintos, así que se pueden tener
las dos corriendo al mismo tiempo sin que una toque los datos de la otra.

| | **Python** (esta) | **Nativa** (`app-nativa/`) |
|---|---|---|
| Atajo | **F8** | **F9** |
| Motor | faster-whisper + CUDA (necesita NVIDIA para ir rápido) | whisper.cpp + Vulkan (NVIDIA, AMD o Intel) |
| Instalación | Python 3.10+ y `instalar.ps1` | un instalador `.exe`, sin Python |
| Interfaz | dashboard de Streamlit en el navegador | ventana propia + ícono en la bandeja |
| Historial | `history.jsonl` (junto al script) | `%APPDATA%\com.mithdata.mithflow\history-nativo.jsonl` |

---

## Instalación

Funciona con o sin GPU. Son dos pasos.

### 1. Instalar Python (una sola vez por máquina)

Descargá **Python 3.10 o superior** desde <https://python.org>.

> Al instalar, **tildá la casilla "Add python.exe to PATH"** (abajo de todo en
> la primera pantalla). Sin eso el instalador no lo encuentra.

### 2. Ejecutar el instalador

Clic derecho en **`instalar.ps1`** → **Ejecutar con PowerShell**.

> Si Windows bloquea el script ("la ejecución de scripts está deshabilitada"),
> abrí PowerShell en la carpeta y corré:
> `powershell -ExecutionPolicy Bypass -File instalar.ps1`

El instalador hace todo solo:

1. Verifica que Python sea 3.10+.
2. Crea el entorno virtual e instala las dependencias de `requirements.txt`.
3. **Detecta si hay GPU NVIDIA** (`nvidia-smi`): si hay, instala las librerías
   CUDA y usa el modelo grande (`large-v3-turbo`); si no, usa `small` en CPU.
4. Crea `.streamlit\config.toml` con el dashboard atado a `127.0.0.1`.
5. Verifica que el micrófono y las dependencias funcionen.
6. Pregunta si querés Ollama (**opcional** — ver más abajo).

Después: doble clic en **`MithFlow-App.vbs`**. La primera corrida descarga el
modelo Whisper (1-2 GB, una sola vez) y tarda unos minutos; las siguientes
arrancan en ~30 segundos.

---

## Uso diario

| Archivo | Qué hace |
|---|---|
| **`MithFlow-App.vbs`** | App completa: motor F8 de fondo + dashboard como ventana de aplicación |
| `MithFlow.bat` | Solo motor, con consola visible (útil para ver errores) |
| `MithFlow-invisible.vbs` | Solo motor, sin ventana; logs en `mithflow.log` |
| `Detener-MithFlow.bat` | Apaga el motor |

**Arranque automático con Windows:** `Win+R` → `shell:startup` → pegá ahí un
acceso directo a `MithFlow-invisible.vbs` (o a `MithFlow-App.vbs` si querés
también el dashboard).

Una sola instancia a la vez: si lanzás un segundo MithFlow, suena un tono grave
y se cierra solo.

---

## Configuración

Todo vive en el bloque `CONFIGURACIÓN` al principio de `mithflow.py`.

| Variable | Qué toca |
|---|---|
| `HOTKEY` | Tecla de dictado (default `F8`) |
| `MODEL_SIZE` | `"auto"` elige según GPU. Fijalo a `small` / `medium` / `large-v3-turbo` si querés |
| `DEVICE` | `"auto"` detecta GPU; forzá `"cpu"` o `"cuda"` |
| `LANGUAGE` | Idioma del dictado (default `"es"`; `None` autodetecta) |
| `INITIAL_PROMPT` | **Tu vocabulario propio** — términos que Whisper suele errar |
| `CLEANUP_MODE` | `"fast"` (default, instantáneo) · `"llm"` (Ollama, +3.5 s) · `"off"` (crudo) |
| `FILLERS` | Muletillas que saca el modo `fast` |
| `STUTTER_WORDS` | Palabras que se colapsan cuando se tartamudean (`"el el informe"`) |
| `BEAM_SIZE` | `1` (default, rápido) o `5` si notás errores en audio difícil |

### ¿Cuánta máquina necesito?

| | Con GPU NVIDIA | Solo CPU |
|---|---|---|
| Modelo | `large-v3-turbo` | `small` |
| Latencia | ~0,3 s | ~2-4 s según el procesador |
| RAM/VRAM | ~2 GB VRAM | ~2 GB RAM |
| Calidad en español | Excelente | Muy buena |

No hace falta tocar nada: `MODEL_SIZE = "auto"` elige solo según lo que
encuentre. En una notebook sin GPU anda perfecto, solo esperás un par de
segundos más.

### Ollama: cuándo sí y cuándo no

Por default **no hace falta**. La limpieza por reglas (`CLEANUP_MODE = "fast"`)
saca muletillas y arregla el espaciado en menos de 1 ms. Instalá Ollama solo si
querés que un LLM reescriba y pula la redacción entera; cuesta ~3,5 s por
dictado. En notebooks sin GPU, mejor no.

---

## Latencia medida

Medido en RTX 3080, 21/7/2026:

| Configuración | Transcripción | Limpieza | Total |
|---|---|---|---|
| `fast` + beam 1 (**actual**) | 0,30 s | 0,002 s | **0,31 s** |
| `llm` + beam 5 (anterior) | 0,53 s | 3,55 s | 4,08 s |

El cuello de botella era el LLM: tardaba ~3,5 s casi sin importar el largo del
texto (overhead de arranque, no trabajo real). El modo `fast` limpia por reglas
en menos de 1 ms, y Whisper ya puntúa y capitaliza bien por su cuenta.

`BEAM_SIZE = 1` da transcripción idéntica a `5` y es 28% más rápido (verificado
con voz real en español generada por TTS).

> Ese `0,31 s` **no es la latencia que se siente**: no incluye las dos esperas
> fijas del pegado (0,15 s antes del Ctrl+V y 0,30 s después). Medida de punta a
> punta —soltar la tecla hasta ver el texto— la versión Python da **0,739 s** de
> mediana (N=20, WAV de 9,5 s, modelo caliente). Es el número que la versión
> nativa se propuso bajar, y lo bajó. Ver `app-nativa/DECISIONES.md`.

---

## Sonidos

Tonos suaves sintetizados (no los beeps estridentes de Windows). Para
cambiarlos, editá las funciones `beep_*` en `mithflow.py`:

| Sonido | Cuándo |
|---|---|
| Acorde ascendente | Empezó a grabar |
| Tono grave corto | Dejó de grabar |
| Campanita | Texto pegado |
| Grave largo | Error / ya hay otra instancia |

Bajá `vol` en `play_tone()` si los querés aún más discretos (default `0.15`).

---

## Privacidad

- **Todo corre local**: la voz nunca sale de la máquina. Sin nube, sin cuenta,
  sin telemetría.
- El dashboard escucha **solo en `127.0.0.1`** (`.streamlit\config.toml`): nadie
  más en la red puede verlo. **No cambies `server.address` a `0.0.0.0`** —
  `history.jsonl` guarda en texto plano todo lo que se dictó.
- `history.jsonl` es privado: no lo copies entre máquinas ni lo subas a ningún
  lado. Para borrar el historial, borrá el archivo. Está en `.gitignore`.

---

## Notas técnicas

- `vad_filter=True` recorta silencios antes de transcribir (más rápido y menos
  alucinación).
- El script preserva el portapapeles: copia el texto, pega con Ctrl+V y
  restaura lo que había.
- El hotkey usa `suppress=True`: F8 no llega a la app enfocada, así el cursor no
  se mueve de donde estaba.
- La limpieza `fast` es conservadora por diseño: si las reglas se comen más de
  la mitad del texto, devuelve el original. No saca `"bueno"`, `"nada"` ni
  `"a ver"` porque también son arranques legítimos.
- En modo `llm`, si el modelo devuelve algo vacío o de largo desproporcionado se
  descarta y se usa el texto crudo. El LLM nunca puede "inventar" el dictado.
- El motor escribe un latido en `status.json` cada 10 s; así el dashboard sabe
  si está vivo sin inspeccionar procesos.

---

## Problemas comunes

| Síntoma | Solución |
|---|---|
| "Python no está instalado o no está en el PATH" | Reinstalá Python tildando "Add python.exe to PATH" |
| `cublas64_12.dll is not found` | Faltan las librerías CUDA: `.venv\Scripts\python.exe -m pip install nvidia-cublas-cu12 nvidia-cudnn-cu12` |
| F8 suena pero no pega | Verificá que haya **una sola** instancia (`Detener-MithFlow.bat` y relanzá). Revisá `mithflow.log` |
| No detecta micrófono | Configuración de Windows → Sistema → Sonido → Entrada; y permitir acceso al micrófono a apps de escritorio |
| El dashboard no abre | Esperá ~10 s tras el doble clic; entrá a mano a <http://localhost:8501> |
| Suena un beep grave al iniciar | Ya hay otro MithFlow corriendo; el nuevo se cierra solo (es lo correcto) |

---

## Archivos de esta versión

```
mithflow.py              el motor de dictado
dashboard.py             el dashboard de analíticas
requirements.txt         lista de dependencias
instalar.ps1             el instalador
MithFlow-App.vbs         lanzador principal (motor + dashboard)
MithFlow.bat             lanzador con consola visible
MithFlow-invisible.vbs   lanzador solo motor, sin ventana
Detener-MithFlow.bat     para apagarlo
.streamlit/config.toml   tema del dashboard + binding a localhost
tests/                   los tests de la limpieza de texto
```

Lo que **no** se versiona ni se copia entre máquinas: `.venv/`, `__pycache__/`,
`history.jsonl`, `status.json` y los `*.log`. Los tres últimos son datos
privados o temporales; los dos primeros se regeneran solos con `instalar.ps1`.
