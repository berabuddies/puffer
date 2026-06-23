<script lang="ts">
  type SaveState = "idle" | "saving" | "saved" | "error";
  type SubmitState = "idle" | "submitting" | "submitted" | "error";

  type Props = {
    label?: string;
    disabled?: boolean;
    disabledLabel?: string;
    message?: string | null;
    saveState: SaveState;
    submitState: SubmitState;
    onSubmit: () => void | Promise<void>;
  };

  let {
    label = "Submit choices",
    disabled = false,
    disabledLabel = "Preparing...",
    message = null,
    saveState,
    submitState,
    onSubmit
  }: Props = $props();

  const buttonDisabled = $derived(disabled || submitState === "submitting");
  const buttonLabel = $derived(disabled ? disabledLabel : label);
  const showSaveState = $derived(saveState === "saving");
</script>

<div class="canvas-submit-row">
  <button
    type="button"
    class="canvas-submit-button"
    onclick={onSubmit}
    disabled={buttonDisabled}
  >
    {buttonLabel}
  </button>
  {#if showSaveState}
    <span class="canvas-submit-state" data-state={saveState}>saving</span>
  {/if}
  {#if submitState !== "idle"}
    <span class="canvas-submit-state" data-state={submitState}>{submitState}</span>
  {/if}
  {#if message}
    <span class="canvas-submit-message" title={message}>{message}</span>
  {/if}
</div>

<style>
  .canvas-submit-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 7px;
    padding-top: 2px;
  }
  .canvas-submit-button {
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 42%, var(--border));
    border-radius: 4px;
    background: color-mix(in oklab, var(--puffer-accent) 12%, var(--background));
    color: var(--foreground);
    cursor: pointer;
    font: inherit;
    font-size: var(--pf-chat-meta-size);
    font-weight: 600;
    padding: 4px 10px;
  }
  .canvas-submit-button:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .canvas-submit-state {
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 6px;
    color: var(--muted-foreground);
    background: color-mix(in oklab, var(--muted) 28%, transparent);
    font-family: var(--font-mono);
    font-size: var(--pf-chat-meta-size);
  }
  .canvas-submit-state[data-state="saving"],
  .canvas-submit-state[data-state="submitting"] {
    color: var(--puffer-accent);
  }
  .canvas-submit-state[data-state="saved"],
  .canvas-submit-state[data-state="submitted"] {
    color: oklch(0.58 0.12 145);
  }
  .canvas-submit-state[data-state="error"] {
    color: var(--destructive);
  }
  .canvas-submit-message {
    min-width: 0;
    max-width: 100%;
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
