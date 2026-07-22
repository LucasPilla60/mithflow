/**
 * El estado de las actualizaciones, en un solo lugar.
 *
 * Lo miran dos pantallas —el aviso del encabezado y la sección de Ajustes— y
 * tienen que decir lo mismo: si cada una consultara por su cuenta, apretar
 * "Buscar actualizaciones" en Ajustes dejaría el aviso de arriba mostrando el
 * resultado anterior. Por eso el estado vive acá arriba y baja como props.
 *
 * # Por qué se pide el presente Y se escucha el evento
 *
 * La consulta automática corre a los quince segundos de arrancar, con la app
 * normalmente en la bandeja y sin ninguna ventana escuchando. `leerActualizacion`
 * devuelve lo que quedó de esa consulta; el evento sólo agrega el caso de la
 * ventana que ya estaba abierta cuando llegó la respuesta.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import {
  alActualizacionDisponible,
  buscarActualizacion,
  cancelarTodas,
  instalarActualizacion,
  leerActualizacion,
  mensajeDeError,
  type EstadoActualizacion,
} from "./api";

export interface ControlActualizacion {
  /** `null` hasta que el primer `leerActualizacion` conteste. */
  estado: EstadoActualizacion | null;
  buscando: boolean;
  instalando: boolean;
  /** Hay versión nueva y el usuario todavía no descartó el aviso. */
  hayQueAvisar: boolean;
  buscar: () => void;
  instalar: () => void;
  descartarAviso: () => void;
}

export function useActualizacion(
  avisar: (texto: string, nivel: "info" | "error") => void,
): ControlActualizacion {
  const [estado, setEstado] = useState<EstadoActualizacion | null>(null);
  const [buscando, setBuscando] = useState(false);
  const [instalando, setInstalando] = useState(false);
  // Descartar el aviso NO se guarda en el disco a propósito: dura lo que dura
  // esta ventana. Una versión nueva que se ignoró para siempre es peor que un
  // aviso que reaparece al abrir de nuevo, y son dos clics de distancia.
  const [descartado, setDescartado] = useState(false);
  const montado = useRef(true);

  useEffect(() => {
    montado.current = true;

    leerActualizacion()
      .then((e) => montado.current && setEstado(e))
      // Que no se pueda leer el estado de las actualizaciones no es algo que
      // valga interrumpir a nadie: la app dicta igual.
      .catch(() => {});

    const suscripciones = [
      alActualizacionDisponible((e) => {
        if (!montado.current) return;
        setEstado(e);
        setDescartado(false);
      }),
    ];

    return () => {
      montado.current = false;
      cancelarTodas(suscripciones);
    };
  }, []);

  const buscar = useCallback(() => {
    setBuscando(true);
    buscarActualizacion()
      .then((e) => {
        if (!montado.current) return;
        setEstado(e);
        setDescartado(false);
      })
      .catch((e) => montado.current && avisar(mensajeDeError(e), "error"))
      .finally(() => montado.current && setBuscando(false));
  }, [avisar]);

  /**
   * En el camino feliz esta promesa nunca resuelve: el backend lanza el
   * instalador y termina el proceso. Por eso `instalando` no se apaga al
   * terminar —el botón se queda en "Actualizando…" hasta que la ventana
   * desaparece— y sólo un rechazo lo devuelve a su estado.
   */
  const instalar = useCallback(() => {
    setInstalando(true);
    instalarActualizacion().catch((e) => {
      if (!montado.current) return;
      avisar(mensajeDeError(e), "error");
      setInstalando(false);
    });
  }, [avisar]);

  return {
    estado,
    buscando,
    instalando,
    hayQueAvisar: estado?.clave === "disponible" && !descartado,
    buscar,
    instalar,
    descartarAviso: () => setDescartado(true),
  };
}
