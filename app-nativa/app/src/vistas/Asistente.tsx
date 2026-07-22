/**
 * El asistente de primer arranque: se muestra cuando no hay ningún modelo en
 * disco, que es el único estado en el que MithFlow no puede dictar.
 *
 * # Por qué el paso del medio es medir y no preguntar
 *
 * El backend no elige el modelo por la ficha técnica de la placa: baja el modelo
 * más liviano, transcribe con él un audio de referencia y cronometra. Ese número
 * —cuántos segundos de audio procesa por segundo de reloj— es lo único que
 * decide. Dos máquinas con la misma especificación rinden distinto, y una
 * integrada con Vulkan andando puede ganarle a una dedicada con drivers viejos.
 * El asistente muestra la medición en castellano para que la recomendación no
 * parezca salida de un sombrero.
 *
 * # El orden de las descargas
 *
 * Medir necesita un modelo, así que el primero que se baja es el de perfilado
 * (`catalogo.modelo_de_perfilado`, el más chico). No se tira después: es un
 * modelo usable, y si termina siendo el recomendado no hay segunda descarga.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import {
  alPerfiladoListo,
  alProgresoDescarga,
  cancelarTodas,
  descargarModelo,
  escribirAjustes,
  leerAjustes,
  leerCatalogo,
  mensajeDeError,
  perfilarHardware,
  type Catalogo,
  type ModeloDto,
  type PerfilDto,
  type ProgresoDescarga,
} from "../api";
import { Progreso } from "../componentes/Basicos";
import { bytesEnMegas, pesoDeArchivo, unDecimal } from "../formato";

type Paso =
  | "intro"
  | "descargando-medicion"
  | "midiendo"
  | "eleccion"
  | "descargando-elegido"
  | "listo";

const TITULOS: Record<Paso, string> = {
  intro: "Bienvenido a MithFlow",
  "descargando-medicion": "Bajando el modelo de medición",
  midiendo: "Midiendo tu máquina",
  eleccion: "Esto encontré en tu máquina",
  "descargando-elegido": "Bajando el modelo",
  listo: "Todo listo",
};

/** En qué etapa de las tres bolitas está cada paso. */
const ETAPA: Record<Paso, number> = {
  intro: 0,
  "descargando-medicion": 1,
  midiendo: 1,
  eleccion: 2,
  "descargando-elegido": 2,
  listo: 3,
};

interface Props {
  catalogo: Catalogo;
  alCambiarCatalogo: (catalogo: Catalogo) => void;
  /** Cierra el asistente y deja la app en la vista indicada. */
  alTerminar: (destino: "dashboard" | "ajustes") => void;
}

export default function Asistente({ catalogo, alCambiarCatalogo, alTerminar }: Props) {
  const [paso, setPaso] = useState<Paso>("intro");
  const [perfil, setPerfil] = useState<PerfilDto | null>(null);
  const [elegido, setElegido] = useState<string | null>(null);
  const [descarga, setDescarga] = useState<ProgresoDescarga | null>(null);
  const [error, setError] = useState<string | null>(null);
  const montado = useRef(true);

  // El paso vigente lo leen los oyentes, que se registran una sola vez. Un
  // `ref` evita re-suscribirse (y perder un evento en el hueco). Se sincroniza
  // en un efecto y no durante el render, que React puede descartar.
  const pasoActual = useRef(paso);
  useEffect(() => {
    pasoActual.current = paso;
  }, [paso]);

  const medir = useCallback(() => {
    setError(null);
    setPaso("midiendo");
    perfilarHardware().catch((e) => {
      if (!montado.current) return;
      setError(mensajeDeError(e));
      setPaso("intro");
    });
  }, []);

  useEffect(() => {
    montado.current = true;
    const suscripciones = [
      alPerfiladoListo((p) => {
        if (!montado.current) return;
        setPerfil(p);
        // Un perfilado fallido no bloquea: se muestra el motivo y se elige a
        // mano. Quedarse sin poder dictar por no poder medir sería peor.
        setElegido(p.error ? catalogo.modelo_de_perfilado : p.modelo_recomendado);
        setPaso("eleccion");
      }),
      alProgresoDescarga((p) => {
        if (!montado.current) return;
        setDescarga(p);
        if (!p.terminado) return;
        if (p.error) {
          setError(`No pude descargar ${p.modelo}: ${p.error}`);
          return;
        }
        leerCatalogo()
          .then((c) => {
            if (!montado.current) return;
            alCambiarCatalogo(c);
            // Terminó la descarga del modelo con el que se mide → a medir.
            // Terminó la del elegido → a la pantalla final.
            if (pasoActual.current === "descargando-medicion") medir();
            else if (pasoActual.current === "descargando-elegido") setPaso("listo");
          })
          .catch((e) => montado.current && setError(mensajeDeError(e)));
      }),
    ];
    return () => {
      montado.current = false;
      cancelarTodas(suscripciones);
    };
  }, [alCambiarCatalogo, catalogo.modelo_de_perfilado, medir]);

  const bajar = (clave: string, siguiente: Paso) => {
    setError(null);
    setDescarga(null);
    setPaso(siguiente);
    descargarModelo(clave).catch((e) => {
      if (!montado.current) return;
      setError(mensajeDeError(e));
      setPaso(siguiente === "descargando-medicion" ? "intro" : "eleccion");
    });
  };

  const empezar = () => {
    const medicion = catalogo.modelos.find((m) => m.clave === catalogo.modelo_de_perfilado);
    if (medicion?.descargado) medir();
    else bajar(catalogo.modelo_de_perfilado, "descargando-medicion");
  };

  /** Deja el modelo elegido en los ajustes y lo baja si hace falta. */
  const confirmarEleccion = async () => {
    if (!elegido) return;
    setError(null);
    try {
      const ajustes = await leerAjustes();
      await escribirAjustes({ ...ajustes, modelo: elegido });
    } catch (e) {
      if (montado.current) setError(mensajeDeError(e));
      return;
    }
    const modelo = catalogo.modelos.find((m) => m.clave === elegido);
    if (modelo?.descargado) setPaso("listo");
    else bajar(elegido, "descargando-elegido");
  };

  return (
    <div className="asistente">
      <div className="pasos">
        {["Presentación", "Medición", "Modelo"].map((nombre, i) => (
          <span key={nombre}>
            <span
              className={
                ETAPA[paso] > i ? "bolita hecha" : ETAPA[paso] === i ? "bolita activa" : "bolita"
              }
            />
            {nombre}
          </span>
        ))}
      </div>

      <h2>{TITULOS[paso]}</h2>

      {error && <p className="error-vista">{error}</p>}

      {paso === "intro" && (
        <>
          <p className="intro">
            MithFlow dicta por voz sin mandar una sola palabra a internet. Para
            empezar necesita un modelo de transcripción: voy a medir tu máquina y
            recomendarte el que mejor le calce.
          </p>
          <p className="porque">
            La medición no mira las especificaciones de tu placa: baja el modelo
            más liviano ({pesoDeArchivo(pesoDeModelo(catalogo, catalogo.modelo_de_perfilado))}),
            transcribe con él un audio de referencia y cronometra cuánto tarda de
            verdad. Por eso acierta igual en una notebook con gráficos integrados
            que en un escritorio con placa dedicada.
          </p>
          <div className="acciones">
            <button type="button" className="boton primario" onClick={empezar}>
              Medir mi máquina
            </button>
            <span className="espacio" />
            <button
              type="button"
              className="boton plano"
              onClick={() => alTerminar("ajustes")}
            >
              Configurar a mano
            </button>
          </div>
        </>
      )}

      {(paso === "descargando-medicion" || paso === "descargando-elegido") && (
        <>
          <p className="intro">
            {paso === "descargando-medicion"
              ? "Bajando el modelo con el que voy a medir. Se queda en tu disco: si termina siendo el recomendado, no hay una segunda descarga."
              : "Bajando el modelo elegido. Se verifica el hash antes de instalarlo, así que una descarga cortada nunca se usa."}
          </p>
          <Progreso
            porcentaje={descarga?.porcentaje ?? 0}
            izquierda={descarga ? descarga.modelo : "Conectando…"}
            derecha={
              descarga && descarga.total > 0
                ? `${bytesEnMegas(descarga.bytes)} / ${bytesEnMegas(descarga.total)}`
                : ""
            }
          />
          {error && (
            <div className="acciones">
              <button
                type="button"
                className="boton"
                onClick={() =>
                  bajar(
                    paso === "descargando-medicion"
                      ? catalogo.modelo_de_perfilado
                      : (elegido ?? catalogo.modelo_de_perfilado),
                    paso,
                  )
                }
              >
                Reintentar
              </button>
            </div>
          )}
        </>
      )}

      {paso === "midiendo" && (
        <>
          <p className="intro">
            Transcribiendo el audio de referencia unas cuantas veces y quedándome
            con la mediana. Tarda unos veinte segundos; la primera pasada se
            descarta porque incluye la compilación de los shaders.
          </p>
          <Progreso porcentaje={100} izquierda="Midiendo…" />
        </>
      )}

      {paso === "eleccion" && perfil && (
        <>
          {perfil.error ? (
            <p className="error-vista">
              No pude medir tu máquina: {perfil.error}. Elegí un modelo a mano —
              el más chico anda en todas.
            </p>
          ) : (
            <>
              <ul className="hallazgos">
                <li>
                  <span className="clave">Placa de video</span>
                  <span className="valor">
                    {perfil.gpu_nombre
                      ? `${perfil.gpu_nombre}${perfil.gpu_clase ? ` (${perfil.gpu_clase})` : ""}`
                      : "Ninguna: va a transcribir con el procesador."}
                  </span>
                </li>
                <li>
                  <span className="clave">Memoria</span>
                  <span className="valor">{unDecimal(perfil.ram_total_gb)} GB de RAM</span>
                </li>
                <li>
                  <span className="clave">Velocidad</span>
                  <span className="valor">
                    {unDecimal(perfil.factor_tiempo_real)}× tiempo real — procesa{" "}
                    {unDecimal(perfil.factor_tiempo_real)} segundos de audio por cada
                    segundo de reloj{perfil.backend ? `, con ${perfil.backend}` : ""}.
                  </span>
                </li>
                <li>
                  <span className="clave">Recomendado</span>
                  <span className="valor">{perfil.etiqueta_recomendado}</span>
                </li>
              </ul>
              <p className="porque">{explicarRecomendacion(perfil)}</p>
            </>
          )}

          <div className="modelos">
            {catalogo.modelos.map((m) => (
              <button
                type="button"
                className="modelo"
                key={m.clave}
                aria-pressed={elegido === m.clave}
                onClick={() => setElegido(m.clave)}
              >
                <div className="datos">
                  <div className="nombre">{m.etiqueta}</div>
                  <div className="meta">
                    {pesoDeArchivo(m.megabytes)}
                    {m.descargado && " · ya está en tu disco"}
                    {noEntra(perfil, m) && (
                      <span className="alerta">
                        {" "}
                        · puede no entrar en tu memoria
                      </span>
                    )}
                  </div>
                </div>
                {!perfil.error && m.clave === perfil.modelo_recomendado && (
                  <span className="insignia">Recomendado</span>
                )}
              </button>
            ))}
          </div>

          <div className="acciones">
            <button
              type="button"
              className="boton primario"
              disabled={!elegido}
              onClick={confirmarEleccion}
            >
              {catalogo.modelos.find((m) => m.clave === elegido)?.descargado
                ? "Usar este modelo"
                : "Descargar y usar"}
            </button>
            <span className="espacio" />
            <button type="button" className="boton plano" onClick={medir}>
              Volver a medir
            </button>
          </div>
        </>
      )}

      {paso === "listo" && (
        <>
          <p className="intro">
            El modelo está en tu disco y quedó elegido en Ajustes. Reiniciá
            MithFlow para que el motor lo cargue, y después apretá la tecla de
            dictado, hablá, y apretala de nuevo: el texto se pega solo donde
            tengas el cursor.
          </p>
          <p className="porque">
            Nada de esto sale de tu máquina: el audio se transcribe acá y se borra.
          </p>
          <div className="acciones">
            <button
              type="button"
              className="boton primario"
              onClick={() => alTerminar("dashboard")}
            >
              Ir al dashboard
            </button>
          </div>
        </>
      )}
    </div>
  );
}

/** El peso del modelo con esa clave, o 0 si no está en el catálogo. */
function pesoDeModelo(catalogo: Catalogo, clave: string): number {
  return catalogo.modelos.find((m) => m.clave === clave)?.megabytes ?? 0;
}

/**
 * ¿Este modelo se le puede quedar corto de memoria a esta máquina?
 *
 * Sólo aplica **sin placa dedicada**: con VRAM propia los pesos no compiten con
 * el resto del sistema. Es la misma regla que usa `hardware::limitar_por_memoria`,
 * y por eso `ram_minima_gb` viaja en el catálogo en vez de escribirse acá.
 */
function noEntra(perfil: PerfilDto, modelo: ModeloDto): boolean {
  if (perfil.error || perfil.gpu_dedicada) return false;
  return modelo.ram_minima_gb > 0 && perfil.ram_total_gb < modelo.ram_minima_gb;
}

/** La recomendación en una frase, armada con los números que se midieron. */
function explicarRecomendacion(perfil: PerfilDto): string {
  // Las cuatro franjas son las de `hardware::franja`: 20x, 8x y 3x.
  const franja =
    perfil.factor_tiempo_real >= 20
      ? "va sobrada"
      : perfil.factor_tiempo_real >= 8
        ? "va cómoda"
        : perfil.factor_tiempo_real >= 3
          ? "va justa"
          : "va ajustada";
  const memoria = perfil.gpu_dedicada
    ? "Tu placa tiene memoria propia, así que el modelo no compite con el resto del sistema."
    : `Como no hay una placa con memoria propia, los pesos del modelo viven en los ${unDecimal(
        perfil.ram_total_gb,
      )} GB de RAM del sistema, y eso también acota cuál entra.`;
  return `Con esa velocidad tu máquina ${franja}. ${memoria} Podés aceptar la recomendación o elegir otro: el más grande transcribe mejor, el más chico responde antes.`;
}
