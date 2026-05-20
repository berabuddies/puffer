<script lang="ts">
  import "../../design/workspace.css";

  import { readFile, writeFile } from "../../api/desktop";
  import Icon from "../../design/Icon.svelte";
  import { sessionDisplayName, sessionDisplayTitle } from "../../sessionDisplay";
  import type { MockAgent } from "../../data/mockProjects";
  import type { ReadFileResult } from "../../api/desktop";
  import type { FolderGroup, SessionListItem } from "../../types";

  type BoardColumn = {
    id: "queued" | "running" | "review" | "done" | "archived";
    label: string;
    hint: string;
    agents: MockAgent[];
  };

  type MemoryFile = {
    id: string;
    name: string;
    path: string;
    title: string;
    body: string;
    updated: string;
    kind: "project" | "session";
    tags: string[];
  };

  type Props = {
    group: FolderGroup;
    pinnedAgentIds?: string[];
    onBack: () => void;
    onOpenAgent?: (id: string) => void;
    onNewAgent?: (cwd: string) => void | Promise<void>;
  };

  let { group, pinnedAgentIds = [], onBack, onOpenAgent, onNewAgent }: Props = $props();
  let tab = $state<"board" | "memory">("board");
  let selectedMemoryId = $state<string | null>(null);
  let memoryContent = $state<string | null>(null);
  let memoryError = $state<string | null>(null);
  let memoryLoading = $state(false);
  let memoryEditing = $state(false);
  let memoryDraft = $state("");
  let memorySaving = $state(false);
  let memoryLoadedPath = $state<string | null>(null);
  let memoryLoadKey = "";
  let memoryLoadGeneration = 0;

  function formatAge(updatedAtMs: number): string {
    const delta = Date.now() - updatedAtMs;
    const mins = Math.round(delta / 60_000);
    if (mins < 1) return "now";
    if (mins < 60) return `${mins}m`;
    const hours = Math.round(mins / 60);
    if (hours < 24) return `${hours}h`;
    const days = Math.round(hours / 24);
    return `${days}d`;
  }

  function agentFromSession(session: SessionListItem): MockAgent {
    const title = sessionDisplayTitle(session);
    return {
      id: session.id,
      project: group.id,
      name: sessionDisplayName(session),
      title,
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

  function pinnedIndex(id: string): number {
    const index = pinnedAgentIds.indexOf(id);
    return index === -1 ? Number.MAX_SAFE_INTEGER : index;
  }

  let sortedSessions = $derived(
    group.sessions.slice().sort((left, right) =>
      pinnedIndex(left.id) - pinnedIndex(right.id)
      || right.updatedAtMs - left.updatedAtMs
    )
  );
  let agents = $derived<MockAgent[]>(sortedSessions.map(agentFromSession));
  let columns = $derived<BoardColumn[]>([
    {
      id: "queued",
      label: "Queued",
      hint: "Ready to continue",
      agents: agents.filter((a) => a.status === "idle")
    },
    {
      id: "running",
      label: "Running",
      hint: "Active turns",
      agents: agents.filter((a) => a.status === "running" || a.status === "awaiting")
    },
    {
      id: "review",
      label: "Review",
      hint: "Needs human review",
      agents: agents.filter((a) => a.status === "review")
    },
    { id: "done", label: "Done", hint: "Recently completed", agents: [] },
    { id: "archived", label: "Archived", hint: "Older work", agents: [] }
  ]);
  let memoryFiles = $derived<MemoryFile[]>([
    {
      id: `${group.id}:project`,
      name: "project.md",
      path: `${group.path}/.puffer/memory/project.md`,
      title: group.label,
      body: `Workspace path: ${group.path}\n\nThis project currently has ${group.sessionCount} ${group.sessionCount === 1 ? "session" : "sessions"} in the local Puffer session store.`,
      updated: agents[0]?.elapsed ?? "now",
      kind: "project",
      tags: ["project", "workspace"]
    },
    ...sortedSessions.map((session, index) => ({
      id: session.id,
      name: `session-${index + 1}.md`,
      path: `${group.path}/.puffer/memory/sessions/session-${index + 1}.md`,
      title: sessionDisplayTitle(session) || sessionDisplayName(session),
      body: `${sessionDisplayName(session)} last updated ${formatAge(session.updatedAtMs)} ago.\n\n${session.note ?? "No pinned session note yet."}`,
      updated: formatAge(session.updatedAtMs),
      kind: "session" as const,
      tags: session.tags.length ? session.tags : ["session"]
    }))
  ]);
  let selectedMemory = $derived(
    memoryFiles.find((file) => file.id === selectedMemoryId) ?? memoryFiles[0] ?? null
  );
  let memoryDisplayBody = $derived(memoryContent ?? selectedMemory?.body ?? "");
  let canEditMemory = $derived(
    Boolean(
      selectedMemory
        && memoryContent !== null
        && memoryLoadedPath === selectedMemory.path
        && !memoryLoading
        && !memorySaving
        && !memoryError
    )
  );

  function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function applyLoadedMemory(file: MemoryFile, result: ReadFileResult): void {
    if (result.encoding !== "utf8") {
      memoryContent = null;
      memoryLoadedPath = null;
      memoryError = "This memory file is binary, so it cannot be edited here.";
      return;
    }
    memoryContent = result.content;
    memoryLoadedPath = file.path;
    memoryError = null;
  }

  async function loadMemoryFile(file: MemoryFile | null): Promise<void> {
    const generation = ++memoryLoadGeneration;
    memoryEditing = false;
    memoryDraft = "";
    memoryContent = null;
    memoryLoadedPath = null;
    memoryError = null;
    if (!file) {
      memoryLoading = false;
      return;
    }
    memoryLoading = true;
    try {
      const result = await readFile(file.path);
      if (generation !== memoryLoadGeneration) return;
      applyLoadedMemory(file, result);
    } catch (error) {
      if (generation !== memoryLoadGeneration) return;
      memoryContent = null;
      memoryLoadedPath = null;
      memoryError = `Showing generated preview because ${errorMessage(error)}`;
    } finally {
      if (generation === memoryLoadGeneration) memoryLoading = false;
    }
  }

  function startMemoryEdit(): void {
    if (!canEditMemory || !selectedMemory) return;
    memoryDraft = memoryContent ?? selectedMemory.body;
    memoryEditing = true;
  }

  function cancelMemoryEdit(): void {
    memoryDraft = "";
    memoryEditing = false;
  }

  async function saveMemoryEdit(): Promise<void> {
    const file = selectedMemory;
    if (!file || memorySaving) return;
    const generation = ++memoryLoadGeneration;
    memorySaving = true;
    memoryError = null;
    try {
      const result = await writeFile(file.path, memoryDraft);
      if (generation !== memoryLoadGeneration || selectedMemory?.path !== file.path) return;
      applyLoadedMemory(file, result);
      memoryEditing = false;
      memoryDraft = "";
    } catch (error) {
      if (generation !== memoryLoadGeneration) return;
      memoryError = `Could not save memory file: ${errorMessage(error)}`;
    } finally {
      if (generation === memoryLoadGeneration) memorySaving = false;
    }
  }

  $effect(() => {
    const file = selectedMemory;
    const key = file ? `${file.id}:${file.path}` : "";
    if (key === memoryLoadKey) return;
    memoryLoadKey = key;
    void loadMemoryFile(file);
  });
</script>

<div class="pf-fpb">
  <div class="pf-fpb-head">
    <button type="button" class="pf-agent-back" onclick={onBack} aria-label="Back to workspace">
      <Icon name="chevL" size={14} />
    </button>
    <div class="pf-fpb-title">
      <div class="name">
        <span>{group.label}</span>
      </div>
      <div class="meta">
        <span class="mono">{group.path}</span>
      </div>
    </div>
    <div class="pf-fpb-counts">
      <span class="count"><span class="pip"></span>{agents.length} agents</span>
      <span class="count done">{memoryFiles.length} memory files</span>
    </div>
    <div class="pf-fpb-tools">
      <button
        type="button"
        class="sc-btn"
        data-variant="default"
        data-size="sm"
        onclick={() => onNewAgent?.(group.path)}
        disabled={!onNewAgent}
      >
        <Icon name="plus" size={12} />New agent
      </button>
    </div>
  </div>

  <div class="pf-fpb-tabs">
    <button type="button" class="pf-fpb-tab" data-active={tab === "board"} onclick={() => (tab = "board")}>
      Board<span class="n">{agents.length}</span>
    </button>
    <button type="button" class="pf-fpb-tab" data-active={tab === "memory"} onclick={() => (tab = "memory")}>
      Memory<span class="n">{memoryFiles.length}</span>
    </button>
  </div>

  {#if tab === "board"}
    <div class="pf-fpb-cols">
      {#each columns as column (column.id)}
        <section class="pf-fpb-col">
          <div class="head">
            <span class={`pip ${column.id}`}></span>
            <span class="t">{column.label}</span>
            <span class="n">{column.agents.length}</span>
          </div>
          <div class="pf-fpb-col-hint">{column.hint}</div>
          <div class="pf-fpb-col-items">
            {#each column.agents as agent (agent.id)}
              <button
                type="button"
                class="pf-pw-chip"
                data-status={column.id}
                onclick={() => onOpenAgent?.(agent.id)}
              >
                <div class="row">
                  <Icon name="bot" size={15} />
                  <span class="agent-name">{agent.name}</span>
                  <span class="elapsed">{agent.elapsed}</span>
                </div>
                <div class="title">{agent.title}</div>
                <div class="meta">
                  {#if agent.branch}<span class="mono">{agent.branch}</span><span class="sep">·</span>{/if}
                  <span>{agent.step}</span>
                </div>
              </button>
            {:else}
              <div class="empty">No agents</div>
            {/each}
          </div>
        </section>
      {/each}
    </div>
  {:else}
    <div class="pf-pmem">
      <div class="pf-pmem-list">
        <div class="pf-pmem-list-head">
          <Icon name="file" size={12} />
          Memory files
          <span class="n">{memoryFiles.length}</span>
        </div>
        {#each memoryFiles as file (file.id)}
          <button
            type="button"
            class="pf-pmem-file"
            data-active={selectedMemory?.id === file.id}
            onclick={() => (selectedMemoryId = file.id)}
          >
            <span class="dot" data-kind={file.kind}></span>
            <span class="name">{file.name}</span>
            <span class="time">{file.updated}</span>
          </button>
        {/each}
      </div>
      {#if selectedMemory}
        <article class="pf-pmem-detail">
          <div class="pf-pmem-detail-head">
            <span class="pf-pmem-kind" data-kind={selectedMemory.kind}>{selectedMemory.kind}</span>
            <span class="path">{selectedMemory.path}</span>
            <div class="pf-pmem-actions">
              {#if memoryEditing}
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="ghost"
                  data-size="sm"
                  disabled={memorySaving}
                  onclick={cancelMemoryEdit}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="default"
                  data-size="sm"
                  disabled={memorySaving}
                  onclick={saveMemoryEdit}
                >
                  <Icon name="check" size={12} />{memorySaving ? "Saving" : "Save"}
                </button>
              {:else}
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="ghost"
                  data-size="sm"
                  disabled={!canEditMemory}
                  title={canEditMemory ? "Edit memory file" : "Memory file is not editable yet"}
                  onclick={startMemoryEdit}
                >
                  <Icon name="edit" size={12} />Edit
                </button>
              {/if}
            </div>
          </div>
          <div class="pf-pmem-detail-body">
            <h2 class="pf-pmem-title">{selectedMemory.title}</h2>
            <div class="pf-pmem-byline">
              <b>{selectedMemory.name}</b>
              <span class="sep">·</span>
              <span>updated {selectedMemory.updated} ago</span>
            </div>
            {#if memoryError}
              <div class="pf-pmem-status" data-kind="error">{memoryError}</div>
            {:else if memoryLoading}
              <div class="pf-pmem-status">Loading memory file…</div>
            {/if}
            {#if memoryEditing}
              <textarea
                class="pf-pmem-editor"
                aria-label="Memory file content"
                bind:value={memoryDraft}
                disabled={memorySaving}
              ></textarea>
            {:else}
              <div class="pf-pmem-body">
                {#if memoryDisplayBody.trim()}
                  {#each memoryDisplayBody.split("\n\n") as paragraph}
                    <p>{paragraph}</p>
                  {/each}
                {:else}
                  <p class="muted">This memory file is empty.</p>
                {/if}
              </div>
            {/if}
            <div class="pf-pmem-tags">
              {#each selectedMemory.tags as tag}
                <span class="pf-pmem-tag">#{tag}</span>
              {/each}
            </div>
          </div>
        </article>
      {:else}
        <div class="pf-pmem-empty">
          <div class="pf-pmem-empty-inner">
            <div class="title">No memory files yet</div>
            <div class="sub">Project memory will appear here as files under <code>.puffer/memory</code>.</div>
          </div>
        </div>
      {/if}
    </div>
  {/if}
</div>
