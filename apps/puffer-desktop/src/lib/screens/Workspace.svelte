<script lang="ts">
  import "../design/workspace.css";

  import Icon from "../design/Icon.svelte";
  import ProjectRow from "./workspace/ProjectRow.svelte";
  import ConnectProjectModal from "./workspace/ConnectProjectModal.svelte";
  import { sessionDisplayName, sessionDisplayTitle } from "../sessionDisplay";
  import type { MockAgent, MockProject } from "../data/mockProjects";
  import type { FolderGroup, SessionListItem, SettingsSnapshot } from "../types";

  type Props = {
    /** Real folder-groups loaded from the daemon. */
    groups: FolderGroup[];
    /** Daemon's default workspace cwd — shown in the header so users know
     *  where new sessions will land. */
    defaultWorkspaceCwd?: string;
    loading?: boolean;
    onOpenAgent?: (id: string) => void;
    onOpenBoard?: (projectId: string) => void;
    /** "New agent" was clicked on a project row. `cwd` is that project's
     *  path. Receives control to create the session + open AgentDetail. */
    onNewAgent?: (cwd: string) => void | Promise<void>;
    /** Connect-project modal just finished with a new session id — the
     *  parent should refresh the workspace and open AgentDetail. */
    onSessionReady?: (sessionId: string) => void | Promise<void>;
    /** User clicked the workspace-cwd chip in the header — the parent
     *  should open the WorkspacePicker. */
    onOpenWorkspacePicker?: () => void;
    pinnedWorkspacePaths?: string[];
    onToggleWorkspacePin?: (path: string, pinned: boolean) => void;
    settingsSnapshot?: SettingsSnapshot | null;
  };

  let {
    groups,
    defaultWorkspaceCwd = "",
    loading = false,
    onOpenAgent,
    onOpenBoard,
    onNewAgent,
    onSessionReady,
    onOpenWorkspacePicker,
    pinnedWorkspacePaths = [],
    onToggleWorkspacePin,
    settingsSnapshot = null
  }: Props = $props();

  let showConnect = $state(false);

  // Stable palette so two renders of the same folder pick the same color.
  const PALETTE = [
    "oklch(0.68 0.17 20)",
    "oklch(0.68 0.16 260)",
    "oklch(0.68 0.15 150)",
    "oklch(0.7 0.13 60)",
    "oklch(0.68 0.16 120)",
    "oklch(0.68 0.17 340)",
    "oklch(0.68 0.15 200)"
  ];
  function hashColor(key: string): string {
    let h = 0;
    for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
    return PALETTE[h % PALETTE.length];
  }

  function formatAge(updatedAtMs: number): string {
    const delta = Date.now() - updatedAtMs;
    const mins = Math.round(delta / 60_000);
    if (mins < 1) return "just now";
    if (mins < 60) return `${mins}m`;
    const hours = Math.round(mins / 60);
    if (hours < 24) return `${hours}h`;
    const days = Math.round(hours / 24);
    return `${days}d`;
  }

  function projectFromGroup(group: FolderGroup): MockProject {
    return {
      id: group.id,
      name: group.label,
      path: group.path,
      branch: "",
      remote: "",
      color: hashColor(group.id),
      remoteHost: false
    };
  }

  function agentFromSession(session: SessionListItem, projectId: string): MockAgent {
    return {
      id: session.id,
      project: projectId,
      name: sessionDisplayName(session),
      title: sessionDisplayTitle(session),
      worktree: "",
      branch: "",
      status: "idle",
      progress: 0,
      step: session.note ?? (session.eventCount > 0 ? `${session.eventCount} transcript events` : "Ready to start"),
      tools: session.eventCount,
      elapsed: formatAge(session.updatedAtMs),
      model: ""
    };
  }

  let projects = $derived<MockProject[]>(groups.map(projectFromGroup));
  let agents = $derived<MockAgent[]>(
    groups.flatMap((g) => g.sessions.slice(0, 6).map((s) => agentFromSession(s, g.id)))
  );

  let agentCount = $derived(agents.length);
  let projectCount = $derived(projects.length);

  let headerSubtitle = $derived(
    loading
      ? "loading…"
      : defaultWorkspaceCwd
        ? defaultWorkspaceCwd
        : `${agentCount} active ${agentCount === 1 ? "agent" : "agents"}`
  );

  async function handleNewAgent(cwd: string) {
    if (!onNewAgent) return;
    await onNewAgent(cwd);
  }
</script>

<div class="pf-pw">
  <div class="pf-pw-top">
    <div class="pf-pw-top-left">
      <span class="pf-screen-top-eyebrow">Workspace</span>
      <h1>{projectCount} {projectCount === 1 ? "project" : "projects"}</h1>
      {#if onOpenWorkspacePicker && defaultWorkspaceCwd}
        <button
          type="button"
          class="pf-pw-sub pf-pw-sub-btn"
          onclick={() => onOpenWorkspacePicker?.()}
          title="Switch workspace"
        >· {headerSubtitle}</button>
      {:else}
        <span class="pf-pw-sub">· {headerSubtitle}</span>
      {/if}
    </div>
    <div class="pf-pw-top-right">
      <div class="pf-pw-search">
        <Icon name="search" size={12} />
        <input placeholder="Search tasks, agents, branches…" />
      </div>
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        onclick={() => (showConnect = true)}
      >
        <Icon name="plus" size={13} />Connect project
      </button>
    </div>
  </div>

  {#if showConnect}
    <ConnectProjectModal
      onClose={() => (showConnect = false)}
      onConnected={async (sessionId) => {
        showConnect = false;
        await onSessionReady?.(sessionId);
      }}
      defaultLocalPath={defaultWorkspaceCwd || "~/code"}
      snapshot={settingsSnapshot}
    />
  {/if}

  <div class="pf-pw-list">
    {#if projectCount === 0 && !loading}
      <div class="pf-pw-empty">
        <div class="pf-pw-empty-inner">
          <h2>No sessions yet</h2>
          <p>
            Start a fresh agent in the default workspace
            {#if defaultWorkspaceCwd}<code>{defaultWorkspaceCwd}</code>{/if}
            — you'll choose Codex, Claude, or Puffer before the chat opens.
          </p>
        </div>
      </div>
    {/if}
    {#each projects as p (p.id)}
      <ProjectRow
        project={p}
        agents={agents.filter((a) => a.project === p.id)}
        pinned={pinnedWorkspacePaths.includes(p.path) || pinnedWorkspacePaths.includes(p.id)}
        {onOpenAgent}
        {onOpenBoard}
        onNewAgent={onNewAgent ? () => handleNewAgent(p.path) : undefined}
        onTogglePin={onToggleWorkspacePin ? () => onToggleWorkspacePin(p.path, !(pinnedWorkspacePaths.includes(p.path) || pinnedWorkspacePaths.includes(p.id))) : undefined}
      />
    {/each}
  </div>
</div>

<style>
  .pf-pw-sub-btn {
    background: transparent;
    border: none;
    color: inherit;
    cursor: pointer;
    padding: 0;
    font: inherit;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border-radius: 4px;
  }
  .pf-pw-sub-btn:hover {
    color: var(--foreground);
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .pf-pw-empty {
    padding: 20px 0 0;
    display: flex;
    justify-content: center;
  }
  .pf-pw-empty-inner {
    max-width: 520px;
    text-align: center;
    padding: 28px 24px;
    border: 1px dashed var(--border);
    border-radius: 12px;
    background: color-mix(in oklab, var(--background) 94%, var(--muted));
    display: flex;
    flex-direction: column;
    gap: 12px;
    align-items: center;
  }
  .pf-pw-empty-inner :global(h2) {
    font-size: 16px;
    font-weight: 600;
    letter-spacing: -0.01em;
    margin: 0;
  }
  .pf-pw-empty-inner :global(p) {
    font-size: 13px;
    color: var(--muted-foreground);
    line-height: 1.55;
    margin: 0;
  }
  .pf-pw-empty-inner :global(code) {
    font-family: var(--font-mono);
    font-size: 11.5px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--muted);
  }
</style>
