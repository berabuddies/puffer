import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

const host =
  (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env
    ?.TAURI_DEV_HOST ?? "127.0.0.1";

export default defineConfig({
  plugins: [
    svelte({
      compilerOptions: {
        compatibility: {
          componentApi: 4
        }
      }
    })
  ],
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
  optimizeDeps: {
    entries: ["index.html"]
  },
  server: {
    host,
    port: 1420,
    strictPort: true,
    hmr: host !== "127.0.0.1"
      ? {
          protocol: "ws",
          host,
          port: 1421
        }
      : undefined
  },
  preview: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true
  }
});
