<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import AgentCard from "./AgentCard.svelte";
  import type { MockAgent, MockProject } from "../../data/mockProjects";

  type Props = {
    project: MockProject;
    agents: MockAgent[];
    pinned?: boolean;
    pinBusy?: boolean;
    onOpenAgent?: (id: string) => void;
    onOpenBoard?: (projectId: string) => void;
    onNewAgent?: () => void;
    onTogglePin?: () => void;
  };

  let { project, agents, pinned = false, pinBusy = false, onOpenAgent, onOpenBoard, onNewAgent, onTogglePin }: Props = $props();

  let collapsed = $state(false);
  let active = $derived(agents.filter((a) => a.status === "running" || a.status === "awaiting").length);
  let review = $derived(agents.filter((a) => a.status === "review").length);
</script>

<div class="pf-pw-project" data-collapsed={collapsed}>
  <div class="pf-pw-project-head">
    <button
      type="button"
      class="sc-btn pf-pw-project-toggle"
      data-variant="ghost"
      data-size="sm"
      onclick={() => (collapsed = !collapsed)}
      aria-expanded={!collapsed}
      aria-label={`${collapsed ? "Expand" : "Collapse"} ${project.name}`}
      title={`${collapsed ? "Expand" : "Collapse"} ${project.name}`}
    >
      <Icon name={collapsed ? "chevR" : "chevD"} size={12} />
    </button>
    <div class="pf-pw-project-title">
      <span class="name">
        {project.name}
        {#if project.remoteHost}
          <span class="remote-chip">remote</span>
        {/if}
      </span>
      {#if project.branch}
        <span class="branch"><Icon name="branch" size={10} />{project.branch}</span>
      {/if}
    </div>
    <div class="pf-pw-project-counts">
      <span class="count">{agents.length} {agents.length === 1 ? "session" : "sessions"}</span>
      <span class="sep">·</span>
      <span class="count running">{active} active</span>
      <span class="sep">·</span>
      <span class="count review">{review} review</span>
    </div>
    <button
      type="button"
      class="sc-btn"
      data-variant="ghost"
      data-size="sm"
      data-pinned={pinned}
      onclick={onTogglePin}
      title={pinned ? "Unpin workspace" : "Pin workspace"}
      aria-label={pinned ? "Unpin workspace" : "Pin workspace"}
      aria-pressed={pinned ? "true" : "false"}
      disabled={!onTogglePin || pinBusy}
    ><Icon name="pin" size={12} />{pinned ? "Pinned" : "Pin"}</button>
    <button
      type="button"
      class="sc-btn"
      data-variant="ghost"
      data-size="sm"
      onclick={() => onOpenBoard?.(project.id)}
      title="Open project details"
    >Details</button>
  </div>

  {#if !collapsed}
    <div class="pf-pw-agents-strip">
      {#each agents as a (a.id)}
        <AgentCard {a} onOpen={() => onOpenAgent?.(a.id)} />
      {/each}
      {#if agents.length === 0}
        <div class="pf-pw-agents-empty">
          <span class="icon"><Icon name="sparkles" size={14} color="var(--muted-foreground)" /></span>
          <span>No sessions.</span>
        </div>
      {/if}
      <button
        type="button"
        class="pf-pw-agent-add"
        onclick={onNewAgent}
        disabled={!onNewAgent}
        title="New agent"
        aria-label={`New agent in ${project.name}`}
      >
        <Icon name="plus" size={15} />
      </button>
    </div>
  {/if}
</div>
