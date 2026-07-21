@echo off
title Detener MithFlow
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='python.exe'\" | Where-Object { $_.CommandLine -like '*mithflow.py*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force; Write-Host ('MithFlow detenido (PID ' + $_.ProcessId + ')') }"
pause
