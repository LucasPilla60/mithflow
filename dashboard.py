"""
MithFlow Dashboard — analíticas del dictado local (Streamlit).
Uso: se abre solo con MithFlow-App.vbs, o a mano:
     .venv\\Scripts\\python.exe -m streamlit run dashboard.py
"""
import json
import os
import subprocess
import time
from datetime import datetime

import altair as alt
import pandas as pd
import psutil
import streamlit as st

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
HISTORY_FILE = os.path.join(BASE_DIR, "history.jsonl")
STATUS_FILE = os.path.join(BASE_DIR, "status.json")
TYPING_WPM = 40  # velocidad de tipeo promedio, para estimar tiempo ahorrado

ACCENT = "#22C3A6"
ACCENT_SOFT = "#1A8F7B"

st.set_page_config(page_title="MithFlow", page_icon="🎙️", layout="wide")

st.markdown("""
<style>
    /* Ocultar cromo de Streamlit para look de app nativa */
    #MainMenu, footer, header {visibility: hidden;}
    .block-container {padding-top: 1.5rem; padding-bottom: 2rem;}

    /* Tarjetas de métricas */
    div[data-testid="stMetric"] {
        background: linear-gradient(160deg, #161B26 0%, #12161F 100%);
        border: 1px solid #232B3A;
        border-radius: 14px;
        padding: 14px 18px;
    }
    div[data-testid="stMetric"] label {color: #8B96A8 !important;}
    div[data-testid="stMetricValue"] {color: #E6EDF3;}

    h1 {letter-spacing: -0.5px;}
    .mf-subtitle {color: #8B96A8; margin-top: -0.6rem; font-size: 0.95rem;}
</style>
""", unsafe_allow_html=True)


# ----------------------------------------------------------------------
# Motor: estado y control
# ----------------------------------------------------------------------
def engine_status():
    """(activo, pid) según el latido que escribe el motor cada 10s."""
    try:
        with open(STATUS_FILE, encoding="utf-8") as f:
            info = json.load(f)
        alive = (time.time() - info.get("ts", 0)) < 30
        return alive, info.get("pid")
    except (OSError, json.JSONDecodeError):
        return False, None


def start_engine():
    vbs = os.path.join(BASE_DIR, "MithFlow-invisible.vbs")
    subprocess.Popen(["wscript.exe", vbs], cwd=BASE_DIR)


def es_proceso_mithflow(proc):
    """Confirma que el PID sigue siendo MithFlow y no un proceso reciclado.

    Windows reutiliza PIDs: si el motor murió sin borrar status.json, matar
    ese PID a ciegas podría cerrarle cualquier otro programa al usuario.
    """
    try:
        return "mithflow.py" in " ".join(proc.cmdline()).lower()
    except (psutil.NoSuchProcess, psutil.AccessDenied, OSError):
        return False


def stop_engine(pid):
    """Detiene el motor. Devuelve True solo si realmente lo detuvo."""
    if pid is None:
        return False
    try:
        proc = psutil.Process(pid)
    except psutil.NoSuchProcess:
        # Ya no existe: el latido quedó viejo, limpiarlo es correcto.
        _borrar_status()
        return True
    if not es_proceso_mithflow(proc):
        st.error(f"El PID {pid} ya no es MithFlow. No se detuvo nada.")
        _borrar_status()
        return False
    try:
        proc.kill()
        proc.wait(timeout=5)
    except (psutil.NoSuchProcess, psutil.TimeoutExpired):
        pass
    except psutil.AccessDenied:
        st.error(f"Sin permisos para detener el PID {pid}.")
        return False  # no borrar el estado: el motor sigue vivo
    _borrar_status()
    return True


def _borrar_status():
    try:
        os.remove(STATUS_FILE)
    except OSError:
        pass


def load_history():
    rows = []
    if os.path.exists(HISTORY_FILE):
        with open(HISTORY_FILE, encoding="utf-8-sig") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    rows.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
    if not rows:
        return pd.DataFrame()
    df = pd.DataFrame(rows)
    df["ts"] = pd.to_datetime(df["ts"], errors="coerce")
    df = df.dropna(subset=["ts"])
    # Las entradas anteriores al 21/7/2026 no tienen el campo `mode` (se agregó
    # cuando aparecieron los modos de limpieza). Sin este relleno, esas filas
    # quedan como NaN y contaminan filtros y tooltips.
    if "mode" not in df.columns:
        df["mode"] = "llm"
    else:
        df["mode"] = df["mode"].fillna("llm")
    for columna, default in (("raw", ""), ("final", ""), ("cleaned", False),
                             ("words", 0), ("audio_s", 0.0),
                             ("transcribe_s", 0.0), ("cleanup_s", 0.0)):
        if columna not in df.columns:
            df[columna] = default
        else:
            df[columna] = df[columna].fillna(default)
    return df


# ----------------------------------------------------------------------
# Header + estado del motor
# ----------------------------------------------------------------------
col_title, col_status = st.columns([3, 1])
with col_title:
    st.title("🎙️ MithFlow")
    st.markdown('<p class="mf-subtitle">Dictado por voz 100% local — F8 para grabar, F8 para pegar. Tu voz nunca sale de tu máquina.</p>', unsafe_allow_html=True)

with col_status:
    @st.fragment(run_every="5s")
    def status_panel():
        alive, pid = engine_status()
        if alive:
            st.success("● Motor activo — F8 listo")
            if st.button("⏹ Detener motor", width='stretch'):
                if stop_engine(pid):
                    st.rerun()
        else:
            st.error("○ Motor detenido")
            if st.button("▶ Iniciar motor", width='stretch', type="primary"):
                start_engine()
                st.toast("Iniciando... el modelo tarda ~30s en cargar.")

    status_panel()

# Todo el cuerpo va dentro de un fragmento con `run_every`: sin esto los datos
# se leen una sola vez al abrir la página y el dashboard queda congelado
# (un fragmento solo re-ejecuta SU código, no el del módulo).
@st.fragment(run_every="3s")
def dashboard_body():
    df = load_history()
    if df.empty:
        st.info("Todavía no hay dictados registrados. Presioná F8, hablá, F8 de nuevo — aparecen acá solos.")
        return

    # ------------------------------------------------------------------
    # Métricas principales (con delta de hoy)
    # ------------------------------------------------------------------
    today = pd.Timestamp(datetime.now().date())
    df_today = df[df["ts"] >= today]

    total_words = int(df["words"].sum())
    total_audio_min = df["audio_s"].sum() / 60
    spoken_wpm = total_words / total_audio_min if total_audio_min > 0 else 0
    saved_min = max(0, total_words / TYPING_WPM - total_audio_min)

    # La latencia se calcula SOLO sobre el modo de limpieza actual. Mezclar la
    # época del LLM (2.4-5.7s de limpieza) con la actual (<1ms) daba un promedio
    # 3.5x peor que la realidad: 1.76s mostrados contra 0.51s reales.
    modo_actual = df.sort_values("ts").iloc[-1]["mode"]
    df_modo = df[df["mode"] == modo_actual]
    avg_latency = (df_modo["transcribe_s"] + df_modo["cleanup_s"]).mean()
    latency_help = (
        f"Transcripción + limpieza, promediado sobre los {len(df_modo)} dictados "
        f"en modo «{modo_actual}» (el actual). Los de modos anteriores se excluyen "
        f"para no falsear el número."
    )

    m1, m2, m3, m4, m5 = st.columns(5)
    m1.metric("Dictados", f"{len(df):,}", delta=f"+{len(df_today)} hoy" if len(df_today) else None)
    m2.metric("Palabras", f"{total_words:,}", delta=f"+{int(df_today['words'].sum()):,} hoy" if len(df_today) else None)
    m3.metric("Velocidad al hablar", f"{spoken_wpm:.0f} ppm", help="Palabras por minuto hablando (tipear ronda las 40)")
    m4.metric("Tiempo ahorrado", f"{saved_min:.0f} min", help=f"Vs. tipear a {TYPING_WPM} palabras/min")
    m5.metric("Latencia promedio", f"{avg_latency:.2f}s", help=latency_help)

    st.write("")

    # ------------------------------------------------------------------
    # Gráficos
    # ------------------------------------------------------------------
    g1, g2 = st.columns(2)
    with g1:
        st.subheader("Palabras por día")
        daily = df.set_index("ts").resample("D")["words"].sum().reset_index()
        chart = alt.Chart(daily).mark_bar(
            cornerRadiusTopLeft=6, cornerRadiusTopRight=6, color=ACCENT, size=28,
        ).encode(
            x=alt.X("ts:T", title=None, axis=alt.Axis(format="%d/%m", labelColor="#8B96A8", grid=False)),
            y=alt.Y("words:Q", title=None, axis=alt.Axis(labelColor="#8B96A8", gridColor="#1E2634")),
            tooltip=[alt.Tooltip("ts:T", title="Día", format="%d/%m"), alt.Tooltip("words:Q", title="Palabras")],
        ).properties(height=280).configure_view(strokeWidth=0)
        st.altair_chart(chart, width='stretch')

    with g2:
        st.subheader("Actividad por hora")
        hourly = df.groupby(df["ts"].dt.hour)["words"].sum().reindex(range(24), fill_value=0).reset_index()
        hourly.columns = ["hora", "palabras"]
        chart = alt.Chart(hourly).mark_bar(
            cornerRadiusTopLeft=4, cornerRadiusTopRight=4, color=ACCENT_SOFT,
        ).encode(
            x=alt.X("hora:O", title=None, axis=alt.Axis(labelColor="#8B96A8", labelAngle=0)),
            y=alt.Y("palabras:Q", title=None, axis=alt.Axis(labelColor="#8B96A8", gridColor="#1E2634")),
            tooltip=[alt.Tooltip("hora:O", title="Hora"), alt.Tooltip("palabras:Q", title="Palabras")],
        ).properties(height=280).configure_view(strokeWidth=0)
        st.altair_chart(chart, width='stretch')

    st.write("")

    # ------------------------------------------------------------------
    # Historial reciente
    # ------------------------------------------------------------------
    st.subheader("Historial de dictados")
    # key= es necesario: sin él, cada refresco del fragmento borraría la búsqueda.
    search = st.text_input("Buscar en el historial", placeholder="Ej: cliente, dashboard, reunión...",
                           label_visibility="collapsed", key="buscar")
    view = df.sort_values("ts", ascending=False)
    if search:
        # regex=False: por defecto pandas interpreta el patrón como expresión
        # regular, así que escribir "(" o "*" en el buscador rompía la página.
        view = view[view["final"].str.contains(search, case=False, na=False, regex=False)]

    view_display = view[["ts", "final", "words", "audio_s", "transcribe_s", "cleaned"]].rename(columns={
        "ts": "Fecha", "final": "Texto", "words": "Palabras",
        "audio_s": "Audio (s)", "transcribe_s": "Transcripción (s)", "cleaned": "Limpiado por IA",
    })
    st.dataframe(
        view_display, width='stretch', hide_index=True, height=380,
        column_config={
            "Fecha": st.column_config.DatetimeColumn(format="DD/MM HH:mm"),
            "Texto": st.column_config.TextColumn(width="large"),
            "Limpiado por IA": st.column_config.CheckboxColumn(),
        },
    )
    st.caption(f"Actualizado {datetime.now():%H:%M:%S} · se refresca solo cada 3 segundos")

    with st.expander("🔍 Comparación crudo vs. limpio (últimos 20 con limpieza)"):
        for _, row in view[view["cleaned"].astype(bool)].head(20).iterrows():
            st.markdown(f"**{row['ts']:%d/%m %H:%M}**")
            st.markdown(f"- Crudo: _{row['raw']}_")
            st.markdown(f"- Limpio: **{row['final']}**")
            st.divider()


dashboard_body()
