import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

const host =
  (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env
    ?.TAURI_DEV_HOST ?? "127.0.0.1";

// Build-time provenance for the forensic watermark. Resolved once at config
// load; failures (no git, shallow CI checkout) degrade to "unknown" rather than
// breaking the build.
function gitShortHash(): string {
  try {
    return execSync("git rev-parse --short HEAD", { encoding: "utf8" }).trim();
  } catch {
    return "unknown";
  }
}

function appVersion(): string {
  try {
    return JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8")).version;
  } catch {
    return "0.0.0";
  }
}

export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(appVersion()),
    __COMMIT_HASH__: JSON.stringify(gitShortHash())
  },
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
