# MithFlow — app de escritorio

El cascarón de Tauri v2 alrededor de `mithflow-core`: ventana, bandeja, atajo
global y persistencia. El motor de dictado vive en `../crates/core`.

> La interfaz de `src/` es **provisoria**. Existe para verificar que los
> comandos y los eventos funcionan; la de verdad es la tarea siguiente.

## Requisitos

- Node 24 y npm 11
- Rust estable y VS Build Tools 2022
- **Vulkan SDK** — es dependencia de compilación, no sólo de ejecución
  (`glslc` compila los shaders de ggml)

```powershell
$env:VULKAN_SDK = "C:\VulkanSDK\1.4.350.0"
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\CMake\bin;C:\VulkanSDK\1.4.350.0\Bin;$env:PATH"
```

## Compilar y correr

```powershell
npm install
npm run tauri dev      # desarrollo, con recarga del frontend

# release: el frontend PRIMERO, porque generate_context! exige que dist/ exista
npm run build
cargo build --release -p mithflow-app --features custom-protocol
```

**`--features custom-protocol` no es opcional.** Sin ella el binario compila en
modo desarrollo y busca el frontend en `http://localhost:1420`: ventana en
blanco y ningún error visible. `npm run tauri build` la activa sola.

## Modelo

Por defecto se busca en `%APPDATA%\MithFlow\models\`. Para desarrollo se puede
apuntar a un `.gguf` concreto:

```powershell
$env:MITHFLOW_MODELO = "D:\MithFlow\app-nativa\models\whisper-large-v3-turbo-Q4_K_M.gguf"
```

Sin ningún modelo descargado la app **igual arranca**, en estado `Error` y con el
motivo a la vista: descargarlo se hace desde Ajustes, o sea desde esta misma app.

## Convivencia con la versión Python

- El atajo por defecto es **F9**; `mithflow.py` usa F8. Pueden correr las dos.
- El historial es `%APPDATA%\com.mithdata.mithflow\history-nativo.jsonl`, nunca
  el `history.jsonl` de Python.

## Dónde está cada cosa

| Archivo | Qué resuelve |
|---|---|
| `src-tauri/src/main.rs` | arranque y cableado de los hilos |
| `src-tauri/src/director.rs` | la máquina de estados; único escritor del estado |
| `src-tauri/src/atajo.rs` | `rdev::grab` en su hilo, con supresión de la tecla |
| `src-tauri/src/motor.rs` | carga del modelo, calentamiento y transcripción |
| `src-tauri/src/comandos.rs` | la API que ve el frontend |
| `src-tauri/build.rs` | junta las 13 DLLs de ggml para el instalador |

Las decisiones y sus porqués están en `../DECISIONES.md`.
