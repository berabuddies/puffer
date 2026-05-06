import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "tests",
  timeout: 120_000,
  expect: {
    timeout: 10_000
  },
  use: {
    headless: true
  }
});
