"""
Verifica el contrato de `paste_text`: si el pegado falla, la transcripción
NO se pierde.

El bug original: el restore del portapapeles estaba fuera del `try`, así que
cuando el Ctrl+V fallaba se restauraba igual el contenido anterior y el texto
dictado desaparecía sin dejar rastro.

Se portan a Rust en el Plan 1 (Task 1.7), donde `paste()` mantiene el mismo
contrato: al fallar, el texto queda en el portapapeles.

Uso: .venv\\Scripts\\python.exe tests\\test_paste.py
"""
import sys
import types
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

PREVIO = "CONTENIDO PREVIO DEL USUARIO"
DICTADO = "Texto dictado importante"

clipboard = {"v": PREVIO}

fake_pyperclip = types.ModuleType("pyperclip")
fake_pyperclip.paste = lambda: clipboard["v"]
fake_pyperclip.copy = lambda t: clipboard.__setitem__("v", t)

fake_keyboard = types.ModuleType("keyboard")

sys.modules["pyperclip"] = fake_pyperclip
sys.modules["keyboard"] = fake_keyboard

import mithflow  # noqa: E402  (después de instalar los dobles)


def caso_pegado_exitoso():
    fake_keyboard.send = lambda _: None
    clipboard["v"] = PREVIO
    ok = mithflow.paste_text(DICTADO)
    assert ok is True, f"debería devolver True, devolvió {ok!r}"
    assert clipboard["v"] == PREVIO, (
        f"debería restaurar el portapapeles previo, quedó {clipboard['v']!r}"
    )
    return "pega y restaura el portapapeles previo"


def caso_pegado_fallido():
    def falla(_):
        raise RuntimeError("simulación: Ctrl+V falló")

    fake_keyboard.send = falla
    clipboard["v"] = PREVIO
    ok = mithflow.paste_text(DICTADO)
    assert ok is False, f"debería devolver False, devolvió {ok!r}"
    assert clipboard["v"] == DICTADO, (
        f"REGRESIÓN: se perdió la transcripción. El portapapeles tiene {clipboard['v']!r}"
    )
    return "al fallar, la transcripción queda en el portapapeles"


def main():
    fallos = 0
    for caso in (caso_pegado_exitoso, caso_pegado_fallido):
        try:
            print(f"OK  {caso()}")
        except AssertionError as e:
            fallos += 1
            print(f"FALLA  {caso.__name__}: {e}")
    print(f"\n{2 - fallos}/2 casos OK")
    return 1 if fallos else 0


if __name__ == "__main__":
    sys.exit(main())
