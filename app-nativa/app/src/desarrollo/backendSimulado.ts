/**
 * Un backend de mentira para mirar la interfaz en un navegador común.
 *
 * # Qué es y qué NO es
 *
 * Es una herramienta de desarrollo: sirve para diseñar y sacar capturas de las
 * tres vistas sin levantar la app de Tauri (y sin el micrófono ni el modelo de
 * 1,5 GB que eso implica). **No prueba el backend**: eso lo hacen los tests de
 * Rust. Si un dato de acá y uno de `comandos.rs` no coinciden, el que manda es
 * el de Rust.
 *
 * # Por qué no llega a producción
 *
 * Este archivo sólo lo importa `mock.html`, que es una entrada aparte de Vite.
 * `vite build` compila únicamente `index.html`, así que nada de esto entra al
 * `dist/` que empaqueta Tauri. La comprobación es mecánica: `dist/` no puede
 * contener la cadena `MITHFLOW_SIMULADO`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type {
  Ajustes,
  Catalogo,
  Dictado,
  EstadoDto,
  Metricas,
  PaginaHistorial,
  PerfilDto,
} from "../api";

/** Marca para poder verificar que este módulo NO está en el build de producción. */
export const MARCA = "MITHFLOW_SIMULADO";

/** Escenarios que se pueden pedir por la barra de direcciones (`?escenario=`). */
export type Escenario = "normal" | "primer-arranque" | "grabando";

const FRASES = [
  "Necesito preparar el informe de cierre para el cliente antes del viernes.",
  "Recordame llamar a Jaé por el tema del presupuesto de la notebook nueva.",
  "El dashboard viejo promediaba la latencia sobre todo el historial y daba un número tres veces peor que el real.",
  "Che, pasame el link de la reunión de mañana a las diez.",
  "Anotá que hay que revisar las políticas de acceso antes de subir esto a producción.",
  "La transcripción local anda bárbaro, no hace falta mandar nada a la nube.",
  "Agendá la visita al depósito para el martes a primera hora.",
  "Falta cerrar el presupuesto de julio y mandarlo por correo.",
];

/** El crudo de una frase: lo mismo con muletillas y un tartamudeo. */
function crudoDe(texto: string): string {
  const palabras = texto.split(" ");
  const primera = palabras[0].toLowerCase();
  return `eh, ${primera} ${primera} ${palabras.slice(1).join(" ")}`.replace(
    " que ",
    " este, que ",
  );
}

/** Un `ts` del historial: hora local sin zona, como el que escribe el backend. */
function marcaDeTiempo(fecha: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return (
    `${fecha.getFullYear()}-${p(fecha.getMonth() + 1)}-${p(fecha.getDate())}` +
    `T${p(fecha.getHours())}:${p(fecha.getMinutes())}:${p(fecha.getSeconds())}`
  );
}

/**
 * Un historial verosímil de los últimos 45 días.
 *
 * Determinista a propósito (generador congruencial con semilla fija): dos
 * capturas de la misma vista tienen que dar el mismo gráfico, o comparar antes y
 * después no diría nada.
 */
function historialDeMentira(): Dictado[] {
  let semilla = 20260721;
  const azar = () => {
    semilla = (semilla * 1103515245 + 12345) % 2147483648;
    return semilla / 2147483648;
  };

  const entradas: Dictado[] = [];
  const hoy = new Date();
  for (let diasAtras = 44; diasAtras >= 0; diasAtras--) {
    // Fines de semana flojos, y algún día sin dictar: un gráfico parejo no
    // muestra si el eje respeta los días vacíos.
    const dia = new Date(hoy);
    dia.setDate(hoy.getDate() - diasAtras);
    const finDeSemana = dia.getDay() === 0 || dia.getDay() === 6;
    // Hoy siempre tiene dictados: es el día que sostiene el delta "+N hoy", y
    // una captura sin ese número no muestra la vista completa.
    if (diasAtras > 0 && azar() < (finDeSemana ? 0.6 : 0.12)) continue;

    const cuantos = 1 + Math.floor(azar() * (finDeSemana ? 3 : 9));
    for (let i = 0; i < cuantos; i++) {
      const hora = 8 + Math.floor(azar() * 12);
      const momento = new Date(dia);
      momento.setHours(hora, Math.floor(azar() * 60), Math.floor(azar() * 60), 0);
      // Un dictado del futuro no existe: hoy sólo cuentan las horas ya pasadas.
      if (momento > hoy) continue;

      const texto = FRASES[Math.floor(azar() * FRASES.length)];
      const palabras = texto.split(/\s+/).length;
      // Los primeros días son de la época del LLM: es el caso que hace visible
      // la corrección de la latencia por modo.
      const viejo = diasAtras > 38;
      const conCrudo = azar() < 0.45;
      entradas.push({
        ts: marcaDeTiempo(momento),
        audio_s: Number((palabras / 2.4 + azar()).toFixed(2)),
        transcribe_s: Number((0.35 + azar() * 0.3).toFixed(2)),
        cleanup_s: viejo ? Number((2.4 + azar() * 3).toFixed(2)) : 0,
        words: palabras,
        cleaned: true,
        mode: viejo ? "llm" : "fast",
        raw: conCrudo ? crudoDe(texto) : "",
        final: texto,
      });
    }
  }
  // El historial real es un archivo al que se le agrega al final: siempre está
  // en orden cronológico, y la vista se apoya en eso para dar vuelta la lista.
  entradas.sort((a, b) => a.ts.localeCompare(b.ts));
  return entradas;
}

/** Las mismas cuentas que `metricas::calcular`, para que la vista tenga qué mostrar. */
function calcularMetricas(entradas: Dictado[]): Metricas {
  const DIAS = 30;
  const PPM_TIPEANDO = 40;
  const hoy = new Date();
  const clave = (f: Date) =>
    `${f.getFullYear()}-${String(f.getMonth() + 1).padStart(2, "0")}-${String(
      f.getDate(),
    ).padStart(2, "0")}`;
  const hoyClave = clave(hoy);

  const porDia = Array.from({ length: DIAS }, (_, i) => {
    const f = new Date(hoy);
    f.setDate(hoy.getDate() - (DIAS - 1 - i));
    return { dia: clave(f), palabras: 0, dictados: 0 };
  });
  const indicePorDia = new Map(porDia.map((d, i) => [d.dia, i]));
  const porHora = Array<number>(24).fill(0);
  const porModo = new Map<string, { dictados: number; suma: number }>();

  let palabras = 0;
  let audio = 0;
  let dictadosHoy = 0;
  let palabrasHoy = 0;

  for (const e of entradas) {
    palabras += e.words;
    audio += e.audio_s;
    const acumulado = porModo.get(e.mode) ?? { dictados: 0, suma: 0 };
    acumulado.dictados += 1;
    acumulado.suma += e.transcribe_s + e.cleanup_s;
    porModo.set(e.mode, acumulado);

    const dia = e.ts.slice(0, 10);
    porHora[Number(e.ts.slice(11, 13))] += e.words;
    if (dia === hoyClave) {
      dictadosHoy += 1;
      palabrasHoy += e.words;
    }
    const indice = indicePorDia.get(dia);
    if (indice !== undefined) {
      porDia[indice].palabras += e.words;
      porDia[indice].dictados += 1;
    }
  }

  const minutosHablando = audio / 60;
  const modoActual = entradas.length ? entradas[entradas.length - 1].mode : null;
  const delModo = modoActual ? porModo.get(modoActual) : undefined;

  return {
    total_dictados: entradas.length,
    total_palabras: palabras,
    total_audio_s: audio,
    dictados_hoy: dictadosHoy,
    palabras_hoy: palabrasHoy,
    palabras_por_minuto: minutosHablando > 0 ? palabras / minutosHablando : 0,
    minutos_ahorrados: Math.max(0, palabras / PPM_TIPEANDO - minutosHablando),
    por_dia: porDia,
    por_hora: porHora,
    por_modo: [...porModo.entries()]
      .map(([modo, a]) => ({ modo, dictados: a.dictados, latencia_s: a.suma / a.dictados }))
      .sort((a, b) => b.dictados - a.dictados),
    modo_actual: modoActual,
    latencia_s: delModo ? delModo.suma / delModo.dictados : null,
    dictados_de_otros_modos: delModo ? entradas.length - delModo.dictados : 0,
  };
}

const AJUSTES: Ajustes = {
  tecla: "F9",
  modelo: "auto",
  sonidos: true,
  volumen: 0.15,
  arranque_con_windows: false,
  vocabulario: "MithData, MithFlow, Jaé, Puerto Madryn",
  muletillas: ["eh", "este", "o sea", "digamos", "viste", "nada"],
  modo_limpieza: "rapido",
  limite_grabacion_s: 180,
  guardar_texto: true,
};

const PERFIL: PerfilDto = {
  ram_total_gb: 63.9,
  gpu_nombre: "NVIDIA GeForce RTX 3080",
  gpu_clase: "dedicada",
  gpu_dedicada: true,
  backend: "vulkan",
  factor_tiempo_real: 43.2,
  modelo_recomendado: "F16",
  etiqueta_recomendado: "large-v3-turbo F16",
  error: null,
};

/**
 * Instala el backend simulado. Devuelve una función para disparar eventos a
 * mano desde la consola del navegador.
 */
export function instalarBackendSimulado(escenario: Escenario) {
  const entradas = escenario === "primer-arranque" ? [] : historialDeMentira();
  const ajustes: Ajustes = { ...AJUSTES };
  const descargados = new Set(escenario === "primer-arranque" ? [] : ["F16", "Q4_K_M"]);

  const estado: EstadoDto =
    escenario === "grabando"
      ? { estado: "grabando", etiqueta: "Grabando", detalle: null, pausado: false }
      : escenario === "primer-arranque"
        ? {
            // Copia exacta de lo que publica `Estado::SinModelo`. NO es
            // "error": el primer arranque no tiene nada roto, y si este
            // simulado mintiera, el próximo que mire la pantalla de bienvenida
            // en el navegador vería el bug que ya no existe.
            estado: "sin-modelo",
            etiqueta: "Falta el modelo",
            detalle:
              "todavía no hay ningún modelo descargado. Bajá uno desde Ajustes para poder dictar.",
            pausado: false,
          }
        : { estado: "listo", etiqueta: "Listo", detalle: null, pausado: false };

  const catalogo = (): Catalogo => ({
    teclas: ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "Insert", "ScrollLock", "Pause"],
    modos_limpieza: ["rapido", "ninguno"],
    modelos: [
      { clave: "F16", etiqueta: "large-v3-turbo F16", megabytes: 1550, descargado: descargados.has("F16"), ram_minima_gb: 12 },
      { clave: "Q5_K_M", etiqueta: "large-v3-turbo Q5_K_M", megabytes: 591, descargado: descargados.has("Q5_K_M"), ram_minima_gb: 6 },
      { clave: "Q4_K_M", etiqueta: "large-v3-turbo Q4_K_M", megabytes: 511, descargado: descargados.has("Q4_K_M"), ram_minima_gb: 0 },
    ],
    modelo_de_perfilado: "Q4_K_M",
    limite_grabacion_minimo: 15,
    limite_grabacion_maximo: 600,
    version: "1.0.0",
  });

  /** Simula una descarga: unos tramos de progreso y el final. */
  function simularDescarga(clave: string) {
    const modelo = catalogo().modelos.find((m) => m.clave === clave);
    const total = (modelo?.megabytes ?? 500) * 1024 * 1024;
    let bytes = 0;
    const paso = total / 12;
    const reloj = setInterval(() => {
      bytes = Math.min(total, bytes + paso);
      const terminado = bytes >= total;
      if (terminado) {
        clearInterval(reloj);
        descargados.add(clave);
      }
      void emit("progreso-descarga", {
        modelo: clave,
        bytes,
        total,
        porcentaje: (bytes / total) * 100,
        terminado,
        error: null,
      });
    }, 350);
  }

  mockIPC(
    (cmd, args) => {
      switch (cmd) {
        case "leer_estado":
          return estado;
        case "leer_catalogo":
          return catalogo();
        case "leer_ajustes":
          return ajustes;
        case "escribir_ajustes": {
          const nuevos = (args as { nuevos: Ajustes }).nuevos;
          Object.assign(ajustes, nuevos);
          return ajustes;
        }
        case "leer_metricas":
          return calcularMetricas(entradas);
        case "leer_historial": {
          const { limite, desplazamiento, buscar } = args as {
            limite: number;
            desplazamiento: number;
            buscar: string | null;
          };
          const aguja = buscar?.trim().toLowerCase();
          const filtradas = [...entradas]
            .reverse()
            .filter((e) => !aguja || e.final.toLowerCase().includes(aguja));
          const pagina = filtradas.slice(desplazamiento, desplazamiento + limite);
          const respuesta: PaginaHistorial = {
            entradas: pagina,
            total: filtradas.length,
            hay_mas: desplazamiento + pagina.length < filtradas.length,
          };
          return respuesta;
        }
        case "borrar_historial":
          entradas.length = 0;
          return null;
        case "perfilar_hardware":
          // El perfilado real tarda ~20 s (carga el modelo, compila los shaders
          // y mide cuatro pasadas). Acá se acorta para poder iterar, pero no a
          // cero: la pantalla de "midiendo" tiene que existir de verdad.
          setTimeout(() => void emit("perfilado-listo", PERFIL), 6000);
          return null;
        case "descargar_modelo":
          simularDescarga((args as { clave: string }).clave);
          return null;
        case "pausar":
        case "reanudar":
        case "alternar_pausa":
          return null;
        default:
          throw new Error(`el backend simulado no conoce el comando '${cmd}'`);
      }
    },
    { shouldMockEvents: true },
  );

  /** Agrega un dictado y lo anuncia, como haría el motor al terminar uno. */
  const dictar = (texto: string): Dictado => {
    const palabras = texto.split(/\s+/).filter(Boolean).length;
    const entrada: Dictado = {
      ts: marcaDeTiempo(new Date()),
      audio_s: Number((palabras / 2.4).toFixed(2)),
      transcribe_s: 0.48,
      cleanup_s: 0,
      words: palabras,
      cleaned: true,
      mode: "fast",
      raw: crudoDe(texto),
      final: texto,
    };
    entradas.push(entrada);
    void emit("dictado-nuevo", entrada);
    return entrada;
  };

  return { emit, dictar, entradas };
}
