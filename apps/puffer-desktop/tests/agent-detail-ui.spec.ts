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
