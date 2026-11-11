import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a fixed dev port and leaves the backend to pick its own host.
const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [svelte()],

  // Prevent Vite from obscuring Rust errors
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      // Don't watch the Rust backend from Vite
      ignored: ["**/src-tauri/**"],
    },
  },

  // Produce a static SPA bundle Tauri can load from ../dist
  build: {
    target: "es2021",
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: process.env.TAURI_ENV_DEBUG ? "inline" : false,
  },
});
