<script lang="ts" module>
  import type { AgentState } from "./tweaks.ts";

  export type ActiveAgent = {
    id: string;
    name: string;
    title: string;
    project: string;
    branch: string;
    state: AgentState;
    updatedAtMs: number;
    pinned: boolean;
  };

  export type UserChip = {
    initials: string;
    name: string;
    meta: string;
  };
</script>

<script lang="ts">
  import BrandLogo from "../design/BrandLogo.svelte";
  import Puffer from "../design/Puffer.svelte";
  import Icon, { type IconName } from "../design/Icon.svelte";
  import type { ScreenId } from "./tweaks.ts";

  type Props = {
    screen: ScreenId;
    collapsed?: boolean;
    onSelectScreen: (id: ScreenId) => void;
    agents: ActiveAgent[];
    activeAgentId?: string | null;
    onOpenAgent?: (id: string) => void;
    onToggleAgentPin?: (id: string, pinned: boolean) => void;
    onToggleCollapse?: () => void;
    user?: UserChip | null;
  };

  let {
    screen,
    collapsed = false,
    onSelectScreen,
    agents,
    activeAgentId = null,
    onOpenAgent,
    onToggleAgentPin,
    onToggleCollapse,
    user = null
  }: Props = $props();

  let filterProject = $state<string>("all");
  let filterState = $state<string>("all");

  const screens: { id: ScreenId; label: string; icon: IconName }[] = [
    { id: "workspace", label: "Workspace", icon: "sparkles" },
    { id: "pipelines", label: "Pipelines", icon: "git" },
    { id: "settings", label: "Settings", icon: "settings" }
  ];

  const states: (AgentState | "all")[] = ["all", "running", "thinking", "awaiting", "idle"];

  let projects = $derived(["all", ...Array.from(new Set(agents.map((a) => a.project)))]);
  let filtered = $derived(
    agents.filter(
      (a) =>
        (filterProject === "all" || a.project === filterProject) &&
        (filterState === "all" || a.state === filterState)
    )
  );
</script>

<aside class="pf-sidebar" data-collapsed={collapsed}>
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
      <select bind:value={filterProject} aria-label="Filter by project">
        {#each projects as p (p)}
          <option value={p}>{p === "all" ? "All projects" : p}</option>
        {/each}
      </select>
      <select bind:value={filterState} aria-label="Filter by state">
        {#each states as s (s)}
          <option value={s}>{s === "all" ? "All states" : s}</option>
        {/each}
      </select>
    </div>
    <div class="pf-sidebar-agents-list">
      {#each filtered as a (a.id)}
        <div class="pf-sidebar-agent-row" data-active={activeAgentId === a.id} data-pinned={a.pinned}>
          <button
            type="button"
            class="pf-sidebar-agent"
            onclick={() => onOpenAgent?.(a.id)}
          >
            <Puffer size={16} state={a.state} />
            <div class="pf-row-stack">
              <span class="title">
                {a.name}
                {#if a.title}
                  · {a.title}
                {/if}
              </span>
              <span class="pf-task-status">{a.project} · {a.state}</span>
            </div>
          </button>
          <button
            type="button"
            class="pf-pin-button"
            data-pinned={a.pinned}
            title={a.pinned ? "Unpin agent" : "Pin agent"}
            aria-label={a.pinned ? "Unpin agent" : "Pin agent"}
            onclick={() => onToggleAgentPin?.(a.id, !a.pinned)}
          >
            <Icon name="pin" size={12} />
          </button>
        </div>
      {/each}
      {#if filtered.length === 0}
        <div class="pf-sidebar-empty">No agents match</div>
      {/if}
    </div>
  </div>

  {#if user}
    <div class="pf-sidebar-section" style="border-top: 1px solid var(--border);">
      <div class="pf-sidebar-item" style="cursor: default;">
        <span
          style="width: 24px; height: 24px; border-radius: 6px; background: color-mix(in oklab, var(--puffer-accent) 18%, var(--background)); display: inline-flex; align-items: center; justify-content: center; font-size: 11px; font-weight: 600; color: var(--puffer-accent); flex-shrink: 0;"
        >{user.initials}</span>
        <div class="pf-row-stack">
          <span class="title" style="font-weight: 500;">{user.name}</span>
          <span class="pf-task-status">{user.meta}</span>
        </div>
        <Icon name="moreH" size={14} color="var(--muted-foreground)" />
      </div>
    </div>
  {/if}
</aside>
