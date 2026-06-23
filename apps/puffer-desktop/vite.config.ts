import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { defineConfig, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

const workspaceDaemonHandshakeUrl = new URL("../../.puffer/daemon.handshake", import.meta.url);
const viteConfigEnv =
  (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {};
const homeDir = viteConfigEnv.HOME ?? viteConfigEnv.USERPROFILE;
const homeDaemonHandshakePath = homeDir
  ? `${homeDir.replace(/\/$/, "")}/.puffer/daemon.handshake`
  : undefined;
const daemonHandshakeCandidates = [
  viteConfigEnv.PUFFER_DAEMON_HANDSHAKE,
  workspaceDaemonHandshakeUrl,
  homeDaemonHandshakePath
].filter((candidate): candidate is string | URL => Boolean(candidate));

const host =
  viteConfigEnv.TAURI_DEV_HOST ?? "127.0.0.1";

// Build commit, injected once at config load for the corner build badge.
// Degrades to "unknown" if git is unavailable (shallow CI checkout) rather than
// breaking the build.
function gitShortHash(): string {
  try {
    return execSync("git rev-parse --short HEAD", { encoding: "utf8" }).trim();
  } catch {
    return "unknown";
  }
}

function workspaceDaemonHandshake(): string | null {
  for (const candidate of daemonHandshakeCandidates) {
    try {
      const raw = readFileSync(candidate, { encoding: "utf8" }).trim();
      if (raw) return raw;
    } catch {
      // Continue through repo-local, explicit env, and user-level fallbacks.
    }
  }
  return null;
}

function workspaceDaemonHandshakePlugin() {
  return {
    name: "puffer-workspace-daemon-handshake",
    configureServer(server: ViteDevServer) {
      server.middlewares.use("/__puffer/daemon-handshake", (_req, res) => {
        const raw = workspaceDaemonHandshake();
        if (!raw) {
          res.statusCode = 404;
          res.setHeader("content-type", "application/json");
          res.end("{}");
          return;
        }
        res.statusCode = 200;
        res.setHeader("cache-control", "no-store");
        res.setHeader("content-type", "application/json");
        res.end(raw);
      });
    }
  };
}

export default defineConfig({
  define: {
    __COMMIT_HASH__: JSON.stringify(gitShortHash())
  },
  plugins: [
    workspaceDaemonHandshakePlugin(),
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
    watch: {
      ignored: [
        "**/src-tauri/target/**",
        "../../target/**"
      ]
    },
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
