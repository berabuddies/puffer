<script lang="ts" module>
  import type { ActiveAgent } from "./SidebarProjects.svelte";

  export type { ActiveAgent } from "./SidebarProjects.svelte";

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
  import SidebarProjects from "./SidebarProjects.svelte";
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

  let resizing = $state(false);
  let dragStartX = 0;
  let dragStartWidth = SIDEBAR_DEFAULT_WIDTH;
  let sidebarStyle = $derived(`--pf-sidebar-width: ${clampSidebarWidth(width)}px;`);

  const screens: { id: ScreenId; label: string; icon: IconName }[] = [
    { id: "workspace", label: "Project", icon: "sparkles" },
    { id: "workflows", label: "Workflows", icon: "git" },
    { id: "tasks", label: "Tasks", icon: "listTodo" },
    { id: "telegram-relationships", label: "Relationships", icon: "plug" },
    { id: "settings", label: "Settings", icon: "settings" }
  ];

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

</script>

<aside class="pf-sidebar" data-collapsed={collapsed} data-resizing={resizing} style={sidebarStyle}>
  <div class="pf-sidebar-section pf-sidebar-nav">
    <div class="pf-sidebar-brand">
      {#if !collapsed}
        <BrandLogo size={24} />
      {/if}
      <div class="pf-sidebar-brand-copy" aria-hidden={collapsed}>
        <strong>Puffer</strong>
      </div>
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

  <SidebarProjects
    {agents}
    {activeAgentId}
    {onOpenAgent}
    {onToggleAgentPin}
  />

  {#if user}
    <div class="pf-sidebar-section pf-sidebar-account">
      <button
        type="button"
        class="pf-sidebar-item pf-sidebar-user"
        aria-label={`Open account for ${user.name}`}
        title={`Open account for ${user.name}`}
        onclick={() => onSelectScreen("settings")}
      >
        <span class="pf-sidebar-avatar">{user.initials}</span>
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
