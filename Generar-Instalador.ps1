<#
.SYNOPSIS
    Publica una versión nueva de MithFlow: sincroniza el número de versión,
    compila, firma el instalador y deja en `instalador\` el `.exe` y el
    `latest.json` listos para adjuntar a una release de GitHub.

.DESCRIPTION
    Un solo comando, porque el proceso de sacar una versión se hace tres veces
    por año y a la cuarta nadie se acuerda de los pasos.

    Lo que hace, en orden:

      1. verifica que el repositorio de GitHub esté configurado (si no, corta
         ACÁ, antes de los diez minutos de compilación);
      2. sincroniza la versión en los TRES archivos que la llevan y lo verifica
         releyéndolos;
      3. compila la interfaz y el instalador;
      4. firma el `.exe` con la clave privada minisign;
      5. arma el `latest.json`, con la URL de descarga derivada del endpoint.

    La versión vive en tres lugares (`app-nativa\Cargo.toml`,
    `app-nativa\app\package.json` y `app-nativa\app\src-tauri\tauri.conf.json`).
    Si se desincronizan, el actualizador se rompe de formas confusas: la app
    diría que tiene una versión y el manifiesto anunciaría otra, así que o no se
    ofrece nunca la actualización o se ofrece para siempre. Por eso el paso 2 no
    sólo escribe: relee los tres y aborta si no coinciden.

.PARAMETER Version
    La versión nueva, como `1.2.3`. Si se omite, se compila la que ya está.

.PARAMETER Notas
    Qué cambió. Va en el `latest.json` y es lo que la app le muestra al usuario
    antes de actualizar.

.EXAMPLE
    .\Generar-Instalador.ps1 -Version 1.1.0 -Notas "Actualizaciones automáticas."

.EXAMPLE
    .\Generar-Instalador.ps1
    Recompila la versión actual sin tocar ningún número.
#>
#requires -Version 5.1

# ⚠ ESTE ARCHIVO TIENE QUE GUARDARSE COMO UTF-8 **CON BOM**.
# Windows PowerShell 5.1 lee los .ps1 sin BOM como ANSI, y ahí una raya larga
# («—») se decodifica como tres caracteres, uno de los cuales es una comilla
# tipográfica que PowerShell acepta como delimitador de cadena. Resultado: el
# script deja de parsear con errores en líneas que no tienen nada malo.
[CmdletBinding()]
param(
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [string]$Notas = "Mejoras y correcciones."
)

$ErrorActionPreference = "Stop"

# El build de whisper.cpp necesita estas tres cosas en el entorno o falla
# buscando el compilador de shaders (glslc).
$env:VULKAN_SDK = "C:\VulkanSDK\1.4.350.0"
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\CMake\bin;$env:VULKAN_SDK\Bin;$env:PATH"

$raiz = $PSScriptRoot
$app = Join-Path $raiz "app-nativa\app"
$destino = Join-Path $raiz "instalador"
$confTauri = Join-Path $app "src-tauri\tauri.conf.json"
$confNpm = Join-Path $app "package.json"
$confCargo = Join-Path $raiz "app-nativa\Cargo.toml"

# La clave privada minisign, DELIBERADAMENTE fuera del árbol del proyecto: acá
# adentro un `git add -A` distraído la publicaría para siempre. Ver el README.
$clavePrivada = Join-Path $env:USERPROFILE ".mithflow\mithflow-updater.key"

if (-not (Test-Path $app)) { throw "No encuentro $app" }

function Leer-Utf8([string]$ruta) {
    # NUNCA `Get-Content -Raw` para esto: en Windows PowerShell 5.1, un archivo
    # sin BOM se lee con la codificación ANSI del sistema, así que cada acento
    # entra como dos caracteres. Combinado con `Escribir-Utf8SinBom`, eso
    # convierte "propósito" en "propÃ³sito" en cada publicación, y a la tercera
    # los comentarios de `Cargo.toml` son ilegibles.
    return [System.IO.File]::ReadAllText($ruta, [System.Text.Encoding]::UTF8)
}

function Escribir-Utf8SinBom([string]$ruta, [string]$texto) {
    # Sin BOM: `Cargo.toml` lo tolera, pero un BOM en un `.json` rompe parsers
    # que no lo esperan, y estos tres archivos los lee gente muy distinta.
    [System.IO.File]::WriteAllText($ruta, $texto, (New-Object System.Text.UTF8Encoding($false)))
}

function Invocar-Nativo([string]$que, [scriptblock]$comando) {
    # Con `$ErrorActionPreference` en "Stop", CUALQUIER línea que un proceso
    # hijo escriba en stderr se convierte en un error terminante aunque termine
    # bien — y el CLI de Tauri manda ahí sus mensajes de progreso. Lo único que
    # dice si algo falló es el código de salida.
    $previo = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $comando
        if ($LASTEXITCODE -ne 0) { throw "$que falló (código de salida $LASTEXITCODE)" }
    } finally {
        $ErrorActionPreference = $previo
    }
}

function Reemplazar-Primera([string]$texto, [string]$patron, [string]$reemplazo) {
    $regex = New-Object System.Text.RegularExpressions.Regex($patron, 'Multiline')
    if (-not $regex.IsMatch($texto)) { throw "no encontré el patrón /$patron/" }
    return $regex.Replace($texto, $reemplazo, 1)
}

# --- Leer la versión de cada archivo, cada uno a su manera -------------------
# A propósito NO se relee con la misma expresión con la que se escribió: si el
# reemplazo fallara en silencio, una verificación que use el mismo patrón
# fallaría igual y no diría nada.

function Leer-VersionCargo {
    $enSeccion = $false
    foreach ($linea in (Leer-Utf8 $confCargo) -split "`r?`n") {
        if ($linea -match '^\s*\[workspace\.package\]\s*$') { $enSeccion = $true; continue }
        if ($enSeccion -and $linea -match '^\s*\[') { break }
        if ($enSeccion -and $linea -match '^\s*version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    throw "no encontré la versión en [workspace.package] de $confCargo"
}

function Leer-Json([string]$ruta) {
    return Leer-Utf8 $ruta | ConvertFrom-Json
}

function Leer-VersionJson([string]$ruta) {
    $leida = (Leer-Json $ruta).version
    if (-not $leida) { throw "no encontré la versión en $ruta" }
    return $leida
}

function Fijar-Version([string]$nueva) {
    # Son tres y no cuatro: `package-lock.json` también lleva el número, pero lo
    # sincroniza `npm install` (que este script corre antes de compilar) y no lo
    # mira nadie más. Los tres de acá SÍ deciden algo — el `CARGO_PKG_VERSION`
    # con el que la app se compara, y el nombre del `.exe` que el bundler emite.
    $cargo = Leer-Utf8 $confCargo
    # Anclado a [workspace.package] para no pisar la versión de otra sección.
    $cargo = Reemplazar-Primera $cargo `
        '(?s)(\[workspace\.package\].*?\bversion\s*=\s*)"[^"]*"' `
        "`${1}""$nueva"""
    Escribir-Utf8SinBom $confCargo $cargo

    foreach ($json in @($confNpm, $confTauri)) {
        $texto = Leer-Utf8 $json
        # La primera aparición y sólo ésa: en `package.json` las dependencias
        # también tienen números, pero ninguna usa la clave "version".
        $texto = Reemplazar-Primera $texto '^(\s*"version"\s*:\s*)"[^"]*"' "`${1}""$nueva"""
        Escribir-Utf8SinBom $json $texto
    }
}

# ============================================================================
# 1. El repositorio de GitHub. Se verifica ANTES de compilar: descubrir que
#    falta configurarlo después de diez minutos de build es el peor momento.
# ============================================================================
$endpoint = @((Leer-Json $confTauri).plugins.updater.endpoints)[0]
if (-not $endpoint) { throw "No hay ningún endpoint en plugins.updater de $confTauri" }

if ($endpoint -match 'REEMPLAZAR') {
    throw @"

  FALTA CONFIGURAR EL REPOSITORIO DE GITHUB.

  Abrí este archivo:
      $confTauri

  y en "plugins" -> "updater" -> "endpoints" reemplazá

      REEMPLAZAR-USUARIO/REEMPLAZAR-REPO

  por tu usuario y tu repositorio, por ejemplo "mithdata/mithflow". Es el
  ÚNICO lugar del proyecto donde va ese dato: este script deriva de ahí la
  URL de descarga del instalador.

  Ojo: la URL queda grabada dentro del .exe. Después de cambiarla hay que
  volver a compilar (o sea, volver a correr este script) para que las
  instalaciones nuevas sepan a dónde consultar.

"@
}

if ($endpoint -notmatch '^https://github\.com/([^/]+)/([^/]+)/releases/') {
    throw "El endpoint '$endpoint' no tiene la forma https://github.com/<usuario>/<repo>/releases/..."
}
$repoGitHub = "$($Matches[1])/$($Matches[2])"

# ============================================================================
# 2. La versión, sincronizada en los tres archivos y verificada releyéndolos.
# ============================================================================
if ($Version) {
    Write-Host "=== Fijando la versión $Version ===" -ForegroundColor Cyan
    Fijar-Version $Version
}

$versiones = [ordered]@{
    "app-nativa\Cargo.toml"                       = Leer-VersionCargo
    "app-nativa\app\package.json"                 = Leer-VersionJson $confNpm
    "app-nativa\app\src-tauri\tauri.conf.json"    = Leer-VersionJson $confTauri
}
# El @(...) no es decorativo: con las tres versiones iguales, `Sort-Object
# -Unique` devuelve UNA CADENA y no un arreglo, y `[0]` sobre una cadena da su
# primer carácter. Sin esto, el caso bueno produce la versión "1".
$distintas = @($versiones.Values | Sort-Object -Unique)
if ($distintas.Count -ne 1) {
    $detalle = ($versiones.GetEnumerator() | ForEach-Object { "    $($_.Key) -> $($_.Value)" }) -join "`n"
    throw "Las versiones no coinciden y el actualizador se rompería:`n$detalle"
}
# `$versionFinal` y no `$version`: PowerShell no distingue mayúsculas en los
# nombres de variable, así que `$version` sería el MISMO que el parámetro
# `$Version` — y asignarle algo revalidaría su `[ValidatePattern]`.
$versionFinal = $distintas[0]
Write-Host "Versión sincronizada en los tres archivos: $versionFinal" -ForegroundColor Green
Write-Host "Repositorio de GitHub: $repoGitHub"

# ============================================================================
# 3. La clave privada. Sin firma no hay actualización posible: el actualizador
#    verifica el .exe contra la clave pública de tauri.conf.json y descarta
#    cualquier cosa que no cierre.
# ============================================================================
if (-not (Test-Path $clavePrivada)) {
    throw @"

  NO ENCUENTRO LA CLAVE PRIVADA DE FIRMA:
      $clavePrivada

  Sin ella el instalador no se puede firmar y NINGUNA instalación existente
  va a poder actualizarse. Si tenés un respaldo, restauralo en esa ruta.

  Generar una clave nueva es posible pero NO arregla las instalaciones que ya
  están afuera: quedarían esperando una firma que ya nadie puede producir, y
  habría que reinstalarlas a mano. Ver la sección "Actualizaciones
  automáticas" del README.md.

"@
}

# ============================================================================
# 4. Compilar.
# ============================================================================
Write-Host "`n=== Compilando la interfaz ===" -ForegroundColor Cyan
Push-Location $app
try {
    Invocar-Nativo "npm install" { npm install }

    Write-Host "`n=== Generando el instalador (varios minutos la primera vez) ===" -ForegroundColor Cyan
    Invocar-Nativo "el build de Tauri" { npm run tauri build }
} finally {
    Pop-Location
}

$origen = Get-ChildItem (Join-Path $raiz "app-nativa\target\release\bundle\nsis\*-setup.exe") |
          Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $origen) { throw "El build terminó pero no encuentro el instalador" }

New-Item -ItemType Directory -Force -Path $destino | Out-Null
Copy-Item $origen.FullName $destino -Force
$final = Join-Path $destino $origen.Name

# ============================================================================
# 5. Firmar. Se hace acá y no dentro del build (`createUpdaterArtifacts`)
#    porque la clave no tiene contraseña, y Windows PowerShell NO PUEDE pasar
#    una variable de entorno vacía a un proceso hijo: asignarle "" la borra, y
#    el CLI de Tauri se queda esperando una contraseña por teclado para
#    siempre. Como argumento (`--password=`) sí viaja vacía.
#
#    La clave viaja por RUTA, nunca por contenido: así no queda en el historial
#    de la consola ni en ningún log.
# ============================================================================
Write-Host "`n=== Firmando el instalador ===" -ForegroundColor Cyan
$cliTauri = Join-Path $app "node_modules\@tauri-apps\cli\tauri.js"
if (-not (Test-Path $cliTauri)) { throw "No encuentro el CLI de Tauri en $cliTauri" }

Invocar-Nativo "la firma de $final" {
    node $cliTauri signer sign --password= -f $clavePrivada $final
}

$rutaFirma = "$final.sig"
if (-not (Test-Path $rutaFirma)) { throw "la firma no quedó en $rutaFirma" }
$firma = (Get-Content $rutaFirma -Raw).Trim()
if (-not $firma) { throw "la firma de $final salió vacía" }

# ============================================================================
# 6. El manifiesto. La URL sale del mismo endpoint que consulta la app, así que
#    no hay un segundo lugar donde el nombre del repositorio pueda quedar mal.
#    La etiqueta de la release TIENE que ser v<version>.
# ============================================================================
$urlDescarga = "https://github.com/$repoGitHub/releases/download/v$versionFinal/$($origen.Name)"
$manifiesto = [ordered]@{
    version   = $versionFinal
    notes     = $Notas
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = [ordered]@{
        # La clave que busca tauri-plugin-updater en esta máquina.
        "windows-x86_64" = [ordered]@{
            signature = $firma
            url       = $urlDescarga
        }
    }
}
$rutaManifiesto = Join-Path $destino "latest.json"
Escribir-Utf8SinBom $rutaManifiesto ($manifiesto | ConvertTo-Json -Depth 5)

# ============================================================================
# 7. Qué hacer con esto.
# ============================================================================
Write-Host "`n=== LISTO — MithFlow $versionFinal ===" -ForegroundColor Green
Write-Host ("{0}  ({1:N1} MB)" -f $final, ((Get-Item $final).Length / 1MB))
Write-Host $rutaManifiesto
Write-Host ""
Write-Host "Para publicarla:" -ForegroundColor Cyan
Write-Host "  1. Creá una release en https://github.com/$repoGitHub/releases/new"
Write-Host "     con la etiqueta EXACTA:  v$versionFinal"
Write-Host "     (si la etiqueta no es esa, la URL del manifiesto apunta a la nada)"
Write-Host "  2. Adjuntá estos dos archivos, con estos nombres:"
Write-Host "        $($origen.Name)"
Write-Host "        latest.json"
Write-Host "  3. Publicala (no la dejes en borrador: 'latest' no ve los borradores)."
Write-Host ""
Write-Host "Las instalaciones existentes van a ver la actualización la próxima vez"
Write-Host "que abran MithFlow."
Write-Host ""
Write-Host "Al instalarlo a mano, Windows va a mostrar 'Windows protegió su PC':" -ForegroundColor DarkGray
Write-Host "el instalador no tiene firma de código (que es otra cosa, y se paga)." -ForegroundColor DarkGray
# La pausa existe porque este script también se corre con clic derecho -> Ejecutar
# con PowerShell, y ahí la ventana se cierra sola al terminar. Desde una consola
# o desde CI no hay a quién preguntarle, y colgarse (o fallar) por eso sería
# arruinar una publicación que ya salió bien.
if ([Environment]::UserInteractive -and $Host.Name -eq "ConsoleHost") {
    try { Read-Host "`nEnter para salir" | Out-Null } catch { }
}
