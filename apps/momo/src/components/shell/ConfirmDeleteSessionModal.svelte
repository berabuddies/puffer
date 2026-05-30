<!--
  ConfirmDeleteSessionModal — small confirm dialog before permanently
  deleting a chat session.

  Used by Sidebar.svelte when the user clicks a session row's Trash icon.
  Deleting wipes the session's transcript + all sidecars on the daemon and
  can't be undone, so the rail collects an explicit yes/no first. On confirm
  the parent calls the sessionStore's `deleteSession(id)`; the modal itself
  has no knowledge of the delete mechanism — it just gathers the decision.
-->
<script lang="ts">
  import Modal from "../common/Modal.svelte";
  import Button from "../common/Button.svelte";

  interface Props {
    open: boolean;
    /** Display title of the session being deleted (for the prompt copy). */
    name: string;
    /** True while the delete RPC is in flight. */
    busy?: boolean;
    onConfirm: () => void;
    onClose: () => void;
  }

  let { open, name, busy = false, onConfirm, onClose }: Props = $props();
</script>

<Modal {open} {onClose} title="Delete chat?" maxWidth="400px">
  <div class="cd-body">
    <p class="cd-help">
      “{name}” and its full conversation history will be permanently
      deleted. This can’t be undone.
    </p>
    <div class="cd-actions">
      <Button
        label="Cancel"
        variant="secondary"
        size="md"
        onclick={onClose}
        disabled={busy}
      />
      <Button
        label={busy ? "Deleting…" : "Delete"}
        variant="primary"
        size="md"
        onclick={onConfirm}
        disabled={busy}
      />
    </div>
  </div>
</Modal>

<style>
  .cd-body {
    padding: var(--space-4) 20px 20px 20px;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .cd-help {
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 18px;
    color: var(--color-text-secondary);
  }

  .cd-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-3);
  }
</style>
