/**
 * La ventanita de grabación con un micrófono de mentira.
 *
 * # Qué es y qué NO es
 *
 * Es la herramienta para mirar y capturar el indicador sin arrancar la app —que
 * el usuario tiene abierta y usando— ni hablarle a un micrófono. **No prueba
 * nada del backend**: las reglas que decide Rust (`superpuesta::intensidad`,
 * `hay_voz`, `cerca_del_tope`) están replicadas acá para poder alimentar el
 * componente, y si alguna vez discrepan, **el que manda es Rust**. Lo que sí se
 * ve fiel es la ventanita: es exactamente el mismo componente que se empaqueta.
 *
 * Escenarios: `?escenario=hablando | silencio | transcribiendo | cerca-del-tope`.
 */
import ReactDOM from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import Superpuesta from "../superpuesta/Superpuesta";

type Escenario = "hablando" | "silencio" | "transcribiendo" | "cerca-del-tope";

const ESCENARIOS: Escenario[] = ["hablando", "silencio", "transcribiendo", "cerca-del-tope"];

/** El mismo umbral que `config::MIN_SPEECH_RMS`. */
const MIN_SPEECH_RMS = 0.01;

/** Los mismos extremos de escala que `superpuesta::PISO_DB` y `TECHO_DB`. */
const PISO_DB = -60;
const TECHO_DB = -12;

/** Copia de `superpuesta::intensidad`. Ver la nota de arriba: manda Rust. */
function intensidad(rms: number): number {
  if (!Number.isFinite(rms) || rms <= 0) return 0;
  const db = 20 * Math.log10(rms);
  return Math.min(1, Math.max(0, (db - PISO_DB) / (TECHO_DB - PISO_DB)));
}

/** Copia de `superpuesta::cerca_del_tope`. */
function cercaDelTope(pasados: number, limite: number): boolean {
  if (limite <= 0) return false;
  return limite - pasados <= Math.min(30, Math.max(5, limite * 0.15));
}

/**
 * Un RMS verosímil para cada escenario, con la cadencia del habla real: sílabas
 * de unos 200 ms y pausas entre frases. Un ruido blanco constante llenaría el
 * medidor y no mostraría lo único que importa —que el medidor SIGUE al habla—.
 *
 * Determinista sobre el reloj: dos capturas del mismo escenario dan la misma
 * forma de onda, o comparar antes y después no diría nada.
 */
function rmsDe(escenario: Escenario, cuadro: number): number {
  if (escenario === "silencio") {
    // Ruido de fondo medido sobre el fixture: 0,0029. Se mueve un poco, porque
    // una habitación en silencio tampoco es una línea recta.
    return 0.0029 * (0.7 + 0.6 * Math.abs(Math.sin(cuadro / 7)));
  }
  const t = cuadro / 25; // segundos
  // Frases de 2,4 s con 0,7 s de pausa entre medio.
  const enLaPausa = t % 3.1 > 2.4;
  if (enLaPausa) return 0.0029;
  const silaba = 0.5 + 0.5 * Math.sin(cuadro / 2.1);
  const enfasis = 0.75 + 0.35 * Math.sin(cuadro / 11);
  return Math.max(MIN_SPEECH_RMS * 1.2, 0.085 * silaba * enfasis);
}

const pedido = new URLSearchParams(window.location.search).get("escenario");
const escenario: Escenario = ESCENARIOS.includes(pedido as Escenario)
  ? (pedido as Escenario)
  : "hablando";

// La ventanita sólo escucha eventos; no invoca ningún comando. El `mockIPC` está
// igual porque es lo que engancha `listen`/`emit` sin backend.
mockIPC(() => null, { shouldMockEvents: true });

const LIMITE = 180;
const ARRANQUE = escenario === "cerca-del-tope" ? 158 : 0;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<Superpuesta />);

if (escenario === "transcribiendo") {
  // Un dictado de 14 s que acaba de terminar: la ventanita se queda con el
  // reloj clavado y el rótulo en ámbar.
  //
  // Los 300 ms no son estética: `listen` devuelve una promesa y la suscripción
  // se arma después del primer render, así que un `emit` inmediato no lo
  // escucharía nadie. En la app real el problema no existe —la ventanita se crea
  // al arrancar y está suscripta mucho antes del primer dictado—, pero acá el
  // escenario entero son dos eventos.
  setTimeout(() => {
    void emit("nivel-audio", {
      rms: 0.06,
      pico: 0.2,
      intensidad: intensidad(0.06),
      hay_voz: true,
      segundos: 14.2,
      limite_s: LIMITE,
      cerca_del_tope: false,
    });
    setTimeout(() => void emit("estado-cambiado", { estado: "transcribiendo" }), 60);
  }, 300);
} else {
  let cuadro = 0;
  // 40 ms, los mismos que `director::LATIDO_CON_MEDIDOR`.
  setInterval(() => {
    const rms = rmsDe(escenario, cuadro);
    const segundos = ARRANQUE + cuadro / 25;
    void emit("nivel-audio", {
      rms,
      pico: rms * 2.8,
      intensidad: intensidad(rms),
      hay_voz: rms >= MIN_SPEECH_RMS,
      segundos,
      limite_s: LIMITE,
      cerca_del_tope: cercaDelTope(segundos, LIMITE),
    });
    cuadro += 1;
  }, 40);
}
