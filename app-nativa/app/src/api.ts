/**
 * El contrato con el backend, en un solo archivo.
 *
 * Los nombres de comandos y eventos son cadenas: escribir mal uno compila,
 * arranca y no hace nada. Concentrarlos acá no lo evita, pero deja un único
 * lugar donde comparar contra `comandos.rs` y `eventos.rs`.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ClaveEstado =
  | "cargando"
  | "listo"
  | "grabando"
  | "transcribiendo"
  | "error";

export interface EstadoDto {
  estado: ClaveEstado;
  etiqueta: string;
  detalle: string | null;
  pausado: boolean;
}

/** Una entrada del historial. `final` se llama así en el archivo. */
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

export interface Aviso {
  texto: string;
  nivel: "info" | "error";
}

export const leerEstado = () => invoke<EstadoDto>("leer_estado");
export const leerHistorial = (limite: number) =>
  invoke<Dictado[]>("leer_historial", { limite });
export const pausar = () => invoke<void>("pausar");
export const reanudar = () => invoke<void>("reanudar");

export const alCambiarEstado = (f: (e: EstadoDto) => void): Promise<UnlistenFn> =>
  listen<EstadoDto>("estado-cambiado", (ev) => f(ev.payload));

export const alDictadoNuevo = (f: (d: Dictado) => void): Promise<UnlistenFn> =>
  listen<Dictado>("dictado-nuevo", (ev) => f(ev.payload));

export const alAviso = (f: (a: Aviso) => void): Promise<UnlistenFn> =>
  listen<Aviso>("aviso", (ev) => f(ev.payload));
