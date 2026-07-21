"""
Línea base reproducible de la versión Python (spec §16.2).

Mide el pipeline COMPLETO —transcripción, limpieza y pegado— sobre un WAV
fijo, N veces, reportando mediana y p95. La latencia percibida incluye las
esperas de `paste_text`, que las métricas de `history.jsonl` no cuentan.

Uso:
    .venv\\Scripts\\python.exe tests\\bench_baseline.py <ruta.wav> [N] [--sin-teclas]

`--sin-teclas` reemplaza el envío de Ctrl+V por una función vacía. El resto
—incluidas las esperas reales— se mantiene, así que el número sigue siendo
representativo, pero no se pega texto en la ventana que tenga el foco. Sin
esa opción, el script pega N veces donde esté el cursor: poné el foco en un
Bloc de notas descartable antes de arrancar.
"""
import statistics
import sys
import time
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import numpy as np


def cargar_wav_16k(ruta):
    """Carga un WAV y lo deja mono a 16 kHz, como espera el modelo."""
    with wave.open(str(ruta)) as w:
        sr, n, ch = w.getframerate(), w.getnframes(), w.getnchannels()
        pcm = np.frombuffer(w.readframes(n), dtype=np.int16).astype("float32") / 32768.0
    if ch == 2:
        pcm = pcm.reshape(-1, 2).mean(axis=1)
    if sr != 16000:
        idx = np.linspace(0, len(pcm), int(len(pcm) * 16000 / sr), endpoint=False)
        pcm = np.interp(idx, np.arange(len(pcm)), pcm).astype("float32")
    return pcm


def resumen(nombre, muestras):
    xs = sorted(muestras)
    p95 = xs[min(len(xs) - 1, int(len(xs) * 0.95))]
    print(f"{nombre}:")
    print(f"    mediana {statistics.median(xs):.3f}s | p95 {p95:.3f}s | "
          f"min {xs[0]:.3f}s | max {xs[-1]:.3f}s")


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    sin_teclas = "--sin-teclas" in sys.argv
    if not args:
        print(__doc__)
        return 1

    wav = Path(args[0])
    if not wav.exists():
        print(f"No encuentro el WAV: {wav}")
        return 1
    n = int(args[1]) if len(args) > 1 else 20

    if sin_teclas:
        import types
        falso = types.ModuleType("keyboard")
        falso.send = lambda _: None
        falso.add_hotkey = lambda *a, **k: None
        falso.wait = lambda: None
        sys.modules["keyboard"] = falso

    import mithflow

    audio = cargar_wav_16k(wav)
    print(f"Audio: {len(audio)/16000:.1f}s | N={n} | "
          f"envío de teclas: {'simulado' if sin_teclas else 'real'}\n")

    modelo = mithflow.load_model()
    mithflow.transcribe(modelo, audio)  # calentar: la primera pasada no cuenta

    solo_stt, percibida = [], []
    for i in range(n):
        t0 = time.perf_counter()
        crudo = mithflow.transcribe(modelo, audio)
        limpio = mithflow.cleanup(crudo)
        t_stt = time.perf_counter() - t0
        mithflow.paste_text(limpio)  # incluye las esperas reales
        t_total = time.perf_counter() - t0
        solo_stt.append(t_stt)
        percibida.append(t_total)
        print(f"  {i+1:2d}/{n}: stt+limpieza {t_stt:.3f}s | percibida {t_total:.3f}s")

    print()
    resumen("Transcripción + limpieza", solo_stt)
    resumen("LATENCIA PERCIBIDA (la vara del criterio 2 del spec)", percibida)
    print(f"\nSobrecosto del pegado: "
          f"{statistics.median(percibida) - statistics.median(solo_stt):.3f}s "
          f"(esperas fijas de paste_text)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
