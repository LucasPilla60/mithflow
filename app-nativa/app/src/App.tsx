/**
 * Interfaz PROVISORIA. La de verdad es la tarea siguiente.
 *
 * Lo único que tiene que demostrar es que las dos mitades del contrato
 * funcionan: el estado y el historial se piden al montarse (un dictado hecho
 * con la ventana cerrada tiene que aparecer igual), y a partir de ahí llegan
 * por evento. Sin la primera mitad, abrir la ventana mostraría una lista vacía.
 */
import { useEffect, useRef, useState } from "react";
import {
  alAviso,
  alCambiarEstado,
  alDictadoNuevo,
  leerEstado,
  leerHistorial,
  pausar,
  reanudar,
  type Aviso,
  type Dictado,
  type EstadoDto,
} from "./api";
import "./App.css";

const ULTIMOS = 20;

const COLOR: Record<string, string> = {
  cargando: "#9aa0a6",
  listo: "#34a853",
  grabando: "#ea4335",
  transcribiendo: "#fbbc04",
  error: "#b31412",
};

export default function App() {
  const [estado, setEstado] = useState<EstadoDto | null>(null);
  const [dictados, setDictados] = useState<Dictado[]>([]);
  const [avisos, setAvisos] = useState<Aviso[]>([]);
  const [fallo, setFallo] = useState<string | null>(null);
  // Un `ref` y no un estado: sólo sirve para descartar una respuesta que llega
  // después de desmontar, y no tiene que redibujar nada.
  const montado = useRef(true);

  useEffect(() => {
    montado.current = true;

    // 1. El presente, de una: los eventos sólo cuentan lo que pasa de acá en
    //    adelante.
    Promise.all([leerEstado(), leerHistorial(ULTIMOS)])
      .then(([e, h]) => {
        if (!montado.current) return;
        setEstado(e);
        setDictados(h);
      })
      .catch((e) => setFallo(String(e)));

    // 2. Y las novedades.
    const suscripciones = [
      alCambiarEstado((e) => setEstado(e)),
      alDictadoNuevo((d) =>
        setDictados((previos) => [d, ...previos].slice(0, ULTIMOS)),
      ),
      alAviso((a) => setAvisos((previos) => [a, ...previos].slice(0, 5))),
    ];

    return () => {
      montado.current = false;
      // `listen` devuelve una promesa: hay que esperarla para poder cancelar.
      suscripciones.forEach((p) =>
        p.then((cancelar) => cancelar()).catch(() => {}),
      );
    };
  }, []);

  const alternar = () => {
    if (!estado) return;
    const accion = estado.pausado ? reanudar : pausar;
    accion().catch((e) => setFallo(String(e)));
  };

  return (
    <main>
      <h1>MithFlow</h1>
      <p className="detalle">
        Interfaz provisoria: sirve para verificar comandos y eventos.
      </p>

      <section className="estado">
        <span
          className="punto"
          style={{ background: COLOR[estado?.estado ?? "cargando"] }}
        />
        <strong>{estado ? estado.etiqueta : "…"}</strong>
        {estado?.pausado && <em> · pausado</em>}
      </section>
      {estado?.detalle && <p className="detalle">{estado.detalle}</p>}

      <button onClick={alternar} disabled={!estado}>
        {estado?.pausado ? "Reanudar dictado" : "Pausar dictado"}
      </button>

      {fallo && <p className="detalle">No pude hablar con el backend: {fallo}</p>}

      {avisos.length > 0 && (
        <ul className="avisos">
          {avisos.map((a, i) => (
            <li key={i} className={a.nivel}>
              {a.texto}
            </li>
          ))}
        </ul>
      )}

      <h2>Últimos dictados</h2>
      {dictados.length === 0 ? (
        <p className="detalle">Todavía no hay ninguno.</p>
      ) : (
        <ol className="dictados">
          {dictados.map((d, i) => (
            <li key={`${d.ts}-${i}`}>
              <time>{d.ts.replace("T", " ")}</time>
              <p>{d.final}</p>
              <small>
                {d.words} palabras · {d.audio_s.toFixed(1)} s de audio ·{" "}
                {d.transcribe_s.toFixed(2)} s de transcripción
              </small>
            </li>
          ))}
        </ol>
      )}
    </main>
  );
}
