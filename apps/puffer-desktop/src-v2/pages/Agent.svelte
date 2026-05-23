<!--
  Agent — chat surface that drives `/agent/:taskId`.

  Two flavours of `taskId` flow through here:

    1. Scripted tasks (`calendar`, `restaurant`) — keyed against
       src-v2/data/conversations.ts and rendered with the rich step
       components (ChatBubble / AgentText / ToolBlock / OptionsCard /
       ResultCard) per the Paper artboards (1UH-0, 1YY-0).
    2. Ad-hoc chat sessions minted by Composer (`chat-<...>`) — rendered
       as a simple user/assistant bubble stream from the chat store.

  Either way the user can keep typing in the bottom composer; Composer
  itself detects the active `/agent/<id>` path and routes new turns to
  `appendUserMessage`. The scripted tasks are seeded into the same chat
  store on first read so subsequent turns sit beneath the static design.

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
  import AgentText from "../components/agent/AgentText.svelte";
  import ToolBlock from "../components/agent/ToolBlock.svelte";
  import OptionsCard from "../components/agent/OptionsCard.svelte";
  import ResultCard from "../components/agent/ResultCard.svelte";
  import Mascot from "../components/common/Mascot.svelte";
  import { findConversation } from "../data/conversations";
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";
  import { chatSessions, ensureSession, type ChatMessage } from "../lib/chat.svelte";

  interface Props {
    taskId?: string;
  }

  let { taskId }: Props = $props();

  let conversation = $derived(taskId ? findConversation(taskId) : undefined);
  /** Treat anything that isn't one of the scripted slugs as a dynamic session. */
  let isDynamic = $derived(!!taskId && !conversation);

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

  // Track which option (by stepIdx + primary text) is currently selected.
  let selectedOption = $state<string | null>(null);

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

  /**
   * Delegated click handler for OptionsCard rows. OptionsCard renders
   * <div class="row"> children; we intercept clicks on them, lift the
   * primary text, and toast + visually mark the row.
   */
  function onOptionsClick(stepIdx: number, event: MouseEvent): void {
    const target = event.target as HTMLElement | null;
    const row = target?.closest(".row") as HTMLElement | null;
    if (!row) return;
    const primary = row.querySelector(".row__primary")?.textContent?.trim() ?? "";
    if (!primary) return;
    selectedOption = `${stepIdx}::${primary}`;
    // Apply a class to mark it (cleared if a sibling is picked).
    const allRows = row.parentElement?.querySelectorAll(".row");
    allRows?.forEach((r) => r.classList.remove("row--picked"));
    row.classList.add("row--picked");
    pushToast(`Selected: ${primary}`, "success");
  }

  function onResultAction(label: string): void {
    pushToast(`Action: ${label}`, "success");
  }

  /** Title for dynamic sessions: use the first user turn so the header reads
   *  like the user's intent rather than a generic placeholder. Falls back to
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
  {:else if conversation}
    <!-- Scripted task: rich timeline backed by conversations.ts, with
         continued turns appended via the shared chat store. -->
    <header class="agent__header">
      <h1 class="text-section">{conversation.title}</h1>
    </header>

    <section class="agent__thread" bind:this={threadEl} aria-label="Conversation">
      <div class="agent__thread-inner">
        {#each conversation.steps as step, index (index)}
          {#if step.kind === "user"}
            <ChatBubble text={step.text} />
          {:else if step.kind === "agent"}
            <AgentText text={step.text} />
          {:else if step.kind === "tool"}
            <div
              role="button"
              tabindex="0"
              class="tool-wrap"
              onclick={() => pushToast(`Tool: ${step.label}`, "info")}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  pushToast(`Tool: ${step.label}`, "info");
                }
              }}
            >
              <ToolBlock icon={step.icon} label={step.label} />
            </div>
          {:else if step.kind === "options"}
            <div
              role="presentation"
              class="options-wrap"
              onclick={(e) => onOptionsClick(index, e)}
            >
              <OptionsCard intro={step.intro} options={step.options} footnote={step.footnote} />
            </div>
          {:else if step.kind === "result"}
            <div class="result-wrap">
              <ResultCard {step} />
              {#if step.actions && step.actions.length > 0}
                <div class="result-actions">
                  {#each step.actions as action (action.label)}
                    <button
                      type="button"
                      class="result-action-btn"
                      onclick={() => onResultAction(action.label)}
                    >
                      {action.label}
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}
        {/each}

        <!-- Continued turns the user adds after the scripted timeline. We
             slice off the seed user/agent turns (matched by id prefix) so
             we don't double-render lines that already showed up above. -->
        {#each chatMessages.filter((m) => !m.id.startsWith("seed-")) as message (message.id)}
          {#if message.role === "user"}
            <ChatBubble text={message.text} />
          {:else}
            <div class="assistant-row">
              <div class="assistant-avatar"><Mascot size="sm" /></div>
              <div class="assistant-bubble">
                {#if message.pending}
                  <span class="typing" aria-label="Momo is typing">
                    <span class="typing__dot"></span>
                    <span class="typing__dot"></span>
                    <span class="typing__dot"></span>
                  </span>
                {:else}
                  <p class="assistant-bubble__text">{message.text}</p>
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
             appends to the active session via the chat store. -->
        <Composer placeholder={conversation.composerPlaceholder} />
      </div>
    </div>
  {:else if isDynamic}
    <!-- Dynamic chat session — purely user / assistant turns from the store. -->
    <header class="agent__header">
      <h1 class="text-section">{dynamicTitle(chatMessages)}</h1>
    </header>

    <section class="agent__thread" bind:this={threadEl} aria-label="Conversation">
      <div class="agent__thread-inner">
        {#each chatMessages as message (message.id)}
          {#if message.role === "user"}
            <ChatBubble text={message.text} />
          {:else}
            <div class="assistant-row">
              <div class="assistant-avatar"><Mascot size="sm" /></div>
              <div class="assistant-bubble">
                {#if message.pending}
                  <span class="typing" aria-label="Momo is typing">
                    <span class="typing__dot"></span>
                    <span class="typing__dot"></span>
                    <span class="typing__dot"></span>
                  </span>
                {:else}
                  <p class="assistant-bubble__text">{message.text}</p>
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
             appends to the active session via the chat store. -->
        <Composer placeholder="Hi, Tomo. How's my luck today?" />
      </div>
    </div>
  {:else}
    <section class="empty">
      <h1 class="text-section">No conversation found</h1>
      <p class="text-body-compact empty__hint">
        We couldn't find a conversation for <code>{taskId}</code>.
      </p>
      <a class="empty__link" href="#/home" onclick={goHome}>← Back to home</a>
    </section>
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

  /* ── Step-component wrappers ─────────────────────────────────────── */

  .options-wrap {
    display: contents;
  }
  .tool-wrap {
    display: inline-flex;
    cursor: pointer;
    border-radius: var(--radius-pill);
  }
  .tool-wrap:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }

  .options-wrap :global(.row.row--picked) {
    background: var(--color-selected-fill, #fff7e8);
    border-color: var(--color-text-primary);
  }

  .result-wrap {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .result-actions {
    display: flex;
    gap: var(--space-2);
  }
  .result-action-btn {
    appearance: none;
    height: var(--height-button-card);
    padding: 0 var(--space-4);
    border-radius: var(--radius-pill);
    border: 0;
    background: var(--color-action-cream);
    color: var(--color-action-cream-text);
    font-family: var(--font-sans);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    transition: filter 120ms ease;
  }
  .result-action-btn:hover {
    filter: brightness(0.97);
  }

  /* ── Assistant bubble (dynamic + continued turns) ────────────────── */

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
