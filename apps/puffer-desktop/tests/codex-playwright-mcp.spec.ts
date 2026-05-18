import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { expect, test } from "@playwright/test";

const execFileAsync = promisify(execFile);

const codexPlaywrightConfig = [
  "-c",
  'mcp_servers.playwright.command="npx"',
  "-c",
  'mcp_servers.playwright.args=["--yes","@playwright/mcp@latest","--headless"]'
];

test("Codex resolves the built-in Playwright MCP server", async () => {
  test.setTimeout(60_000);
  const { stdout } = await execFileAsync("codex", ["mcp", "list", ...codexPlaywrightConfig], {
    timeout: 60_000
  });

  expect(stdout).toContain("playwright");
  expect(stdout).toContain("npx");
  expect(stdout).toContain("@playwright/mcp");
});

test("Playwright can drive a browser page for agent browser workflows", async ({ page }) => {
  await page.setContent(`
    <main>
      <h1>Corbina browser check</h1>
      <button data-testid="run">Run</button>
      <output data-testid="status">idle</output>
      <script>
        document.querySelector('[data-testid="run"]').addEventListener('click', () => {
          document.querySelector('[data-testid="status"]').textContent = 'ready';
        });
      </script>
    </main>
  `);

  await page.getByTestId("run").click();
  await expect(page.getByTestId("status")).toHaveText("ready");
});
