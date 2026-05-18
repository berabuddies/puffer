import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "tests",
  timeout: 120_000,
  expect: {
    timeout: 10_000
  },
  webServer: {
    command: "npm run dev",
    url: "http://127.0.0.1:1420/?skipOnboarding",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000
  },
  use: {
    baseURL: "http://127.0.0.1:1420",
    headless: true
  }
});
