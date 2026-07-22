# ============================================================
# Genera el instalador de la app nativa y lo deja en D:\MithFlow\instalador\
#
# El bundler de Tauri lo escupe en app-nativa\target\release\bundle\nsis\,
# que es la carpeta de artefactos de compilación: descartable y enterrada.
# Este script hace el build y lo copia a un lugar visible.
#
# Uso: clic derecho -> Ejecutar con PowerShell
# ============================================================
$ErrorActionPreference = "Stop"

# El build de whisper.cpp necesita estas tres cosas en el entorno o falla
# buscando el compilador de shaders (glslc).
$env:VULKAN_SDK = "C:\VulkanSDK\1.4.350.0"
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\CMake\bin;$env:VULKAN_SDK\Bin;$env:PATH"

$raiz = $PSScriptRoot
$app = Join-Path $raiz "app-nativa\app"
$destino = Join-Path $raiz "instalador"

if (-not (Test-Path $app)) { throw "No encuentro $app" }

Write-Host "=== Compilando la interfaz ===" -ForegroundColor Cyan
Push-Location $app
try {
    npm install
    if ($LASTEXITCODE -ne 0) { throw "npm install falló" }

    Write-Host "`n=== Generando el instalador (varios minutos la primera vez) ===" -ForegroundColor Cyan
    npm run tauri build
    if ($LASTEXITCODE -ne 0) { throw "el build de Tauri falló" }
} finally {
    Pop-Location
}

$origen = Get-ChildItem (Join-Path $raiz "app-nativa\target\release\bundle\nsis\*-setup.exe") |
          Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $origen) { throw "El build terminó pero no encuentro el instalador" }

New-Item -ItemType Directory -Force -Path $destino | Out-Null
Copy-Item $origen.FullName $destino -Force

$final = Join-Path $destino $origen.Name
Write-Host "`n=== LISTO ===" -ForegroundColor Green
Write-Host ("{0}  ({1:N1} MB)" -f $final, ((Get-Item $final).Length / 1MB))
Write-Host "`nAl ejecutarlo, Windows va a mostrar 'Windows protegió su PC':"
Write-Host "el instalador no está firmado. Más información -> Ejecutar de todas formas."
Read-Host "`nEnter para salir"
