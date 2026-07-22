/**
 * Entrada de la ventanita de grabación.
 *
 * Sin `StrictMode` a propósito, al revés que `main.tsx`: en desarrollo monta y
 * desmonta cada componente dos veces, y acá eso significa suscribirse a
 * `nivel-audio` dos veces y correr la historia del medidor de a dos posiciones
 * por cuadro. En el dashboard el doble montaje no se nota; en un medidor a
 * veinticinco cuadros por segundo, sí.
 */
import ReactDOM from "react-dom/client";
import Superpuesta from "./Superpuesta";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<Superpuesta />);
