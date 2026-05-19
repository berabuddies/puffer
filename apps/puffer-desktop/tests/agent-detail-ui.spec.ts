import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

async function openAgent(page: Page, name: RegExp): Promise<void> {
  await page.locator(".pf-sidebar-agents-list").getByRole("button", { name }).click();
}

function gitDiff(): Record<string, unknown> {
  return {
    id: "git-diff",
    source: "git",
    title: "Working tree",
    command: "git diff",
    status: "1 file changed",
    unstagedDiffstat: "src/main.rs | 2 +-",
    stagedDiffstat: "",
    patch: [
      "diff --git a/src/main.rs b/src/main.rs",
      "--- a/src/main.rs",
      "+++ b/src/main.rs",
      "@@ -1,2 +1,2 @@",
      " fn main() {",
      "-    println!(\"old git line\");",
      "+    println!(\"new git line\");",
      " }"
    ].join("\n")
  };
}

function agentDiff(): Record<string, unknown> {
  return {
    files: [
      {
        path: "src/agent.rs",
        latestKind: "Replace",
        editCount: 2,
        latestSummary: "-old agent note\n+new needle agent note"
      }
    ],
    entries: [
      {
        callId: "call-agent-edit",
        toolId: "apply_patch",
        kind: "replace",
        path: "src/agent.rs",
        success: true,
        summary: "-old agent note\n+new needle agent note"
      }
    ]
  };
}

function agentDetailDaemon(): FakeDaemon {
  return new FakeDaemon({
    sessions: [
      {
        sessionId: "session-agent-detail",
        displayName: "Agent detail",
        title: "Agent detail",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "user_message",
            id: "detail-user",
            text: "Show me the transcript.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "assistant_message",
            id: "detail-assistant",
            text: "The needle is visible in the transcript.",
            createdAtMs: baseTime - 40_000
          }
        ],
        latestDiff: gitDiff(),
        agentDiff: agentDiff(),
        divergence: {
          agentOnly: ["src/agent.rs"],
          gitOnly: ["src/main.rs"],
          agentTotal: 1,
          gitTotal: 1
        }
      }
    ]
  });
}

test("Diff tab reconciles agent edits with git changes", async ({ page }) => {
  const daemon = agentDetailDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /Agent detail/);
  await page.locator(".pf-agent-tabs").getByRole("button", { name: /Diff/ }).click();

  await expect(page.getByText("src/agent.rs").first()).toBeVisible();
  await expect(page.getByText("new needle agent note")).toBeVisible();

  await page.locator(".diff-subtabs").getByRole("button").nth(1).click();
  await expect(page.getByText("src/main.rs").first()).toBeVisible();
  await expect(page.getByText("new git line")).toBeVisible();

  await page.locator(".diff-subtabs").getByRole("button").nth(2).click();
  await expect(page.getByText("Changed-file reconciliation")).toBeVisible();
  const reconciliationCounts = page.locator(".agent-git-counts");
  await expect(reconciliationCounts.getByText("1 agent", { exact: true })).toBeVisible();
  await expect(reconciliationCounts.getByText("1 git", { exact: true })).toBeVisible();
  await expect(reconciliationCounts.getByText("2 drift", { exact: true })).toBeVisible();
  await expect(page.getByText("src/agent.rs")).toBeVisible();
  await expect(page.getByText("src/main.rs")).toBeVisible();
});

test("Agent detail find covers chat plus side panel diff without corrupting text", async ({ page }) => {
  const daemon = agentDetailDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /Agent detail/);
  await expect(page.getByText("The needle is visible in the transcript.")).toBeVisible();

  await page.locator(".pf-agent-tabs").getByRole("button", { name: /Diff/ }).click({
    modifiers: ["Meta"]
  });
  await expect(page.locator(".pf-side-panel")).toBeVisible();
  await expect(page.locator(".pf-side-head")).toContainText("Diff");
  await expect(page.locator(".pf-agent-detail-body")).toContainText("The needle is visible in the transcript.");
  await expect(page.locator(".pf-side-panel")).toContainText("new needle agent note");

  await page.keyboard.press("Control+F");
  const find = page.getByRole("search", { name: "Find in agent view" });
  await expect(find).toBeVisible();
  await find.getByRole("textbox").fill("needle");

  await expect(page.locator("mark.pf-search-mark")).toHaveCount(2);
  await expect(find.locator(".find-count")).toContainText("1 / 2");

  await find.getByRole("button", { name: "Next match" }).click();
  await expect(find.locator(".find-count")).toContainText("2 / 2");

  await find.getByRole("button", { name: "Close find" }).click();
  await expect(page.locator("mark.pf-search-mark")).toHaveCount(0);
  await expect(page.getByText("The needle is visible in the transcript.")).toBeVisible();
  await expect(page.locator(".pf-side-panel")).toContainText("new needle agent note");

  await page.getByRole("button", { name: "Close side page" }).click();
  await expect(page.locator(".pf-side-panel")).toHaveCount(0);
});

test("find query clears when switching sessions", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-find-alpha",
        displayName: "Alpha find",
        title: "Alpha find",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-find-message",
            text: "Needle only belongs to alpha.",
            createdAtMs: baseTime - 50_000
          }
        ]
      },
      {
        sessionId: "session-find-beta",
        displayName: "Beta find",
        title: "Beta find",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-find-message",
            text: "Needle also appears in beta.",
            createdAtMs: baseTime - 110_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^Alpha find\b/);
  await page.keyboard.press("Control+F");
  const find = page.getByRole("search", { name: "Find in agent view" });
  await expect(find).toBeVisible();
  await find.getByRole("textbox").fill("Needle");
  await expect(page.locator("mark.pf-search-mark")).toHaveCount(1);

  await openAgent(page, /^Beta find\b/);

  await expect(find).toHaveCount(0);
  await expect(page.locator("mark.pf-search-mark")).toHaveCount(0);
  await expect(page.locator(".pf-agent-identity")).toContainText("Beta find");
});

test("title edit draft clears when switching sessions", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-title-alpha",
        displayName: "Alpha title",
        title: "Alpha title",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: []
      },
      {
        sessionId: "session-title-beta",
        displayName: "Beta title",
        title: "Beta title",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^Alpha title\b/);
  await page.getByRole("button", { name: "Edit session title" }).click();
  await page.getByLabel("Session title").fill("Unsaved Alpha Draft");

  await openAgent(page, /^Beta title\b/);

  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Session title", exact: true })).toHaveCount(0);
  await expect(page.locator(".pf-agent-identity")).toContainText("Beta title");
  await expect(page.locator(".pf-agent-identity")).not.toContainText("Unsaved Alpha Draft");
});

test("activity expansion state clears when switching sessions", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-activity-alpha",
        displayName: "Alpha activity",
        title: "Alpha activity",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "user_message",
            id: "shared-user-id",
            text: "Inspect the main file.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "tool_call",
            id: "shared-tool-id",
            toolId: "read_file",
            status: "success",
            inputText: JSON.stringify({ path: "/tmp/puffer-alpha/src/main.rs" }),
            outputText: "fn alpha() {}\n",
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "assistant_message",
            id: "shared-assistant-id",
            text: "Alpha file inspected.",
            createdAtMs: baseTime - 40_000
          }
        ]
      },
      {
        sessionId: "session-activity-beta",
        displayName: "Beta activity",
        title: "Beta activity",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        timeline: [
          {
            kind: "user_message",
            id: "shared-user-id",
            text: "Inspect the main file.",
            createdAtMs: baseTime - 110_000
          },
          {
            kind: "tool_call",
            id: "shared-tool-id",
            toolId: "read_file",
            status: "success",
            inputText: JSON.stringify({ path: "/tmp/puffer-beta/src/main.rs" }),
            outputText: "fn beta() {}\n",
            createdAtMs: baseTime - 105_000
          },
          {
            kind: "assistant_message",
            id: "shared-assistant-id",
            text: "Beta file inspected.",
            createdAtMs: baseTime - 100_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^Alpha activity\b/);
  const alphaActivity = page.getByRole("button", { name: /Agent activity/ });
  await expect(alphaActivity).toHaveAttribute("aria-expanded", "false");
  await alphaActivity.click();
  await expect(alphaActivity).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator(".activity-action").filter({ hasText: "/tmp/puffer-alpha/src/main.rs" })).toBeVisible();

  await openAgent(page, /^Beta activity\b/);
  const betaActivity = page.getByRole("button", { name: /Agent activity/ });
  await expect(betaActivity).toHaveAttribute("aria-expanded", "false");
  await expect(page.locator(".activity-action").filter({ hasText: "/tmp/puffer-beta/src/main.rs" })).toHaveCount(0);
});

test("Side panel does not duplicate effectful Browser or Terminal panes", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^Browser regression\b/);
  const tabs = page.locator(".pf-agent-tabs");

  await tabs.getByRole("button", { name: "Browser", exact: true }).click();
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  await page.waitForTimeout(50);
  const browserAgentCount = daemon.requests.filter((request) => request.method === "browser_agent").length;
  await tabs.getByRole("button", { name: "Browser", exact: true }).click({ modifiers: ["Meta"] });
  await expect(page.locator(".pf-side-panel")).toHaveCount(0);
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "browser_agent")).toHaveLength(browserAgentCount);

  await tabs.getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.sessionId === "session-browser");
  await daemon.waitForRequest("pty_replay");
  await page.waitForTimeout(50);
  const terminalAttachCount = daemon.requests.filter((request) =>
    ["pty_list", "pty_open", "pty_focus", "pty_replay"].includes(request.method)
  ).length;
  await tabs.getByRole("button", { name: "Terminal", exact: true }).click({ modifiers: ["Meta"] });
  await expect(page.locator(".pf-side-panel")).toHaveCount(0);
  await page.waitForTimeout(50);
  expect(
    daemon.requests.filter((request) =>
      ["pty_list", "pty_open", "pty_focus", "pty_replay"].includes(request.method)
    )
  ).toHaveLength(terminalAttachCount);
});
