<!--
  ConfirmDisconnectModal — small confirm dialog before wiping a
  connector's persisted credentials.

  Used by ConnectedApps.svelte when the user clicks a card that is
  already "Connected". On confirm, the parent calls
  `connectorDisconnect(kind)` and re-queries status. The modal itself
  has no knowledge of the disconnect mechanism — it just collects the
  yes/no decision.
-->
<script lang="ts">
  import Modal from "../common/Modal.svelte";
  import Button from "../common/Button.svelte";

  interface Props {
    open: boolean;
    /** Display name of the app being disconnected (e.g. "Telegram"). */
    name: string;
    /** True while the disconnect RPC is in flight. */
    busy?: boolean;
    onConfirm: () => void;
    onClose: () => void;
  }

  let { open, name, busy = false, onConfirm, onClose }: Props = $props();
</script>

<Modal {open} {onClose} title={`Disconnect ${name}?`} maxWidth="400px">
  <div class="cd-body">
    <p class="cd-help">
      Momo will forget your {name} credentials. You'll need to sign in
      again the next time you want to use it.
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
        label={busy ? "Disconnecting…" : "Disconnect"}
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
