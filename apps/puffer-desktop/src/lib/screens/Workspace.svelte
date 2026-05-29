<script lang="ts">
  import "../design/workspace.css";

  import { tick } from "svelte";
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
    loadedSessions?: number;
    totalSessions?: number | null;
    hasMoreSessions?: boolean;
    loadingMoreSessions?: boolean;
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
    onLoadMoreSessions?: () => void | Promise<void>;
    pinnedWorkspacePaths?: string[];
    pinningWorkspacePaths?: string[];
    onToggleWorkspacePin?: (path: string, pinned: boolean) => void;
    onDeleteSession?: (sessionId: string) => void | Promise<void>;
    onSetSessionTags?: (sessionId: string, tags: string[]) => void | Promise<void>;
    onDeleteProject?: (folderPath: string) => void | Promise<void>;
    onSetProjectTags?: (folderPath: string, tags: string[]) => void | Promise<void>;
    settingsSnapshot?: SettingsSnapshot | null;
  };

  let {
    groups,
    defaultWorkspaceCwd = "",
    loading = false,
    loadedSessions = 0,
    totalSessions = null,
    hasMoreSessions = false,
    loadingMoreSessions = false,
    onOpenAgent,
    onOpenBoard,
    onNewAgent,
    onSessionReady,
    onLoadMoreSessions,
    pinnedWorkspacePaths = [],
    pinningWorkspacePaths = [],
    onToggleWorkspacePin,
    onDeleteSession,
    onSetSessionTags,
    onDeleteProject,
    onSetProjectTags,
    settingsSnapshot = null
  }: Props = $props();

  let showConnect = $state(false);
  let projectPendingDelete: MockProject | null = $state(null);
  let deletingProject = $state(false);
  let searchQuery = $state("");
  let searchInput: HTMLInputElement | null = $state(null);

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
      remoteHost: false,
      tags: group.tags ?? []
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
      model: "",
      tags: session.tags ?? []
    };
  }

  let projects = $derived<MockProject[]>(groups.map(projectFromGroup));
  let agents = $derived<MockAgent[]>(
    groups.flatMap((g) => g.sessions.map((s) => agentFromSession(s, g.id)))
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
      includesNeedle(agent.status, needle) ||
      includesNeedle(agent.step, needle) ||
      includesNeedle(agent.model, needle)
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
  let projectCount = $derived(projects.length);
  let visibleProjectCount = $derived(visibleProjects.length);
  let visibleSessionCount = $derived(visibleProjects.reduce((count, project) => count + visibleAgentsFor(project).length, 0));

  $effect(() => {
    visibleProjects;
    if (!searchNeedle) return;
    void tick().then(() => {
      if (document.activeElement !== document.body) return;
      searchInput?.focus();
    });
  });

  async function handleNewAgent(cwd: string) {
    if (!onNewAgent) return;
    await onNewAgent(cwd);
  }

  function requestDeleteProject(project: MockProject) {
    if (!onDeleteProject) return;
    projectPendingDelete = project;
  }

  async function confirmDeleteProject() {
    const project = projectPendingDelete;
    if (!project || !onDeleteProject || deletingProject) return;
    deletingProject = true;
    try {
      await onDeleteProject(project.path);
      projectPendingDelete = null;
    } finally {
      deletingProject = false;
    }
  }
</script>

<div class="pf-pw">
  <div class="pf-pw-top">
    <div class="pf-pw-top-left">
      <h1>Projects</h1>
    </div>
    <div class="pf-pw-top-right">
      <div class="pf-pw-search">
        <Icon name="search" size={12} />
        <input
          bind:this={searchInput}
          placeholder="Search..."
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

  {#if projectPendingDelete}
    <div class="pf-project-delete-scrim" role="presentation" onkeydown={() => {}}>
      <div
        class="pf-project-delete-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={`delete-project-title-${projectPendingDelete.id}`}
        aria-describedby={`delete-project-description-${projectPendingDelete.id}`}
      >
        <div class="pf-project-delete-head">
          <div class="pf-project-delete-title-group">
            <div class="pf-project-delete-title" id={`delete-project-title-${projectPendingDelete.id}`}>Delete project?</div>
          </div>
          <button
            type="button"
            class="pf-project-delete-close"
            onclick={() => (projectPendingDelete = null)}
            aria-label="Close"
            disabled={deletingProject}
          >
            <Icon name="x" size={14} />
          </button>
        </div>
        <div class="pf-project-delete-body">
          <div class="pf-project-delete-icon" aria-hidden="true">
            <Icon name="alert" size={18} />
          </div>
          <div class="pf-project-delete-copy">
            <p id={`delete-project-description-${projectPendingDelete.id}`}>
              This will remove <strong>{projectPendingDelete.name}</strong> and all of its sessions from this workspace.
            </p>
          </div>
        </div>
        <div class="pf-project-delete-foot">
          <div class="pf-project-delete-actions">
            <button
              type="button"
              class="pf-project-delete-cancel"
              onclick={() => (projectPendingDelete = null)}
              disabled={deletingProject}
            >
              Cancel
            </button>
            <button
              type="button"
              class="pf-project-delete-confirm"
              onclick={confirmDeleteProject}
              disabled={deletingProject}
            >
              {#if deletingProject}
                <Icon name="refresh" size={13} />Deleting...
              {:else}
                Delete project
              {/if}
            </button>
          </div>
        </div>
      </div>
    </div>
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
        onDeleteProject={onDeleteProject ? () => requestDeleteProject(p) : undefined}
        onSetProjectTags={onSetProjectTags ? (tags) => onSetProjectTags(p.path, tags) : undefined}
        onDeleteSession={onDeleteSession}
        onSetSessionTags={onSetSessionTags}
      />
    {/each}
    {#if totalSessions !== null && totalSessions > 0 && !searchNeedle}
      <div class="pf-pw-load-more">
        <span>
          Showing {Math.min(loadedSessions, totalSessions)} of {totalSessions} sessions
        </span>
        {#if hasMoreSessions}
          <button
            type="button"
            class="sc-btn"
            data-variant="outline"
            data-size="sm"
            onclick={() => onLoadMoreSessions?.()}
            disabled={!onLoadMoreSessions || loadingMoreSessions}
          >
            {#if loadingMoreSessions}
              <Icon name="refresh" size={13} />Loading...
            {:else}
              Load more
            {/if}
          </button>
        {/if}
      </div>
    {:else if searchNeedle && totalSessions !== null}
      <div class="pf-pw-load-more">
        <span>Showing {visibleSessionCount} matching loaded sessions</span>
      </div>
    {/if}
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
  .pf-pw-load-more {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 18px 0 28px;
    color: var(--muted-foreground);
    font-size: 12px;
  }
</style>
