# ============================================================
# MithFlow — Instalador para cualquier PC/notebook (Windows)
# Uso: clic derecho -> "Ejecutar con PowerShell"
#      (o en consola:  powershell -ExecutionPolicy Bypass -File instalar.ps1)
# Requiere: Python 3.10+ instalado con "Add python.exe to PATH".
# ============================================================
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
Write-Host "=== Instalando MithFlow en $root ===" -ForegroundColor Cyan

# --- 1. Python -----------------------------------------------------------
$py = Get-Command python -ErrorAction SilentlyContinue
if (-not $py) {
    Write-Host "ERROR: Python no esta instalado o no esta en el PATH." -ForegroundColor Red
    Write-Host "Instalalo desde https://python.org (tildar 'Add python.exe to PATH') y volve a correr esto."
    Read-Host "Enter para salir"; exit 1
}
$pyVersion = (python --version) -replace 'Python\s+',''
Write-Host "Python encontrado: $pyVersion"
$major, $minor = $pyVersion.Split('.')[0..1]
if ([int]$major -lt 3 -or ([int]$major -eq 3 -and [int]$minor -lt 10)) {
    Write-Host "ERROR: se necesita Python 3.10 o superior (tenes $pyVersion)." -ForegroundColor Red
    Read-Host "Enter para salir"; exit 1
}

# --- 2. Archivos necesarios ----------------------------------------------
$required = @("mithflow.py", "dashboard.py", "requirements.txt")
$missing = $required | Where-Object { -not (Test-Path (Join-Path $root $_)) }
if ($missing) {
    Write-Host "ERROR: faltan archivos: $($missing -join ', ')" -ForegroundColor Red
    Write-Host "Copia la carpeta completa (sin .venv) desde la PC original."
    Read-Host "Enter para salir"; exit 1
}

# --- 3. Venv + dependencias ----------------------------------------------
if (-not (Test-Path "$root\.venv")) {
    Write-Host "Creando entorno virtual..."
    python -m venv "$root\.venv"
}
$venvPy = "$root\.venv\Scripts\python.exe"
Write-Host "Instalando dependencias (puede tardar unos minutos)..."
& $venvPy -m pip install --upgrade pip -q
& $venvPy -m pip install -r "$root\requirements.txt" -q

# --- 4. GPU NVIDIA -> librerias CUDA -------------------------------------
# Sin GPU no se instalan: mithflow.py detecta CPU y usa el modelo 'small'.
if (Get-Command nvidia-smi -ErrorAction SilentlyContinue) {
    Write-Host "GPU NVIDIA detectada: instalando librerias CUDA..." -ForegroundColor Green
    & $venvPy -m pip install nvidia-cublas-cu12 nvidia-cudnn-cu12 -q
    Write-Host "  -> se usara el modelo large-v3-turbo (rapido y preciso)."
} else {
    Write-Host "Sin GPU NVIDIA: se usara CPU con el modelo 'small'." -ForegroundColor Yellow
    Write-Host "  -> funciona bien, solo agrega unos segundos por dictado."
}

# --- 5. Config de Streamlit (privacidad: solo localhost) ------------------
$cfgDir = Join-Path $root ".streamlit"
$cfgFile = Join-Path $cfgDir "config.toml"
if (-not (Test-Path $cfgFile)) {
    Write-Host "Creando .streamlit\config.toml (dashboard solo accesible desde esta PC)..."
    New-Item -ItemType Directory -Force -Path $cfgDir | Out-Null
    @'
[theme]
base = "dark"
primaryColor = "#22C3A6"
backgroundColor = "#0D1117"
secondaryBackgroundColor = "#161B26"
textColor = "#E6EDF3"
font = "sans serif"

[browser]
gatherUsageStats = false

[server]
port = 8501
# Solo accesible desde esta maquina: el historial de dictados es privado.
address = "127.0.0.1"
'@ | Out-File -FilePath $cfgFile -Encoding utf8
}

# --- 6. Verificacion -----------------------------------------------------
Write-Host ""
Write-Host "Verificando instalacion..." -ForegroundColor Cyan
& $venvPy -c "import sounddevice as sd, keyboard, pyperclip, faster_whisper, streamlit, pandas; d = sd.query_devices(kind='input'); print('  Microfono: ' + d['name']); print('  Dependencias: OK')"
if ($LASTEXITCODE -ne 0) {
    Write-Host "ADVERTENCIA: fallo la verificacion. Revisa el error de arriba." -ForegroundColor Yellow
    Write-Host "Si dice que no hay dispositivo de entrada, conecta un microfono."
}

# --- 7. Ollama (OPCIONAL) ------------------------------------------------
# Por default CLEANUP_MODE = "fast" (limpieza por reglas, <1ms). Ollama solo
# hace falta si queres el pulido completo con LLM (CLEANUP_MODE = "llm").
Write-Host ""
Write-Host "Ollama es OPCIONAL: solo si queres limpieza con LLM (suma ~3.5s por dictado)." -ForegroundColor DarkGray
$resp = Read-Host "Instalar Ollama + llama3.2:3b? (~3 GB) [s/N]"
if ($resp -match '^[sS]') {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        Write-Host "winget no disponible: instalalo a mano desde https://ollama.com" -ForegroundColor Yellow
    } else {
        winget install --id Ollama.Ollama -e --accept-source-agreements --accept-package-agreements --silent
        $env:PATH += ";$env:LOCALAPPDATA\Programs\Ollama"
        ollama pull llama3.2:3b
        Write-Host "Listo. Para usarlo, poné CLEANUP_MODE = 'llm' en mithflow.py" -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "=== INSTALACION COMPLETA ===" -ForegroundColor Cyan
Write-Host "Para usar: doble clic en MithFlow-App.vbs (motor + dashboard)."
Write-Host "La primera corrida descarga el modelo Whisper (1-2 GB, una sola vez)."
Write-Host "Arranque automatico: Win+R -> shell:startup -> pegar acceso directo a MithFlow-invisible.vbs"
Read-Host "Enter para salir"
