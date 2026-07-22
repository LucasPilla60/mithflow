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
 * # Se agarra con el mouse
 *
 * Toda la tarjeta es el asa: apretar y mover la corre, y ahí se queda entre
 * dictados y entre reinicios. Antes los clics la atravesaban
 * (`WS_EX_TRANSPARENT`) y era imposible agarrarla; se sacó ese estilo después de
 * medir en `spike-superpuesta/` que **no** es el que sostiene la garantía del
 * foco —ésa la da `WS_EX_NOACTIVATE`, que sigue puesta—, así que clickearla no
 * mueve el cursor de texto del usuario.
 *
 * El arrastre no arranca en el `mousedown` sino cuando el mouse **se movió** con
 * el botón apretado: ver [`agarrar`].
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
import { invoke } from "@tauri-apps/api/core";
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

/**
 * Cuántas barras tiene el medidor.
 *
 * Eran 30 cuando la ventanita medía 232 px de ancho. Con 168 px, 30 barras
 * quedarían de 3 px y el medidor se leería como una textura en vez de como una
 * onda: veinte barras de ~5,5 px dicen lo mismo y se ven. A 25 cuadros por
 * segundo son 0,8 s de historia, suficiente para ver la cadencia del habla.
 */
const BARRAS = 20;

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

/**
 * Cuántos píxeles hay que mover el mouse, con el botón apretado, para que esto
 * cuente como un arrastre y no como un clic.
 */
const UMBRAL_DE_ARRASTRE = 3;

/**
 * Agarrar la ventanita para moverla.
 *
 * # Por qué no arranca en el `mousedown`
 *
 * El arrastre lo hace el bucle modal de movimiento de Windows, que **termina
 * con el `WM_LBUTTONUP`**. Si se lo lanzara en el `mousedown`, un clic corto
 * —el caso más probable de todos, porque desde que los clics no la atraviesan
 * el usuario va a intentar apretar cosas que la ventanita tapa— podría terminar
 * *antes* de que el bucle arranque: entonces el `up` ya pasó, nunca llega otro,
 * y la ventanita queda pegada al mouse paseando por la pantalla hasta el
 * próximo clic. Con el umbral, un clic sin movimiento no arrastra nada y cuando
 * el arrastre arranca el botón está garantizadamente apretado.
 *
 * De paso, un clic que no mueve nada tampoco le hace creer al backend que el
 * usuario eligió una posición.
 *
 * # Un solo viaje de ida
 *
 * `arrastrar_indicador` hace todo del lado de Rust: anota que el arrastre es
 * del usuario, corre el bucle modal y guarda dónde quedó. No devuelve hasta que
 * se suelta el botón, así que la promesa se resuelve recién ahí.
 *
 * Si algo falla no hay nada que hacer salvo dejarla quieta: es el mismo estado
 * que tenía la versión anterior de la app y el dictado no depende de esto. Se
 * anota en la consola porque un arrastre que no anda es un defecto silencioso.
 * En la vista de desarrollo (un navegador común, sin IPC) falla siempre y por
 * eso no se le muestra nada al usuario.
 */
function agarrar(evento: React.MouseEvent<HTMLElement>): void {
  if (evento.button !== 0) return;
  const desde = { x: evento.screenX, y: evento.screenY };

  const soltar = () => {
    window.removeEventListener("mousemove", alMover);
    window.removeEventListener("mouseup", soltar);
  };

  const alMover = (movimiento: MouseEvent) => {
    const corrido =
      Math.abs(movimiento.screenX - desde.x) >= UMBRAL_DE_ARRASTRE ||
      Math.abs(movimiento.screenY - desde.y) >= UMBRAL_DE_ARRASTRE;
    if (!corrido) return;
    // Antes de arrastrar, porque el bucle modal se come el `mouseup` y este
    // handler no volvería a correr nunca.
    soltar();
    invoke("arrastrar_indicador").catch((error) => {
      console.warn("no pude arrastrar la ventanita:", error);
    });
  };

  window.addEventListener("mousemove", alMover);
  window.addEventListener("mouseup", soltar);
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
    <div
      className="tarjeta"
      data-modo={modo}
      role="status"
      aria-live="off"
      title="Arrastrala para moverla"
      onMouseDown={agarrar}
    >
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
