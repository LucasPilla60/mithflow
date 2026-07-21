' MithFlow en segundo plano, sin ventana de consola.
' Los logs quedan en mithflow.log (misma carpeta).
' Para detenerlo: doble clic en Detener-MithFlow.bat
Set fso = CreateObject("Scripting.FileSystemObject")
folder = fso.GetParentFolderName(WScript.ScriptFullName)
Set shell = CreateObject("WScript.Shell")
shell.CurrentDirectory = folder
shell.Run "cmd /c "".venv\Scripts\python.exe mithflow.py > mithflow.log 2>&1""", 0, False
