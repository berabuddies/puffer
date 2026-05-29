<script lang="ts" module>
  import type { AgentState } from "./tweaks.ts";

  export type ActiveAgent = {
    id: string;
    name: string;
    title: string;
    project: string;
    projectKey: string;
    branch: string;
    state: AgentState;
    updatedAtMs: number;
    pinned: boolean;
    pinBusy?: boolean;
  };
</script>

<script lang="ts">
  import Icon from "../design/Icon.svelte";

  type ProjectGroup = {
    project: string;
    projectKey: string;
    agents: ActiveAgent[];
  };

  type Props = {
    agents: ActiveAgent[];
    activeAgentId?: string | null;
    onOpenAgent?: (id: string) => void;
    onToggleAgentPin?: (id: string, pinned: boolean) => void;
  };

  let {
    agents,
    activeAgentId = null,
    onOpenAgent,
    onToggleAgentPin
  }: Props = $props();

  const COLLAPSED_STORAGE_KEY = "puffer.sidebar.collapsedProjects";
  const initialCollapsedProjects = loadCollapsedProjects();

  let collapsedProjects = $state<Set<string>>(new Set(initialCollapsedProjects));
  let manuallyCollapsedProjects = $state<Set<string>>(new Set(initialCollapsedProjects));
  let lastAutoExpandedActiveKey: string | null = null;
  let groupedAgents = $derived(groupByProject(agents));

  $effect(() => {
    const active = activeAgentId ? agents.find((agent) => agent.id === activeAgentId) : null;
    if (!active) {
      lastAutoExpandedActiveKey = null;
      return;
    }
    const activeKey = `${active.id}\u0000${active.projectKey}`;
    if (lastAutoExpandedActiveKey === activeKey) return;
    lastAutoExpandedActiveKey = activeKey;
    if (collapsedProjects.has(active.projectKey) && !manuallyCollapsedProjects.has(active.projectKey)) {
      const next = new Set(collapsedProjects);
      next.delete(active.projectKey);
      collapsedProjects = next;
      saveCollapsedProjects(next);
    }
  });

  function groupByProject(list: ActiveAgent[]): ProjectGroup[] {
    const order: string[] = [];
    const labels = new Map<string, string>();
    const map = new Map<string, ActiveAgent[]>();
    for (const agent of list) {
      if (!map.has(agent.projectKey)) {
        order.push(agent.projectKey);
        labels.set(agent.projectKey, agent.project);
        map.set(agent.projectKey, []);
      }
      map.get(agent.projectKey)!.push(agent);
    }
    return order.map((projectKey) => ({
      project: labels.get(projectKey) ?? projectKey,
      projectKey,
      agents: map.get(projectKey)!
    }));
  }

  function loadCollapsedProjects(): Set<string> {
    if (typeof window === "undefined") return new Set();
    try {
      const raw = window.localStorage.getItem(COLLAPSED_STORAGE_KEY);
      if (!raw) return new Set();
      const parsed = JSON.parse(raw);
      return Array.isArray(parsed) ? new Set(parsed.filter((x) => typeof x === "string")) : new Set();
    } catch {
      return new Set();
    }
  }

  function saveCollapsedProjects(set: Set<string>) {
    if (typeof window === "undefined") return;
    try {
      window.localStorage.setItem(COLLAPSED_STORAGE_KEY, JSON.stringify(Array.from(set)));
    } catch {
      /* storage full or unavailable - silently skip */
    }
  }

  function toggleProjectCollapsed(project: string) {
    const next = new Set(collapsedProjects);
    const manual = new Set(manuallyCollapsedProjects);
    if (next.has(project)) {
      next.delete(project);
      manual.delete(project);
    } else {
      next.add(project);
      manual.add(project);
    }
    collapsedProjects = next;
    manuallyCollapsedProjects = manual;
    saveCollapsedProjects(next);
  }

  function formatAge(updatedAtMs: number): string {
    const delta = Date.now() - updatedAtMs;
    const mins = Math.round(delta / 60_000);
    if (mins < 1) return "just now";
    if (mins < 60) return `${mins}m`;
    const hours = Math.round(mins / 60);
    if (hours < 24) return `${hours}h`;
    const days = Math.round(hours / 24);
    if (days < 7) return `${days}d`;
    const weeks = Math.round(days / 7);
    return `${weeks}w`;
  }
</script>

<div class="pf-sidebar-section pf-sidebar-agents">
  <div class="pf-sidebar-label">Projects</div>
  <div class="pf-sidebar-agents-list">
    {#each groupedAgents as group (group.projectKey)}
      {@const isCollapsed = collapsedProjects.has(group.projectKey)}
      <div class="pf-sidebar-project-group" data-collapsed={isCollapsed}>
        <button
          type="button"
          class="pf-sidebar-project-header"
          onclick={() => toggleProjectCollapsed(group.projectKey)}
          aria-expanded={!isCollapsed}
          aria-label={`${isCollapsed ? "Expand" : "Collapse"} ${group.project}`}
        >
          <Icon name={isCollapsed ? "chevR" : "chevD"} size={12} />
          <Icon name={isCollapsed ? "folder" : "folderOpen"} size={13} />
          <span class="name">{group.project}</span>
          <span class="count">{group.agents.length}</span>
        </button>
        {#if !isCollapsed}
          <div class="pf-sidebar-project-children">
            {#each group.agents as a (a.id)}
              <div class="pf-sidebar-agent-row" data-active={activeAgentId === a.id} data-pinned={a.pinned}>
                <button
                  type="button"
                  class="pf-sidebar-agent"
                  onclick={() => onOpenAgent?.(a.id)}
                >
                  <div class="pf-row-stack">
                    <div class="line-1">
                      <span class="title">{a.name || a.title}</span>
                      <span class="age">{formatAge(a.updatedAtMs)}</span>
                    </div>
                    <span class="state" data-state={a.state}>{a.state}</span>
                  </div>
                </button>
                <button
                  type="button"
                  class="pf-pin-button"
                  data-pinned={a.pinned}
                  title={a.pinned ? "Unpin agent" : "Pin agent"}
                  aria-label={a.pinned ? "Unpin agent" : "Pin agent"}
                  disabled={a.pinBusy ?? false}
                  onclick={() => onToggleAgentPin?.(a.id, !a.pinned)}
                >
                  <Icon name="pin" size={12} />
                </button>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
    {#if agents.length === 0}
      <div class="pf-sidebar-empty">No projects</div>
    {/if}
  </div>
</div>
