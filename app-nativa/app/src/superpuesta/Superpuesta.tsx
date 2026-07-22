/**
 * La ventanita de grabación.
 *
 * # Qué tiene que contestar, y en qué orden
 *
 * 1. **¿Está grabando?** — la palabra, arriba a la izquierda, con su punto de
 *    color. Los cuatro tonos ya decían "arrancó"; lo que faltaba era poder
 *    mirarlo.
 * 2. **¿Me está escuchando?** — el medidor. Barras teal cuando lo que entra
 *    llega a voz, grises cuando es ruido de fondo. El umbral que las separa es
 *    el mismo con el que el motor decide si vale la pena transcribir, así que lo
 *    que se ve acá es lo que la app va a hacer con ese audio.
 * 3. **¿Cuánto llevo?** — el reloj, que pasa a ámbar cuando el tope está por
 *    cortar la grabación sola.
 *
 * Y cuando el usuario suelta la tecla, la ventanita **no desaparece**: cambia a
 * "Transcribiendo…" en ámbar. Ese segundo o dos, en blanco, se ve igual que un
 * dictado que se perdió.
 *
 * # Por qué el medidor no pasa por React
 *
 * Llegan veinticinco cuadros por segundo y hay treinta barras: re-renderizar la
 * lista sería setecientos elementos por segundo de trabajo de reconciliación
 * para mover unos píxeles. Las barras se montan una vez y después se les toca el
 * `transform` directamente. React sólo se entera de lo que cambia de a poco: el
 * modo, si hay voz y el reloj, que cambia una vez por segundo.
 */
import { useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./superpuesta.css";

/** Espejo de `superpuesta::NivelAudio`. */
interface NivelAudio {
  rms: number;
  pico: number;
  /** Altura relativa de la barra, de 0 a 1. Ya viene calculada del backend. */
  intensidad: number;
  hay_voz: boolean;
  segundos: number;
  limite_s: number;
  cerca_del_tope: boolean;
}

/** Espejo de `estado::EstadoDto`, con lo único que esta ventana mira. */
interface EstadoDto {
  estado: string;
}

/** Cuántas barras tiene el medidor. */
const BARRAS = 30;

/**
 * Altura mínima de una barra, como fracción de su alto. No es decoración: con
 * `scaleY(0)` las barras desaparecen y el medidor se ve apagado, que es lo que
 * no puede pasar mientras el micrófono está abierto.
 */
const PISO = 0.06;

/**
 * El modo arranca en "grabando" y no en vacío: esta ventana **sólo se muestra**
 * grabando o transcribiendo (ver `superpuesta::visible_en`), así que si todavía
 * no llegó ningún `estado-cambiado` —el webview puede haber montado un instante
 * tarde— lo que corresponde mostrar es lo primero de los dos.
 */
type Modo = "grabando" | "transcribiendo";

/** `7` → `"0:07"`. Sin horas: el tope máximo son diez minutos. */
function reloj(segundos: number): string {
  const total = Number.isFinite(segundos) ? Math.max(0, Math.floor(segundos)) : 0;
  const minutos = Math.floor(total / 60);
  return `${minutos}:${String(total % 60).padStart(2, "0")}`;
}

export default function Superpuesta() {
  const [modo, setModo] = useState<Modo>("grabando");
  const [hayVoz, setHayVoz] = useState(false);
  const [cercaDelTope, setCercaDelTope] = useState(false);
  // Segundos enteros y no el `f32` crudo: guardar el decimal re-renderizaría
  // veinticinco veces por segundo para mostrar el mismo "0:07".
  const [segundos, setSegundos] = useState(0);

  const barras = useRef<(HTMLSpanElement | null)[]>([]);
  const historia = useRef<number[]>(new Array<number>(BARRAS).fill(0));

  useEffect(() => {
    let vivo = true;
    const suscripciones: Promise<UnlistenFn>[] = [
      listen<EstadoDto>("estado-cambiado", (ev) => {
        if (!vivo) return;
        const estado = ev.payload.estado;
        // Cualquier otro estado esconde la ventana (ver `superpuesta::visible_en`)
        // y la deja **armada en "grabando"**, que es con lo que va a volver a
        // aparecer. Sin ese rearme, el dictado siguiente arrancaría mostrando
        // "Transcribiendo…" —lo último que quedó— hasta que llegara el primer
        // nivel cuarenta milisegundos después.
        setModo(estado === "transcribiendo" ? "transcribiendo" : "grabando");
      }),
      listen<NivelAudio>("nivel-audio", (ev) => {
        if (!vivo) return;
        const nivel = ev.payload;
        // Un nivel sólo llega mientras se graba: alcanza para corregir el modo
        // si el `estado-cambiado` se perdió por haber montado tarde.
        setModo("grabando");
        setHayVoz(nivel.hay_voz);
        setCercaDelTope(nivel.cerca_del_tope);
        setSegundos(Math.floor(nivel.segundos));

        // La historia se corre una posición: la barra de la derecha es lo que
        // acaba de entrar, y el resto es lo que se dijo hace un segundo.
        const valores = historia.current;
        valores.shift();
        valores.push(Math.min(1, Math.max(0, nivel.intensidad)));
        for (let i = 0; i < valores.length; i++) {
          const barra = barras.current[i];
          if (barra) barra.style.transform = `scaleY(${PISO + valores[i] * (1 - PISO)})`;
        }
      }),
    ];

    return () => {
      vivo = false;
      suscripciones.forEach((p) => p.then((cancelar) => cancelar()).catch(() => {}));
    };
  }, []);

  // Al volver a grabar, el medidor tiene que arrancar en cero: si no, el primer
  // cuadro mostraría la forma de onda del dictado anterior.
  useEffect(() => {
    if (modo !== "grabando") return;
    historia.current.fill(0);
    barras.current.forEach((barra) => {
      if (barra) barra.style.transform = `scaleY(${PISO})`;
    });
  }, [modo]);

  const transcribiendo = modo === "transcribiendo";

  return (
    <div className="tarjeta" data-modo={modo} role="status" aria-live="off">
      <div className="fila">
        <span className="punto" />
        <span className="rotulo">{transcribiendo ? "Transcribiendo…" : "Grabando"}</span>
        <span className="espacio" />
        <span className={cercaDelTope && !transcribiendo ? "reloj cerca" : "reloj"}>
          {reloj(segundos)}
        </span>
      </div>

      <div className={hayVoz && !transcribiendo ? "medidor con-voz" : "medidor"}>
        {Array.from({ length: BARRAS }, (_, i) => (
          <span
            key={i}
            className="barra"
            ref={(nodo) => {
              barras.current[i] = nodo;
            }}
            style={{ transform: `scaleY(${PISO})` }}
          />
        ))}
      </div>
    </div>
  );
}
