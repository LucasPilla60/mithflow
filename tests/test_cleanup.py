"""
Suite de la limpieza por reglas de MithFlow.

Estos 12 casos son la especificación ejecutable del comportamiento de
`fast_cleanup`. Los seis primeros verifican que limpia; los seis últimos, que
NO toca texto legítimo — y esos son los importantes: los dos últimos surgieron
de fallos reales de la primera implementación, que borraba "Bueno," al inicio
de una frase y convertía "muy muy bueno" en "muy bueno".

Se portan a Rust en la migración a la app nativa (ver
docs/superpowers/plans/2026-07-21-mithflow-nativo-fase-0-1.md, Task 1.3).

Uso: .venv\\Scripts\\python.exe tests\\test_cleanup.py
"""
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import mithflow

# (entrada, esperado). None = el texto NO debe cambiar.
CASES = [
    # --- Debe limpiar ---
    ("Eh, quería decirte que el dashboard está listo.",
     "Quería decirte que el dashboard está listo."),
    ("Bueno, este, quería mostrarte el CRM.",
     "Bueno, quería mostrarte el CRM."),
    ("El cliente, o sea, pidió el reporte.",
     "El cliente, pidió el reporte."),
    ("Vamos a ver el el dashboard de MithData.",
     "Vamos a ver el dashboard de MithData."),
    ("Hola  ,  qué tal .", "Hola, qué tal."),
    ("el reporte ya está.", "El reporte ya está."),

    # --- NO debe tocar (usos legítimos) ---
    ("Este dashboard está listo para el cliente.", None),
    ("No quedó nada pendiente.", None),
    ("Bueno el resultado, malo el proceso.", None),
    ("Vamos a ver qué dice la API del CRM.", None),
    ("Ahora me parece que sí está funcionando, aunque en el frontend local el motor no se inicia.", None),
    ("Muy muy bueno el resultado.", None),
]


def main():
    fallos = 0
    for entrada, esperado in CASES:
        objetivo = entrada if esperado is None else esperado
        salida = mithflow.fast_cleanup(entrada)
        if salida != objetivo:
            fallos += 1
            print(f"[FALLA] entrada:  {entrada!r}")
            print(f"        esperado: {objetivo!r}")
            print(f"        obtenido: {salida!r}\n")

    muestra = "Eh, bueno, este, quería decirte que el el dashboard ya está listo, viste."
    t0 = time.perf_counter()
    for _ in range(1000):
        mithflow.fast_cleanup(muestra)
    ms = (time.perf_counter() - t0)

    print(f"{len(CASES) - fallos}/{len(CASES)} casos OK")
    print(f"Latencia fast_cleanup: {ms:.3f} ms por dictado")
    return 1 if fallos else 0


if __name__ == "__main__":
    sys.exit(main())
