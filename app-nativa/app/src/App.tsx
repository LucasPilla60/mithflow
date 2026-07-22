/**
 * El cascarón: barra de estado, navegación y avisos.
 *
 * Lo que vive acá y no en las vistas es lo que no depende de cuál esté abierta:
 * el estado del motor —que tiene que verse igual mirando el dashboard o los
 * ajustes— y los avisos que el backend emite en cualquier momento.
 *
 * # Cuándo aparece el asistente
 *
 * Cuando el catálogo dice que **ningún** modelo está descargado, que es el único
 * estado en el que la app no puede dictar. No es una bandera guardada: si el
 * usuario borra los modelos, el asistente vuelve solo.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import {
  alAviso,
  alCambiarEstado,
  cancelarTodas,
  leerCatalogo,
  leerEstado,
  mensajeDeError,
  pausar,
  reanudar,
  type Aviso,
  type Catalogo,
  type EstadoDto,
} from "./api";
import Dashboard from "./vistas/Dashboard";
import VistaAjustes from "./vistas/Ajustes";
import Asistente from "./vistas/Asistente";
import "./estilos.css";

type Vista = "dashboard" | "ajustes";

/** Cuánto queda en pantalla un aviso antes de irse solo. */
const VIDA_DEL_AVISO_MS = 6000;

/** Cuántos avisos se apilan como mucho. */
const MAX_AVISOS = 4;

interface AvisoEnPantalla extends Aviso {
  id: number;
}

export default function App() {
  const [estado, setEstado] = useState<EstadoDto | null>(null);
  const [catalogo, setCatalogo] = useState<Catalogo | null>(null);
  const [vista, setVista] = useState<Vista>("dashboard");
  const [asistenteAbierto, setAsistenteAbierto] = useState(false);
  const [asistenteCerrado, setAsistenteCerrado] = useState(false);
  const [avisos, setAvisos] = useState<AvisoEnPantalla[]>([]);
  const [fallo, setFallo] = useState<string | null>(null);
  const montado = useRef(true);
  const proximoId = useRef(1);

  const avisar = useCallback((texto: string, nivel: "info" | "error") => {
    const id = proximoId.current++;
    setAvisos((previos) => [{ id, texto, nivel }, ...previos].slice(0, MAX_AVISOS));
    setTimeout(
      () => setAvisos((previos) => previos.filter((a) => a.id !== id)),
      VIDA_DEL_AVISO_MS,
    );
  }, []);

  useEffect(() => {
    montado.current = true;

    // 1. El presente, de una: los eventos sólo cuentan lo que pasa de acá en
    //    adelante, así que sin esto abrir la ventana mostraría una app "vacía".
    Promise.all([leerEstado(), leerCatalogo()])
      .then(([e, c]) => {
        if (!montado.current) return;
        setEstado(e);
        setCatalogo(c);
      })
      .catch((e) => montado.current && setFallo(mensajeDeError(e)));

    // 2. Y las novedades.
    const suscripciones = [
      alCambiarEstado((e) => montado.current && setEstado(e)),
      alAviso((a) => avisar(a.texto, a.nivel)),
    ];

    return () => {
      montado.current = false;
      cancelarTodas(suscripciones);
    };
  }, [avisar]);

  const alternarPausa = () => {
    if (!estado) return;
    const accion = estado.pausado ? reanudar : pausar;
    accion().catch((e) => avisar(mensajeDeError(e), "error"));
  };

  const cambiarCatalogo = useCallback((c: Catalogo) => setCatalogo(c), []);

  // El asistente se ABRE cuando no hay ningún modelo, pero después se queda
  // hasta que el usuario lo cierre. Sin esta traba se cerraría solo en la mitad:
  // el asistente baja el modelo de medición, el catálogo deja de estar vacío y
  // la condición se apagaría justo antes de mostrar la recomendación.
  useEffect(() => {
    if (catalogo && catalogo.modelos.every((m) => !m.descargado)) {
      setAsistenteAbierto(true);
    }
  }, [catalogo]);

  const mostrarAsistente = asistenteAbierto && !asistenteCerrado;
  const clave = estado?.estado ?? "cargando";

  // Con el asistente en pantalla, "Falta el modelo" arriba repite lo que la
  // pantalla entera ya está diciendo ("Bajando el modelo…", con su barra de
  // progreso), y pausar un dictado que todavía no puede existir no significa
  // nada. Los dos se guardan SÓLO para ese estado: un error de verdad se sigue
  // mostrando, porque esconderlo sería mentir en el otro sentido.
  const ocultarPastilla = mostrarAsistente && clave === "sin-modelo";

  return (
    <div className="app" data-estado={clave}>
      <header className="encabezado">
        <div className="marca">
          <h1>MithFlow</h1>
          {catalogo && <span className="version">v{catalogo.version}</span>}
        </div>

        {!ocultarPastilla && (
          <div
            className={estado?.pausado ? "estado pausado" : "estado"}
            data-estado={clave}
            role="status"
            aria-live="polite"
          >
            <span className="punto" />
            <span className="etiqueta">{estado ? estado.etiqueta : "Conectando…"}</span>
            {estado?.pausado && <span className="pausa">en pausa</span>}
          </div>
        )}

        {estado && !ocultarPastilla && (
          <button type="button" className="boton chico" onClick={alternarPausa}>
            {estado.pausado ? "Reanudar" : "Pausar"}
          </button>
        )}

        <span className="espacio" />

        {!mostrarAsistente && (
          <nav className="pestanas">
            <button
              type="button"
              aria-current={vista === "dashboard" ? "page" : undefined}
              onClick={() => setVista("dashboard")}
            >
              Dashboard
            </button>
            <button
              type="button"
              aria-current={vista === "ajustes" ? "page" : undefined}
              onClick={() => setVista("ajustes")}
            >
              Ajustes
            </button>
          </nav>
        )}
      </header>

      <main className="contenido">
        {fallo && <p className="error-vista">No pude hablar con el backend: {fallo}</p>}

        {/* Con el asistente abierto el detalle sobra: lo único que puede decir
            ahí es "todavía no hay ningún modelo descargado", y el asistente ES
            la respuesta a eso. Repetirlo arriba hace ver rota una pantalla de
            bienvenida. */}
        {estado?.detalle && !mostrarAsistente && (
          <p className="detalle-estado">{estado.detalle}</p>
        )}

        {mostrarAsistente && catalogo ? (
          <Asistente
            catalogo={catalogo}
            alCambiarCatalogo={cambiarCatalogo}
            alTerminar={(destino) => {
              setAsistenteCerrado(true);
              setVista(destino);
            }}
          />
        ) : vista === "dashboard" ? (
          <Dashboard />
        ) : (
          <VistaAjustes alCambiarCatalogo={cambiarCatalogo} avisar={avisar} />
        )}
      </main>

      {avisos.length > 0 && (
        <ul className="avisos">
          {avisos.map((a) => (
            <li key={a.id} className={a.nivel}>
              <span>{a.texto}</span>
              <button
                type="button"
                className="boton plano chico"
                aria-label="Descartar"
                onClick={() => setAvisos((previos) => previos.filter((o) => o.id !== a.id))}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
