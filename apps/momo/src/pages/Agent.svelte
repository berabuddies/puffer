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
  import ConversationView from "../lib/agent/ConversationView.svelte";
  import { createController } from "../lib/agent/agentChat.svelte";
  import { navigate } from "../router.svelte";
  import type { TimelineItem } from "../lib/agent/types";

  interface Props {
    taskId?: string;
  }

  let { taskId }: Props = $props();

  // Bind a chat controller to this session id. `createController` calls
  // `ensureState`, which *mutates* the module-level `chatStates` $state. Svelte 5
  // forbids that in a `$derived` / template read scope (`state_unsafe_mutation`,
  // fatal in WebKit/Tauri — see agentChat.svelte.ts:129-134), and direct
  // navigation to `/agent/<id>` (sidebar click / Home task card / page reload)
  // hits exactly that path with no pre-seeded state. So the controller is seeded
  // inside a `$effect` (mutation allowed) and stays null until the first effect
  // run; every template/derived read below null-guards it. The underlying state
  // lives at module scope, so the controller survives the `{#key}` route remount.
  let controller = $state<ReturnType<typeof createController> | null>(null);

  // Seed the controller, subscribe this session's daemon events into the
  // reducer, and hydrate the persisted transcript. All idempotent; the effect
  // re-runs when `taskId` changes (new route → new session).
  $effect(() => {
    if (!taskId) {
      controller = null;
      return;
    }
    const c = createController(taskId);
    c.ensureSubscription();
    void c.loadDetail();
    controller = c;
  });

  // Pull the rendered timeline through the controller getters. These recompute
  // from `$state` on read, so they stay reactive inside the template/derived.
  // `controller` is null on the very first frame (before the $effect runs),
  // hence the guards.
  let timeline = $derived<TimelineItem[]>(controller ? controller.combinedTimeline() : []);
  let pendingPermissions = $derived(controller ? controller.pendingPermissions() : []);
  let pendingQuestions = $derived(controller ? controller.pendingQuestions() : []);
  let turnRunning = $derived(controller ? controller.turnRunning() : false);
  // Surface the live turn timing/status so ConversationView's typing indicator
  // can show "Thinking (3.2s)" / the current status hint instead of a static
  // "Running". Without these props its 100ms elapsed-timer `$effect` had no
  // consumer (formatElapsed always returned "") and just span-idled.
  let turnStartedAtMs = $derived(controller ? controller.state().turnStartedAtMs : null);
  let turnThinking = $derived(controller ? controller.state().turnThinking : false);
  let turnStatusHint = $derived(controller ? controller.state().turnStatusHint : null);
  // Hydration loading state: the persisted detail hasn't landed yet and the
  // live stream is empty. `ConversationView` shows its own "Loading…" row when
  // `loading && timeline.length === 0`. Treat the pre-seed frame as loading too.
  let loading = $derived(
    controller ? controller.state().sessionDetail === null && timeline.length === 0 : Boolean(taskId)
  );
  // ConversationView's composer is gated on `session`; momo drives input from
  // the shell <Composer> below instead, so the session lets the timeline +
  // approval prompts scope correctly without surfacing a second input.
  let session = $derived(controller ? (controller.state().sessionDetail?.session ?? null) : null);

  function goHome(event: MouseEvent): void {
    event.preventDefault();
    navigate("/home");
  }

  /** Title for the page header: use the first user turn so it reads like
   *  the user's intent rather than a generic placeholder. Falls back to
   *  "New chat" before the first turn lands. */
  function dynamicTitle(items: TimelineItem[]): string {
    const first = items.find((item) => item.kind === "user");
    const txt = (first?.body ?? "").trim();
    if (!txt) return "New chat";
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
      <h1 class="text-section">{dynamicTitle(timeline)}</h1>
    </header>

    <!--
      Thread surface. `ConversationView` is the desktop-style timeline renderer
      (text / tools / thinking / approvals / questions / diffs) — it consumes
      the controller's `TimelineItem[]` directly. Its own internal composer is
      hidden via the `:global(.pf-composer-wrap)` rule below; momo drives input
      through the shell <Composer> row underneath so the page keeps a single
      visible input. `onSubmitMessage` is therefore intentionally inert here
      (the hidden composer never fires it) — sending goes through the shell
      Composer, NOT this prop, so there's no double-send. The Approval /
      QuestionPrompt callbacks still route through ConversationView so pending
      requests resolve inline with their turn.
    -->
    <section class="agent__thread" aria-label="Conversation">
      <ConversationView
        {session}
        {timeline}
        {pendingPermissions}
        {pendingQuestions}
        {turnRunning}
        {turnStartedAtMs}
        {turnThinking}
        {turnStatusHint}
        {loading}
        onSubmitMessage={(message) => controller?.appendUserMessage(message)}
        onResolvePermission={(id, choice) => controller?.resolvePermission(id, choice)}
        onResolveUserQuestion={(id, answers, annotations) =>
          controller?.resolveUserQuestion(id, answers, annotations)}
        onCancelTurn={() => controller?.cancelCurrentTurn()}
      />
    </section>

    <div class="agent__composer">
      <div class="agent__composer-inner">
        <!-- No onsubmit: Composer's default branch sees /agent/<id> and
             appends to the active session via the chat controller. Pass
             `running` so the Composer renders a red Stop button while a
             turn is in flight; click routes through `cancelCurrentTurn`
             which fires `cancel_turn` via the daemon client. -->
        <Composer
          placeholder="Hi, Tomo. How's my luck today?"
          running={turnRunning}
          onCancel={() => controller?.cancelCurrentTurn()}
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

  /* The thread row hosts <ConversationView>, which owns its own internal
   * scroller (`.pf-chat-thread`) and centering. We just give it the flex
   * slot between the sticky header and the composer; the negative margin
   * lets the conversation surface span the full page width while the Shell
   * column keeps its horizontal padding for the header / composer. */
  .agent__thread {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    margin: 0 calc(-1 * var(--shell-page-padding));
  }

  /* ConversationView is a self-contained chat surface (thread + composer).
   * momo drives input from the shell <Composer> below, so suppress
   * ConversationView's own composer to avoid a duplicate input. */
  .agent__thread :global(.pf-composer-wrap) {
    display: none;
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
</style>
