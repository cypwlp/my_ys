import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));

// Tauri drives the dev server itself; these settings are what `tauri.conf.json`
// expects, so the two files must not drift.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { "@": resolve(here, "src") },
  },
  build: {
    outDir: "dist",
    sourcemap: true,
    target: "es2022",
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
