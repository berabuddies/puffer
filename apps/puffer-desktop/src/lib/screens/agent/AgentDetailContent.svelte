<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import ConversationView from "./ConversationView.svelte";
  import DiffView from "../../components/DiffView.svelte";
  import BrowserPane from "./BrowserPane.svelte";
  import FilesPane from "./FilesPane.svelte";
  import TerminalPane from "./TerminalPane.svelte";
  import type {
    PermissionTimelineItem,
    DiffSnapshot,
    SessionDetail,
    SessionListItem,
    SettingsSnapshot,
    TimelineItem,
    UserQuestionTimelineItem
  } from "../../types";
  import type { AgentState } from "../../shell/tweaks";
  import type { AgentTurnOptions } from "../../api/desktop";

  type Tab = "chat" | "diff" | "terminal" | "files" | "browser";
  type DiffSubTab = "agent" | "git" | "divergence";
  type SubmitMessageResult = boolean | void | Promise<boolean | void>;
  type FileOpenTarget = { path: string; line: number | null; requestId: number };

  type Props = {
    tab: Tab;
    session: SessionListItem | null;
    sessionDetail: SessionDetail | null;
    timeline: TimelineItem[];
    pendingPermissions: PermissionTimelineItem[];
    pendingQuestions: UserQuestionTimelineItem[];
    resolvingPermissionIds?: string[];
    resolvingQuestionIds?: string[];
    loading: boolean;
    displayName: string;
    pufferState: AgentState;
    projectCwd: string;
    turnRunning: boolean;
    turnCancelable: boolean;
    turnStartedAtMs: number | null;
    turnThinking: boolean;
    turnStatusHint: string | null;
    settingsSnapshot?: SettingsSnapshot | null;
    backendConnected?: boolean;
    userDisplayName?: string;
    onSubmitMessage: (message: string, options?: AgentTurnOptions) => SubmitMessageResult;
    onResolvePermission: (permissionId: string, choice: string) => void;
    onResolveUserQuestion: (
      questionId: string,
      answers: Record<string, string | string[]>,
      annotations?: Record<string, Record<string, string>>
    ) => void;
    onCancelTurn?: () => void;
    onOpenFileLink?: (path: string, line?: number | null) => void;
    onDraftChange?: (hasDraft: boolean) => void;
    fileToOpen?: FileOpenTarget | null;
  };

  let {
    tab,
    session,
    sessionDetail,
    timeline,
    pendingPermissions,
    pendingQuestions,
    resolvingPermissionIds = [],
    resolvingQuestionIds = [],
    loading,
    displayName,
    pufferState,
    projectCwd,
    turnRunning,
    turnCancelable,
    turnStartedAtMs,
    turnThinking,
    turnStatusHint,
    settingsSnapshot = null,
    backendConnected = true,
    userDisplayName = "Otter",
    onSubmitMessage,
    onResolvePermission,
    onResolveUserQuestion,
    onCancelTurn,
    onOpenFileLink,
    onDraftChange,
    fileToOpen = null
  }: Props = $props();

  let diffTab = $state<DiffSubTab>("agent");
  let agentDiff = $derived(sessionDetail?.agentDiff ?? { files: [], entries: [] });
  let agentDiffSnapshot = $derived(agentDiffToSnapshot(agentDiff));
  let divergence = $derived(
    sessionDetail?.divergence ?? { agentOnly: [], gitOnly: [], agentTotal: 0, gitTotal: 0 }
  );
  let divergenceCount = $derived(divergence.agentOnly.length + divergence.gitOnly.length);

  function agentDiffToSnapshot(diff: SessionDetail["agentDiff"]): DiffSnapshot | null {
    if (diff.files.length === 0) return null;

    const patch = diff.files.map(agentFilePatch).join("\n");
    return {
      id: "agent-diff",
      source: "session_history",
      title: "Agent edits",
      command: "agent edits",
      status: `${diff.files.length} files touched`,
      unstagedDiffstat: "",
      stagedDiffstat: "",
      patch
    };
  }

  function agentFilePatch(file: SessionDetail["agentDiff"]["files"][number]): string {
    const kind = file.latestKind.toLowerCase();
    const rawLines = file.latestSummary.split("\n").filter((line) => line.length > 0);
    const bodyLines = normalizeAgentSummaryLines(rawLines, kind, file.path);
    const oldCount = Math.max(0, bodyLines.filter((line) => !line.startsWith("+")).length);
    const newCount = Math.max(0, bodyLines.filter((line) => !line.startsWith("-")).length);
    const oldPath = kind === "write" || kind === "add" ? "/dev/null" : `a/${file.path}`;
    const newPath = kind === "remove" || kind === "delete" ? "/dev/null" : `b/${file.path}`;
    const oldRange = oldCount === 0 ? "0,0" : `1,${oldCount}`;
    const newRange = newCount === 0 ? "0,0" : `1,${newCount}`;

    return [
      `diff --git a/${file.path} b/${file.path}`,
      `--- ${oldPath}`,
      `+++ ${newPath}`,
      `@@ -${oldRange} +${newRange} @@ ${file.latestKind}${file.editCount > 1 ? ` x${file.editCount}` : ""}`,
      ...bodyLines
    ].join("\n");
  }

  function normalizeAgentSummaryLines(lines: string[], kind: string, path: string): string[] {
    if (lines.length === 0) {
      return kind === "remove" || kind === "delete" ? [`-${path}`] : [`+${kind}`];
    }

    return lines.map((line) => {
      if (kind === "write" || kind === "add") return line.startsWith("+") ? line : `+${line}`;
      if (kind === "remove" || kind === "delete") return line.startsWith("-") ? line : `-${line}`;
      if (line.startsWith("+") || line.startsWith("-") || line.startsWith(" ")) return line;
      return ` ${line}`;
    });
  }
</script>

<div class="pf-agent-detail-content">
  {#if tab === "chat"}
    <ConversationView
      session={session}
      agentState={pufferState}
      timeline={timeline}
      pendingPermissions={pendingPermissions}
      pendingQuestions={pendingQuestions}
      resolvingPermissionIds={resolvingPermissionIds}
      resolvingQuestionIds={resolvingQuestionIds}
      loading={loading}
      turnRunning={turnRunning}
      turnCancelable={turnCancelable}
      turnStartedAtMs={turnStartedAtMs}
      turnThinking={turnThinking}
      turnStatusHint={turnStatusHint}
      settingsSnapshot={settingsSnapshot}
      {backendConnected}
      {userDisplayName}
      onSubmitMessage={onSubmitMessage}
      onResolvePermission={onResolvePermission}
      onResolveUserQuestion={onResolveUserQuestion}
      onCancelTurn={onCancelTurn}
      onOpenFileLink={onOpenFileLink}
      onDraftChange={onDraftChange}
    />
  {:else if tab === "diff"}
    <div class="diff-subtabs" role="group" aria-label="Diff sources">
      <button
        type="button"
        class="diff-subtab"
        class:on={diffTab === "agent"}
        aria-pressed={diffTab === "agent"}
        onclick={() => (diffTab = "agent")}
      >
        <Icon name="sparkles" size={11} />Agent
        {#if agentDiff.files.length > 0}
          <span class="pf-agent-tab-badge">{agentDiff.files.length}</span>
        {/if}
      </button>
      <button
        type="button"
        class="diff-subtab"
        class:on={diffTab === "git"}
        aria-pressed={diffTab === "git"}
        onclick={() => (diffTab = "git")}
      >
        <Icon name="git" size={11} />Git
        {#if divergence.gitTotal > 0}
          <span class="pf-agent-tab-badge">{divergence.gitTotal}</span>
        {/if}
      </button>
      <button
        type="button"
        class="diff-subtab"
        class:on={diffTab === "divergence"}
        aria-pressed={diffTab === "divergence"}
        onclick={() => (diffTab = "divergence")}
        title={divergenceCount > 0 ? "Agent and git disagree on which files changed" : "Agent and git agree"}
      >
        <Icon name="bolt" size={11} />Agent/Git
        {#if divergenceCount > 0}
          <span class="pf-agent-tab-badge warn">{divergenceCount}</span>
        {/if}
      </button>
    </div>

    {#if diffTab === "agent"}
      {#if agentDiffSnapshot}
        <div class="diff-wrap">
          <DiffView diff={agentDiffSnapshot} />
        </div>
      {:else}
        <div class="pane-empty">
          <Icon name="sparkles" size={20} color="var(--muted-foreground)" />
          <div class="title">No agent edits yet</div>
          <div class="sub">
            Once the agent writes or replaces a file, the per-edit summary lands here,
            independent of git.
          </div>
        </div>
      {/if}
    {:else if diffTab === "git"}
      {#if sessionDetail?.latestDiff}
        <div class="diff-wrap">
          <DiffView diff={sessionDetail.latestDiff} />
        </div>
      {:else}
        <div class="pane-empty">
          <Icon name="git" size={20} color="var(--muted-foreground)" />
          <div class="title">No git changes</div>
          <div class="sub">The session has no working-tree changes against HEAD.</div>
        </div>
      {/if}
    {:else}
      <div class="diff-wrap agent-git-pane">
        {#if divergenceCount === 0}
          <div class="agent-git-empty">
            <Icon name="check" size={20} color="var(--diff-add-fg)" />
            <div>
              <div class="title">Agent and git agree</div>
              <div class="sub">
                Every file the agent edited is represented in git diff, and nothing else has changed on disk.
              </div>
            </div>
          </div>
        {:else}
          <header class="agent-git-head">
            <div>
              <div class="eyebrow">Agent/Git diff</div>
              <h3>Changed-file reconciliation</h3>
              <p>Compare what the agent reports touching against the current working tree.</p>
            </div>
            <div class="agent-git-counts">
              <span>{divergence.agentTotal} agent</span>
              <span>{divergence.gitTotal} git</span>
              <span class="warn">{divergenceCount} drift</span>
            </div>
          </header>
          <div class="agent-git-grid">
            <section class="agent-git-card">
              <header>
                <Icon name="sparkles" size={13} />
                <span>Agent edited, not in git</span>
                <strong>{divergence.agentOnly.length}</strong>
              </header>
              <p>The agent transcript contains edits for these paths, but the working tree diff does not.</p>
              <div class="reconcile-list">
                {#if divergence.agentOnly.length === 0}
                  <div class="reconcile-empty">No agent-only files.</div>
                {:else}
                  {#each divergence.agentOnly as path (path)}
                    <div class="reconcile-row">
                      <span class="file-dot agent"></span>
                      <span class="mono" title={path}>{path}</span>
                    </div>
                  {/each}
                {/if}
              </div>
            </section>
            <section class="agent-git-card">
              <header>
                <Icon name="git" size={13} />
                <span>Changed on disk, no agent edit</span>
                <strong>{divergence.gitOnly.length}</strong>
              </header>
              <p>Git sees these paths as changed, but no agent edit event touched them.</p>
              <div class="reconcile-list">
                {#if divergence.gitOnly.length === 0}
                  <div class="reconcile-empty">No git-only files.</div>
                {:else}
                  {#each divergence.gitOnly as path (path)}
                    <div class="reconcile-row">
                      <span class="file-dot git"></span>
                      <span class="mono" title={path}>{path}</span>
                    </div>
                  {/each}
                {/if}
              </div>
            </section>
          </div>
        {/if}
      </div>
    {/if}
  {:else if tab === "terminal"}
    <TerminalPane cwd={projectCwd} sessionId={session?.id ?? "preview"} />
  {:else if tab === "files"}
    <FilesPane
      cwd={projectCwd}
      sessionId={session?.id ?? "preview"}
      openPath={fileToOpen?.path ?? null}
      openLine={fileToOpen?.line ?? null}
      openRequestId={fileToOpen?.requestId ?? null}
    />
  {:else if tab === "browser"}
    <BrowserPane sessionId={session?.id ?? "preview"} />
  {/if}
</div>

<style>
  .pf-agent-detail-content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .diff-wrap {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .pane-empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 40px;
    color: var(--muted-foreground);
    text-align: center;
  }

  .pane-empty .title { font-size: 14px; font-weight: 600; color: var(--foreground); }
  .pane-empty .sub { font-size: 12.5px; max-width: 360px; line-height: 1.55; }

  .diff-subtabs {
    display: flex;
    gap: 4px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
  }

  .diff-subtab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border: 1px solid transparent;
    border-radius: 999px;
    background: transparent;
    color: var(--muted-foreground);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
    transition: color 100ms, border-color 100ms, background 100ms;
  }

  .diff-subtab:hover { color: var(--foreground); }
  .diff-subtab.on {
    color: var(--foreground);
    background: var(--background);
    border-color: var(--border);
  }

  .pf-agent-tab-badge {
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 1px 5px;
    border-radius: 3px;
    background: oklch(0.7 0.16 40);
    color: white;
    margin-left: 2px;
  }

  .pf-agent-tab-badge.warn {
    background: color-mix(in oklab, oklch(0.62 0.22 25) 18%, var(--background));
    color: oklch(0.55 0.2 30);
    border: 1px solid color-mix(in oklab, oklch(0.62 0.22 25) 35%, var(--border));
  }

  .agent-git-pane {
    --diff-add-fg: oklch(0.42 0.13 145);
    --diff-add-marker: oklch(0.55 0.16 145);
    --diff-del-fg: oklch(0.48 0.18 25);
    --diff-del-marker: oklch(0.62 0.2 25);
    padding: 14px 16px;
    background: var(--background);
  }

  .agent-git-head {
    min-height: 64px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in oklab, var(--background) 97%, var(--muted));
    margin-bottom: 14px;
  }

  .agent-git-head .eyebrow {
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 10px;
    text-transform: uppercase;
  }

  .agent-git-head h3 {
    margin: 1px 0 0;
    font-size: 15px;
    letter-spacing: 0;
  }

  .agent-git-head p,
  .agent-git-card p {
    margin: 2px 0 0;
    color: var(--muted-foreground);
    font-size: 12px;
    line-height: 1.45;
  }

  .agent-git-counts {
    display: inline-flex;
    gap: 6px;
    flex-wrap: wrap;
    justify-content: flex-end;
    font-family: var(--font-mono);
    font-size: 11px;
  }

  .agent-git-counts span {
    height: 22px;
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    padding: 0 8px;
    background: var(--muted);
    color: var(--muted-foreground);
  }

  .agent-git-counts .warn {
    color: var(--diff-del-fg);
    background: color-mix(in oklab, var(--diff-del-marker) 13%, transparent);
  }

  .agent-git-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 14px;
  }

  .agent-git-card {
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
    background: var(--background);
  }

  .agent-git-card > header {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
    font-size: 13px;
    font-weight: 600;
  }

  .agent-git-card > header strong {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--muted-foreground);
  }

  .agent-git-card > p {
    padding: 10px 12px 0;
  }

  .reconcile-list {
    padding: 10px 8px 12px;
    display: grid;
    gap: 4px;
  }

  .reconcile-row {
    min-width: 0;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 8px;
    align-items: center;
    padding: 6px 8px;
    border-radius: 5px;
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
    font-size: 12px;
  }

  .reconcile-row .mono {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--muted-foreground);
  }

  .file-dot.agent {
    background: var(--diff-add-marker);
  }

  .file-dot.git {
    background: var(--diff-del-marker);
  }

  .reconcile-empty,
  .agent-git-empty {
    color: var(--muted-foreground);
    font-size: 12px;
  }

  .agent-git-empty {
    min-height: 260px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    text-align: left;
  }

  .agent-git-empty .title {
    font-size: 14px;
    font-weight: 600;
    color: var(--foreground);
  }

  .agent-git-empty .sub {
    margin-top: 3px;
    max-width: 420px;
  }

  @media (max-width: 900px) {
    .agent-git-grid {
      grid-template-columns: 1fr;
    }
  }

  .mono {
    font-family: var(--font-mono);
  }
</style>
