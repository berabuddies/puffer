<script lang="ts">
  import { onMount } from "svelte";
  import { emit } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";

  // Compact launcher rendered in the hotkey-summoned mini window. It does not
  // stream the conversation itself — it hands the prompt to the main window
  // (which owns sessions + streaming) and dismisses itself.
  let text = $state("");
  let sending = $state(false);
  let input: HTMLTextAreaElement | undefined;

  async function hide() {
    try {
      await getCurrentWindow().hide();
    } catch {
      // running outside Tauri (storybook/web) — no-op
    }
  }

  async function submit() {
    const value = text.trim();
    if (!value || sending) return;
    sending = true;
    try {
      await emit("puffer://mini-submit", value);
      text = "";
      await hide();
    } finally {
      sending = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void submit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      void hide();
    }
  }

  onMount(() => {
    input?.focus();
    // ChatGPT-mini behavior: dismiss when it loses focus.
    const onBlur = () => void hide();
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  });
</script>

<div class="mini">
  <div class="drag" data-tauri-drag-region>
    <span class="dot"></span>
    <span class="label">puffer</span>
  </div>
  <textarea
    bind:this={input}
    bind:value={text}
    onkeydown={onKeydown}
    rows="3"
    placeholder="Ask puffer…"
    spellcheck="false"
  ></textarea>
  <div class="row">
    <span class="hint">Enter to send · Esc to dismiss</span>
    <button onclick={() => void submit()} disabled={!text.trim() || sending}>Send</button>
  </div>
</div>

<style>
  :global(html.is-mini),
  :global(html.is-mini body),
  :global(html.is-mini #app) {
    background: transparent;
    margin: 0;
    height: 100%;
  }

  .mini {
    display: flex;
    flex-direction: column;
    height: 100vh;
    box-sizing: border-box;
    padding: 8px 10px 10px;
    gap: 6px;
    background: var(--mini-bg, #1c1d22);
    color: var(--mini-fg, #e8e8ea);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 12px;
    font: 13px/1.4 -apple-system, system-ui, sans-serif;
  }

  .drag {
    display: flex;
    align-items: center;
    gap: 6px;
    -webkit-app-region: drag;
    cursor: default;
    user-select: none;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #5b8cff;
  }
  .label {
    font-size: 11px;
    letter-spacing: 0.04em;
    opacity: 0.6;
  }

  textarea {
    flex: 1;
    resize: none;
    border: none;
    outline: none;
    background: transparent;
    color: inherit;
    font: inherit;
    padding: 0;
  }
  textarea::placeholder {
    color: rgba(255, 255, 255, 0.35);
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .hint {
    font-size: 10px;
    opacity: 0.4;
  }
  button {
    border: none;
    border-radius: 7px;
    padding: 4px 12px;
    font: inherit;
    font-weight: 600;
    color: #fff;
    background: #5b8cff;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.4;
    cursor: default;
  }
</style>
