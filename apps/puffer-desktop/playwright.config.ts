import { defineConfig } from "@playwright/test";

const nodeExecutable = JSON.stringify(process.execPath);
const shouldReuseExistingServer = !process.env.CI && !process.env.CODEX_CI;

export default defineConfig({
  testDir: "tests",
  timeout: 120_000,
  expect: {
    timeout: 10_000
  },
  webServer: {
    command: `${nodeExecutable} ./node_modules/vite/bin/vite.js --host 127.0.0.1 --port 1420`,
    url: "http://127.0.0.1:1420/?skipOnboarding",
    reuseExistingServer: shouldReuseExistingServer,
    timeout: 120_000
  },
  use: {
    baseURL: "http://127.0.0.1:1420",
    headless: true
  }
});
