# Publicar una versión (mantenimiento)

Cómo se saca una versión nueva de MithFlow y cómo funciona la firma que hace
segura la actualización automática. Si sólo querés **usar** la app, no necesitás
nada de esto: mirá el [README](../README.md).

---

## Un comando

```powershell
.\Generar-Instalador.ps1 -Version 1.2.0 -Notas "Qué cambió en esta versión."
```

Sin `-Version` recompila la que ya está. (Clic derecho → *Ejecutar con
PowerShell* también sirve, pero ahí no se puede pasar la versión.)

El script hace, en este orden:

1. **Corta si el endpoint del actualizador no tiene forma de URL de GitHub.** Se
   verifica antes de compilar: descubrirlo después de diez minutos de build es
   el peor momento.
2. Escribe la versión en los **tres** archivos que la llevan
   (`app-nativa/Cargo.toml`, `app-nativa/app/package.json`,
   `app-nativa/app/src-tauri/tauri.conf.json`) y **los relee para verificar que
   quedaron iguales**. Si se desincronizan, el actualizador se rompe de formas
   confusas: la app diría que tiene una versión y el manifiesto anunciaría otra.
3. Compila la interfaz y el instalador (`npm install` + `npm run tauri build`).
4. Firma el `.exe` con la clave privada minisign.
5. Arma el `latest.json`, con la URL de descarga derivada del mismo endpoint que
   consulta la app.

Deja en `instalador\` los **dos archivos que hay que subir**.

## Después, a mano en GitHub

1. *Releases* → *Draft a new release*.
2. La etiqueta tiene que ser **exactamente `v` + la versión**: para la 1.2.0, la
   etiqueta es `v1.2.0`. Si no, la URL del manifiesto apunta a la nada.
3. Adjuntá los **dos** archivos de `instalador\`, con esos nombres:
   - `MithFlow_1.2.0_x64-setup.exe`
   - `latest.json`
4. **Publicala**, no la dejes en borrador: `/releases/latest/` no ve los
   borradores y la consulta devolvería 404.

Las instalaciones existentes ven la actualización la próxima vez que abran
MithFlow.

---

## El nombre del repositorio vive en un solo lugar

**Archivo:** `app-nativa/app/src-tauri/tauri.conf.json`
**Dónde:** `plugins` → `updater` → `endpoints`

```json
"endpoints": [
  "https://github.com/LucasPilla60/mithflow/releases/latest/download/latest.json"
]
```

`Generar-Instalador.ps1` lee esa URL y deriva de ahí la de descarga del
instalador, así que **no hay un segundo lugar** donde el nombre del repositorio
pueda quedar mal.

Esa URL queda grabada dentro del `.exe`: después de cambiarla hay que volver a
compilar para que las instalaciones nuevas sepan a dónde consultar (y las que ya
están instaladas van a seguir consultando la vieja hasta que actualicen una vez).
El repositorio puede ser público o privado, pero si es privado las releases no
son descargables sin credenciales y el actualizador no va a poder bajar nada.

---

## La clave privada de firma

```
%USERPROFILE%\.mithflow\mithflow-updater.key       ← la privada (SECRETA)
%USERPROFILE%\.mithflow\mithflow-updater.key.pub   ← la pública (va en tauri.conf.json)
```

**Está fuera del árbol del proyecto a propósito.** Adentro, un `git add -A`
distraído la publicaría para siempre en un repositorio público, y quien la tenga
puede firmar un instalador que todas las instalaciones van a bajar y ejecutar
solas, sin preguntar nada. **No hay forma de revocarla.** El `.gitignore` tiene
además `*.key`, `*.pem` y compañía como segunda red.

**La clave pública sí es pública**: está en `tauri.conf.json` y tiene que estar
ahí, es la que verifica la firma.

### Si se pierde la clave privada

**Nadie puede volver a actualizar las instalaciones que ya están afuera.** Se
puede generar una clave nueva y poner la pública nueva en `tauri.conf.json`,
pero las copias ya instaladas siguen esperando la firma de la clave vieja y van
a rechazar todo lo que se publique. La única salida es reinstalar a mano en cada
máquina.

Por eso: **respaldala.** Son unos 500 bytes.

```powershell
# Generar el par de nuevo (sólo si se perdió y ya se asumió el costo de arriba)
cd app-nativa\app
npm run tauri --silent -- signer generate --ci --password= -w "$env:USERPROFILE\.mithflow\mithflow-updater.key"
```

Después hay que copiar el contenido de `mithflow-updater.key.pub` al campo
`pubkey` de `tauri.conf.json`.

### Sobre la contraseña de la clave

La clave se generó **sin contraseña**, para que publicar sea un comando y no un
comando más una contraseña que nadie recuerda. El costo es real: quien consiga
el archivo puede firmar actualizaciones sin nada más. Como el archivo vive en el
perfil de usuario de la máquina que publica y nunca sale de ahí, el riesgo es
"alguien con acceso a esa computadora", que ya podría hacer cosas peores.

Para ponerle contraseña: generá el par con `--password "loquesea"` en vez de
`--password=`, y en `Generar-Instalador.ps1` cambiá el `--password=` del paso de
firma. **No la escribas en el script**: se commitea junto con él y no habrías
ganado nada.

---

## Esto NO es la firma de código de Windows

Son dos cosas distintas:

| | Firma minisign (esto) | Firma de código (SmartScreen) |
|---|---|---|
| Para qué | que la app confíe en la actualización | que Windows confíe en el instalador |
| Cuesta | nada | cientos de dólares por año |
| Estado | **hecha** | no la tenemos |

Al instalar a mano se va a seguir viendo "Windows protegió su PC" (ese aviso lo
dispara la marca que el navegador le pone a lo que se baja). Al **actualizar**
desde la app el instalador no pasa por el navegador, así que lo esperable es que
no aparezca — pero no está verificado en instalaciones limpias, así que si en
alguna sale, es eso y no un problema: **Más información → Ejecutar de todas
formas**.

---

## Trampas del script, ya resueltas

Están acá porque volverían a morder a quien lo modifique:

- **`Generar-Instalador.ps1` tiene que guardarse como UTF-8 CON BOM.** Windows
  PowerShell 5.1 lee un `.ps1` sin BOM como ANSI, y ahí una raya larga («—») se
  decodifica como tres caracteres, uno de los cuales es una comilla tipográfica
  que PowerShell acepta como delimitador de cadena: el script deja de parsear
  con errores en líneas que no tienen nada malo.
- **Nunca `Get-Content -Raw` para reescribir los archivos de versión**, por lo
  mismo al revés: cada acento entraría como dos caracteres y saldría
  re-codificado. Una publicación convertía `propósito` en `propÃ³sito`. Se lee
  con `[System.IO.File]::ReadAllText(..., UTF8)`.
- **La firma va después del build, no con `createUpdaterArtifacts`.** Firmar
  durante el build exige la clave por variable de entorno, y Windows PowerShell
  5.1 **no puede pasar una variable de entorno vacía** a un proceso hijo
  (asignarle la cadena vacía la borra), así que el CLI se quedaría esperando la
  contraseña por teclado para siempre. Como argumento (`--password=`) sí viaja
  vacía.
- **La clave viaja por ruta, nunca por contenido**: así no queda en el historial
  de la consola ni en ningún log.

Los porqués completos están en `app-nativa/DECISIONES.md`, sección «Plan 8».
