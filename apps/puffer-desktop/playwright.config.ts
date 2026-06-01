import { defineConfig } from "@playwright/test";

const nodeExecutable = JSON.stringify(process.execPath);
const shouldReuseExistingServer = !process.env.CI && !process.env.CODEX_CI;
const serverPort = Number(process.env.PUFFER_DESKTOP_TEST_PORT ?? "1420");
const baseURL = `http://127.0.0.1:${serverPort}`;

export default defineConfig({
  testDir: "tests",
  timeout: 120_000,
  expect: {
    timeout: 10_000
  },
  webServer: {
    command: `${nodeExecutable} ./node_modules/vite/bin/vite.js --host 127.0.0.1 --port ${serverPort}`,
    url: `${baseURL}/?skipOnboarding`,
    reuseExistingServer: shouldReuseExistingServer,
    timeout: 120_000
  },
  use: {
    baseURL,
    headless: true
  }
});
