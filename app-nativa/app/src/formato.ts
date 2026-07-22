/**
 * Cómo se escriben los números y las fechas en la interfaz.
 *
 * Está aparte de las vistas porque las mismas cifras aparecen en varios lados
 * (el total de palabras en la métrica y en el gráfico, la duración en el
 * historial y en Ajustes) y dos formatos distintos para el mismo dato leen como
 * dos datos distintos.
 */

const NUMERO = new Intl.NumberFormat("es-AR");

/** `12345` → `"12.345"`. */
export const numero = (n: number): string => NUMERO.format(Math.round(n));

/**
 * Minutos a algo que se lee de un vistazo: `95` → `"1 h 35 min"`.
 *
 * Por encima de la hora se parte, porque "1.847 min" obliga a dividir de cabeza
 * y el número existe justamente para poder decir "me ahorré una tarde".
 */
export function duracionEnMinutos(minutos: number): { valor: string; unidad: string } {
  if (!Number.isFinite(minutos) || minutos < 1) return { valor: "0", unidad: "min" };
  if (minutos < 60) return { valor: numero(minutos), unidad: "min" };
  const horas = Math.floor(minutos / 60);
  const resto = Math.round(minutos - horas * 60);
  if (resto === 0) return { valor: numero(horas), unidad: "h" };
  return { valor: `${numero(horas)} h ${resto}`, unidad: "min" };
}

/** Segundos a `mm:ss`, para el tope de grabación. */
export function minutosYSegundos(segs: number): string {
  const m = Math.floor(segs / 60);
  const s = Math.round(segs % 60);
  return m > 0 ? `${m}:${String(s).padStart(2, "0")} min` : `${s} s`;
}

/** Megabytes del catálogo a algo legible: `1550` → `"1,5 GB"`. */
export function pesoDeArchivo(megabytes: number): string {
  if (megabytes >= 1024) return `${(megabytes / 1024).toFixed(1).replace(".", ",")} GB`;
  return `${numero(megabytes)} MB`;
}

/** Bytes a MB, para el avance de la descarga. */
export const bytesEnMegas = (bytes: number): string =>
  `${numero(bytes / (1024 * 1024))} MB`;

/**
 * Un `ts` del historial (`2026-07-21T11:49:40`, hora local sin zona) a
 * `21/07 11:49`.
 *
 * **No se construye un `Date`**: la cadena ya viene en hora local y puede traer
 * sufijos (fracciones de segundo, zona pegada), así que alcanza con partirla.
 * Si no tiene la forma esperada se devuelve tal cual, que es más honesto que
 * mostrar "Invalid Date".
 */
export function fechaCorta(ts: string): string {
  const partes = ts.match(/^\d{4}-(\d{2})-(\d{2})T(\d{2}):(\d{2})/);
  if (!partes) return ts;
  const [, mes, dia, hora, minuto] = partes;
  return `${dia}/${mes} ${hora}:${minuto}`;
}

/** `2026-07-21` → `21/07`, para el eje del gráfico diario. */
export function diaCorto(dia: string): string {
  const partes = dia.match(/^\d{4}-(\d{2})-(\d{2})$/);
  return partes ? `${partes[2]}/${partes[1]}` : dia;
}

/** Segundos con dos decimales y coma decimal: `0.512` → `"0,51"`. */
export const conDosDecimales = (s: number): string => s.toFixed(2).replace(".", ",");

/** Un decimal con coma: `43.21` → `"43,2"`. */
export const unDecimal = (n: number): string => n.toFixed(1).replace(".", ",");

/** Nombre del modo de limpieza tal como se muestra. */
export function nombreDeModo(modo: string): string {
  const nombres: Record<string, string> = {
    rapido: "Rápido",
    ninguno: "Ninguno",
    fast: "rápido",
    llm: "LLM (histórico)",
    none: "ninguno",
  };
  return nombres[modo] ?? modo;
}
