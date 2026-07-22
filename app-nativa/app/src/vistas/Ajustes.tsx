/**
 * Ajustes. Todos los controles de esta pantalla hacen algo de verdad: el
 * backend aplica la tecla, los sonidos y el autoarranque en caliente, y le pasa
 * al motor el vocabulario, las muletillas, el modo de limpieza, el tope de
 * grabación y la privacidad del historial.
 *
 * # Por qué hay un botón "Guardar" y no autoguardado
 *
 * `escribir_ajustes` **normaliza** lo que recibe: recorta el vocabulario,
 * deduplica muletillas y acota el tope de grabación. Guardar en cada tecla haría
 * que el campo se reescriba solo mientras se escribe. Con un guardado explícito,
 * lo que vuelve del backend reemplaza el formulario de una vez y el usuario ve
 * exactamente qué quedó guardado.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import {
  alProgresoDescarga,
  borrarHistorial,
  cancelarTodas,
  descargarModelo,
  desinstalar,
  escribirAjustes,
  leerAjustes,
  leerCatalogo,
  leerResumenDesinstalacion,
  mensajeDeError,
  MODELO_AUTOMATICO,
  type Ajustes,
  type Catalogo,
  type ProgresoDescarga,
  type ResumenDesinstalacion,
} from "../api";
import { Campo, Interruptor, Progreso, Segmentado } from "../componentes/Basicos";
import {
  bytesEnMegas,
  minutosYSegundos,
  nombreDeModo,
  nombreDePosicion,
  numero,
  pesoDeArchivo,
  pesoEnBytes,
} from "../formato";

/** Tope del vocabulario propio, el mismo que `ajustes::MAX_VOCABULARIO`. */
const MAX_VOCABULARIO = 1000;

/** Tope de muletillas, el mismo que `ajustes::MAX_MULETILLAS`. */
const MAX_MULETILLAS = 100;

interface Props {
  /** Para avisarle al resto de la app que el catálogo cambió (un modelo nuevo). */
  alCambiarCatalogo: (catalogo: Catalogo) => void;
  avisar: (texto: string, nivel: "info" | "error") => void;
}

export default function VistaAjustes({ alCambiarCatalogo, avisar }: Props) {
  const [catalogo, setCatalogo] = useState<Catalogo | null>(null);
  const [guardados, setGuardados] = useState<Ajustes | null>(null);
  const [borrador, setBorrador] = useState<Ajustes | null>(null);
  const [guardando, setGuardando] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [descarga, setDescarga] = useState<ProgresoDescarga | null>(null);
  const [confirmandoBorrado, setConfirmandoBorrado] = useState(false);
  const [muletillaNueva, setMuletillaNueva] = useState("");
  // La desinstalación tiene su propia confirmación de dos pasos: `resumen` es
  // el paso 2 (existe sólo mientras el panel está abierto, con los tamaños ya
  // medidos a la vista).
  const [resumen, setResumen] = useState<ResumenDesinstalacion | null>(null);
  const [midiendo, setMidiendo] = useState(false);
  const [conservarDatos, setConservarDatos] = useState(false);
  const [desinstalando, setDesinstalando] = useState(false);
  const montado = useRef(true);

  useEffect(() => {
    montado.current = true;
    Promise.all([leerAjustes(), leerCatalogo()])
      .then(([a, c]) => {
        if (!montado.current) return;
        setGuardados(a);
        setBorrador(a);
        setCatalogo(c);
      })
      .catch((e) => montado.current && setError(mensajeDeError(e)));
    return () => {
      montado.current = false;
    };
  }, []);

  // Una descarga lanzada desde acá avisa por evento, igual que la del asistente.
  useEffect(() => {
    const suscripciones = [
      alProgresoDescarga((p) => {
        setDescarga(p);
        if (!p.terminado) return;
        if (p.error) {
          avisar(`No pude descargar ${p.modelo}: ${p.error}`, "error");
          return;
        }
        // El catálogo dice qué modelos están en disco: sin releerlo, el que se
        // acaba de bajar seguiría apareciendo como "no descargado".
        leerCatalogo()
          .then((c) => {
            if (!montado.current) return;
            setCatalogo(c);
            alCambiarCatalogo(c);
          })
          .catch(() => {});
      }),
    ];
    return () => cancelarTodas(suscripciones);
  }, [alCambiarCatalogo, avisar]);

  const sucio = useMemo(
    () => JSON.stringify(guardados) !== JSON.stringify(borrador),
    [guardados, borrador],
  );

  if (error && !borrador) return <p className="error-vista">{error}</p>;
  if (!borrador || !catalogo || !guardados) {
    return <p className="cargando-vista">Cargando ajustes…</p>;
  }

  const cambiar = <C extends keyof Ajustes>(campo: C, valor: Ajustes[C]) =>
    setBorrador((previo) => (previo ? { ...previo, [campo]: valor } : previo));

  const guardar = () => {
    setGuardando(true);
    setError(null);
    escribirAjustes(borrador)
      .then((normalizados) => {
        if (!montado.current) return;
        // Lo que vuelve es lo que EFECTIVAMENTE quedó guardado, que puede venir
        // recortado. El formulario pasa a mostrar eso y no lo que se pidió.
        setGuardados(normalizados);
        setBorrador(normalizados);
        avisar("Ajustes guardados.", "info");
      })
      .catch((e) => montado.current && setError(mensajeDeError(e)))
      .finally(() => montado.current && setGuardando(false));
  };

  const agregarMuletilla = () => {
    const limpia = muletillaNueva.trim().toLowerCase();
    if (!limpia || borrador.muletillas.includes(limpia)) {
      setMuletillaNueva("");
      return;
    }
    if (borrador.muletillas.length >= MAX_MULETILLAS) {
      setError(`La lista de muletillas admite hasta ${MAX_MULETILLAS}.`);
      return;
    }
    cambiar("muletillas", [...borrador.muletillas, limpia]);
    setMuletillaNueva("");
  };

  const borrarTodoElHistorial = () => {
    borrarHistorial()
      .then(() => setConfirmandoBorrado(false))
      .catch((e) => setError(mensajeDeError(e)));
  };

  /**
   * Paso 1 de la desinstalación: medir qué se perdería.
   *
   * Se mide acá y no al montar la vista porque recorre tres directorios —uno con
   * dos gigabytes adentro—, y abrir Ajustes para subir el volumen no tiene por
   * qué pagar eso.
   */
  const medirDesinstalacion = () => {
    setMidiendo(true);
    setError(null);
    leerResumenDesinstalacion()
      .then((medido) => montado.current && setResumen(medido))
      .catch((e) => montado.current && setError(mensajeDeError(e)))
      .finally(() => montado.current && setMidiendo(false));
  };

  /**
   * Paso 2: ya vio qué pierde y eligió qué hacer con los datos.
   *
   * En el camino feliz esta promesa resuelve y medio segundo después la
   * aplicación se cierra sola, así que `desinstalando` no se apaga: el botón
   * queda en "Desinstalando…" hasta que la ventana desaparece. Sólo un error lo
   * devuelve a su estado.
   */
  const confirmarDesinstalacion = () => {
    setDesinstalando(true);
    setError(null);
    desinstalar(conservarDatos)
      .then(() => avisar("Desinstalando MithFlow…", "info"))
      .catch((e) => {
        if (!montado.current) return;
        setError(mensajeDeError(e));
        setDesinstalando(false);
      });
  };

  const enDescarga = descarga !== null && !descarga.terminado;

  return (
    <>
      {error && <p className="error-vista">{error}</p>}

      <section className="seccion">
        <h2>Dictado</h2>

        <Campo
          rotulo="Tecla de dictado"
          ayuda="Se aprieta para empezar y de nuevo para pegar. MithFlow la suprime, así que no llega a la ventana que estés usando."
        >
          <select
            value={borrador.tecla}
            onChange={(e) => cambiar("tecla", e.currentTarget.value)}
          >
            {catalogo.teclas.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        </Campo>

        <Campo
          rotulo="Modo de limpieza"
          ayuda="«Rápido» saca muletillas y tartamudeos con reglas locales, en menos de un milisegundo. «Ninguno» pega la transcripción tal cual."
        >
          <Segmentado
            opciones={catalogo.modos_limpieza.map((m) => ({
              valor: m,
              etiqueta: nombreDeModo(m),
            }))}
            elegida={borrador.modo_limpieza}
            alElegir={(m) => cambiar("modo_limpieza", m)}
          />
        </Campo>

        <Campo
          rotulo="Límite de duración"
          ayuda="Si te olvidás la grabación abierta, se corta sola y transcribe lo que haya."
        >
          <input
            type="range"
            min={catalogo.limite_grabacion_minimo}
            max={catalogo.limite_grabacion_maximo}
            step={15}
            value={borrador.limite_grabacion_s}
            onChange={(e) => cambiar("limite_grabacion_s", Number(e.currentTarget.value))}
          />
          <span className="cifra">{minutosYSegundos(borrador.limite_grabacion_s)}</span>
        </Campo>
      </section>

      <section className="seccion">
        <h2>Indicador de grabación</h2>
        <p className="porque">
          Una ventanita chica que aparece mientras dictás, con el nivel de lo que
          entra por el micrófono y el tiempo que llevás. Existe para cuando no
          tenés MithFlow abierto: sin ella, los tonos avisan que la grabación
          arrancó pero no que te esté escuchando, y se puede hablar cinco minutos
          al pedo con el micrófono equivocado. No toma el foco ni recibe clics:
          el cursor se queda donde estabas escribiendo.
        </p>

        <div className="separado">
          <Campo
            rotulo="Mostrar la ventanita"
            ayuda="Apagada no se crea ninguna ventana y no se mide ningún nivel."
          >
            <Interruptor
              activo={borrador.indicador}
              alCambiar={(v) => cambiar("indicador", v)}
              etiqueta={borrador.indicador ? "Sí, mostrarla" : "No mostrarla"}
            />
          </Campo>

          <Campo
            rotulo="Dónde aparece"
            ayuda="En la pantalla donde tengas el mouse, para que en dos monitores salga en el que estás mirando."
          >
            <select
              value={borrador.indicador_posicion}
              disabled={!borrador.indicador}
              onChange={(e) => cambiar("indicador_posicion", e.currentTarget.value)}
            >
              {catalogo.posiciones_indicador.map((p) => (
                <option key={p} value={p}>
                  {nombreDePosicion(p)}
                </option>
              ))}
            </select>
          </Campo>
        </div>
      </section>

      <section className="seccion">
        <h2>Vocabulario</h2>

        <Campo
          rotulo="Términos propios"
          ayuda={`Nombres que el modelo suele errar ("MithData" → "Middata"). Hasta ${numero(
            MAX_VOCABULARIO,
          )} caracteres: compiten por el contexto con lo que estás dictando.`}
        >
          <div className="ancho-total">
            <textarea
              value={borrador.vocabulario}
              maxLength={MAX_VOCABULARIO}
              placeholder="MithData, MithFlow, Jaé, Puerto Madryn…"
              onChange={(e) => cambiar("vocabulario", e.currentTarget.value)}
            />
            <div className="progreso-info">
              <span />
              <span>
                {numero(borrador.vocabulario.length)} / {numero(MAX_VOCABULARIO)}
              </span>
            </div>
          </div>
        </Campo>

        <Campo
          rotulo="Muletillas"
          ayuda="Las palabras que la limpieza rápida borra del texto antes de pegarlo."
        >
          <div className="ancho-total">
            <div className="fichas">
              {borrador.muletillas.length === 0 && (
                <span className="aclaracion">
                  Sin muletillas: la limpieza rápida sólo va a colapsar tartamudeos.
                </span>
              )}
              {borrador.muletillas.map((m) => (
                <span className="ficha" key={m}>
                  {m}
                  <button
                    type="button"
                    aria-label={`Quitar ${m}`}
                    onClick={() =>
                      cambiar(
                        "muletillas",
                        borrador.muletillas.filter((otra) => otra !== m),
                      )
                    }
                  >
                    ×
                  </button>
                </span>
              ))}
            </div>
            <div className="control">
              <input
                type="text"
                value={muletillaNueva}
                placeholder="Agregar una muletilla…"
                onChange={(e) => setMuletillaNueva(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    agregarMuletilla();
                  }
                }}
              />
              <button type="button" className="boton chico" onClick={agregarMuletilla}>
                Agregar
              </button>
            </div>
          </div>
        </Campo>
      </section>

      <section className="seccion">
        <h2>Modelo</h2>
        <p className="porque">
          Cambiar de modelo aplica recién la próxima vez que abras MithFlow:
          cargarlo son casi dos segundos y más de un gigabyte de memoria, así que
          no se hace a escondidas mientras dictás. La única excepción es que el
          motor todavía no haya arrancado por no haber ningún modelo —arriba dice
          "Falta el modelo"—: ahí el primero que bajes lo arranca solo, sin
          reiniciar. Si arriba dice "Error" no alcanza con bajar un modelo,
          porque lo que falla es otra cosa y el motivo está a la vista. Y si
          elegís justo el que el motor ya tiene entre manos no cambia nada, así
          que tampoco te va a pedir que reinicies.
        </p>
        <div className="modelos separado">
          <div className="fila-modelo">
            <button
              type="button"
              className="modelo"
              aria-pressed={borrador.modelo === MODELO_AUTOMATICO}
              onClick={() => cambiar("modelo", MODELO_AUTOMATICO)}
            >
              <div className="datos">
                <div className="nombre">Automático</div>
                <div className="meta">El que recomiende la medición de tu máquina.</div>
              </div>
            </button>
          </div>

          {catalogo.modelos.map((m) => (
            <div className="fila-modelo" key={m.clave}>
              <button
                type="button"
                className="modelo"
                aria-pressed={borrador.modelo === m.clave}
                onClick={() => cambiar("modelo", m.clave)}
              >
                <div className="datos">
                  <div className="nombre">{m.etiqueta}</div>
                  <div className="meta">
                    {pesoDeArchivo(m.megabytes)}
                    {m.ram_minima_gb > 0 &&
                      ` · pide ${m.ram_minima_gb} GB de RAM sin placa dedicada`}
                  </div>
                </div>
                <span className="insignia gris">
                  {m.descargado ? "Descargado" : "No descargado"}
                </span>
              </button>
              {!m.descargado && (
                <button
                  type="button"
                  className="boton chico"
                  disabled={enDescarga}
                  onClick={() => {
                    setDescarga(null);
                    descargarModelo(m.clave).catch((e) => setError(mensajeDeError(e)));
                  }}
                >
                  Descargar
                </button>
              )}
            </div>
          ))}
        </div>

        {descarga && (
          <div className="separado">
            <Progreso
              porcentaje={descarga.terminado && !descarga.error ? 100 : descarga.porcentaje}
              izquierda={
                descarga.error
                  ? `Falló: ${descarga.error}`
                  : descarga.terminado
                    ? // Qué significa la descarga —motor arrancando o cambio
                      // para el próximo arranque— lo sabe sólo el backend, que
                      // lo cuenta por `aviso`. Repetirlo acá a ciegas es cómo se
                      // llegó a que la app pidiera reiniciar cuando ya no hacía
                      // falta.
                      `${descarga.modelo} descargado.`
                    : `Descargando ${descarga.modelo}…`
              }
              derecha={
                descarga.total > 0 &&
                `${bytesEnMegas(descarga.bytes)} / ${bytesEnMegas(descarga.total)}`
              }
            />
          </div>
        )}
      </section>

      <section className="seccion">
        <h2>Sonidos y arranque</h2>

        <Campo rotulo="Tonos de aviso" ayuda="Un tono al empezar y otro al pegar.">
          <Interruptor
            activo={borrador.sonidos}
            alCambiar={(v) => cambiar("sonidos", v)}
            etiqueta={borrador.sonidos ? "Activados" : "Silenciados"}
          />
        </Campo>

        <Campo rotulo="Volumen">
          <input
            type="range"
            min={0}
            max={1}
            step={0.05}
            value={borrador.volumen}
            disabled={!borrador.sonidos}
            onChange={(e) => cambiar("volumen", Number(e.currentTarget.value))}
          />
          <span className="cifra">{Math.round(borrador.volumen * 100)} %</span>
        </Campo>

        <Campo
          rotulo="Arrancar con Windows"
          ayuda="Arranca en la bandeja, sin abrir esta ventana."
        >
          <Interruptor
            activo={borrador.arranque_con_windows}
            alCambiar={(v) => cambiar("arranque_con_windows", v)}
            etiqueta={borrador.arranque_con_windows ? "Sí" : "No"}
          />
        </Campo>
      </section>

      <section className="seccion">
        <h2>Privacidad</h2>
        <p className="porque">
          El historial es lo único que MithFlow deja escrito, en claro y en el
          disco, con todo lo que dijiste. Estas dos opciones existen para que eso
          sea una decisión tuya y no un efecto colateral.
        </p>

        <div className="separado">
          <Campo
            rotulo="Guardar el texto dictado"
            ayuda="Apagado, el historial sigue registrando fecha, palabras y tiempos —así los gráficos y las métricas no se pierden— pero no lo que dijiste."
          >
            <Interruptor
              activo={borrador.guardar_texto}
              alCambiar={(v) => cambiar("guardar_texto", v)}
              etiqueta={borrador.guardar_texto ? "Sí, guardar el texto" : "Sólo métricas"}
            />
          </Campo>

          <Campo
            rotulo="Borrar el historial"
            ayuda="Borra el archivo entero, sin vuelta atrás. Las métricas y los gráficos quedan en cero."
          >
            {confirmandoBorrado ? (
              <>
                <span className="aclaracion ojo">¿Seguro? No hay deshacer.</span>
                <button type="button" className="boton peligro" onClick={borrarTodoElHistorial}>
                  Sí, borrar todo
                </button>
                <button
                  type="button"
                  className="boton plano"
                  onClick={() => setConfirmandoBorrado(false)}
                >
                  Cancelar
                </button>
              </>
            ) : (
              <button
                type="button"
                className="boton peligro"
                onClick={() => setConfirmandoBorrado(true)}
              >
                Borrar el historial
              </button>
            )}
          </Campo>
        </div>
      </section>

      {/* Última y aparte: es la única acción de esta pantalla que se lleva
          puesta la aplicación entera, y no puede quedar al lado de un
          interruptor que se toca todos los días. */}
      <section className="seccion zona-peligro">
        <h2>Desinstalar MithFlow</h2>
        <p className="porque">
          El desinstalador de Windows saca el programa y <strong>deja los datos
          donde están</strong>: los modelos y el historial quedarían ocupando
          disco sin que ninguna aplicación los nombre, y el historial es texto
          plano con todo lo que dictaste. Desde acá se decide qué pasa con ellos
          antes de sacar el programa.
        </p>

        {!resumen ? (
          <div className="separado acciones">
            <button
              type="button"
              className="boton peligro"
              disabled={midiendo}
              onClick={medirDesinstalacion}
            >
              {midiendo ? "Midiendo…" : "Desinstalar MithFlow…"}
            </button>
            <span className="aclaracion">
              Todavía no borra nada: primero te digo qué se va y cuánto pesa.
            </span>
          </div>
        ) : (
          <div className="separado">
            <ul className="que-se-borra">
              <li>
                <div>
                  <div className="que">El programa</div>
                  <div className="ruta">
                    {resumen.programa_ruta ?? "no encontré la instalación"}
                  </div>
                </div>
                <span className="peso">
                  {resumen.programa_ruta ? pesoEnBytes(resumen.programa_bytes) : "—"}
                </span>
              </li>
              <li className={conservarDatos ? "se-conserva" : undefined}>
                <div>
                  <div className="que">
                    Los modelos ({numero(resumen.modelos_cantidad)}{" "}
                    {resumen.modelos_cantidad === 1 ? "archivo" : "archivos"})
                  </div>
                  <div className="ruta">{resumen.modelos_ruta}</div>
                </div>
                <span className="peso">{pesoEnBytes(resumen.modelos_bytes)}</span>
              </li>
              <li className={conservarDatos ? "se-conserva" : undefined}>
                <div>
                  <div className="que">
                    Los ajustes y el historial ({numero(resumen.dictados)}{" "}
                    {resumen.dictados === 1 ? "dictado" : "dictados"})
                  </div>
                  <div className="ruta">{resumen.datos_ruta}</div>
                </div>
                <span className="peso">{pesoEnBytes(resumen.datos_bytes)}</span>
              </li>
            </ul>

            {resumen.hay_desinstalador ? (
              <>
                <Campo
                  rotulo="Qué hago con tus datos"
                  ayuda="Conservarlos deja los modelos y el historial en el disco tal como están: si reinstalás MithFlow no tenés que volver a bajar nada."
                >
                  <Segmentado
                    opciones={[
                      { valor: "borrar", etiqueta: "Borrar todo" },
                      {
                        valor: "conservar",
                        etiqueta: "Conservar modelos e historial",
                      },
                    ]}
                    elegida={conservarDatos ? "conservar" : "borrar"}
                    alElegir={(elegida) => setConservarDatos(elegida === "conservar")}
                  />
                </Campo>

                <p className="porque separado">
                  Al confirmar borro lo que elegiste y abro el desinstalador de
                  Windows, que pide su propia confirmación final. Para entonces
                  los datos ya no están: si cancelás ahí, se cancela sacar el
                  programa, no el borrado.
                </p>

                <div className="separado acciones">
                  <span className="aclaracion ojo">
                    {conservarDatos
                      ? "¿Seguro? Se saca el programa; los modelos y el historial se quedan."
                      : `¿Seguro? Se borran ${pesoEnBytes(
                          resumen.modelos_bytes + resumen.datos_bytes,
                        )} de datos. No hay deshacer.`}
                  </span>
                  <span className="espacio" />
                  <button
                    type="button"
                    className="boton plano"
                    disabled={desinstalando}
                    onClick={() => setResumen(null)}
                  >
                    Cancelar
                  </button>
                  <button
                    type="button"
                    className="boton peligro"
                    disabled={desinstalando}
                    onClick={confirmarDesinstalacion}
                  >
                    {desinstalando
                      ? "Desinstalando…"
                      : conservarDatos
                        ? "Sí, desinstalar y conservar"
                        : "Sí, borrar todo y desinstalar"}
                  </button>
                </div>
              </>
            ) : (
              // Sin desinstalador no hay nada que elegir: no se ofrece la
              // opción de borrar los datos, porque borrarlos sin poder sacar el
              // programa es el peor de los desenlaces.
              <div className="separado acciones">
                <span className="aclaracion ojo">
                  {resumen.motivo_sin_desinstalador}
                </span>
                <span className="espacio" />
                <button
                  type="button"
                  className="boton plano"
                  onClick={() => setResumen(null)}
                >
                  Cerrar
                </button>
              </div>
            )}
          </div>
        )}
      </section>

      {sucio && (
        <div className="barra-guardar">
          <span className="mensaje">Hay cambios sin guardar.</span>
          <span className="espacio" />
          <button
            type="button"
            className="boton plano"
            disabled={guardando}
            onClick={() => setBorrador(guardados)}
          >
            Descartar
          </button>
          <button
            type="button"
            className="boton primario"
            disabled={guardando}
            onClick={guardar}
          >
            {guardando ? "Guardando…" : "Guardar cambios"}
          </button>
        </div>
      )}
    </>
  );
}
