<!--
  Composer — bottom-of-page command line. Anatomy:
    [+ attach 48×48]  [ pill input  flex 1  50h ]  [↑ send 36×36 cream]
  See DESIGN_SYSTEM.md → Composer.

  Behaviors:
    - Submit (Enter or send button) on a non-empty value:
        * If the caller passed `onsubmit`, hand the trimmed text off and
          let the page decide what to do (HomeEmpty still keeps its toast
          for the connect-an-app hero state).
        * Otherwise default to the chat-session flow:
            – If we're already on `/agent/:sessionId`, push the user's
              text into that session's transcript via `appendUserMessage`.
            – Otherwise, mint a fresh session id with
              `createSessionFromText` and `navigate` to `/agent/<id>` so
              the user sees their message land in a brand-new thread.
      Empty / whitespace-only values are ignored — no nav, no clear.
    - The `+` attach button is mock-only for now and surfaces
      `pushToast('Attach: coming soon', 'info')` on every click.
-->
<script lang="ts">
  import { Plus, ArrowUp } from "lucide-svelte";

  import { pushToast } from "../../lib/toast.svelte";
  import { appendUserMessage, createSessionFromText } from "../../lib/chat.svelte";
  import { currentRoute, navigate } from "../../router.svelte";

  interface Props {
    placeholder: string;
    value?: string;
    onsubmit?: (text: string) => void;
  }

  let { placeholder, value = $bindable(""), onsubmit }: Props = $props();

  /**
   * Pull the session id out of `/agent/<id>` when we're already inside
   * the agent surface. Returns null otherwise so the submit handler can
   * decide whether to create a new session or append to the active one.
   */
  function activeAgentSessionId(): string | null {
    const path = currentRoute.path;
    if (!path.startsWith("/agent/")) return null;
    const rest = path.slice("/agent/".length);
    if (!rest) return null;
    // Guard against trailing segments — `/agent/calendar/foo` shouldn't
    // count as the calendar session.
    if (rest.includes("/")) return null;
    return rest;
  }

  function handleSubmit(event?: SubmitEvent): void {
    event?.preventDefault();
    const text = (value ?? "").trim();
    if (!text) return;
    if (onsubmit) {
      onsubmit(text);
    } else {
      const active = activeAgentSessionId();
      if (active) {
        appendUserMessage(active, text);
      } else {
        const sessionId = createSessionFromText(text);
        navigate(`/agent/${sessionId}`);
      }
    }
    value = "";
  }

  function handleKey(event: KeyboardEvent): void {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      handleSubmit();
    }
  }

  function handleAttach(): void {
    pushToast("Attach: coming soon", "info");
  }
</script>

<form class="composer" onsubmit={handleSubmit}>
  <button
    class="composer__attach"
    type="button"
    aria-label="Add attachment"
    onclick={handleAttach}
  >
    <Plus size={22} strokeWidth={1.5} aria-hidden="true" />
  </button>

  <div class="composer__pill">
    <input
      class="composer__input"
      type="text"
      {placeholder}
      bind:value
      onkeydown={handleKey}
      aria-label="Message"
    />
    <button
      class="composer__send"
      type="submit"
      aria-label="Send"
      disabled={!value || !value.trim()}
    >
      <ArrowUp size={16} strokeWidth={2} aria-hidden="true" />
    </button>
  </div>
</form>

<style>
  .composer {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 18px 0;
    border-top: 1px solid var(--color-card-border);
    width: 100%;
  }

  .composer__attach {
    width: var(--height-button-utility);
    height: var(--height-button-utility);
    border-radius: var(--radius-pill);
    background: var(--color-surface-input);
    border: 1px solid var(--color-input-border-soft);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--color-text-primary);
    flex-shrink: 0;
    transition: background-color 120ms ease;
  }
  .composer__attach:hover {
    background: var(--color-surface-rail);
  }

  .composer__pill {
    display: flex;
    align-items: center;
    flex: 1;
    height: var(--height-composer-input);
    border-radius: var(--radius-pill);
    padding: 6px 8px 6px 16px;
    gap: var(--space-2);
    background: var(--color-surface-input);
    border: 1px solid var(--color-input-border);
    min-width: 0;
  }

  .composer__input {
    flex: 1;
    height: 100%;
    border: 0;
    outline: 0;
    background: transparent;
    font-family: var(--font-system);
    font-size: var(--font-size-task);
    line-height: var(--line-height-task);
    color: var(--color-text-primary);
    min-width: 0;
  }

  .composer__input::placeholder {
    color: var(--color-text-muted);
  }

  .composer__send {
    width: var(--height-button-composer);
    height: var(--height-button-composer);
    border-radius: var(--radius-pill);
    background: var(--color-action-cream-soft);
    color: var(--color-action-cream-text);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    transition: filter 120ms ease, opacity 120ms ease;
  }
  .composer__send:hover:not(:disabled) {
    filter: brightness(0.97);
  }
  .composer__send:disabled {
    opacity: 0.6;
    cursor: default;
  }
</style>
