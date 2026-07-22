import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

const enEstaCarpeta = (archivo: string) =>
  fileURLToPath(new URL(archivo, import.meta.url));

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  build: {
    rollupOptions: {
      // Dos entradas y no una: la ventanita de grabación es su propia página
      // para no arrastrar el dashboard —métricas, gráficos, historial— a una
      // ventana que sólo dibuja unas barras.
      //
      // `mock.html` queda deliberadamente afuera: es la vista de desarrollo con
      // el backend simulado, y nada de `src/desarrollo/` puede llegar al `dist/`
      // que empaqueta Tauri.
      input: {
        principal: enEstaCarpeta("index.html"),
        superpuesta: enEstaCarpeta("superpuesta.html"),
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
