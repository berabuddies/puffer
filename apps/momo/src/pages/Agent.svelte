<!--
  Agent — chat surface that drives `/agent/:sessionId`.

  Every session here is a puffer-backed dynamic chat. The scripted
  calendar / restaurant demos were removed; the rich timeline components
  (OptionsCard / ResultCard / ToolBlock) remain on disk as reusable
  primitives but are no longer imported here.

  Composer detects the active `/agent/<id>` path and routes new turns to
  `appendUserMessage` against the same session.

  Layout (verified against the Shell's overflow column):
    - Page fills 100% height with a column flex layout, min-height: 0 so
      the middle scroller can shrink.
    - Header is `position: sticky; top: 0` with the app surface fill so
      the title doesn't slide under the traffic-light spacer.
    - Conversation stream is the only scrolling region (`overflow-y:
      auto`, max-width 760, 24px gutter).
    - Composer is the third row, pinned to the bottom of the page column
      with a top divider.
-->
<script lang="ts">
  import Composer from "../components/shell/Composer.svelte";
  import ChatBubble from "../components/agent/ChatBubble.svelte";
  import Mascot from "../components/common/Mascot.svelte";
  import MessageBody from "../components/common/MessageBody.svelte";
  import { formatTime } from "../lib/timeFormat";
  import { navigate } from "../router.svelte";
  import {
    chatSessions,
    ensureSession,
    getHydrationState,
    retryHydration,
    runningTurnBySessionId,
    cancelRunningTurn,
    type ChatMessage
  } from "../lib/chat.svelte";

  interface Props {
    taskId?: string;
  }

  let { taskId }: Props = $props();

  // Materialise the session list lazily via $effect so the mutation lives
  // outside derived/template scope (otherwise Svelte 5 raises
  // `state_unsafe_mutation` — fatal in WebKit/Tauri). Once ensureSession
  // writes the entry, `chatSessions[taskId]` becomes a reactive read and
  // re-renders just like a $derived would.
  $effect(() => {
    if (taskId) ensureSession(taskId);
  });
  let chatMessages = $derived<ChatMessage[]>(
    taskId ? (chatSessions[taskId] ?? []) : []
  );
  // Track whether this session has a live turn so the Composer can swap
  // its Send button for a red Stop. Cleared from chat.svelte.ts on
  // turn-complete / turn-error.
  let isRunning = $derived(taskId ? Boolean(runningTurnBySessionId[taskId]) : false);
  let hydrationPhase = $derived(taskId ? getHydrationState(taskId) : "idle");
  let showLoadingState = $derived(hydrationPhase === "loading" && chatMessages.length === 0);
  let showErrorState = $derived(hydrationPhase === "error" && chatMessages.length === 0);

  /** The scroll viewport for the conversation. Bound in the template. */
  let threadEl = $state<HTMLElement | null>(null);

  // Auto-scroll to the bottom whenever the visible message count changes
  // (covers both new user turns and pending → resolved assistant flips).
  $effect(() => {
    // Read length so this effect tracks `chatMessages` reactively.
    chatMessages.length;
    // Also re-run when a pending bubble flips to resolved (text fills in).
    chatMessages.forEach((m) => {
      void m.pending;
      void m.text;
    });
    if (!threadEl) return;
    // Defer one frame so freshly inserted nodes are measured first.
    queueMicrotask(() => {
      if (threadEl) threadEl.scrollTop = threadEl.scrollHeight;
    });
  });

  function goHome(event: MouseEvent): void {
    event.preventDefault();
    navigate("/home");
  }

  /** Title for the page header: use the first user turn so it reads like
   *  the user's intent rather than a generic placeholder. Falls back to
   *  "New chat" before the first turn lands. */
  function dynamicTitle(messages: ChatMessage[]): string {
    const first = messages.find((m) => m.role === "user");
    if (!first) return "New chat";
    const txt = first.text.trim();
    return txt.length > 60 ? `${txt.slice(0, 57)}…` : txt;
  }
</script>

<div class="agent">
  {#if !taskId}
    <section class="empty">
      <h1 class="text-section">No conversation found</h1>
      <p class="text-body-compact empty__hint">
        We couldn't find a conversation for <code>?</code>.
      </p>
      <a class="empty__link" href="#/home" onclick={goHome}>← Back to home</a>
    </section>
  {:else}
    <header class="agent__header">
      <h1 class="text-section">{dynamicTitle(chatMessages)}</h1>
    </header>

    <section class="agent__thread" bind:this={threadEl} aria-label="Conversation">
      <div class="agent__thread-inner">
        {#if showLoadingState}
          <div class="hydration-status" data-testid="hydration-loading">
            <span class="hydration-status__spinner" aria-hidden="true">
              <span class="hydration-status__dot"></span>
              <span class="hydration-status__dot"></span>
              <span class="hydration-status__dot"></span>
            </span>
            <p class="hydration-status__text">Loading conversation…</p>
          </div>
        {:else if showErrorState}
          <div class="hydration-status" data-testid="hydration-error">
            <p class="hydration-status__text">Failed to load history</p>
            <button
              type="button"
              class="hydration-status__retry"
              onclick={() => taskId && retryHydration(taskId)}
            >
              Retry
            </button>
          </div>
        {/if}
        {#each chatMessages as message (message.id)}
          {#if message.role === "user"}
            <ChatBubble text={message.text} createdAt={message.createdAt} />
          {:else}
            <div class="assistant-row" data-error={message.error ? "true" : undefined}>
              <div class="assistant-avatar"><Mascot size="sm" /></div>
              <div class="assistant-bubble">
                {#if message.pending}
                  <span class="typing" aria-label="Momo is typing">
                    <span class="typing__dot"></span>
                    <span class="typing__dot"></span>
                    <span class="typing__dot"></span>
                  </span>
                {:else}
                  <div class="assistant-bubble__text">
                    <MessageBody body={message.text} />
                  </div>
                {/if}
                {#if !message.pending && message.createdAt}
                  <span class="assistant-bubble__time">{formatTime(message.createdAt)}</span>
                {/if}
              </div>
            </div>
          {/if}
        {/each}
      </div>
    </section>

    <div class="agent__composer">
      <div class="agent__composer-inner">
        <!-- No onsubmit: Composer's default branch sees /agent/<id> and
             appends to the active session via the chat store. Pass
             `running` so the Composer renders a red Stop button while a
             turn is in flight; click routes through `cancelRunningTurn`
             which fires `cancel_turn` via the WS client. -->
        <Composer
          placeholder="Hi, Tomo. How's my luck today?"
          running={isRunning}
          onCancel={() => taskId && cancelRunningTurn(taskId)}
        />
      </div>
    </div>
  {/if}
</div>

<style>
  /* Three-row vertical layout. `agent` claims the full Shell column so the
   * middle thread is the only thing that scrolls — the header stays
   * pinned at the top and the composer stays pinned at the bottom.
   *
   * This relies on the Shell rendering us in `fullbleed` mode (see
   * routes.ts `/agent/:taskId` → `fullbleed: true`), which locks
   * .page__column to viewport height and drops its top padding. Without
   * fullbleed the column would grow with content and the composer would
   * scroll off-screen. */
  .agent {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    height: 100%;
    width: 100%;
  }

  .agent__header {
    flex-shrink: 0;
    background: var(--color-surface-app);
    /* Header pushes itself beneath the traffic-light spacer; fullbleed
     * column has no top padding so we do it here. */
    padding: calc(var(--shell-traffic-spacer) + var(--space-2)) 0 var(--space-4);
    border-bottom: 1px solid var(--color-card-border);
  }

  .agent__thread {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    /* Counteract the Shell column's horizontal padding so the scroll
     * region spans the full page width — content is re-centered inside. */
    margin: 0 calc(-1 * var(--shell-page-padding));
    padding: var(--space-5) var(--shell-page-padding);
  }

  .agent__thread-inner {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: var(--shell-page-max);
    margin: 0 auto;
    width: 100%;
  }

  .agent__composer {
    flex-shrink: 0;
    background: var(--color-surface-app);
    border-top: 1px solid var(--color-card-border);
    margin: 0 calc(-1 * var(--shell-page-padding));
    padding: 0 var(--shell-page-padding);
  }

  .agent__composer-inner {
    max-width: var(--shell-page-max);
    margin: 0 auto;
    width: 100%;
  }

  /* The shared Composer ships its own top border + 18px vertical padding;
   * we already provide the divider on `.agent__composer`, so suppress the
   * inner one to avoid a double rule. */
  .agent__composer :global(.composer) {
    border-top: 0;
  }

  /* ── Empty / fallback view ───────────────────────────────────────── */

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    justify-content: center;
    gap: var(--space-3);
    padding-bottom: var(--space-7);
  }

  .empty__hint {
    color: var(--color-text-muted);
  }

  .empty__hint code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    background: var(--color-surface-rail);
    padding: 1px 6px;
    border-radius: var(--radius-control);
  }

  .empty__link {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    color: var(--color-action-cream-text);
  }
  .empty__link:hover {
    text-decoration: underline;
  }

  /* ── Assistant bubble ────────────────────────────────────────────── */

  .assistant-row {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    width: 100%;
  }

  .assistant-avatar {
    flex-shrink: 0;
    margin-top: 2px;
  }

  .assistant-bubble {
    background: transparent;
    padding: 4px 0;
    min-height: 24px;
    max-width: 600px;
  }

  .assistant-bubble__text {
    margin: 0;
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-primary);
    white-space: pre-wrap;
    word-wrap: break-word;
  }

  .assistant-bubble__time {
    display: block;
    margin-top: 4px;
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
    color: var(--color-text-secondary);
    text-align: right;
  }

  /* Error variant — `data-error="true"` is set by Agent.svelte when the
   * assistant bubble represents a turn-error or session-fetch failure
   * (see chat.svelte.ts:handleSessionEvent turn-error). Cascades into the
   * Markdown subtree via :global(*) so paragraph text inside MessageBody
   * picks up the danger color too. */
  .assistant-row[data-error="true"] .assistant-bubble :global(*) {
    color: var(--color-danger-text, #c0392b);
  }
  .assistant-row[data-error="true"] .assistant-bubble::before {
    content: "⚠ ";
    color: var(--color-danger-text, #c0392b);
  }

  /* ── Hydration loading / error overlays ──────────────────────────
   * Sit at the top of the thread (not the page) so the header and
   * composer stay anchored. The spinner reuses the `.typing` keyframe
   * to stay on brand without pulling in a new asset. */
  .hydration-status {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    padding: var(--space-5) var(--space-3);
    color: var(--color-text-muted);
  }

  .hydration-status__text {
    margin: 0;
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
  }

  .hydration-status__spinner {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  .hydration-status__dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-text-muted);
    opacity: 0.4;
    animation: typing-bounce 1.2s infinite ease-in-out;
  }
  .hydration-status__dot:nth-child(2) {
    animation-delay: 0.15s;
  }
  .hydration-status__dot:nth-child(3) {
    animation-delay: 0.3s;
  }

  .hydration-status__retry {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 28px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-control);
    background: var(--color-action-cream);
    color: var(--color-action-cream-text);
    border: 1px solid var(--color-action-cream-border);
    font-family: var(--font-system);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    cursor: pointer;
  }
  .hydration-status__retry:hover {
    background: var(--color-selected-fill);
  }

  .typing {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 6px 2px;
  }

  .typing__dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-text-muted);
    opacity: 0.4;
    animation: typing-bounce 1.2s infinite ease-in-out;
  }

  .typing__dot:nth-child(2) {
    animation-delay: 0.15s;
  }
  .typing__dot:nth-child(3) {
    animation-delay: 0.3s;
  }

  @keyframes typing-bounce {
    0%, 60%, 100% {
      transform: translateY(0);
      opacity: 0.4;
    }
    30% {
      transform: translateY(-3px);
      opacity: 0.9;
    }
  }
</style>
