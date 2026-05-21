import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();
const ONE_PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==";

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

function timelineDiff(): Record<string, unknown> {
  return {
    id: "timeline-diff",
    source: "agent",
    commandLabel: "apply_patch",
    statusText: "1 file changed",
    unstagedDiffstat: "",
    stagedDiffstat: "",
    patch: [
      "diff --git a/src/timeline.rs b/src/timeline.rs",
      "--- a/src/timeline.rs",
      "+++ b/src/timeline.rs",
      "@@ -1,2 +1,2 @@",
      " pub fn status() -> &'static str {",
      "-    \"old\"",
      "+    \"new\"",
      " }"
    ].join("\n"),
    patchExcerpt: ""
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

test("Diff timeline items stay visible in the chat activity stream", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-diff-timeline",
        displayName: "Diff timeline",
        title: "Diff timeline",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "user_message",
            id: "diff-timeline-user",
            text: "Patch the file.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "diff_snapshot",
            id: "diff-timeline-snapshot",
            snapshot: timelineDiff(),
            createdAtMs: baseTime - 40_000
          },
          {
            kind: "assistant_message",
            id: "diff-timeline-assistant",
            text: "Patched the file.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /Diff timeline/);
  await expect(page.getByText("Patched the file.")).toBeVisible();
  await expect(page.getByText("Updated 1 diff")).toBeVisible();
});

test("sub-agent tool activity renders spawn_agent as a sub-agent action", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-subagent-activity",
        displayName: "Subagent activity",
        title: "Subagent activity",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "user_message",
            id: "subagent-user",
            text: "Use a helper agent.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "tool_call",
            id: "subagent-spawn",
            toolId: "spawn_agent",
            status: "success",
            inputText: JSON.stringify({
              agent_type: "worker",
              model: "gpt-5.4",
              reasoning_effort: "high",
              message: "Audit provider picker"
            }),
            outputText: JSON.stringify({
              status: "completed",
              receiverThreadIds: ["agent-thread-1"]
            }),
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "assistant_message",
            id: "subagent-assistant",
            text: "The helper agent finished.",
            createdAtMs: baseTime - 40_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /Subagent activity/);
  const activity = page.getByRole("button", { name: /Agent activity/ });
  await expect(activity).toContainText("Used subagent");
  await expect(activity).not.toContainText("Used 1 tool");

  await activity.click();
  const action = page.locator(".activity-action").filter({ hasText: "Spawn sub-agent" });
  await expect(action).toBeVisible();
  await expect(action).toContainText("worker");
  await expect(action).toContainText("gpt-5.4");
  await expect(action).not.toContainText("spawn_agent");

  await action.click();
  const panel = page.locator(".activity-panel").filter({ hasText: "Spawn sub-agent" });
  await expect(panel).toContainText("Audit provider picker");
});

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
  const mainTabs = page.locator(".pf-agent-tabs");
  await expect(mainTabs.getByRole("button", { name: "Chat", exact: true })).toHaveAttribute("type", "button");
  await expect(mainTabs.getByRole("button", { name: "Chat", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect(mainTabs.getByRole("button", { name: /Diff/ })).toHaveAttribute("aria-pressed", "false");

  await mainTabs.getByRole("button", { name: /Diff/ }).click();
  await expect(mainTabs.getByRole("button", { name: "Chat", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(mainTabs.getByRole("button", { name: /Diff/ })).toHaveAttribute("aria-pressed", "true");

  await expect(page.getByText("src/agent.rs").first()).toBeVisible();
  await expect(page.getByText("new needle agent note")).toBeVisible();

  const diffSubtabs = page.locator(".diff-subtabs");
  await expect(diffSubtabs.getByRole("button", { name: /Agent/ }).first()).toHaveAttribute("type", "button");
  await expect(diffSubtabs.getByRole("button", { name: /Agent/ }).first()).toHaveAttribute("aria-pressed", "true");
  await expect(diffSubtabs.getByRole("button", { name: /Git/ }).first()).toHaveAttribute("aria-pressed", "false");

  await diffSubtabs.getByRole("button").nth(1).click();
  await expect(diffSubtabs.getByRole("button", { name: /Agent/ }).first()).toHaveAttribute("aria-pressed", "false");
  await expect(diffSubtabs.getByRole("button", { name: /Git/ }).first()).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByText("src/main.rs").first()).toBeVisible();
  await expect(page.getByText("new git line")).toBeVisible();

  await diffSubtabs.getByRole("button").nth(2).click();
  await expect(diffSubtabs.getByRole("button", { name: /Agent\/Git/ })).toHaveAttribute("aria-pressed", "true");
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

test("opening a side panel tab as the main tab closes the duplicate side panel", async ({ page }) => {
  const daemon = agentDetailDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /Agent detail/);
  const tabs = page.locator(".pf-agent-tabs");
  const diffTab = tabs.getByRole("button", { name: /Diff/ });

  await diffTab.click({ modifiers: ["Meta"] });
  await expect(page.locator(".pf-side-panel")).toBeVisible();
  await expect(page.locator(".pf-side-head")).toContainText("Diff");

  await diffTab.click();

  await expect(page.locator(".pf-agent-detail-body")).toContainText("new needle agent note");
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

test("idle sessions with dirty repositories stay idle in detail", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-dirty-idle",
        displayName: "Dirty idle session",
        title: "Dirty idle session",
        cwd: "/tmp/puffer-dirty",
        folderPath: "/tmp/puffer-dirty",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        activityStatus: "idle",
        repoStatus: {
          isClean: false,
          statusLines: [" M src/main.rs"]
        }
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const row = page.locator(".pf-sidebar-agent-row").filter({ hasText: "Dirty idle session" });
  await expect(row.locator(".state")).toHaveText("idle");
  await row.getByRole("button", { name: /Dirty idle session/ }).click();

  const status = page.locator(".pf-agent-status-pill");
  await expect(status).toHaveAttribute("data-status", "idle");
  await expect(status).toContainText("Idle");
  await expect(page.locator(".pf-composer textarea")).toBeEnabled();
});

test("Review sessions keep review state in the agent detail orb", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-review-detail",
        displayName: "Review detail",
        title: "Review detail",
        cwd: "/tmp/puffer-review",
        folderPath: "/tmp/puffer-review",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        activityStatus: "review"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^Review detail\b/);
  const status = page.locator(".pf-agent-status-pill");
  await expect(status).toHaveAttribute("data-status", "review");
  await expect(status).toContainText("Ready to review");
  await expect(page.locator(".pf-agent-detail .pf-puffer").first()).toHaveAttribute("data-state", "review");
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

test("MCP browser actions render recorded browser frames in activity details", async ({ page }) => {
  const browserInput = {
    arguments: {
      url: "https://example.com",
      tabId: "tab-1"
    }
  };
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-mcp-browser-action",
        displayName: "MCP browser action",
        title: "MCP browser action",
        cwd: "/tmp/puffer-browser-action",
        folderPath: "/tmp/puffer-browser-action",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "user_message",
            id: "mcp-browser-user",
            text: "Open example.com.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "tool_call",
            id: "mcp-browser-tool",
            toolId: "mcp__browser__open",
            status: "success",
            inputText: JSON.stringify(browserInput),
            inputJson: browserInput,
            outputText: "Done.",
            createdAtMs: baseTime - 40_000
          },
          {
            kind: "assistant_message",
            id: "mcp-browser-assistant",
            text: "Opened example.com.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setBrowserRecording("session-mcp-browser-action", [
    {
      frameId: "mcp-browser-frame",
      backendSessionId: "session-mcp-browser-action:browser:tab-1",
      rootSessionId: "session-mcp-browser-action",
      tabId: "tab-1",
      url: "https://example.com",
      title: "Example Domain",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 960,
      height: 720,
      recordedAtMs: baseTime - 35_000
    }
  ]);
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^MCP browser action\b/);
  await page.getByRole("button", { name: /Agent activity/ }).click();
  await page.getByRole("button", { name: /Browser · Open https:\/\/example\.com/ }).click();

  const recording = page.locator(".pf-browser-recording-render");
  await expect(recording.getByRole("img", { name: "Example Domain" })).toBeVisible();
  await expect(recording).toContainText("Example Domain");
  await expect(page.getByText("No browser frames recorded for this action yet.")).toHaveCount(0);
});

test("browser activity details reload recordings when selecting another action", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-browser-multi-action",
        displayName: "Browser multi action",
        title: "Browser multi action",
        cwd: "/tmp/puffer-browser-multi",
        folderPath: "/tmp/puffer-browser-multi",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "user_message",
            id: "browser-multi-user",
            text: "Open two pages.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "tool_call",
            id: "browser-alpha-tool",
            toolId: "Browser",
            status: "success",
            inputText: JSON.stringify({
              action: "open",
              url: "https://alpha.example",
              tabId: "tab-alpha"
            }),
            inputJson: {
              action: "open",
              url: "https://alpha.example",
              tabId: "tab-alpha"
            },
            outputText: "Done.",
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "tool_call",
            id: "browser-beta-tool",
            toolId: "Browser",
            status: "success",
            inputText: JSON.stringify({
              action: "open",
              url: "https://beta.example",
              tabId: "tab-beta"
            }),
            inputJson: {
              action: "open",
              url: "https://beta.example",
              tabId: "tab-beta"
            },
            outputText: "Done.",
            createdAtMs: baseTime - 40_000
          },
          {
            kind: "assistant_message",
            id: "browser-multi-assistant",
            text: "Opened both pages.",
            createdAtMs: baseTime - 35_000
          }
        ]
      }
    ]
  });
  daemon.setBrowserRecording("session-browser-multi-action", [
    {
      frameId: "browser-alpha-frame",
      backendSessionId: "session-browser-multi-action:browser:tab-alpha",
      rootSessionId: "session-browser-multi-action",
      tabId: "tab-alpha",
      url: "https://alpha.example",
      title: "Alpha Domain",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 960,
      height: 720,
      recordedAtMs: baseTime - 44_000
    },
    {
      frameId: "browser-beta-frame",
      backendSessionId: "session-browser-multi-action:browser:tab-beta",
      rootSessionId: "session-browser-multi-action",
      tabId: "tab-beta",
      url: "https://beta.example",
      title: "Beta Domain",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 960,
      height: 720,
      recordedAtMs: baseTime - 39_000
    }
  ]);
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^Browser multi action\b/);
  await page.getByRole("button", { name: /Agent activity/ }).click();
  await page.getByRole("button", { name: /Browser Open https:\/\/alpha\.example/ }).click();
  const recording = page.locator(".pf-browser-recording-render");
  await expect(recording.getByRole("img", { name: "Alpha Domain" })).toBeVisible();

  await page.getByRole("button", { name: /Browser Open https:\/\/beta\.example/ }).click();

  await expect(recording.getByRole("img", { name: "Beta Domain" })).toBeVisible();
  await expect(recording.getByRole("img", { name: "Alpha Domain" })).toHaveCount(0);
});

test("browser activity details match recordings by URL when tab ids are absent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-browser-url-actions",
        displayName: "Browser URL actions",
        title: "Browser URL actions",
        cwd: "/tmp/puffer-browser-url-actions",
        folderPath: "/tmp/puffer-browser-url-actions",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "user_message",
            id: "browser-url-user",
            text: "Open two URL-only pages.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "tool_call",
            id: "browser-url-alpha-tool",
            toolId: "Browser",
            status: "success",
            inputText: JSON.stringify({
              action: "open",
              url: "https://alpha-url.example"
            }),
            inputJson: {
              action: "open",
              url: "https://alpha-url.example"
            },
            outputText: "Done.",
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "tool_call",
            id: "browser-url-beta-tool",
            toolId: "Browser",
            status: "success",
            inputText: JSON.stringify({
              action: "open",
              url: "https://beta-url.example"
            }),
            inputJson: {
              action: "open",
              url: "https://beta-url.example"
            },
            outputText: "Done.",
            createdAtMs: baseTime - 40_000
          },
          {
            kind: "assistant_message",
            id: "browser-url-assistant",
            text: "Opened both URL-only pages.",
            createdAtMs: baseTime - 35_000
          }
        ]
      }
    ]
  });
  daemon.setBrowserRecording("session-browser-url-actions", [
    {
      frameId: "browser-url-alpha-frame",
      backendSessionId: "session-browser-url-actions:browser:tab-1",
      rootSessionId: "session-browser-url-actions",
      tabId: "tab-1",
      url: "https://alpha-url.example",
      title: "Alpha URL Domain",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 960,
      height: 720,
      recordedAtMs: baseTime - 44_000
    },
    {
      frameId: "browser-url-beta-frame",
      backendSessionId: "session-browser-url-actions:browser:tab-2",
      rootSessionId: "session-browser-url-actions",
      tabId: "tab-2",
      url: "https://beta-url.example",
      title: "Beta URL Domain",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 960,
      height: 720,
      recordedAtMs: baseTime - 39_000
    }
  ]);
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^Browser URL actions\b/);
  await page.getByRole("button", { name: /Agent activity/ }).click();
  await page.getByRole("button", { name: /Browser Open https:\/\/alpha-url\.example/ }).click();
  const recording = page.locator(".pf-browser-recording-render");
  await expect(recording.getByRole("img", { name: "Alpha URL Domain" })).toBeVisible();
  await expect(recording.getByRole("img", { name: "Beta URL Domain" })).toHaveCount(0);

  await page.getByRole("button", { name: /Browser Open https:\/\/beta-url\.example/ }).click();
  await expect(recording.getByRole("img", { name: "Beta URL Domain" })).toBeVisible();
  await expect(recording.getByRole("img", { name: "Alpha URL Domain" })).toHaveCount(0);
});

test("browser activity details keep query-specific URL recordings scoped", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-browser-query-actions",
        displayName: "Browser query actions",
        title: "Browser query actions",
        cwd: "/tmp/puffer-browser-query-actions",
        folderPath: "/tmp/puffer-browser-query-actions",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "user_message",
            id: "browser-query-user",
            text: "Open two searches.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "tool_call",
            id: "browser-query-alpha-tool",
            toolId: "Browser",
            status: "success",
            inputText: JSON.stringify({
              action: "open",
              url: "https://search.example/results?q=alpha"
            }),
            inputJson: {
              action: "open",
              url: "https://search.example/results?q=alpha"
            },
            outputText: "Done.",
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "tool_call",
            id: "browser-query-beta-tool",
            toolId: "Browser",
            status: "success",
            inputText: JSON.stringify({
              action: "open",
              url: "https://search.example/results?q=beta"
            }),
            inputJson: {
              action: "open",
              url: "https://search.example/results?q=beta"
            },
            outputText: "Done.",
            createdAtMs: baseTime - 40_000
          },
          {
            kind: "assistant_message",
            id: "browser-query-assistant",
            text: "Opened both searches.",
            createdAtMs: baseTime - 35_000
          }
        ]
      }
    ]
  });
  daemon.setBrowserRecording("session-browser-query-actions", [
    {
      frameId: "browser-query-alpha-frame",
      backendSessionId: "session-browser-query-actions:browser:tab-1",
      rootSessionId: "session-browser-query-actions",
      tabId: "tab-1",
      url: "https://search.example/results?q=alpha",
      title: "Alpha Search",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 960,
      height: 720,
      recordedAtMs: baseTime - 44_000
    },
    {
      frameId: "browser-query-beta-frame",
      backendSessionId: "session-browser-query-actions:browser:tab-2",
      rootSessionId: "session-browser-query-actions",
      tabId: "tab-2",
      url: "https://search.example/results?q=beta",
      title: "Beta Search",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 960,
      height: 720,
      recordedAtMs: baseTime - 39_000
    }
  ]);
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^Browser query actions\b/);
  await page.getByRole("button", { name: /Agent activity/ }).click();
  await page.getByRole("button", { name: /Browser Open https:\/\/search\.example\/results\?q=alpha/ }).click();
  const recording = page.locator(".pf-browser-recording-render");
  await expect(recording.getByRole("img", { name: "Alpha Search" })).toBeVisible();
  await expect(recording.getByRole("img", { name: "Beta Search" })).toHaveCount(0);

  await page.getByRole("button", { name: /Browser Open https:\/\/search\.example\/results\?q=beta/ }).click();
  await expect(recording.getByRole("img", { name: "Beta Search" })).toBeVisible();
  await expect(recording.getByRole("img", { name: "Alpha Search" })).toHaveCount(0);
});

test("browser activity details keep redirected URL-only recordings scoped", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-browser-redirect-actions",
        displayName: "Browser redirect actions",
        title: "Browser redirect actions",
        cwd: "/tmp/puffer-browser-redirect-actions",
        folderPath: "/tmp/puffer-browser-redirect-actions",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: [
          {
            kind: "user_message",
            id: "browser-redirect-user",
            text: "Open two redirected URL-only pages.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "tool_call",
            id: "browser-redirect-alpha-tool",
            toolId: "Browser",
            status: "success",
            inputText: JSON.stringify({
              action: "open",
              url: "https://alpha-redirect.example"
            }),
            inputJson: {
              action: "open",
              url: "https://alpha-redirect.example"
            },
            outputText: "Done.",
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "tool_call",
            id: "browser-redirect-beta-tool",
            toolId: "Browser",
            status: "success",
            inputText: JSON.stringify({
              action: "open",
              url: "https://beta-redirect.example"
            }),
            inputJson: {
              action: "open",
              url: "https://beta-redirect.example"
            },
            outputText: "Done.",
            createdAtMs: baseTime - 40_000
          },
          {
            kind: "assistant_message",
            id: "browser-redirect-assistant",
            text: "Opened both redirected URL-only pages.",
            createdAtMs: baseTime - 35_000
          }
        ]
      }
    ]
  });
  daemon.setBrowserRecording("session-browser-redirect-actions", [
    {
      frameId: "browser-redirect-alpha-frame",
      backendSessionId: "session-browser-redirect-actions:browser:tab-1",
      rootSessionId: "session-browser-redirect-actions",
      tabId: "tab-1",
      url: "https://www.alpha-redirect.example/final",
      title: "Alpha Redirect Domain",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 960,
      height: 720,
      recordedAtMs: baseTime - 44_000
    },
    {
      frameId: "browser-redirect-beta-frame",
      backendSessionId: "session-browser-redirect-actions:browser:tab-2",
      rootSessionId: "session-browser-redirect-actions",
      tabId: "tab-2",
      url: "https://www.beta-redirect.example/final",
      title: "Beta Redirect Domain",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 960,
      height: 720,
      recordedAtMs: baseTime - 39_000
    }
  ]);
  await daemon.install(page);
  await daemon.open(page);

  await openAgent(page, /^Browser redirect actions\b/);
  await page.getByRole("button", { name: /Agent activity/ }).click();
  await page.getByRole("button", { name: /Browser Open https:\/\/alpha-redirect\.example/ }).click();
  const recording = page.locator(".pf-browser-recording-render");
  await expect(recording.getByRole("img", { name: "Alpha Redirect Domain" })).toBeVisible();
  await expect(recording.getByRole("img", { name: "Beta Redirect Domain" })).toHaveCount(0);

  await page.getByRole("button", { name: /Browser Open https:\/\/beta-redirect\.example/ }).click();
  await expect(recording.getByRole("img", { name: "Beta Redirect Domain" })).toBeVisible();
  await expect(recording.getByRole("img", { name: "Alpha Redirect Domain" })).toHaveCount(0);
});
