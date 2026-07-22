# MithFlow — app de escritorio

El cascarón de Tauri v2 alrededor de `mithflow-core`: ventana, bandeja, atajo
global y persistencia. El motor de dictado vive en `../crates/core`.

La interfaz de `src/` son tres vistas en React + TypeScript, sin librería de
gráficos ni de componentes: dashboard, ajustes y asistente de primer arranque.

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

## El instalador

```powershell
npm run tauri build
# -> ../target/release/bundle/nsis/MithFlow_1.0.0_x64-setup.exe
```

Sale un instalador NSIS de **12,1 MiB** que ocupa **~99 MB** instalado: adentro
van el ejecutable y **las 13 DLLs de ggml** (84 MB sin comprimir, que es el
motor). Que estén ahí no es gratis ni automático —lo arma `build.rs` y lo declara
`bundle.resources`— así que **verificalo, no lo asumas**:

```powershell
7z l ..\target\release\bundle\nsis\MithFlow_1.0.0_x64-setup.exe   # tienen que aparecer 13 .dll
```

La prueba que de verdad cierra el tema es extraer el instalador a un directorio
aislado y correr el `.exe` desde ahí (sin `target/release` cerca): en la salida
tiene que decir `load_backend: loaded Vulkan backend from <ese directorio>`. Si
la ruta apunta a `target/release`, la prueba no vale. El binario de release no
tiene consola (`windows_subsystem = "windows"`), así que hay que capturarla con
`Start-Process -RedirectStandardError`.

No se firma digitalmente: la primera ejecución dispara SmartScreen y hay que
pasar por "Más información" → "Ejecutar de todas formas". Está documentado en el
README raíz.

## La interfaz sin el backend

`mock.html` levanta las mismas tres vistas contra un backend simulado
(`src/desarrollo/`), para diseñar y sacar capturas sin micrófono ni modelo:

```powershell
npm run dev
# http://localhost:1420/mock.html?escenario=normal
#                                 ?escenario=primer-arranque   (asistente)
#                                 ?escenario=grabando
```

Desde la consola del navegador, `mithflow.dictar("una frase")` dispara un
`dictado-nuevo` y `mithflow.emit("estado-cambiado", …)` cambia el estado, para
ver la actualización en vivo.

**No llega a producción**: `vite build` compila sólo `index.html`, así que ni
`mock.html` ni `src/desarrollo/` entran al `dist/` que empaqueta Tauri. La
comprobación es mecánica —`dist/` no puede contener la cadena
`MITHFLOW_SIMULADO`— y el simulado **no reemplaza a los tests de Rust**: si un
dato de ahí y uno de `comandos.rs` no coinciden, manda el de Rust.

## Modelo

Por defecto se busca en `%APPDATA%\MithFlow\models\`. Para desarrollo se puede
apuntar a un `.gguf` concreto:

```powershell
$env:MITHFLOW_MODELO = "D:\MithFlow\app-nativa\models\whisper-large-v3-turbo-Q4_K_M.gguf"
```

Sin ningún modelo descargado la app **igual arranca**, en estado `SinModelo`
("Falta el modelo", en ámbar) y con el motivo a la vista: descargarlo se hace
desde Ajustes, o sea desde esta misma app.

Ese estado **no es `Error`** a propósito. En el primer arranque no hay nada roto
—el asistente está bajando el modelo, con su barra de progreso— y una pastilla
roja ahí arriba diría lo contrario. `Error` queda para lo que sí es una falla: el
modelo está en el disco pero no carga, el atajo no se pudo enganchar, no hay
dónde escribir el historial. La distinción se decide por tipo
(`stt::ErrorDeModelo` en el núcleo, `estado::FalloDelMotor` en la app), nunca
comparando el texto del mensaje.

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
| `src/api.ts` | el contrato con el backend: comandos, eventos y tipos |
| `src/App.tsx` | estado del motor, navegación y avisos |
| `src/vistas/Dashboard.tsx` | métricas, gráficos e historial paginado |
| `src/vistas/Ajustes.tsx` | los ajustes, incluido borrar el historial |
| `src/vistas/Asistente.tsx` | primer arranque: medir, recomendar, descargar |
| `src/desarrollo/` | backend simulado; **no entra al build de producción** |

Los tests del núcleo corren con `cargo test`; los de la app quedan fuera del
default del workspace (necesitan `dist/`), así que van con
`cargo test --workspace` después de `npm run build`.

Las decisiones y sus porqués están en `../DECISIONES.md`.
