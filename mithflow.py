#!/usr/bin/env python3
"""
MithFlow — Dictado local estilo Wispr Flow, 100% offline y gratis.
Autor: MithData + Claude
Uso: python mithflow.py
     Presioná la tecla configurada (default: F8) para empezar a grabar,
     presionala de nuevo para detener. El texto transcripto (y limpiado
     opcionalmente por un LLM local vía Ollama) se pega donde esté el cursor.

Requisitos: ver requirements.txt. Windows 10/11 (adaptable a Linux/Mac).
"""

import json
import os
import queue
import sys
import threading
import time

# ==============================================================
# CONFIGURACIÓN — editá esto a gusto
# ==============================================================
HOTKEY = "F8"              # Tecla de toggle grabar/detener
MODEL_SIZE = "auto"        # "auto": large-v3-turbo con GPU NVIDIA, small en CPU.
                           # O fijalo a mano: tiny, base, small, medium, large-v3, large-v3-turbo
DEVICE = "auto"            # "auto" detecta GPU; forzá "cpu" o "cuda" si querés
LANGUAGE = "es"            # Idioma del dictado (None = autodetectar)
SAMPLE_RATE = 16000

# Vocabulario propio: Whisper lo usa como contexto para no "inventar" palabras.
INITIAL_PROMPT = (
    "Transcripción de dictado en español rioplatense sobre negocios y tecnología. "
    "Términos frecuentes: MithData, PyME, dashboard, frontend, backend, UX, UI, "
    "lead, CRM, IA, ciencia de datos, machine learning, API, Canva."
)

# Velocidad de transcripción: beam_size 1 es ~2x más rápido que 5, con
# diferencia mínima en dictados cortos. Subilo si notás errores.
BEAM_SIZE = 1

# Modo de limpieza del texto:
#   "fast" — reglas locales, instantáneo (<1ms). Recomendado: latencia tipo Wispr Flow.
#   "llm"  — Ollama pule la redacción, pero suma ~3s por dictado.
#   "off"  — pega la transcripción cruda de Whisper.
CLEANUP_MODE = "fast"

# Muletillas a eliminar en modo "fast". Solo se sacan cuando Whisper las aisló
# con comas (o al inicio), para no romper usos legítimos: "Este dashboard..."
# se mantiene, "Este, quería decirte..." se limpia.
FILLERS = [
    "eh", "ehh", "em", "mmm", "este", "esto", "o sea", "osea",
    "digamos", "viste", "tipo",
]
# Deliberadamente NO incluidas: "bueno", "nada", "a ver". Son muletillas
# frecuentes, pero también arranques legítimos ("Bueno, quedamos así") y
# sacarlas cambia el tono. Agregalas acá si preferís texto más seco.

# Palabras que se tartamudean al dictar ("el el informe"). Lista explícita
# para no tocar repeticiones intencionales ("muy muy bueno", "no no").
STUTTER_WORDS = [
    "el", "la", "los", "las", "un", "una", "unos", "unas", "de", "del",
    "que", "y", "a", "en", "con", "por", "para", "se", "lo", "le", "les",
    "es", "al", "su", "mi", "te", "me",
]

OLLAMA_URL = "http://localhost:11434/api/generate"
OLLAMA_MODEL = "llama3.2:3b"   # liviano y suficiente para limpieza de texto
CLEANUP_PROMPT = (
    "Sos un corrector de dictado por voz. Recibís una transcripción cruda y "
    "devolvés SOLO el texto corregido, sin comentarios ni comillas. Reglas: "
    "eliminá muletillas (eh, este, o sea, digamos, viste), corregí puntuación y "
    "mayúsculas, mantené el significado y el tono EXACTOS, no resumas ni agregues "
    "nada. Si el texto ya está bien, devolvelo igual.\n\nTranscripción: {text}"
)
# Historial local para el dashboard (Streamlit). Solo en tu disco.
HISTORY_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "history.jsonl")
STATUS_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "status.json")
# ==============================================================

recording = False
audio_q = queue.Queue()
frames = []
stream = None          # se abre al grabar y se cierra al terminar (ver toggle)
_instance_mutex = None  # hay que mantener el handle vivo mientras corra el proceso

# El acorde de inicio dura 0.14s y el micrófono lo capta por los parlantes.
# Se descarta ese tramo inicial para no meterle un chirrido a la transcripción.
TONE_GUARD_S = 0.2


# Con salida redirigida a archivo, Windows usa cp1252 y los emojis/tildes
# rompen print() — y una excepción en el hilo del hotkey lo mata para siempre.
for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass


def log(msg):
    try:
        print(f"[MithFlow] {msg}", flush=True)
    except Exception:
        pass


def play_tone(freqs, dur=0.14, vol=0.15):
    """Tono suave con fade-in/out — mucho más amigable que winsound.Beep.
    `freqs` es una lista: varias frecuencias suenan como acorde."""
    try:
        import numpy as np
        import sounddevice as sd
        sr = 44100
        t = np.linspace(0, dur, int(sr * dur), False)
        wave = sum(np.sin(2 * np.pi * f * t) for f in freqs) / len(freqs)
        n_fade = max(1, int(sr * 0.02))
        envelope = np.ones_like(wave)
        envelope[:n_fade] = np.linspace(0, 1, n_fade)
        envelope[-n_fade:] = np.linspace(1, 0, n_fade)
        sd.play((wave * envelope * vol).astype("float32"), sr)
    except Exception:
        pass


def beep_start():
    play_tone([523, 659])          # acorde suave ascendente: empezó a grabar


def beep_stop():
    play_tone([392])               # tono corto y grave: dejó de grabar


def beep_done():
    play_tone([659, 784], dur=0.18)  # campanita: texto pegado


def beep_error():
    play_tone([220], dur=0.3)      # grave y largo: algo falló


def register_cuda_dlls():
    """Windows no encuentra las DLLs de cuBLAS/cuDNN instaladas vía pip
    (nvidia-cublas-cu12 / nvidia-cudnn-cu12) porque no están en el PATH.
    Las registramos explícitamente antes de cargar CTranslate2."""
    base = os.path.join(sys.prefix, "Lib", "site-packages", "nvidia")
    for sub in ("cublas", "cudnn", "cuda_nvrtc"):
        dll_dir = os.path.join(base, sub, "bin")
        if os.path.isdir(dll_dir):
            os.add_dll_directory(dll_dir)
            os.environ["PATH"] = dll_dir + os.pathsep + os.environ["PATH"]


def load_model():
    register_cuda_dlls()
    from faster_whisper import WhisperModel
    device = DEVICE
    compute = "int8"
    if device == "auto":
        try:
            import ctranslate2
            device = "cuda" if ctranslate2.get_cuda_device_count() > 0 else "cpu"
        except Exception:
            device = "cpu"
    if device == "cuda":
        compute = "float16"
    model_size = MODEL_SIZE
    if model_size == "auto":
        model_size = "large-v3-turbo" if device == "cuda" else "small"
    log(f"Cargando modelo '{model_size}' en {device} ({compute})... "
        "(la primera vez descarga el modelo, puede tardar)")
    model = WhisperModel(model_size, device=device, compute_type=compute)
    log("Modelo listo.")
    return model


def audio_callback(indata, frames_count, time_info, status):
    if recording:
        audio_q.put(indata.copy())


def transcribe(model, audio):
    segments, info = model.transcribe(
        audio,
        language=LANGUAGE,
        beam_size=BEAM_SIZE,
        vad_filter=True,               # recorta silencios
        initial_prompt=INITIAL_PROMPT,
    )
    return " ".join(s.text.strip() for s in segments).strip()


def fast_cleanup(text):
    """Limpieza por reglas: instantánea, sin LLM. Conservadora por diseño —
    ante la duda deja el texto como está (Whisper ya puntúa y capitaliza bien)."""
    import re
    if not text:
        return text
    cleaned = text

    # 1. Muletillas aisladas por comas: "Bueno, este, quería..." -> "Bueno, quería..."
    for filler in FILLERS:
        pattern = re.escape(filler)
        cleaned = re.sub(rf",\s*{pattern}\s*,", ",", cleaned, flags=re.IGNORECASE)
        # Al inicio de la frase o después de un punto: "Eh, quería..." -> "Quería..."
        cleaned = re.sub(rf"(^|(?<=[.!?])\s){pattern}\s*,\s*", r"\1", cleaned, flags=re.IGNORECASE)

    # 2. Tartamudeos: "el el dashboard" -> "el dashboard"
    stutter_re = "|".join(re.escape(w) for w in STUTTER_WORDS)
    cleaned = re.sub(rf"\b({stutter_re})(\s+\1)+\b", r"\1", cleaned, flags=re.IGNORECASE)

    # 3. Espacios y puntuación: dobles espacios, espacio antes de coma/punto,
    #    comas duplicadas que quedan al sacar muletillas.
    cleaned = re.sub(r"\s+", " ", cleaned)
    cleaned = re.sub(r"\s+([,.;:!?])", r"\1", cleaned)
    cleaned = re.sub(r",\s*(,\s*)+", ", ", cleaned)
    cleaned = re.sub(r"^[\s,]+", "", cleaned)
    cleaned = cleaned.strip()

    # 4. Mayúscula inicial (sin tocar el resto: puede tener siglas como CRM, API)
    if cleaned and cleaned[0].islower():
        cleaned = cleaned[0].upper() + cleaned[1:]

    # Red de seguridad: si las reglas se comieron más de la mitad del texto,
    # algo salió mal — devolver el original.
    if len(cleaned) < len(text) * 0.5:
        return text
    return cleaned


def cleanup(text):
    """Aplica el modo de limpieza configurado."""
    if CLEANUP_MODE == "off" or not text:
        return text
    if CLEANUP_MODE == "fast":
        return fast_cleanup(text)
    return ollama_cleanup(text)


def ollama_cleanup(text):
    """Limpieza con LLM local. Si Ollama no está corriendo, devuelve el texto crudo."""
    if not text:
        return text
    try:
        import requests
        r = requests.post(
            OLLAMA_URL,
            json={
                "model": OLLAMA_MODEL,
                "prompt": CLEANUP_PROMPT.format(text=text),
                "stream": False,
                "keep_alive": "30m",     # evita recargar el modelo en cada dictado
                "options": {"temperature": 0.1, "num_predict": 300},
            },
            timeout=60,
        )
        r.raise_for_status()
        cleaned = r.json().get("response", "").strip()
        # Sanidad: si el LLM devolvió algo vacío o desproporcionado, usar el crudo
        if cleaned and 0.4 < len(cleaned) / max(len(text), 1) < 2.5:
            return cleaned
        return text
    except Exception as e:
        log(f"Ollama no disponible ({e}); uso transcripción cruda.")
        return text


def paste_text(text):
    """Pega el texto donde esté el cursor, preservando el portapapeles previo.

    Devuelve True si pegó. Si falla, deja el texto dictado en el portapapeles
    a propósito: restaurar el contenido anterior perdería la transcripción.
    """
    import keyboard
    import pyperclip
    old_clip = None
    try:
        old_clip = pyperclip.paste()
    except Exception:
        pass
    try:
        pyperclip.copy(text)
        time.sleep(0.15)
        keyboard.send("ctrl+v")
        log("Texto pegado.")
    except Exception as e:
        log(f"ERROR al pegar: {e} — el texto quedó en el portapapeles.")
        return False
    time.sleep(0.3)
    if old_clip is not None:
        try:
            pyperclip.copy(old_clip)
        except Exception:
            pass
    return True


def process_recording(model, captured):
    """Procesa un bloque de audio ya capturado.

    `captured` se recibe por parámetro y no por variable global a propósito:
    si se leyera el global, un segundo F8 disparado mientras esto corre
    borraría el buffer de la grabación nueva.
    """
    import numpy as np
    if not captured:
        log("No se capturó audio.")
        return
    audio = np.concatenate(captured, axis=0).flatten().astype("float32")
    # Descartar el tramo donde suena el tono de inicio (ver TONE_GUARD_S)
    guard = int(TONE_GUARD_S * SAMPLE_RATE)
    if len(audio) > guard:
        audio = audio[guard:]
    dur = len(audio) / SAMPLE_RATE
    if dur < 0.5:
        log("Audio demasiado corto, ignorado.")
        return
    log(f"Transcribiendo {dur:.1f}s de audio...")
    t0 = time.time()
    raw = transcribe(model, audio)
    transcribe_s = time.time() - t0
    log(f"Crudo ({transcribe_s:.1f}s): {raw}")
    if not raw:
        # Sin realimentación el usuario no sabe si falló o si no lo escuchó.
        log("Transcripción vacía (¿silencio o ruido?).")
        beep_error()
        return
    t0 = time.time()
    final = cleanup(raw)
    cleanup_s = time.time() - t0
    if final != raw:
        log(f"Limpio: {final}")
    # El sonido debe reflejar lo que pasó de verdad: campanita solo si pegó.
    beep_done() if paste_text(final) else beep_error()
    save_history(dur, transcribe_s, cleanup_s, raw, final)


def save_history(audio_s, transcribe_s, cleanup_s, raw, final):
    """Registro local para el dashboard. Nunca debe romper el dictado."""
    try:
        entry = {
            "ts": time.strftime("%Y-%m-%dT%H:%M:%S"),
            "audio_s": round(audio_s, 2),
            "transcribe_s": round(transcribe_s, 2),
            "cleanup_s": round(cleanup_s, 2),
            "words": len(final.split()),
            "cleaned": final != raw,
            "mode": CLEANUP_MODE,
            "raw": raw,
            "final": final,
        }
        with open(HISTORY_FILE, "a", encoding="utf-8") as f:
            f.write(json.dumps(entry, ensure_ascii=False) + "\n")
    except Exception as e:
        log(f"No pude guardar historial: {e}")


def toggle(model):
    global recording, frames, stream
    import sounddevice as sd

    if not recording:
        frames = []
        while not audio_q.empty():
            audio_q.get()
        beep_start()
        # El micrófono se abre recién ahora y se cierra al terminar: mantenerlo
        # abierto todo el tiempo deja prendido el indicador de micrófono de
        # Windows y consume batería en las notebooks.
        try:
            stream = sd.InputStream(
                samplerate=SAMPLE_RATE,
                channels=1,
                dtype="float32",
                callback=audio_callback,
            )
            stream.start()
        except Exception as e:
            log(f"ERROR al abrir el micrófono: {e}")
            beep_error()
            stream = None
            return
        recording = True
        log("🎙️  Grabando... (presioná la tecla de nuevo para detener)")
    else:
        recording = False
        beep_stop()
        if stream is not None:
            try:
                stream.stop()
                stream.close()
            except Exception as e:
                log(f"No pude cerrar el micrófono: {e}")
            stream = None
        # Drenar la cola
        while not audio_q.empty():
            frames.append(audio_q.get())
        # Entregar el buffer al hilo y dejar el global limpio, para que una
        # grabación nueva no comparta estado con la que se está procesando.
        captured, frames = frames, []
        threading.Thread(
            target=process_recording, args=(model, captured), daemon=True
        ).start()


def ensure_single_instance():
    """Si ya hay un MithFlow corriendo, salir: dos instancias duplican
    el hotkey y pegan el texto dos veces.

    Usa el espacio de nombres `Local\\` a propósito: `Global\\` requiere el
    privilegio SeCreateGlobalPrivilege y, sin él, CreateMutexW falla con
    ERROR_ACCESS_DENIED (5) en vez de 183 — el chequeo pasaba de largo y
    terminaban corriendo dos instancias. `Local\\` alcanza: solo hace falta
    detectar otra instancia de la misma sesión de usuario.
    """
    global _instance_mutex
    import ctypes
    kernel32 = ctypes.windll.kernel32
    handle = kernel32.CreateMutexW(None, False, "Local\\MithFlowSingleInstance")
    err = kernel32.GetLastError()
    if err == 183:  # ERROR_ALREADY_EXISTS
        log("Ya hay otro MithFlow corriendo. Cerrando este.")
        beep_error()
        time.sleep(0.5)  # dejar sonar el aviso antes de salir
        sys.exit(0)
    if not handle:
        # Fail closed: si no se puede verificar, no arrancar a ciegas.
        log(f"No pude verificar si ya hay otra instancia (error {err}). Cerrando.")
        beep_error()
        time.sleep(0.5)
        sys.exit(1)
    _instance_mutex = handle  # mantenerlo vivo mientras dure el proceso


def heartbeat_loop():
    """Latido cada 10s para que el dashboard sepa que el motor está vivo,
    sin depender de inspeccionar procesos (falla entre sandboxes/sesiones)."""
    while True:
        try:
            with open(STATUS_FILE, "w", encoding="utf-8") as f:
                json.dump({"pid": os.getpid(), "ts": time.time()}, f)
        except Exception:
            pass
        time.sleep(10)


def main():
    import keyboard

    ensure_single_instance()
    threading.Thread(target=heartbeat_loop, daemon=True).start()

    model = load_model()

    def safe_toggle():
        # Nunca dejar escapar una excepción: mataría el hilo de eventos
        # de `keyboard` y el hotkey dejaría de responder silenciosamente.
        try:
            toggle(model)
        except Exception as e:
            log(f"Error en hotkey: {e}")
            beep_error()

    # suppress=True: la tecla NO llega a la app enfocada — el cursor se queda
    # donde estaba (sin esto, F8 puede mover el foco y el pegado va a otro lado).
    keyboard.add_hotkey(HOTKEY.lower(), safe_toggle, suppress=True)
    log(f"Listo. Tecla de dictado: {HOTKEY}. Ctrl+C para salir.")
    try:
        keyboard.wait()
    except KeyboardInterrupt:
        log("Chau.")
        if stream is not None:
            try:
                stream.stop()
                stream.close()
            except Exception:
                pass
        sys.exit(0)


if __name__ == "__main__":
    main()
