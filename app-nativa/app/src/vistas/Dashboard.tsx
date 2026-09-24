/**
 * La vista principal: métricas, gráficos e historial.
 *
 * # Las dos mitades de la carga
 *
 * Al montarse se piden las métricas y la primera página del historial
 * (`leer_metricas` / `leer_historial`), y recién después se escucha
 * `dictado-nuevo`. Sólo con eventos, un dictado hecho con la ventana cerrada no
 * aparecería nunca; sólo con la carga inicial, la lista quedaría congelada
 * mientras la ventana está abierta. Hacen falta las dos.
 *
 * # Por qué el historial se pagina en el backend
 *
 * `dashboard.py` levantaba el `.jsonl` entero y dibujaba todas las filas. Acá se
 * piden {@link PAGINA} entradas por vez y la búsqueda viaja al backend, que
 * filtra sobre TODO el historial: buscar sólo en lo ya cargado sería un buscador
 * que miente.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  alDictadoNuevo,
  cancelarTodas,
  leerHistorial,
  leerMetricas,
  mensajeDeError,
  type Dictado,
  type Metricas,
} from "../api";
import { GraficoBarras, type PuntoDelGrafico } from "../componentes/GraficoBarras";
import { Metrica } from "../componentes/Basicos";
import {
  conDosDecimales,
  diaCorto,
  duracionEnMinutos,
  fechaCorta,
  nombreDeModo,
  numero,
} from "../formato";

/** Cuántos dictados trae cada página. */
const PAGINA = 25;

/** Cuánto se espera antes de buscar, para no pedir una página por tecla. */
const ESPERA_BUSQUEDA_MS = 250;

/**
 * Cuánto se espera antes de recalcular las métricas tras un dictado nuevo.
 * Recalcular recorre el historial entero: dictar tres frases seguidas no tiene
 * por qué costar tres recorridas.
 */
const ESPERA_METRICAS_MS = 400;

/** La velocidad de tipeo contra la que compara `metricas::minutos_ahorrados`. */
const PALABRAS_POR_MINUTO_TIPEANDO = 40;

export default function Dashboard() {
  const [metricas, setMetricas] = useState<Metricas | null>(null);
  const [dictados, setDictados] = useState<Dictado[]>([]);
  const [total, setTotal] = useState(0);
  const [hayMas, setHayMas] = useState(false);
  const [buscar, setBuscar] = useState("");
  const [cargando, setCargando] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Cada petición lleva número: si el usuario escribe rápido, la respuesta de
  // "cli" puede llegar después de la de "cliente" y pisar la lista correcta.
  const ultimaPeticion = useRef(0);
  const montado = useRef(true);
  // El oyente de `dictado-nuevo` se registra una sola vez y necesita saber si
  // hay una búsqueda activa AHORA. Un `ref` y no el estado: volver a suscribirse
  // en cada tecla perdería eventos entre el `unlisten` y el `listen`. Se
  // sincroniza en un efecto y no durante el render, que React puede descartar.
  const busquedaActual = useRef(buscar);
  useEffect(() => {
    busquedaActual.current = buscar;
  }, [buscar]);

  const refrescarMetricas = useCallback(() => {
    leerMetricas()
      .then((m) => montado.current && setMetricas(m))
      .catch((e) => montado.current && setError(mensajeDeError(e)));
  }, []);

  const cargarPagina = useCallback(async (desplazamiento: number, texto: string) => {
    const mia = ++ultimaPeticion.current;
    // También acá y no sólo en el efecto de la búsqueda: es lo que apaga el
    // botón "Cargar más" mientras la página viaja, para que dos clics seguidos
    // no pidan dos veces el mismo tramo.
    setCargando(true);
    try {
      const pagina = await leerHistorial(PAGINA, desplazamiento, texto);
      if (!montado.current || mia !== ultimaPeticion.current) return;
      setDictados((previos) =>
        desplazamiento === 0 ? pagina.entradas : [...previos, ...pagina.entradas],
      );
      setTotal(pagina.total);
      setHayMas(pagina.hay_mas);
      setError(null);
    } catch (e) {
      if (montado.current) setError(mensajeDeError(e));
    } finally {
      if (montado.current && mia === ultimaPeticion.current) setCargando(false);
    }
  }, []);

  // 1. El presente, al montarse.
  useEffect(() => {
    montado.current = true;
    refrescarMetricas();
    return () => {
      montado.current = false;
    };
  }, [refrescarMetricas]);

  // 2. La primera página, y otra cada vez que cambia la búsqueda.
  useEffect(() => {
    setCargando(true);
    const reloj = setTimeout(() => cargarPagina(0, buscar), ESPERA_BUSQUEDA_MS);
    return () => clearTimeout(reloj);
  }, [buscar, cargarPagina]);

  // 3. Y las novedades.
  useEffect(() => {
    let reloj: ReturnType<typeof setTimeout> | undefined;
    const suscripciones = [
      alDictadoNuevo((nuevo) => {
        // Con una búsqueda activa no se agrega a ciegas: el dictado nuevo puede
        // no coincidir, y meterlo igual mostraría un resultado que no lo es. El
        // total tampoco se toca por lo mismo.
        if (!busquedaActual.current.trim()) {
          setDictados((previos) => [nuevo, ...previos]);
          setTotal((t) => t + 1);
        }
        clearTimeout(reloj);
        reloj = setTimeout(refrescarMetricas, ESPERA_METRICAS_MS);
      }),
    ];
    return () => {
      clearTimeout(reloj);
      cancelarTodas(suscripciones);
    };
  }, [refrescarMetricas]);

  const porDia = useMemo<PuntoDelGrafico[]>(
    () =>
      (metricas?.por_dia ?? []).map((d) => ({
        clave: d.dia,
        valor: d.palabras,
        etiqueta: diaCorto(d.dia),
        descripcion: `${diaCorto(d.dia)} · ${numero(d.palabras)} palabras · ${numero(
          d.dictados,
        )} dictados`,
      })),
    [metricas],
  );

  const porHora = useMemo<PuntoDelGrafico[]>(
    () =>
      (metricas?.por_hora ?? []).map((palabras, hora) => ({
        clave: String(hora),
        valor: palabras,
        etiqueta: String(hora),
        descripcion: `${String(hora).padStart(2, "0")}:00 · ${numero(palabras)} palabras`,
      })),
    [metricas],
  );

  if (error && !metricas) {
    return <p className="error-vista">No pude leer los datos: {error}</p>;
  }
  if (!metricas) return <p className="cargando-vista">Cargando…</p>;

  const ahorrado = duracionEnMinutos(metricas.minutos_ahorrados);

  return (
    <>
      {error && <p className="error-vista">{error}</p>}

      <section className="metricas">
        <Metrica
          titulo="Dictados"
          valor={numero(metricas.total_dictados)}
          delta={metricas.dictados_hoy > 0 ? `+${numero(metricas.dictados_hoy)} hoy` : null}
        />
        <Metrica
          titulo="Palabras"
          valor={numero(metricas.total_palabras)}
          delta={metricas.palabras_hoy > 0 ? `+${numero(metricas.palabras_hoy)} hoy` : null}
        />
        <Metrica
          titulo="Velocidad al hablar"
          valor={numero(metricas.palabras_por_minuto)}
          unidad="ppm"
          nota={`tipear ronda las ${PALABRAS_POR_MINUTO_TIPEANDO}`}
          ayuda="Palabras por minuto hablando, sobre el audio grabado."
        />
        <Metrica
          titulo="Tiempo ahorrado"
          valor={ahorrado.valor}
          unidad={ahorrado.unidad}
          nota={`vs. tipear a ${PALABRAS_POR_MINUTO_TIPEANDO} ppm`}
        />
        <Metrica
          titulo="Latencia promedio"
          valor={metricas.latencia_s === null ? "—" : conDosDecimales(metricas.latencia_s)}
          unidad={metricas.latencia_s === null ? undefined : "s"}
          nota={
            metricas.modo_actual
              ? `modo «${nombreDeModo(metricas.modo_actual)}»${
                  metricas.dictados_de_otros_modos > 0
                    ? ` · ${numero(metricas.dictados_de_otros_modos)} de otros modos fuera`
                    : ""
                }`
              : null
          }
          ayuda={
            "Transcripción + limpieza, promediado SÓLO sobre los dictados del modo " +
            "vigente. Mezclar los modos anteriores falsearía el número."
          }
        />
      </section>

      {metricas.por_modo.length > 1 && (
        <p className="porque">
          Por modo de limpieza:{" "}
          {metricas.por_modo
            .map(
              (m) =>
                `${nombreDeModo(m.modo)} ${conDosDecimales(m.latencia_s)} s (${numero(
                  m.dictados,
                )} dictados)`,
            )
            .join(" · ")}
        </p>
      )}

      <section className="graficos">
        <div className="panel">
          <header>
            <h2>Palabras por día</h2>
            <span className="apunte">últimos {porDia.length} días</span>
          </header>
          <GraficoBarras
            puntos={porDia}
            cadaCuantas={5}
            vacio="Todavía no hay dictados en este período."
          />
        </div>
        <div className="panel">
          <header>
            <h2>Actividad por hora</h2>
            <span className="apunte">todo el historial</span>
          </header>
          <GraficoBarras
            puntos={porHora}
            cadaCuantas={3}
            vacio="Todavía no hay dictados registrados."
          />
        </div>
      </section>

      <section className="panel">
        <header>
          <h2>Historial</h2>
          <span className="apunte">
            {buscar.trim()
              ? `${numero(total)} coinciden`
              : `${numero(total)} dictados`}
          </span>
        </header>

        <input
          className="buscador"
          type="text"
          value={buscar}
          placeholder="Buscar en todo el historial…"
          aria-label="Buscar en el historial"
          onChange={(e) => setBuscar(e.currentTarget.value)}
        />

        {cargando && dictados.length === 0 ? (
          <p className="vacio">Buscando…</p>
        ) : dictados.length === 0 ? (
          <p className="vacio">
            {buscar.trim()
              ? "Ningún dictado coincide con la búsqueda."
              : "Todavía no hay dictados. Apretá la tecla de dictado, hablá, y apretala de nuevo."}
          </p>
        ) : (
          <>
            <ul className="historial">
              {dictados.map((d, i) => (
                <FilaDictado dictado={d} key={`${d.ts}-${i}`} />
              ))}
            </ul>
            <div className="pie-historial">
              <span>
                Mostrando {numero(dictados.length)} de {numero(total)}
              </span>
              {hayMas && (
                <button
                  type="button"
                  className="boton chico"
                  disabled={cargando}
                  onClick={() => cargarPagina(dictados.length, buscar)}
                >
                  {cargando ? "Cargando…" : "Cargar más"}
                </button>
              )}
            </div>
          </>
        )}
      </section>
    </>
  );
}

/**
 * Una fila del historial.
 *
 * `final` puede venir vacío a propósito: con "no guardar el texto dictado"
 * activado, el backend escribe la entrada con las métricas y sin las palabras.
 * Eso se dice, no se muestra como una fila en blanco.
 */
function FilaDictado({ dictado }: { dictado: Dictado }) {
  const latencia = dictado.transcribe_s + dictado.cleanup_s;
  const hayCrudo = dictado.raw.trim().length > 0 && dictado.raw !== dictado.final;
  const hayTexto = dictado.final.trim().length > 0;

  return (
    <li>
      <div className="cabecera">
        <time dateTime={dictado.ts}>{fechaCorta(dictado.ts)}</time>
        <span>{numero(dictado.words)} palabras</span>
        <span>{conDosDecimales(dictado.audio_s)} s de audio</span>
        <span>{conDosDecimales(latencia)} s de latencia</span>
        <span className="etiqueta-modo">{nombreDeModo(dictado.mode)}</span>
        {hayTexto && <BotonCopiar texto={dictado.final} />}
      </div>
      {hayTexto ? (
        <p className="texto seleccionable">{dictado.final}</p>
      ) : (
        <p className="texto ausente">
          Texto no guardado (sólo se registraron las métricas).
        </p>
      )}
      {hayCrudo && (
        <details className="crudo-caja">
          <summary>Ver el crudo</summary>
          <p className="crudo seleccionable">{dictado.raw}</p>
        </details>
      )}
    </li>
  );
}

/** Cuánto queda a la vista el "Copiado" antes de volver a "Copiar". */
const AVISO_COPIADO_MS = 1500;

type EstadoCopia = "listo" | "copiado" | "fallo";

const ROTULO_COPIA: Record<EstadoCopia, string> = {
  listo: "Copiar",
  copiado: "Copiado",
  fallo: "No se pudo copiar",
};

/**
 * Copia el texto de un dictado al portapapeles. Existe para el caso en que el
 * pegado automático no encontró un campo con foco: sin esto había que
 * seleccionar el texto a mano.
 */
function BotonCopiar({ texto }: { texto: string }) {
  const [estado, setEstado] = useState<EstadoCopia>("listo");
  const reloj = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => () => clearTimeout(reloj.current), []);

  const copiar = async () => {
    try {
      await navigator.clipboard.writeText(texto);
      setEstado("copiado");
    } catch {
      setEstado("fallo");
    }
    clearTimeout(reloj.current);
    reloj.current = setTimeout(() => setEstado("listo"), AVISO_COPIADO_MS);
  };

  return (
    <button
      type="button"
      className="boton plano chico copiar"
      aria-label="Copiar el texto del dictado"
      onClick={copiar}
    >
      <span aria-live="polite">{ROTULO_COPIA[estado]}</span>
    </button>
  );
}
