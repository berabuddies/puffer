<script lang="ts">
  import { onDestroy } from "svelte";
  import Icon, { type IconName } from "../../design/Icon.svelte";
  import { KIND_META, MEMORY, type Deployment, type MemoryItem } from "../../data/mockDeployments";

  type Props = {
    d: Deployment;
    drafts: MemoryItem[];
    onAddMemory: (item: MemoryItem) => void;
  };
  let { d, drafts, onAddMemory }: Props = $props();

  const kindOptions: MemoryItem["kind"][] = ["incident", "runbook", "fact", "pitfall", "convention"];

  let baseItems = $derived(MEMORY[d.id] ?? MEMORY["d-prod-api"]);
  let items = $derived([...drafts, ...baseItems]);
  let filter = $state<string>("all");
  let addNoteOpen = $state(false);
  let noteTitle = $state("");
  let noteBody = $state("");
  let noteKind = $state<MemoryItem["kind"]>("fact");
  let noteConfidence = $state<MemoryItem["confidence"]>("medium");
  let noteTags = $state("");
  let noteTitleInput = $state<HTMLInputElement | null>(null);
  let memoryStatus = $state("");
  let statusDeploymentId = $state("");
  let statusTimer = 0;
  let openActionId = $state<string | null>(null);
  let pinnedIds = $state<Record<string, boolean>>({});
  const kinds = ["all", ...kindOptions];
  let filtered = $derived(filter === "all" ? items : items.filter((m) => m.kind === filter));
  let canAddNote = $derived(noteTitle.trim().length > 0 && noteBody.trim().length > 0);

  onDestroy(() => {
    if (statusTimer) window.clearTimeout(statusTimer);
  });

  $effect(() => {
    const deploymentId = d.id;
    if (deploymentId === statusDeploymentId) return;
    statusDeploymentId = deploymentId;
    resetAddNote();
    memoryStatus = "";
    openActionId = null;
    pinnedIds = {};
    if (statusTimer) window.clearTimeout(statusTimer);
    statusTimer = 0;
  });

  function srcIcon(kind: string): IconName {
    if (kind === "deploy") return "rocket";
    if (kind === "pr") return "git";
    if (kind === "logs") return "logs";
    return "bolt";
  }

  function resetAddNote(): void {
    addNoteOpen = false;
    noteTitle = "";
    noteBody = "";
    noteKind = "fact";
    noteConfidence = "medium";
    noteTags = "";
  }

  function openAddNote(): void {
    resetAddNote();
    addNoteOpen = true;
    openActionId = null;
    window.setTimeout(() => noteTitleInput?.focus({ preventScroll: true }), 20);
  }

  function showMemoryStatus(message: string): void {
    memoryStatus = message;
    if (statusTimer) window.clearTimeout(statusTimer);
    statusTimer = window.setTimeout(() => {
      memoryStatus = "";
      statusTimer = 0;
    }, 4000);
  }

  function tagList(value: string): string[] {
    const tags = value
      .split(/[,\s]+/)
      .map((tag) => tag.trim().replace(/^#/, "").toLowerCase())
      .filter(Boolean);
    return Array.from(new Set(tags));
  }

  function createNote(): void {
    if (!canAddNote) return;
    const title = noteTitle.trim();
    const body = noteBody.trim();
    const next: MemoryItem = {
      id: `draft-${d.id}-${Date.now()}`,
      kind: noteKind,
      title,
      body,
      source: { kind: "manual", ref: "local draft" },
      confidence: noteConfidence,
      savedBy: "Otter",
      time: "just now",
      tags: tagList(noteTags),
      uses: 0
    };
    onAddMemory(next);
    if (filter !== "all" && filter !== next.kind) filter = next.kind;
    showMemoryStatus(`Added memory note "${title}" to ${d.name}.`);
    resetAddNote();
  }

  function toggleMemoryActions(id: string): void {
    openActionId = openActionId === id ? null : id;
  }

  function togglePin(item: MemoryItem): void {
    const pinned = pinnedIds[item.id] === true;
    pinnedIds = { ...pinnedIds, [item.id]: !pinned };
    openActionId = null;
    showMemoryStatus(`${pinned ? "Unpinned" : "Pinned"} "${item.title}" for ${d.name}.`);
  }

  function queueAskContext(item: MemoryItem): void {
    openActionId = null;
    showMemoryStatus(`Queued "${item.title}" as Ask Puffer context for ${d.name}.`);
  }
</script>

<div class="pf-dep-pane">
  <div class="pf-dep-pane-head">
    <div>
      <h3>Memory</h3>
      <p class="sub">
        {items.length} notes Puffer has learned running <strong>{d.name}</strong> — surfaced automatically on future deploys and debug sessions.
      </p>
    </div>
    <div class="pf-dep-pane-actions">
      {#if memoryStatus}
        <div class="pf-dep-pane-status" role="status" aria-live="polite">
          {memoryStatus}
        </div>
      {/if}
      <button type="button" class="sc-btn" data-variant="default" data-size="sm" onclick={openAddNote}>
        <Icon name="plus" size={12} />Add note
      </button>
    </div>
  </div>

  {#if addNoteOpen}
    <form
      class="pf-dep-mem-form"
      aria-label="Add deployment memory note"
      onsubmit={(event) => {
        event.preventDefault();
        createNote();
      }}
    >
      <label>
        <span>Title</span>
        <input
          bind:this={noteTitleInput}
          aria-label="Memory note title"
          value={noteTitle}
          placeholder="Runbook shortcut"
          oninput={(event) => (noteTitle = event.currentTarget.value)}
        />
      </label>
      <label>
        <span>Kind</span>
        <select aria-label="Memory note kind" bind:value={noteKind}>
          {#each kindOptions as kind (kind)}
            <option value={kind}>{KIND_META[kind].label}</option>
          {/each}
        </select>
      </label>
      <label>
        <span>Confidence</span>
        <select aria-label="Memory note confidence" bind:value={noteConfidence}>
          <option value="high">high</option>
          <option value="medium">medium</option>
          <option value="low">low</option>
        </select>
      </label>
      <label class="wide">
        <span>Body</span>
        <textarea
          aria-label="Memory note body"
          value={noteBody}
          placeholder="What should Puffer remember for this deployment?"
          oninput={(event) => (noteBody = event.currentTarget.value)}
        ></textarea>
      </label>
      <label>
        <span>Tags</span>
        <input
          aria-label="Memory note tags"
          value={noteTags}
          placeholder="stripe, queue"
          oninput={(event) => (noteTags = event.currentTarget.value)}
        />
      </label>
      <div class="pf-dep-mem-form-actions">
        <button type="button" class="sc-btn" data-variant="ghost" data-size="sm" onclick={resetAddNote}>
          Cancel
        </button>
        <button type="submit" class="sc-btn" data-variant="default" data-size="sm" disabled={!canAddNote}>
          Add note
        </button>
      </div>
    </form>
  {/if}

  <div class="pf-dep-mem-filters">
    {#each kinds as k (k)}
      {@const meta = k !== "all" ? KIND_META[k] : null}
      <button type="button" class="pf-dep-mem-filter" data-active={filter === k} onclick={() => (filter = k)}>
        {#if meta}
          <Icon name={meta.icon as IconName} size={11} color={meta.color} />
        {/if}
        {k === "all" ? "All" : meta?.label}
        <span class="pf-dep-mem-filter-count">
          {k === "all" ? items.length : items.filter((m) => m.kind === k).length}
        </span>
      </button>
    {/each}
  </div>

  <div class="pf-dep-mem-list">
    {#each filtered as m (m.id)}
      {@const meta = KIND_META[m.kind]}
      <div class="pf-dep-mem">
        <div class="pf-dep-mem-gutter" style="background: {meta.color};"></div>
        <div class="pf-dep-mem-body">
          <div class="pf-dep-mem-head">
            <span
              class="pf-dep-mem-kind"
              style="color: {meta.color}; border-color: color-mix(in oklab, {meta.color} 35%, var(--border)); background: color-mix(in oklab, {meta.color} 8%, transparent);"
            >
              <Icon name={meta.icon as IconName} size={10} />{meta.label}
            </span>
            <span class="pf-dep-mem-title">{m.title}</span>
            {#if pinnedIds[m.id]}
              <span class="pf-dep-pin-chip">
                <Icon name="pin" size={10} />pinned
              </span>
            {/if}
            <span class="pf-dep-mem-conf" data-conf={m.confidence}>
              <span class="dot"></span>{m.confidence}
            </span>
          </div>
          <div class="pf-dep-mem-text">{m.body}</div>
          <div class="pf-dep-mem-foot">
            <span class="pf-dep-mem-src">
              <Icon name={srcIcon(m.source.kind)} size={10} />
              {m.source.kind}: <span class="mono">{m.source.ref}</span>
            </span>
            <span class="pf-dep-mem-tags">
              {#each m.tags as t (t)}
                <span class="pf-dep-mem-tag">#{t}</span>
              {/each}
            </span>
            <span style="flex: 1;"></span>
            <span class="pf-dep-mem-meta">saved by <strong>{m.savedBy}</strong> · {m.time}</span>
            <span class="pf-dep-mem-uses" title={`Referenced ${m.uses} times`}>
              <Icon name="refresh" size={10} />×{m.uses}
            </span>
            <span class="pf-dep-row-menu-wrap">
              <button
                type="button"
                class="pf-dep-ico"
                title="More actions"
                aria-label={`More actions for ${m.title}`}
                aria-expanded={openActionId === m.id}
                aria-controls={`memory-actions-${m.id}`}
                onclick={() => toggleMemoryActions(m.id)}
              >
                <Icon name="moreH" size={11} />
              </button>
              {#if openActionId === m.id}
                <span
                  class="pf-dep-row-menu"
                  id={`memory-actions-${m.id}`}
                  role="menu"
                  aria-label={`Actions for ${m.title}`}
                >
                  <button type="button" role="menuitem" onclick={() => togglePin(m)}>
                    <Icon name="pin" size={11} />{pinnedIds[m.id] ? "Unpin note" : "Pin note"}
                  </button>
                  <button type="button" role="menuitem" onclick={() => queueAskContext(m)}>
                    <Icon name="sparkles" size={11} />Use in Ask Puffer
                  </button>
                </span>
              {/if}
            </span>
          </div>
        </div>
      </div>
    {/each}
  </div>
</div>
