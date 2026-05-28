<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import { AGENT_STATE_LABELS, type MockAgent } from "../../data/mockProjects";

  type Props = {
    a: MockAgent;
    onOpen?: () => void;
    onDelete?: () => void | Promise<void>;
    onSetTags?: (tags: string[]) => void | Promise<void>;
  };
  let { a, onOpen, onDelete, onSetTags }: Props = $props();

  let title = $derived(a.title || a.name || "New Session");
  let statusLabel = $derived((AGENT_STATE_LABELS[a.status] ?? a.status).toLowerCase());

  function parseTags(input: string): string[] {
    return input
      .split(/[\s,]+/)
      .map((tag) => tag.trim())
      .filter((tag) => tag.length > 0);
  }

  function handleEditTags(event: Event) {
    event.stopPropagation();
    if (!onSetTags) return;
    const current = (a.tags ?? []).join(", ");
    const raw = window.prompt(
      `Tags for ${title} (comma- or space-separated, blank to clear):`,
      current
    );
    if (raw === null) return;
    void onSetTags(parseTags(raw));
  }

  function handleDelete(event: Event) {
    event.stopPropagation();
    if (!onDelete) return;
    const ok = window.confirm(
      `Delete session "${title}"?\nThis cannot be undone.`
    );
    if (!ok) return;
    void onDelete();
  }
</script>

<div class="pf-pw-agent-wrap" data-status={a.status} title={`${title} - ${statusLabel} - ${a.elapsed}`}>
  <button
    type="button"
    class="pf-pw-agent"
    data-status={a.status}
    onclick={onOpen}
    aria-label={`Open session ${title}`}
  >
    <span class="title">{title}</span>
    <span class="status-pill" data-status={a.status}>{statusLabel}</span>
    <span class="activity">{a.elapsed}</span>
    {#if a.tags && a.tags.length > 0}
      <span class="pf-pw-tags pf-pw-tags-agent" aria-label="Session tags">
        {#each a.tags as tag (tag)}
          <span class="pf-pw-tag">{tag}</span>
        {/each}
      </span>
    {/if}
  </button>
  {#if onSetTags || onDelete}
    <div class="pf-pw-agent-actions">
      {#if onSetTags}
        <button
          type="button"
          class="pf-pw-agent-action"
          onclick={handleEditTags}
          title="Edit session tags"
          aria-label={`Edit tags for ${title}`}
        ><Icon name="edit" size={11} /></button>
      {/if}
      {#if onDelete}
        <button
          type="button"
          class="pf-pw-agent-action"
          onclick={handleDelete}
          title="Delete session"
          aria-label={`Delete session ${title}`}
        ><Icon name="trash" size={11} /></button>
      {/if}
    </div>
  {/if}
</div>
