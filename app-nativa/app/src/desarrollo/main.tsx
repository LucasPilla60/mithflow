/**
 * Entrada de desarrollo: la misma app, con el backend simulado enganchado.
 *
 * La usa `mock.html`, que Vite sirve en `npm run dev` y NO compila en
 * `npm run build` (el `input` por defecto de Rollup es sólo `index.html`).
 * Escenarios: `mock.html?escenario=normal|primer-arranque|grabando|sin-instalar`.
 */
import React from "react";
import ReactDOM from "react-dom/client";
import App from "../App";
import { instalarBackendSimulado, type Escenario } from "./backendSimulado";

const ESCENARIOS: Escenario[] = [
  "normal",
  "primer-arranque",
  "grabando",
  "sin-instalar",
];

const pedido = new URLSearchParams(window.location.search).get("escenario");
const escenario: Escenario = ESCENARIOS.includes(pedido as Escenario)
  ? (pedido as Escenario)
  : "normal";

// Antes de montar: el primer `invoke` sale del `useEffect` de `App`, y para
// entonces el simulador ya tiene que estar puesto.
const simulado = instalarBackendSimulado(escenario);

// Para disparar eventos a mano desde la consola del navegador y ver la
// actualización en vivo sin dictar:
//   mithflow.emit("estado-cambiado", { estado: "grabando", etiqueta: "Grabando",
//                                      detalle: null, pausado: false })
//   mithflow.dictar("una frase de prueba")
(window as unknown as { mithflow: typeof simulado }).mithflow = simulado;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
