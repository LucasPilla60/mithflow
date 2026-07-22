/**
 * El gráfico de barras del dashboard, hecho con `div`s y no con una librería.
 *
 * Son dos series de 30 y 24 puntos que ya llegan calculadas desde Rust: traer
 * una librería de gráficos para eso serían cientos de kilobytes de JavaScript
 * en una app que se abre para mirar un número. Con cajas flexibles las barras se
 * reparten solas al cambiar el ancho de la ventana y el texto del eje se dibuja
 * al tamaño real, sin escalar.
 */

export interface PuntoDelGrafico {
  /** Clave estable para React. */
  clave: string;
  valor: number;
  /** Lo que se lee al pasar el mouse: "21/07 · 320 palabras". */
  descripcion: string;
  /** Lo que va debajo de la barra, si le toca etiqueta. */
  etiqueta: string;
}

interface Props {
  puntos: PuntoDelGrafico[];
  /** Cada cuántas barras se escribe una etiqueta en el eje. */
  cadaCuantas: number;
  vacio: string;
}

export function GraficoBarras({ puntos, cadaCuantas, vacio }: Props) {
  const maximo = puntos.reduce((mayor, p) => Math.max(mayor, p.valor), 0);

  // Sin un solo valor, escalar contra cero daría `NaN` en cada altura y todas
  // las barras saldrían llenas. Es un estado normal (una instalación nueva),
  // así que se dice y no se dibuja.
  if (maximo <= 0) return <p className="vacio">{vacio}</p>;

  return (
    <>
      <div className="grafico">
        {puntos.map((p) => (
          <div className="columna" key={p.clave} title={p.descripcion}>
            <div
              className={p.valor > 0 ? "barra" : "barra cero"}
              style={{ height: `${(p.valor / maximo) * 100}%` }}
            />
          </div>
        ))}
      </div>
      <div className="eje-x" aria-hidden="true">
        {puntos.map((p, i) => (
          <span key={p.clave}>{i % cadaCuantas === 0 ? p.etiqueta : ""}</span>
        ))}
      </div>
    </>
  );
}
