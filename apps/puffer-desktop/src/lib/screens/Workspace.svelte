<script lang="ts">
  import "../design/workspace.css";

  import Icon from "../design/Icon.svelte";
  import ProjectRow from "./workspace/ProjectRow.svelte";
  import ConnectProjectModal from "./workspace/ConnectProjectModal.svelte";
  import type { createSession } from "../api/desktop";
  import { sessionDisplayName, sessionDisplayTitle } from "../sessionDisplay";
  import type { MockAgent, MockProject } from "../data/mockProjects";
  import type { FolderGroup, SessionListItem, SettingsSnapshot } from "../types";

  type CreatedSessionResult = Awaited<ReturnType<typeof createSession>>;

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
    /** Connect-project modal just finished with a new session — the
     *  parent should refresh the workspace and open AgentDetail. */
    onSessionReady?: (created: CreatedSessionResult) => void | Promise<void>;
    /** User clicked the workspace-cwd chip in the header — the parent
     *  should open the WorkspacePicker. */
    onOpenWorkspacePicker?: () => void;
    pinnedWorkspacePaths?: string[];
    pinningWorkspacePaths?: string[];
    onToggleWorkspacePin?: (path: string, pinned: boolean) => void;
    settingsSnapshot?: SettingsSnapshot | null;
  };

  type RecentSession = {
    session: SessionListItem;
    projectLabel: string;
    projectPath: string;
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
    pinningWorkspacePaths = [],
    onToggleWorkspacePin,
    settingsSnapshot = null
  }: Props = $props();

  let showConnect = $state(false);
  let searchQuery = $state("");

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
      status: session.activityStatus,
      progress: 0,
      step: session.note ?? (session.eventCount > 0 ? `${session.eventCount} transcript events` : "Ready to start"),
      tools: session.eventCount,
      elapsed: formatAge(session.updatedAtMs),
      model: ""
    };
  }

  let projects = $derived<MockProject[]>(groups.map(projectFromGroup));
  let agents = $derived<MockAgent[]>(
    groups.flatMap((g) => g.sessions.map((s) => agentFromSession(s, g.id)))
  );
  let recentSessions = $derived<RecentSession[]>(
    groups
      .flatMap((group) =>
        group.sessions.map((session) => ({
          session,
          projectLabel: group.label,
          projectPath: group.path
        }))
      )
      .sort((left, right) =>
        right.session.updatedAtMs - left.session.updatedAtMs ||
        left.projectLabel.localeCompare(right.projectLabel)
      )
  );

  function normalizeSearch(value: string): string {
    return value.trim().toLowerCase();
  }

  function includesNeedle(value: string | null | undefined, needle: string): boolean {
    return Boolean(value?.toLowerCase().includes(needle));
  }

  function projectMatches(project: MockProject, needle: string): boolean {
    return (
      includesNeedle(project.name, needle) ||
      includesNeedle(project.path, needle) ||
      includesNeedle(project.branch, needle) ||
      includesNeedle(project.remote, needle)
    );
  }

  function agentMatches(agent: MockAgent, needle: string): boolean {
    return (
      includesNeedle(agent.name, needle) ||
      includesNeedle(agent.title, needle) ||
      includesNeedle(agent.branch, needle) ||
      includesNeedle(agent.step, needle) ||
      includesNeedle(agent.model, needle)
    );
  }

  function recentSessionMatches(row: RecentSession, needle: string): boolean {
    return (
      includesNeedle(sessionDisplayName(row.session), needle) ||
      includesNeedle(sessionDisplayTitle(row.session), needle) ||
      includesNeedle(row.session.note, needle) ||
      includesNeedle(row.session.cwd, needle) ||
      includesNeedle(row.projectLabel, needle) ||
      includesNeedle(row.projectPath, needle)
    );
  }

  function projectAgents(projectId: string): MockAgent[] {
    return agents.filter((a) => a.project === projectId);
  }

  function visibleAgentsFor(project: MockProject): MockAgent[] {
    const projectScopedAgents = projectAgents(project.id);
    const needle = searchNeedle;
    if (!needle || projectMatches(project, needle)) return projectScopedAgents;
    return projectScopedAgents.filter((a) => agentMatches(a, needle));
  }

  let searchNeedle = $derived(normalizeSearch(searchQuery));
  let visibleProjects = $derived<MockProject[]>(
    projects.filter((project) => {
      if (!searchNeedle) return true;
      return projectMatches(project, searchNeedle) || projectAgents(project.id).some((a) => agentMatches(a, searchNeedle));
    })
  );
  let visibleAgentCount = $derived(
    visibleProjects.reduce((count, project) => count + visibleAgentsFor(project).length, 0)
  );
  let agentCount = $derived(agents.length);
  let projectCount = $derived(projects.length);
  let visibleProjectCount = $derived(visibleProjects.length);
  let visibleRecentSessions = $derived<RecentSession[]>(
    searchNeedle
      ? recentSessions.filter((row) => recentSessionMatches(row, searchNeedle))
      : recentSessions
  );
  let headerSubtitle = $derived(
    loading
      ? "loading..."
      : defaultWorkspaceCwd
        ? defaultWorkspaceCwd
        : `${agentCount} active ${agentCount === 1 ? "agent" : "agents"}`
  );

  async function handleNewAgent(cwd: string) {
    if (!onNewAgent) return;
    await onNewAgent(cwd);
  }

  function sessionEventLabel(count: number): string {
    return `${count} ${count === 1 ? "event" : "events"}`;
  }

  function recentSessionTitle(session: SessionListItem): string {
    return sessionDisplayTitle(session) || sessionDisplayName(session);
  }
</script>

<div class="pf-pw">
  <div class="pf-pw-top">
    <div class="pf-pw-top-left">
      <h1>{projectCount === 1 ? "Project" : "Projects"} {projectCount}</h1>
      {#if onOpenWorkspacePicker && defaultWorkspaceCwd}
        <button
          type="button"
          class="pf-pw-sub pf-pw-sub-btn"
          onclick={() => onOpenWorkspacePicker?.()}
          title="Switch workspace"
          aria-label="Switch workspace"
        >{headerSubtitle}</button>
      {:else}
        <span class="pf-pw-sub">{headerSubtitle}</span>
      {/if}
    </div>
    <div class="pf-pw-top-right">
      <div class="pf-pw-search">
        <Icon name="search" size={12} />
        <input
          placeholder="Search tasks, agents, branches…"
          aria-label="Search workspace"
          bind:value={searchQuery}
        />
      </div>
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        onclick={() => (showConnect = true)}
      >
        <Icon name="plus" size={13} />Create Project
      </button>
    </div>
  </div>

  {#if showConnect}
    <ConnectProjectModal
      onClose={() => (showConnect = false)}
      onConnected={async (created) => {
        await onSessionReady?.(created);
      }}
      defaultLocalPath={defaultWorkspaceCwd || "~/code"}
      snapshot={settingsSnapshot}
    />
  {/if}

  {#if visibleRecentSessions.length > 0}
    <section class="pf-pw-history" aria-label="Session history">
      <div class="pf-pw-history-head">
        <div class="copy">
          <span class="pf-screen-top-eyebrow">History</span>
          <h2>Session history</h2>
        </div>
        <span class="count">{visibleRecentSessions.length} {visibleRecentSessions.length === 1 ? "session" : "sessions"}</span>
      </div>
      <div class="pf-pw-history-list">
        {#each visibleRecentSessions as row (row.session.id)}
          <button
            type="button"
            class="pf-pw-history-row"
            onclick={() => onOpenAgent?.(row.session.id)}
            title={`${recentSessionTitle(row.session)} - ${row.projectPath}`}
          >
            <span class="main">
              <span class="title">{recentSessionTitle(row.session)}</span>
              <span class="status-pill" data-status={row.session.activityStatus}>{row.session.activityStatus}</span>
            </span>
            <span class="meta">
              <span>{row.projectLabel}</span>
              <span class="sep">/</span>
              <span>{sessionEventLabel(row.session.eventCount)}</span>
              <span class="sep">/</span>
              <span>{formatAge(row.session.updatedAtMs)}</span>
            </span>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  <div class="pf-pw-list">
    {#if projectCount === 0 && !searchNeedle && !loading}
      <div class="pf-pw-empty">
        <div class="pf-pw-empty-inner">
          <h2>No sessions yet</h2>
          <p>
            Start a fresh agent in the default workspace
            {#if defaultWorkspaceCwd}<code>{defaultWorkspaceCwd}</code>{/if}
            — you'll choose Codex, Claude, or Puffer before the chat opens.
          </p>
          <button
            type="button"
            class="sc-btn"
            data-variant="default"
            data-size="sm"
            onclick={() => handleNewAgent(defaultWorkspaceCwd)}
            disabled={!onNewAgent || !defaultWorkspaceCwd}
            aria-label="New agent in default workspace"
          >
            <Icon name="plus" size={13} />New agent
          </button>
        </div>
      </div>
    {/if}
    {#if searchNeedle && visibleProjectCount === 0 && !loading}
      <div class="pf-pw-empty">
        <div class="pf-pw-empty-inner">
          <h2>No workspace results</h2>
          <p>No projects or agents match <code>{searchQuery.trim()}</code>.</p>
          <button
            type="button"
            class="sc-btn"
            data-variant="outline"
            data-size="sm"
            onclick={() => (searchQuery = "")}
          >
            Clear search
          </button>
        </div>
      </div>
    {/if}
    {#each visibleProjects as p (p.id)}
      {@const projectPinned = pinnedWorkspacePaths.includes(p.path) || pinnedWorkspacePaths.includes(p.id)}
      <ProjectRow
        project={p}
        agents={visibleAgentsFor(p)}
        pinned={projectPinned}
        pinBusy={pinningWorkspacePaths.includes(p.path) || pinningWorkspacePaths.includes(p.id)}
        {onOpenAgent}
        {onOpenBoard}
        onNewAgent={onNewAgent ? () => handleNewAgent(p.path) : undefined}
        onTogglePin={onToggleWorkspacePin ? () => onToggleWorkspacePin(p.path, !projectPinned) : undefined}
      />
    {/each}
  </div>
</div>

<style>
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
