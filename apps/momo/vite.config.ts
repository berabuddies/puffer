import { defineConfig, loadEnv } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig(({ mode }) => {
  const proc = (
    globalThis as {
      process?: {
        env?: Record<string, string | undefined>;
        cwd?: () => string;
      };
    }
  ).process;
  const host = proc?.env?.TAURI_DEV_HOST ?? "127.0.0.1";

  // U-card wallet REST backend. backendFetch.ts issues same-origin `/api`
  // requests in dev (the backend's CORS allowlist only admits the packaged
  // tauri://localhost origin, not the dev http://localhost:1466 one), so we
  // forward `/api` here, server-side, to the real backend. Single source of
  // truth: VITE_BACKEND_BASE_URL (same var the prod build hits directly).
  const env = loadEnv(mode, proc?.cwd?.() ?? ".", "VITE_");
  const backendTarget = env.VITE_BACKEND_BASE_URL || "http://127.0.0.1:8080";

  return {
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
    server: {
      host,
      port: 1466,
      strictPort: true,
      proxy: {
        "/api": {
          target: backendTarget,
          changeOrigin: true
        }
      },
      hmr:
        host !== "127.0.0.1"
          ? { protocol: "ws", host, port: 1431 }
          : undefined
    },
    preview: {
      host: "127.0.0.1",
      port: 1466,
      strictPort: true
    }
  };
});
