' MithFlow App: motor de dictado (F8) de fondo + dashboard como aplicacion.
' Doble clic y listo, como Wispr Flow.
Set fso = CreateObject("Scripting.FileSystemObject")
folder = fso.GetParentFolderName(WScript.ScriptFullName)
Set shell = CreateObject("WScript.Shell")
shell.CurrentDirectory = folder

' 1. Motor de dictado (si ya hay uno corriendo, el nuevo se cierra solo)
shell.Run "cmd /c "".venv\Scripts\python.exe mithflow.py > mithflow.log 2>&1""", 0, False

' 2. Dashboard Streamlit en segundo plano (puerto fijo 8501)
shell.Run "cmd /c "".venv\Scripts\python.exe -m streamlit run dashboard.py --server.headless true --server.port 8501 --server.address 127.0.0.1 --browser.gatherUsageStats false > dashboard.log 2>&1""", 0, False

' 3. Esperar a que el server levante y abrir como ventana de aplicacion (Edge)
WScript.Sleep 8000
shell.Run "cmd /c start msedge --app=http://localhost:8501", 0, False
