import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

test("opens the Browser tab against a mocked desktop daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();

  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  await expect(page.getByLabel("URL")).toHaveValue("about:blank");
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
  await expect(page.locator(".pf-browser-canvas")).toBeVisible();
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
});

test("sends Browser tab navigation through the daemon bridge", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
  await daemon.waitForRequest("browser_open");

  await page.getByLabel("URL").fill("example.com");
  await page.getByLabel("URL").press("Enter");

  const request = await daemon.waitForRequest("browser_navigate");
  expect(request.params).toMatchObject({
    sessionId: "session-browser:browser:tab-1",
    url: "example.com"
  });
  await expect(page.getByLabel("URL")).toHaveValue("https://example.com");
});

test("renders Browser devtools events from the daemon stream", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
  await daemon.waitForRequest("browser_open");

  await page.getByRole("button", { name: "DevTools" }).click();
  daemon.emit("browser:session-browser:browser:tab-1:devtools", {
    kind: "console",
    level: "log",
    text: "hello from browser fixture"
  });

  await expect(page.getByText("hello from browser fixture")).toBeVisible();
});
