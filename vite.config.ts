import { defineConfig } from "vite";
export default defineConfig({
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "chrome105", outDir: "dist", sourcemap: false, chunkSizeWarningLimit: 4000 },
});
