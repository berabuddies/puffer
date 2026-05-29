import { defineConfig } from "@playwright/test";

const nodeExecutable = JSON.stringify(process.execPath);
const shouldReuseExistingServer = !process.env.CI && !process.env.CODEX_CI;
// Worktree-local dev port. The main checkout's `vite dev` runs on 1466; using
// 1466 here would make Playwright reuse THAT stale main-checkout server (which
// lacks the FakeDaemon env below), producing misleading 404 / connect hangs.
// REVERT to 1466 before merging fix/momo-chat-bugs back to feat/momo-desktop.
const PORT = 1478;

export default defineConfig({
  testDir: "tests",
  // v1 specs target the legacy Puffer UI and only run in apps/puffer-desktop.
  testIgnore: ["v1/**"],
  timeout: 120_000,
  expect: {
    timeout: 10_000
  },
  webServer: {
    command: `${nodeExecutable} ./node_modules/vite/bin/vite.js --host localhost --port ${PORT}`,
    url: `http://localhost:${PORT}/?skipOnboarding`,
    reuseExistingServer: shouldReuseExistingServer,
    timeout: 120_000,
    // wsClient.ts reads VITE_PUFFER_WS_URL at module load; this points the
    // Momo frontend at the in-process FakeDaemon (tests/support/fakeDaemon.ts,
    // default URL ws://127.0.0.1:17777/ws) instead of the real Tauri host
    // (ws://127.0.0.1:1431/ws), which isn't running under `playwright test`.
    env: { VITE_PUFFER_WS_URL: "ws://127.0.0.1:17777/ws", VITE_USE_MOCK_WALLET: "true" }
  },
  use: {
    baseURL: `http://localhost:${PORT}`,
    headless: true
  }
});
