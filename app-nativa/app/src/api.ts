/**
 * El contrato con el backend, en un solo archivo.
 *
 * Los nombres de comandos y eventos son cadenas: escribir mal uno compila,
 * arranca y no hace nada. Concentrarlos acá no lo evita, pero deja un único
 * lugar donde comparar contra `comandos.rs` y `eventos.rs`.
 *
 * **Cada evento tiene su comando espejo.** Los eventos cuentan novedades y sólo
 * llegan a quien esté escuchando; un dictado hecho con la ventana cerrada no le
 * llega a nadie. Por eso las vistas piden el presente al montarse Y se suscriben
 * a las novedades: sin la primera mitad, abrir la ventana mostraría una lista
 * vacía.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/* ------------------------------------------------------------------ tipos */

/**
 * Las claves de `estado::Estado::clave()`. `"sin-modelo"` es su propia clave y
 * no un `"error"` con otro texto a propósito: es lo que decide el color, y
 * pintar de rojo un primer arranque que va bien es mentirle al usuario.
 */
export type ClaveEstado =
  | "cargando"
  | "listo"
  | "grabando"
  | "transcribiendo"
  | "sin-modelo"
  | "error";

/** Espejo de `estado::EstadoDto`. */
export interface EstadoDto {
  estado: ClaveEstado;
  etiqueta: string;
  detalle: string | null;
  pausado: boolean;
}

/**
 * Una entrada del historial (`history::Entry`). El campo se llama `final` en el
 * archivo, que en Rust es palabra reservada y por eso allá se llama `final_text`.
 */
export interface Dictado {
  ts: string;
  audio_s: number;
  transcribe_s: number;
  cleanup_s: number;
  words: number;
  cleaned: boolean;
  mode: string;
  raw: string;
  final: string;
}

/** Espejo de `comandos::PaginaHistorial`. */
export interface PaginaHistorial {
  entradas: Dictado[];
  /** Total **después** de buscar: es el número de "mostrando 25 de 312". */
  total: number;
  hay_mas: boolean;
}

/** Espejo de `metricas::DiaConPalabras`. */
export interface DiaConPalabras {
  dia: string;
  palabras: number;
  dictados: number;
}

/** Espejo de `metricas::ResumenModo`. */
export interface ResumenModo {
  modo: string;
  dictados: number;
  latencia_s: number;
}

/** Espejo de `metricas::Metricas`: todo ya calculado del lado de Rust. */
export interface Metricas {
  total_dictados: number;
  total_palabras: number;
  total_audio_s: number;
  dictados_hoy: number;
  palabras_hoy: number;
  palabras_por_minuto: number;
  minutos_ahorrados: number;
  por_dia: DiaConPalabras[];
  por_hora: number[];
  por_modo: ResumenModo[];
  modo_actual: string | null;
  /** Latencia del modo vigente, **no** el promedio de todas las épocas. */
  latencia_s: number | null;
  dictados_de_otros_modos: number;
}

/** Espejo de `ajustes::Ajustes`. */
export interface Ajustes {
  tecla: string;
  modelo: string;
  sonidos: boolean;
  volumen: number;
  arranque_con_windows: boolean;
  vocabulario: string;
  muletillas: string[];
  modo_limpieza: string;
  limite_grabacion_s: number;
  guardar_texto: boolean;
  /** La ventanita que aparece al grabar. */
  indicador: boolean;
  /** Una de `Catalogo.posiciones_indicador`. */
  indicador_posicion: string;
}

/** Espejo de `comandos::ModeloDto`. */
export interface ModeloDto {
  clave: string;
  etiqueta: string;
  megabytes: number;
  descargado: boolean;
  ram_minima_gb: number;
}

/** Espejo de `comandos::Catalogo`: las listas salen del backend, no de acá. */
export interface Catalogo {
  teclas: string[];
  modos_limpieza: string[];
  modelos: ModeloDto[];
  modelo_de_perfilado: string;
  limite_grabacion_minimo: number;
  limite_grabacion_maximo: number;
  /** Dónde puede aparecer la ventanita de grabación. La primera es la de fábrica. */
  posiciones_indicador: string[];
  version: string;
}

/** Espejo de `comandos::PerfilDto`. */
export interface PerfilDto {
  ram_total_gb: number;
  gpu_nombre: string | null;
  gpu_clase: string | null;
  gpu_dedicada: boolean;
  backend: string;
  factor_tiempo_real: number;
  modelo_recomendado: string;
  etiqueta_recomendado: string;
  error: string | null;
}

/** Espejo de `eventos::ProgresoDescarga`. */
export interface ProgresoDescarga {
  modelo: string;
  bytes: number;
  total: number;
  porcentaje: number;
  terminado: boolean;
  error: string | null;
}

/**
 * Espejo de `desinstalar::ResumenDesinstalacion`: qué se va a borrar y cuánto
 * pesa, medido sobre este disco antes de que el usuario confirme.
 */
export interface ResumenDesinstalacion {
  /** `null` cuando no hay instalación que medir (ver `hay_desinstalador`). */
  programa_ruta: string | null;
  programa_bytes: number;
  modelos_ruta: string;
  modelos_bytes: number;
  modelos_cantidad: number;
  datos_ruta: string;
  datos_bytes: number;
  dictados: number;
  hay_desinstalador: boolean;
  /** Por qué no lo hay. `null` cuando sí está. */
  motivo_sin_desinstalador: string | null;
}

/** Espejo de `eventos::Aviso`. */
export interface Aviso {
  texto: string;
  nivel: "info" | "error";
}

/** Valor de `Ajustes.modelo` que significa "el que diga el perfilado". */
export const MODELO_AUTOMATICO = "auto";

/* --------------------------------------------------------------- comandos */

export const leerEstado = () => invoke<EstadoDto>("leer_estado");
export const leerAjustes = () => invoke<Ajustes>("leer_ajustes");
export const leerCatalogo = () => invoke<Catalogo>("leer_catalogo");
export const leerMetricas = () => invoke<Metricas>("leer_metricas");

/** Devuelve lo que EFECTIVAMENTE se guardó, que puede venir recortado. */
export const escribirAjustes = (nuevos: Ajustes) =>
  invoke<Ajustes>("escribir_ajustes", { nuevos });

export const leerHistorial = (
  limite: number,
  desplazamiento: number,
  buscar?: string,
) =>
  invoke<PaginaHistorial>("leer_historial", {
    limite,
    desplazamiento,
    // La búsqueda se hace en Rust sobre TODO el historial, no sobre la página
    // ya cargada: es lo que el usuario espera de un buscador.
    buscar: buscar?.trim() ? buscar : null,
  });

export const borrarHistorial = () => invoke<void>("borrar_historial");
export const pausar = () => invoke<void>("pausar");
export const reanudar = () => invoke<void>("reanudar");
export const alternarPausa = () => invoke<void>("alternar_pausa");

/** Vuelve enseguida: el resultado llega por `perfilado-listo` (~20 s). */
export const perfilarHardware = () => invoke<void>("perfilar_hardware");

/** Vuelve enseguida: el avance llega por `progreso-descarga`. */
export const descargarModelo = (clave: string) =>
  invoke<void>("descargar_modelo", { clave });

/** Mide lo que se va a borrar. No borra nada: es lo que se muestra antes. */
export const leerResumenDesinstalacion = () =>
  invoke<ResumenDesinstalacion>("resumen_desinstalacion");

/**
 * Borra lo que corresponda, lanza el desinstalador de Windows y cierra la app.
 *
 * `conservar` deja los modelos y el historial donde están. El parámetro es una
 * sola palabra a propósito: Tauri traduce `snake_case` a `camelCase` entre Rust
 * y JavaScript, y un nombre de una palabra no puede quedar del lado equivocado
 * de esa conversión.
 */
export const desinstalar = (conservar: boolean) =>
  invoke<void>("desinstalar", { conservar });

/* ---------------------------------------------------------------- eventos */

export const alCambiarEstado = (f: (e: EstadoDto) => void): Promise<UnlistenFn> =>
  listen<EstadoDto>("estado-cambiado", (ev) => f(ev.payload));

export const alDictadoNuevo = (f: (d: Dictado) => void): Promise<UnlistenFn> =>
  listen<Dictado>("dictado-nuevo", (ev) => f(ev.payload));

export const alAviso = (f: (a: Aviso) => void): Promise<UnlistenFn> =>
  listen<Aviso>("aviso", (ev) => f(ev.payload));

export const alProgresoDescarga = (
  f: (p: ProgresoDescarga) => void,
): Promise<UnlistenFn> =>
  listen<ProgresoDescarga>("progreso-descarga", (ev) => f(ev.payload));

export const alPerfiladoListo = (f: (p: PerfilDto) => void): Promise<UnlistenFn> =>
  listen<PerfilDto>("perfilado-listo", (ev) => f(ev.payload));

/**
 * Cancela un conjunto de suscripciones. `listen` devuelve una promesa, así que
 * hay que esperarla para poder cancelar: soltarla sin esperar deja al oyente
 * vivo después de desmontar el componente.
 */
export function cancelarTodas(suscripciones: Promise<UnlistenFn>[]): void {
  suscripciones.forEach((p) => p.then((cancelar) => cancelar()).catch(() => {}));
}

/**
 * El mensaje de un error del backend. `invoke` rechaza con la cadena que
 * devolvió el `Result::Err` de Rust, pero un fallo de IPC rechaza con un
 * `Error`: mostrar `[object Object]` sería peor que no mostrar nada.
 */
export function mensajeDeError(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return "Ocurrió un error inesperado.";
}
