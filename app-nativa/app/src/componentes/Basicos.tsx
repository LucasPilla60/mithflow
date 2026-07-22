/**
 * Las piezas chicas que usan las tres vistas.
 *
 * Están juntas a propósito: son componentes de presentación de diez líneas sin
 * estado propio, y un archivo por cada uno sería más navegación que código.
 */
import type { ReactNode } from "react";

/* --------------------------------------------------------------- métrica */

interface MetricaProps {
  titulo: string;
  valor: string;
  unidad?: string;
  /** El "+N hoy". Se omite cuando no hubo actividad hoy. */
  delta?: string | null;
  /** Letra chica que explica de dónde sale el número. */
  nota?: string | null;
  /** Texto largo para el `title` nativo; el `nota` es la versión visible. */
  ayuda?: string;
}

export function Metrica({ titulo, valor, unidad, delta, nota, ayuda }: MetricaProps) {
  return (
    <div className="metrica" title={ayuda}>
      <div className="titulo">{titulo}</div>
      <div className="valor">
        {valor}
        {unidad && <span className="unidad">{unidad}</span>}
      </div>
      {delta && <div className="delta">{delta}</div>}
      {nota && <div className="nota">{nota}</div>}
    </div>
  );
}

/* ------------------------------------------------------------ interruptor */

interface InterruptorProps {
  activo: boolean;
  alCambiar: (activo: boolean) => void;
  etiqueta: string;
  deshabilitado?: boolean;
}

export function Interruptor({
  activo,
  alCambiar,
  etiqueta,
  deshabilitado,
}: InterruptorProps) {
  return (
    <label className="interruptor">
      <input
        type="checkbox"
        checked={activo}
        disabled={deshabilitado}
        onChange={(e) => alCambiar(e.currentTarget.checked)}
      />
      <span className="riel" />
      <span>{etiqueta}</span>
    </label>
  );
}

/* ------------------------------------------------------------- segmentado */

interface SegmentadoProps<T extends string> {
  opciones: { valor: T; etiqueta: string }[];
  elegida: T;
  alElegir: (valor: T) => void;
}

export function Segmentado<T extends string>({
  opciones,
  elegida,
  alElegir,
}: SegmentadoProps<T>) {
  return (
    <div className="segmentado" role="group">
      {opciones.map((o) => (
        <button
          key={o.valor}
          type="button"
          aria-pressed={o.valor === elegida}
          onClick={() => alElegir(o.valor)}
        >
          {o.etiqueta}
        </button>
      ))}
    </div>
  );
}

/* --------------------------------------------------------------- progreso */

interface ProgresoProps {
  porcentaje: number;
  izquierda?: ReactNode;
  derecha?: ReactNode;
}

export function Progreso({ porcentaje, izquierda, derecha }: ProgresoProps) {
  // El porcentaje viene del backend, pero acotarlo acá evita que un total en
  // cero (o un dato viejo) dibuje una barra que se sale de su caja.
  const ancho = Math.min(100, Math.max(0, Number.isFinite(porcentaje) ? porcentaje : 0));
  return (
    <div>
      <div
        className="progreso"
        role="progressbar"
        aria-valuenow={Math.round(ancho)}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <div className="relleno" style={{ width: `${ancho}%` }} />
      </div>
      {(izquierda || derecha) && (
        <div className="progreso-info">
          <span>{izquierda}</span>
          <span>{derecha}</span>
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ campo */

interface CampoProps {
  rotulo: string;
  ayuda?: string;
  children: ReactNode;
}

/** Una fila de Ajustes: rótulo a la izquierda, control a la derecha. */
export function Campo({ rotulo, ayuda, children }: CampoProps) {
  return (
    <div className="campo">
      <div className="rotulo">
        {rotulo}
        {ayuda && <span className="ayuda">{ayuda}</span>}
      </div>
      <div className="control">{children}</div>
    </div>
  );
}
