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

  export type UserChip = {
    initials: string;
    name: string;
    meta: string;
  };
</script>

<script lang="ts">
  import { onDestroy } from "svelte";
  import BrandLogo from "../design/BrandLogo.svelte";
  import Icon, { type IconName } from "../design/Icon.svelte";
  import {
    SIDEBAR_DEFAULT_WIDTH,
    clampSidebarWidth,
    type ScreenId
  } from "./tweaks";

  type Props = {
    screen: ScreenId | null;
    collapsed?: boolean;
    width?: number;
    onSelectScreen: (id: ScreenId) => void;
    agents: ActiveAgent[];
    activeAgentId?: string | null;
    onOpenAgent?: (id: string) => void;
    onToggleAgentPin?: (id: string, pinned: boolean) => void;
    onToggleCollapse?: () => void;
    onResize?: (width: number) => void;
    user?: UserChip | null;
  };

  let {
    screen,
    collapsed = false,
    width = SIDEBAR_DEFAULT_WIDTH,
    onSelectScreen,
    agents,
    activeAgentId = null,
    onOpenAgent,
    onToggleAgentPin,
    onToggleCollapse,
    onResize,
    user = null
  }: Props = $props();

  const COLLAPSED_STORAGE_KEY = "puffer.sidebar.collapsedProjects";
  const initialCollapsedProjects = loadCollapsedProjects();

  let filterState = $state<string>("all");
  let collapsedProjects = $state<Set<string>>(new Set(initialCollapsedProjects));
  let manuallyCollapsedProjects = $state<Set<string>>(new Set(initialCollapsedProjects));
  let resizing = $state(false);
  let dragStartX = 0;
  let dragStartWidth = SIDEBAR_DEFAULT_WIDTH;
  let lastAutoExpandedActiveKey: string | null = null;
  let sidebarStyle = $derived(`--pf-sidebar-width: ${clampSidebarWidth(width)}px;`);

  const screens: { id: ScreenId; label: string; icon: IconName }[] = [
    { id: "workspace", label: "Project", icon: "sparkles" },
    { id: "pipelines", label: "Pipelines", icon: "git" },
    { id: "deployments", label: "Deployments", icon: "rocket" },
    { id: "settings", label: "Settings", icon: "settings" }
  ];

  const states: (AgentState | "all")[] = [
    "all",
    "running",
    "thinking",
    "awaiting",
    "review",
    "idle"
  ];

  let filtered = $derived(
    agents.filter((a) => filterState === "all" || a.state === filterState)
  );
  let groupedAgents = $derived(groupByProject(filtered));

  $effect(() => {
    if (!states.includes(filterState as AgentState | "all")) filterState = "all";
  });
  $effect(() => {
    const active = activeAgentId ? agents.find((agent) => agent.id === activeAgentId) : null;
    if (!active) {
      lastAutoExpandedActiveKey = null;
      return;
    }
    if (filterState !== "all" && active.state !== filterState) filterState = "all";
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

  function groupByProject(list: ActiveAgent[]): { project: string; projectKey: string; agents: ActiveAgent[] }[] {
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
      /* storage full or unavailable — silently skip */
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

  function startResize(event: PointerEvent) {
    if (collapsed) return;
    event.preventDefault();
    resizing = true;
    dragStartX = event.clientX;
    dragStartWidth = clampSidebarWidth(width);
    window.addEventListener("pointermove", handleResizeMove);
    window.addEventListener("pointerup", stopResize, { once: true });
  }

  function handleResizeMove(event: PointerEvent) {
    if (!resizing) return;
    onResize?.(clampSidebarWidth(dragStartWidth + event.clientX - dragStartX));
  }

  function stopResize() {
    resizing = false;
    window.removeEventListener("pointermove", handleResizeMove);
  }

  function handleResizeKeydown(event: KeyboardEvent) {
    if (collapsed) return;
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight" && event.key !== "Home") return;
    event.preventDefault();
    const step = event.shiftKey ? 32 : 16;
    if (event.key === "Home") {
      onResize?.(SIDEBAR_DEFAULT_WIDTH);
      return;
    }
    onResize?.(clampSidebarWidth(width + (event.key === "ArrowRight" ? step : -step)));
  }

  onDestroy(() => {
    window.removeEventListener("pointermove", handleResizeMove);
    window.removeEventListener("pointerup", stopResize);
  });

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

<aside class="pf-sidebar" data-collapsed={collapsed} data-resizing={resizing} style={sidebarStyle}>
  <div class="pf-sidebar-section">
    <div class="pf-sidebar-brand">
      <BrandLogo size={24} />
      <button
        type="button"
        class="pf-sidebar-collapse"
        onclick={() => onToggleCollapse?.()}
        aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
        title={collapsed ? "Expand navigation" : "Collapse navigation"}
      >
        <Icon name={collapsed ? "panelOpen" : "panelClose"} size={14} />
      </button>
    </div>
    {#each screens as s (s.id)}
      <button
        type="button"
        class="pf-sidebar-item"
        data-active={screen === s.id}
        aria-current={screen === s.id ? "page" : undefined}
        aria-label={s.label}
        title={s.label}
        onclick={() => onSelectScreen(s.id)}
      >
        <Icon name={s.icon} size={14} />
        <span>{s.label}</span>
      </button>
    {/each}
  </div>

  <div class="pf-sidebar-section pf-sidebar-agents">
    <div class="pf-sidebar-label">
      Active agents
      <span class="count">{filtered.length}</span>
    </div>
    <div class="pf-sidebar-filters">
      <select bind:value={filterState} aria-label="Filter by state">
        {#each states as s (s)}
          <option value={s}>{s === "all" ? "All states" : s}</option>
        {/each}
      </select>
    </div>
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
                    <Icon name="bot" size={14} />
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
      {#if filtered.length === 0}
        <div class="pf-sidebar-empty">No agents match</div>
      {/if}
    </div>
  </div>

  {#if user}
    <div class="pf-sidebar-section" style="border-top: 1px solid var(--border);">
      <button
        type="button"
        class="pf-sidebar-item pf-sidebar-user"
        aria-label={`Open account for ${user.name}`}
        title={`Open account for ${user.name}`}
        onclick={() => onSelectScreen("settings")}
      >
        <span
          style="width: 24px; height: 24px; border-radius: 6px; background: color-mix(in oklab, var(--puffer-accent) 18%, var(--background)); display: inline-flex; align-items: center; justify-content: center; font-size: 11px; font-weight: 600; color: var(--puffer-accent); flex-shrink: 0;"
        >{user.initials}</span>
        <div class="pf-row-stack">
          <span class="title" style="font-weight: 500;">{user.name}</span>
          <span class="pf-task-status">{user.meta}</span>
        </div>
        <Icon name="moreH" size={14} color="var(--muted-foreground)" />
      </button>
    </div>
  {/if}
  <button
    type="button"
    class="pf-sidebar-resizer"
    aria-label="Adjust navigation size"
    onpointerdown={startResize}
    onkeydown={handleResizeKeydown}
    ondblclick={() => onResize?.(SIDEBAR_DEFAULT_WIDTH)}
  ></button>
</aside>
