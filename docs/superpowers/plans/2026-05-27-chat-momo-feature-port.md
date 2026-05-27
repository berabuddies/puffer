# V2 Chat Feature Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port a curated subset of v1 chat features into v2 so end users (Tomo wallet consumers) see streaming text, simple agent activity, interactive forms, and basic chat polish — without v1's developer-tool baggage (model picker, permission modes, thinking levels, file panes).

**Architecture:**
- Extend v2's `ChatMessage` union with three new roles — `thinking`, `tool`, `question` — keeping the store as a flat per-session array (no v1-style row grouping; the small chat surface doesn't need it).
- Reuse v2's existing `ToolBlock.svelte` pill as the visual primitive for transient agent activity (thinking + tool calls).
- Port `MessageBody.svelte` from v1 verbatim (596 lines, zero deps) for Markdown rendering; adapt to Svelte 5 runes.
- Add `import.meta.env.DEV` debug toggle so dev builds reveal raw `toolId` / thinking text for testing, while prod builds collapse everything unknown to a fixed `"I'm working on it now..."` label.

**Tech Stack:** Svelte 5 (runes), TypeScript, Tauri 2 (Rust host in `apps/momo/src-tauri/`), `tauri-plugin-opener` (for external URL opening — already installed), Playwright (tests/ with FakeDaemon).

**Scope guardrails (as of 2026-05-27, post-momo-extraction):**
- 🟢 `apps/momo/**` — full ownership: Svelte UI, src-tauri Rust host, tests, tauri.conf.json, Cargo.toml, etc.
- 🔴 `crates/**` (puffer agent core), `apps/puffer-desktop/**` (legacy v1 — reference-only for porting).

Most backend events listed in this plan are already emitted by `apps/momo/src-tauri/src/{backend.rs, codex_app_server.rs}`. **Exception: `user-question-request` event + `resolve_user_question` RPC handler are NOT yet wired in momo's Rust host** — Task 5 includes the Rust additions (porting from `apps/puffer-desktop/src-tauri/src/turn.rs:155-160, 295-315`).

---

## File Structure (new + modified)

**Create:**
- `apps/momo/src/components/common/MessageBody.svelte` — Markdown renderer ported from v1
- `apps/momo/src/components/agent/ThinkingBlock.svelte` — Streaming thinking display
- `apps/momo/src/components/agent/ToolCallPill.svelte` — Tool status pill (wraps `ToolBlock`)
- `apps/momo/src/components/agent/AnswerForm.svelte` — Interactive ask-user-question form
- `apps/momo/src/lib/toolLabels.ts` — `toolId → { icon, label }` mapping table
- `apps/momo/src/lib/debugFlags.ts` — Centralized `import.meta.env.DEV` toggle
- `apps/momo/src/lib/timeFormat.ts` — `formatTime(ms)` helper

**Modify:**
- `apps/momo/src/lib/chat.svelte.ts` — Extend `ChatMessage` union; handle `thinking-delta`, `tool-calls-requested`, `tool-invocations`, `user-question-request`; track running `turnId`; expose `cancelRunningTurn(sessionId)`
- `apps/momo/src/lib/agentClient.ts` — Add `resolveUserQuestion(...)` wrapper (RPC name `resolve_user_question` already exists per `src-tauri/src/lib.rs:50`)
- `apps/momo/src/pages/Agent.svelte` — Render the new message roles; pass `running` + `onCancel` to Composer; use MessageBody for text
- `apps/momo/src/components/agent/ChatBubble.svelte` — Use `MessageBody`; add timestamp meta row
- `apps/momo/src/components/shell/Composer.svelte` — Accept `running`, `onCancel` props; swap send button to red Stop when running

**Test harness (Phase 0, see Test Strategy section below):**
- Rename: `apps/momo/tests/chat.spec.ts` → `apps/momo/tests/chat-smoke.spec.ts` (4 of 5 existing tests migrate to new structure; only the Promise-stringification regression stays as smoke)
- Create: `apps/momo/tests/chat/{assistant-bubble,stop-button,thinking,tool-pills,answer-form,composer,session-lifecycle,session-isolation,reconnection}.spec.ts`
- Create: `apps/momo/tests/support/{bootHelpers,chatEmit,chatTiming,chatLocators,composerHelpers,sessionFixtures,README}.{ts,md}`

---

## Test Strategy

Chat has ~30 known race / lifecycle / cross-session bug shapes (see v1 `apps/puffer-desktop/tests/chat-session-ui.spec.ts`, 8943 lines). A heavy harness lets feature work add edge-case coverage without re-deriving boilerplate.

### Design principles (from Plan-agent review, 2026-05-27)

1. **Primitives, not lifecycle bundles.** `emitTurnLifecycle({ deltas, ... })` hides exactly the gaps where bugs live (user clicked Stop *between* delta 1 and delta 2; transcript reload *between* tool-request and tool-invocation). Ship primitives — `emitTurnStart`, `emitTextDelta`, `emitTurnComplete`, `emitTurnError` — and keep `emitTurnLifecycle` as a happy-path sugar only.
2. **Locators, not bundled waits.** `expectAssistantText(page, text)` folds typing-dot-clearance + text-match into one wait; identity-preservation tests then can't observe the bubble across phases. Provide `locate*` helpers that return a `Locator`; let test authors chain `await expect(locator).toHaveText(...)`.
3. **Race tests stay close to `daemon.emit`.** Helpers exist to remove boilerplate from happy paths. Document this rule in `tests/support/README.md` — don't dress up timing-sensitive sequences with sugar.
4. **No per-feature × per-category grid.** Cross-cutting concerns (session isolation, hydration mid-flow, reconnection) go in dedicated specs that *parameterize* over feature payloads, instead of being duplicated 5 times.

### Test file layout

```
apps/momo/tests/
  chat-smoke.spec.ts            # tiny — 1 retained regression + new harness-wide smoke
  chat/
    assistant-bubble.spec.ts    # Task 1 + streaming: Markdown variants, URL click, timestamp,
                                # error variant, delta accumulation, DOM identity across phases
    stop-button.spec.ts         # Task 2: enabled state machine, cancel RPC, post-cancel UI
    thinking.spec.ts            # Task 3: thinking-delta accumulation, dev/prod toggle, turn-end pending flip
    tool-pills.spec.ts          # Task 4: callId lifecycle, pending → success/failed, missing-request edge,
                                # callId reuse across turns
    answer-form.spec.ts         # Task 5: form render, single-select submit, double-submit guard,
                                # answered state lock, RPC failure rollback
    composer.spec.ts            # input edges: IME composition, Enter while disabled, draft persistence,
                                # paste, very-long text, empty submit
    session-lifecycle.spec.ts   # cold hydration, loading state, error+retry, creation race,
                                # mid-flow hydration (transcript reload during live turn)
    session-isolation.spec.ts   # cross-session: events for sessionA don't bleed into sessionB,
                                # composer state per session, background activity badges
    reconnection.spec.ts        # WS disconnect + reconnect, in-flight turn survival, replay-on-reconnect
  support/
    fakeDaemon.ts               # UNCHANGED (2043 lines, already supports cancel_turn + resolve_user_question)
    bootHelpers.ts              # bootOnboarded(page, daemon) — auth/onboard stub
                                # openSession(page, predicate) — sidebar click navigation
                                # NOT merged — v1 splits these and we follow the same pattern
    chatEmit.ts                 # emitTurnStart, emitTextDelta, emitThinkingDelta, emitTurnComplete,
                                # emitTurnError, emitToolRequest, emitToolInvocation, emitQuestion
                                # PLUS: emitTurnLifecycle({...}) as opt-in convenience for happy paths
    chatTiming.ts               # daemon.deferRpc(method, predicate?) → { resolve, reject }
                                # daemon.reconnect(page), daemon.dropNextEvent(channel)
    chatLocators.ts             # locateAssistantBubble(page, { turnId? }) → Locator
                                # locateToolPill(page, { callId | toolId }) → Locator
                                # locateQuestionForm(page, { requestId }) → Locator
                                # locateUserBubble(page, { text? }) → Locator
                                # locateThinkingBlock(page) → Locator
    composerHelpers.ts          # composerType(page, text)
                                # composerSubmit(page)  // Enter
                                # composerIME(page, { phase: "start" | "commit", text })
                                # composerExpectState(page, "idle" | "running" | "disabled")
                                # composerExpectDraft(page, text)
    sessionFixtures.ts          # makeSession({ id, title?, timeline?, providerId? }) — defaults filled
                                # hydrateMidFlow(page, daemon, sessionId) — force load_session_detail re-fire
    README.md                   # rules: when to use helpers vs raw daemon.emit; how to add new helpers
```

### Coverage taxonomy

Each **feature spec** (`chat/{feature}.spec.ts`) covers: happy path · error path · edge (empty / overlong / unknown values) · feature-specific lifecycle.

Each **cross-cutting spec** parameterizes across the 5 features:
- `session-isolation.spec.ts` — for every event type, prove sessionA emissions don't render in sessionB's open view
- `session-lifecycle.spec.ts` — for every message kind, prove hydration replays it (or documents what's intentionally skipped)
- `reconnection.spec.ts` — for an in-flight turn carrying each event type, prove WS drop + reconnect resumes without dup/loss
- `composer.spec.ts` — input-level concerns: IME, draft persistence, disabled while running, keyboard

**A11y category** (cheap, added per Plan-agent recommendation): `aria-busy` on thread during running turn · accessible name on Stop button · `role="form"` on AnswerForm.

### Migration of existing tests

Current `apps/momo/tests/chat.spec.ts` has 5 tests:
1. "home composer → create_session → run_agent_turn → streamed assistant text" → migrate to `chat/assistant-bubble.spec.ts` as the canonical streaming happy path.
2. "existing session with historical timeline renders past messages" → migrate to `chat/session-lifecycle.spec.ts` (cold hydration).
3. "shows a loading indicator while load_session_detail is in flight" → migrate to `chat/session-lifecycle.spec.ts` (hydration loading state).
4. "shows error state with retry when load_session_detail fails" → migrate to `chat/session-lifecycle.spec.ts` (hydration error + retry).
5. "sidebar '+' New chat in Work navigates to a real session id, not [object Promise]" → keep in `chat-smoke.spec.ts` as smoke regression (it tests a stringification bug, not a feature flow).

Migration happens in Phase 0 — the new helpers must be capable of expressing all 5 tests before they're considered done.

### v1 reference catalogue (for porting hard cases)

Pull from `apps/puffer-desktop/tests/chat-session-ui.spec.ts`:
- Tool callId reuse across turns → `:1112-1187`
- Tool invocation missed → `:1189`
- Tool-calls-requested event flow → `:1218`
- Stop button + turnId latency → `:1253`
- Turn error keeps draft visible → `:1779`
- IME composition + Enter → `:554`
- Persisted prompt during pending turn → `:936`
- Cross-session leak (turn completion) → `:19`
- Cross-session leak (turn start) → `:99`
- Cross-session leak (composer state) → `:410`
- Cross-session leak (background running) → `:479`

Each Task's "Test Steps" in the task sections below was written *before* the harness existed and shows raw `daemon.emit(...)` calls. **When implementing, rewrite those test snippets to use the harness helpers above** — the raw-emit snippets are illustrative of the scenario, not the final shape.

---

## Task Order

**Phase 0 — Test Harness** (single sub-agent, sequential prerequisite)
Builds the file structure + helper API described in the Test Strategy section, AND migrates the 4 existing tests into the new structure as harness-validation. Phase 0 must complete before any feature work starts.

**Phase 1 — Foundational features** (parallel sub-agents)
- **Task 1** — Message rendering polish (Markdown + timestamps + error variant + URL clicks)
- **Task 2** — Stop button (cancel in-flight turn)

**Phase 2 — Agent observability + interactivity** (parallel sub-agents)
- **Task 3** — Thinking display
- **Task 4** — Tool call pills
- **Task 5** — Interactive answer form (includes Rust backend port: `user-question-request` event + `resolve_user_question` RPC handler)

---

## Task 1: Message Rendering Polish

**Goal:** Replace plain-text rendering in chat bubbles with Markdown, add timestamps, give error messages a visual variant, and make external URLs clickable.

**Files:**
- Create: `apps/momo/src/components/common/MessageBody.svelte`
- Create: `apps/momo/src/lib/timeFormat.ts`
- Modify: `apps/momo/src/components/agent/ChatBubble.svelte`
- Modify: `apps/momo/src/pages/Agent.svelte` (assistant-row block, lines ~132-146)
- Test: `apps/momo/tests/chat.spec.ts` (extend)

### Steps

- [ ] **Step 1: Port `MessageBody.svelte` to v2**

  Copy `apps/puffer-desktop/src/lib/components/MessageBody.svelte` → `apps/momo/src/components/common/MessageBody.svelte`.

  **Adapt to Svelte 5 runes:**
  - Replace `export let body = ""` with `let { body = "", onOpenFile = undefined } = $props<{ body?: string; onOpenFile?: (path: string, line?: number | null) => void }>();`
  - Replace `export let onOpenFile = undefined;` similarly
  - Search for any other `export let` and convert. No reactive `$:` statements exist in v1 MessageBody, but verify.

  **Adapt URL-click handling per user decision (A+B):**
  - Keep URL link rendering (markdown `[text](url)` and bare URLs) — these become `<a>` tags.
  - Drop the "local file path detection" branches: remove the `urlPattern` check for paths starting with `/` and the `fileTarget(...)` helper that interprets `file://` and `/abs/path` strings. Only http(s):// URLs survive.
  - For an http(s) `<a>` click, intercept and call `tauri-plugin-opener`'s `openUrl(href)`. Import:
    ```ts
    import { openUrl } from "@tauri-apps/plugin-opener";
    ```
    Wire to the click handler so the OS browser opens, not the webview.

- [ ] **Step 2: Create `timeFormat.ts`**

  ```ts
  // apps/momo/src/lib/timeFormat.ts
  export function formatTime(ms: number | null | undefined): string {
    if (!ms) return "";
    const d = new Date(ms);
    const hh = String(d.getHours()).padStart(2, "0");
    const mm = String(d.getMinutes()).padStart(2, "0");
    return `${hh}:${mm}`;
  }
  ```

- [ ] **Step 3: Update `ChatBubble.svelte`**

  Current state: 53 lines, takes `text` prop, renders `<p>{text}</p>` inside a styled bubble.

  Change shape to:
  ```svelte
  <script lang="ts">
    import MessageBody from "../common/MessageBody.svelte";
    import { formatTime } from "../../lib/timeFormat.ts";

    interface Props {
      text: string;
      createdAt?: number;
    }
    let { text, createdAt }: Props = $props();
  </script>

  <div class="chat-bubble">
    <MessageBody body={text} />
    {#if createdAt}
      <span class="chat-bubble__time">{formatTime(createdAt)}</span>
    {/if}
  </div>
  ```

  Add `.chat-bubble__time` style: 11px, secondary color, right-aligned, margin-top 4px.

- [ ] **Step 4: Update `Agent.svelte` assistant-row**

  Find the block at `apps/momo/src/pages/Agent.svelte:132-146`. Replace `<p class="assistant-bubble__text">{message.text}</p>` with:

  ```svelte
  <MessageBody body={message.text} />
  {#if message.createdAt}
    <span class="assistant-bubble__time">{formatTime(message.createdAt)}</span>
  {/if}
  ```

  Add `import MessageBody from "../components/common/MessageBody.svelte";` and `import { formatTime } from "../lib/timeFormat";` at the top.

  At the user-bubble call site (`Agent.svelte:131`), pass `createdAt={message.createdAt}`.

  Also add error variant: when `message.error === true`, add `data-error="true"` to the `.assistant-row`. Style block:
  ```css
  .assistant-row[data-error="true"] .assistant-bubble {
    color: var(--color-danger-text, #c0392b);
  }
  .assistant-row[data-error="true"] .assistant-bubble::before {
    content: "⚠ ";
  }
  ```

- [ ] **Step 5: Add Playwright test for Markdown + timestamp**

  Append to `apps/momo/tests/chat.spec.ts`:

  ```ts
  test("assistant bubble renders Markdown bold and a timestamp", async ({ page }) => {
    const daemon = new FakeDaemon({ sessions: [] });
    await bootOnboarded(page, daemon);
    await page.getByLabel("Message").fill("hi");
    await page.getByLabel("Message").press("Enter");
    const sessionId = "session-created-1";
    const turnId = `turn-${sessionId}`;
    const channel = `session:${sessionId}:event`;
    daemon.emit(channel, { type: "turn-start", turnId });
    daemon.emit(channel, { type: "text-delta", turnId, delta: "Hello **world**" });
    daemon.emit(channel, { type: "turn-complete", turnId });

    await expect(page.locator(".assistant-row strong")).toHaveText("world");
    await expect(page.locator(".assistant-bubble__time")).toBeVisible();
  });
  ```

- [ ] **Step 6: Verify**

  Run:
  ```bash
  cd apps/momo && pnpm check
  pnpm test:desktop-ui -- chat.spec.ts
  ```
  Expected: type-check passes; the new test plus the existing 5 tests all pass.

- [ ] **Step 7: Commit**

  ```bash
  git add apps/momo/src/components/common/MessageBody.svelte \
          apps/momo/src/lib/timeFormat.ts \
          apps/momo/src/components/agent/ChatBubble.svelte \
          apps/momo/src/pages/Agent.svelte \
          apps/momo/tests/chat.spec.ts
  git commit -m "feat(desktop-v2): port Markdown rendering, timestamps, error variant for chat bubbles"
  ```

---

## Task 2: Stop Button

**Goal:** When an agent turn is running, swap the Composer's Send button for a red Stop that cancels via `agent.cancelTurn(turnId)`.

**Files:**
- Modify: `apps/momo/src/lib/chat.svelte.ts` (track running turnId per session, expose helpers)
- Modify: `apps/momo/src/components/shell/Composer.svelte` (accept `running`, `onCancel` props)
- Modify: `apps/momo/src/pages/Agent.svelte` (wire props)
- Test: `apps/momo/tests/chat.spec.ts` (extend)

### Steps

- [ ] **Step 1: Track running turn in `chat.svelte.ts`**

  Add near the existing state declarations:
  ```ts
  /** Per-session id of the in-flight turn (if any). Cleared on turn-complete/turn-error. */
  export const runningTurnBySessionId = $state<Record<string, string>>({});
  ```

  In `fireTurn`'s `.then(...)`, after `pendingByTurn.set(...)`:
  ```ts
  runningTurnBySessionId[sessionId] = res.turnId;
  ```

  In `handleSessionEvent`'s `turn-complete` and `turn-error` cases, after deleting from `pendingByTurn`:
  ```ts
  if (runningTurnBySessionId[sessionId] === turnId) {
    delete runningTurnBySessionId[sessionId];
  }
  ```

  Add public helper:
  ```ts
  export async function cancelRunningTurn(sessionId: string): Promise<void> {
    const turnId = runningTurnBySessionId[sessionId];
    if (!turnId) return;
    try {
      await agent.cancelTurn(turnId);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      pushToast(`Cancel failed: ${msg}`, "error");
    }
  }
  ```

- [ ] **Step 2: Read Composer source to learn props pattern**

  Read `apps/momo/src/components/shell/Composer.svelte` first to understand its current API. The plan assumes it has a send button rendered by some internal handler; the change is to:
  - Add `running?: boolean` and `onCancel?: () => void` to its `Props` interface
  - When `running` is true: render the button red with a stop icon (use `lucide-svelte`'s `Square` or `Pause`); on click call `onCancel?.()`
  - Otherwise: keep existing send behavior unchanged

  Style: red fill, white icon. Reuse `--color-danger` or hardcode `#dc2626`.

- [ ] **Step 3: Wire from `Agent.svelte`**

  At the top, import:
  ```ts
  import { runningTurnBySessionId, cancelRunningTurn } from "../lib/chat.svelte";
  ```

  Add a derived:
  ```ts
  let isRunning = $derived(taskId ? Boolean(runningTurnBySessionId[taskId]) : false);
  ```

  Pass to Composer (at `Agent.svelte:156` area):
  ```svelte
  <Composer
    placeholder="Hi, Tomo. How's my luck today?"
    running={isRunning}
    onCancel={() => taskId && cancelRunningTurn(taskId)}
  />
  ```

- [ ] **Step 4: Playwright test**

  ```ts
  test("send button becomes red Stop while turn is running, and cancel_turn fires on click", async ({ page }) => {
    const daemon = new FakeDaemon({ sessions: [] });
    await bootOnboarded(page, daemon);
    await page.getByLabel("Message").fill("long task");
    await page.getByLabel("Message").press("Enter");
    const sessionId = "session-created-1";
    const turnId = `turn-${sessionId}`;
    const channel = `session:${sessionId}:event`;
    daemon.emit(channel, { type: "turn-start", turnId });

    // Stop button should appear (data-stop attribute we add in Composer)
    await expect(page.locator("[data-stop]")).toBeVisible();
    await page.locator("[data-stop]").click();
    await daemon.waitForRequest("cancel_turn", (req) => req.params.turnId === turnId);

    // Cleanup the turn so the test doesn't hang
    daemon.emit(channel, { type: "turn-complete", turnId });
  });
  ```

  Add `data-stop` attribute on the Stop button in `Composer.svelte` for test addressability.

- [ ] **Step 5: Verify + commit**

  ```bash
  cd apps/momo && pnpm check && pnpm test:desktop-ui -- chat.spec.ts
  git add apps/momo/src/lib/chat.svelte.ts \
          apps/momo/src/components/shell/Composer.svelte \
          apps/momo/src/pages/Agent.svelte \
          apps/momo/tests/chat.spec.ts
  git commit -m "feat(desktop-v2): add red Stop button that cancels the in-flight turn"
  ```

---

## Task 3: Thinking Display

**Goal:** Show streaming `thinking-delta` content as a soft-grey block during dev, and as a fixed `"I'm working on it now..."` pill in prod.

**Files:**
- Create: `apps/momo/src/lib/debugFlags.ts`
- Create: `apps/momo/src/components/agent/ThinkingBlock.svelte`
- Modify: `apps/momo/src/lib/chat.svelte.ts`
- Modify: `apps/momo/src/pages/Agent.svelte`
- Test: `apps/momo/tests/chat.spec.ts`

### Steps

- [ ] **Step 1: Create debug flag module**

  ```ts
  // apps/momo/src/lib/debugFlags.ts

  /**
   * Show raw agent activity (thinking content, tool ids) in dev builds so
   * developers can verify what the model is doing. Prod builds collapse
   * everything unknown to "I'm working on it now..." per product spec
   * (see docs/superpowers/plans/2026-05-27-chat-v2-feature-port.md).
   *
   * `import.meta.env.DEV` is true under `vite dev` and false under
   * `vite build`. No runtime cost in prod — Vite tree-shakes the dev branch.
   */
  export const SHOW_RAW_AGENT_ACTIVITY = import.meta.env.DEV;
  ```

- [ ] **Step 2: Extend `ChatMessage` union**

  In `chat.svelte.ts`, replace the current `ChatMessage` interface with a union:

  ```ts
  export type ChatMessage =
    | { id: string; role: "user"; text: string; createdAt: number }
    | { id: string; role: "assistant"; text: string; pending?: boolean; createdAt: number; error?: boolean }
    | { id: string; role: "thinking"; text: string; pending: boolean; createdAt: number; turnId: string };
  ```

  Update existing functions (`pushUser`, `pushPendingAssistant`, `hydrateSession`) to set explicit `role` discriminants. Existing call sites should keep compiling.

- [ ] **Step 3: Handle `thinking-delta` in `handleSessionEvent`**

  Add a new case before `default`:

  ```ts
  case "thinking-delta": {
    const turnId = (payload as { turnId: string }).turnId;
    const delta = (payload as { delta: string }).delta ?? "";
    // Find or create the thinking bubble for this turn. We attach by
    // turnId rather than callId because thinking deltas have no callId.
    let bubble = list.find(
      (m): m is Extract<ChatMessage, { role: "thinking" }> =>
        m.role === "thinking" && m.turnId === turnId
    );
    if (!bubble) {
      bubble = {
        id: nextMessageId(),
        role: "thinking",
        text: "",
        pending: true,
        createdAt: Date.now(),
        turnId
      };
      list.push(bubble);
    }
    bubble.text = (bubble.text ?? "") + delta;
    break;
  }
  ```

  In the `turn-complete` and `turn-error` cases, also flip any active thinking bubble for that turn to `pending: false`:

  ```ts
  for (const m of list) {
    if (m.role === "thinking" && m.turnId === turnId) m.pending = false;
  }
  ```

- [ ] **Step 4: Build `ThinkingBlock.svelte`**

  ```svelte
  <script lang="ts">
    import { Bot } from "lucide-svelte";
    import ToolBlock from "./ToolBlock.svelte";
    import { SHOW_RAW_AGENT_ACTIVITY } from "../../lib/debugFlags";

    interface Props {
      text: string;
      pending: boolean;
    }
    let { text, pending }: Props = $props();

    let trimmed = $derived(text.trim());
  </script>

  {#if SHOW_RAW_AGENT_ACTIVITY && trimmed.length > 0}
    <div class="thinking-block" data-pending={pending}>
      <span class="thinking-block__label">Thinking</span>
      <p class="thinking-block__text">{trimmed}</p>
    </div>
  {:else}
    <ToolBlock icon="user" label="I'm working on it now..." />
  {/if}

  <style>
    .thinking-block {
      max-width: 600px;
      padding: 8px 12px;
      background: var(--color-surface-rail);
      border-radius: 10px;
      color: var(--color-text-muted);
      font-family: var(--font-system);
      font-size: 13px;
      line-height: 18px;
      font-style: italic;
    }
    .thinking-block__label {
      display: block;
      font-size: 11px;
      font-style: normal;
      font-weight: var(--font-weight-medium);
      text-transform: uppercase;
      letter-spacing: 0.05em;
      margin-bottom: 4px;
    }
    .thinking-block__text {
      margin: 0;
      white-space: pre-wrap;
    }
  </style>
  ```

  Note: `ToolBlock`'s current `iconMap` doesn't have a generic "bot/working" icon; for the prod fallback, pick `"user"` or extend `iconMap` to add a `"bot"` key wired to `lucide-svelte`'s `Bot`. (The latter is one-line and cleaner — do it.)

- [ ] **Step 5: Render in `Agent.svelte`**

  In the `{#each chatMessages as message ...}` loop, add a third branch:

  ```svelte
  {:else if message.role === "thinking"}
    <ThinkingBlock text={message.text} pending={message.pending} />
  ```

  Add the import at top.

- [ ] **Step 6: Playwright test**

  ```ts
  test("thinking-delta accumulates into a thinking block in dev builds", async ({ page }) => {
    const daemon = new FakeDaemon({ sessions: [] });
    await bootOnboarded(page, daemon);
    await page.getByLabel("Message").fill("think hard");
    await page.getByLabel("Message").press("Enter");
    const sessionId = "session-created-1";
    const turnId = `turn-${sessionId}`;
    const channel = `session:${sessionId}:event`;
    daemon.emit(channel, { type: "turn-start", turnId });
    daemon.emit(channel, { type: "thinking-delta", turnId, delta: "Let me " });
    daemon.emit(channel, { type: "thinking-delta", turnId, delta: "ponder…" });

    // Under playwright (dev build), the raw block shows. Asserts both
    // (a) the delta wiring works and (b) the dev branch renders.
    await expect(page.locator(".thinking-block__text")).toHaveText("Let me ponder…");

    daemon.emit(channel, { type: "turn-complete", turnId });
  });
  ```

- [ ] **Step 7: Verify + commit**

  ```bash
  cd apps/momo && pnpm check && pnpm test:desktop-ui -- chat.spec.ts
  git add apps/momo/src/lib/debugFlags.ts \
          apps/momo/src/components/agent/ThinkingBlock.svelte \
          apps/momo/src/components/agent/ToolBlock.svelte \
          apps/momo/src/lib/chat.svelte.ts \
          apps/momo/src/pages/Agent.svelte \
          apps/momo/tests/chat.spec.ts
  git commit -m "feat(desktop-v2): stream thinking-delta as a soft-grey block (dev) / 'working on it' pill (prod)"
  ```

---

## Task 4: Tool Call Pills

**Goal:** Show a pill for each tool call: pending → success / failed. Use the friendly label from `toolLabels.ts` when mapped; show raw `toolId` in dev / `"I'm working on it now..."` in prod when unmapped.

**Files:**
- Create: `apps/momo/src/lib/toolLabels.ts`
- Create: `apps/momo/src/components/agent/ToolCallPill.svelte`
- Modify: `apps/momo/src/lib/chat.svelte.ts`
- Modify: `apps/momo/src/pages/Agent.svelte`
- Test: `apps/momo/tests/chat.spec.ts`

### Steps

- [ ] **Step 1: Create `toolLabels.ts`**

  ```ts
  // apps/momo/src/lib/toolLabels.ts
  import type { IconName } from "../data/types";

  export interface ToolLabel {
    icon: IconName;
    label: string;
  }

  /**
   * Maps backend toolId (e.g. "wallet.balance") to a consumer-friendly
   * label + icon. UNMAPPED tools fall back to debugFlags-controlled text
   * (raw toolId in dev, fixed "I'm working on it now..." in prod).
   *
   * Populate this lazily as the puffer agent adds tools. Keys must match
   * the toolId emitted by `src-tauri/src/turn.rs` ToolCallsRequested.
   */
  export const TOOL_LABELS: Record<string, ToolLabel> = {
    // Example seed entries. Replace once real tool ids are known.
    // "wallet.balance": { icon: "wallet", label: "Checking your balance" },
    // "wallet.send":    { icon: "wallet", label: "Preparing a transfer" },
    // "calendar.list":  { icon: "calendar", label: "Checking your calendar" },
  };

  export function lookupToolLabel(toolId: string): ToolLabel | null {
    return TOOL_LABELS[toolId] ?? null;
  }
  ```

- [ ] **Step 2: Extend `ChatMessage` union with `tool` role**

  ```ts
  | {
      id: string;
      role: "tool";
      toolId: string;
      callId: string;
      status: "running" | "success" | "failed";
      input?: unknown;
      output?: unknown;
      createdAt: number;
      turnId: string;
    }
  ```

- [ ] **Step 3: Handle `tool-calls-requested` and `tool-invocations`**

  In `handleSessionEvent`, add two cases:

  ```ts
  case "tool-calls-requested": {
    const turnId = (payload as { turnId: string }).turnId;
    const requests = ((payload as { requests?: unknown }).requests ?? []) as Array<{
      callId: string;
      toolId: string;
      input?: unknown;
    }>;
    for (const r of requests) {
      // De-dupe: if a card already exists for this callId, do nothing
      // (tool-invocations will update it). Same callId → same card.
      if (list.some((m) => m.role === "tool" && m.callId === r.callId)) continue;
      list.push({
        id: nextMessageId(),
        role: "tool",
        toolId: r.toolId,
        callId: r.callId,
        status: "running",
        input: r.input,
        createdAt: Date.now(),
        turnId
      });
    }
    break;
  }
  case "tool-invocations": {
    const invocations = ((payload as { invocations?: unknown }).invocations ?? []) as Array<{
      callId: string;
      toolId: string;
      input?: unknown;
      output?: unknown;
      success: boolean;
    }>;
    for (const i of invocations) {
      const target = list.find(
        (m): m is Extract<ChatMessage, { role: "tool" }> =>
          m.role === "tool" && m.callId === i.callId
      );
      if (target) {
        target.status = i.success ? "success" : "failed";
        target.output = i.output;
      } else {
        // Edge: invocation arrived without a prior request event (some
        // backends batch the result without firing tool-calls-requested).
        // Synthesize the pill at terminal status.
        list.push({
          id: nextMessageId(),
          role: "tool",
          toolId: i.toolId,
          callId: i.callId,
          status: i.success ? "success" : "failed",
          input: i.input,
          output: i.output,
          createdAt: Date.now(),
          turnId: "" // unknown — only matters for thinking; OK to leave blank
        });
      }
    }
    break;
  }
  ```

- [ ] **Step 4: Build `ToolCallPill.svelte`**

  ```svelte
  <script lang="ts">
    import ToolBlock from "./ToolBlock.svelte";
    import { lookupToolLabel } from "../../lib/toolLabels";
    import { SHOW_RAW_AGENT_ACTIVITY } from "../../lib/debugFlags";

    interface Props {
      toolId: string;
      status: "running" | "success" | "failed";
    }
    let { toolId, status }: Props = $props();

    let mapped = $derived(lookupToolLabel(toolId));
    let label = $derived(
      mapped?.label
        ?? (SHOW_RAW_AGENT_ACTIVITY ? `Calling: ${toolId}` : "I'm working on it now...")
    );
    let icon = $derived(mapped?.icon ?? "user");
  </script>

  <div class="tool-call-pill" data-status={status}>
    <ToolBlock {icon} {label} />
  </div>

  <style>
    .tool-call-pill[data-status="running"] {
      opacity: 0.85;
    }
    .tool-call-pill[data-status="failed"] :global(.tool-block) {
      border-color: var(--color-danger, #c0392b);
      color: var(--color-danger, #c0392b);
    }
  </style>
  ```

- [ ] **Step 5: Render in `Agent.svelte`**

  Add another branch in the `{#each chatMessages}` loop:

  ```svelte
  {:else if message.role === "tool"}
    <ToolCallPill toolId={message.toolId} status={message.status} />
  ```

- [ ] **Step 6: Playwright test**

  ```ts
  test("tool-calls-requested shows pill; tool-invocations flips it to success", async ({ page }) => {
    const daemon = new FakeDaemon({ sessions: [] });
    await bootOnboarded(page, daemon);
    await page.getByLabel("Message").fill("do a thing");
    await page.getByLabel("Message").press("Enter");
    const sessionId = "session-created-1";
    const turnId = `turn-${sessionId}`;
    const channel = `session:${sessionId}:event`;
    daemon.emit(channel, { type: "turn-start", turnId });
    daemon.emit(channel, {
      type: "tool-calls-requested",
      turnId,
      requests: [{ callId: "c1", toolId: "wallet.balance", input: {} }]
    });
    await expect(page.locator('.tool-call-pill[data-status="running"]')).toBeVisible();
    daemon.emit(channel, {
      type: "tool-invocations",
      turnId,
      invocations: [{ callId: "c1", toolId: "wallet.balance", input: {}, output: { amount: 100 }, success: true }]
    });
    await expect(page.locator('.tool-call-pill[data-status="success"]')).toBeVisible();
    daemon.emit(channel, { type: "turn-complete", turnId });
  });
  ```

- [ ] **Step 7: Verify + commit**

  ```bash
  cd apps/momo && pnpm check && pnpm test:desktop-ui -- chat.spec.ts
  git add apps/momo/src/lib/toolLabels.ts \
          apps/momo/src/components/agent/ToolCallPill.svelte \
          apps/momo/src/lib/chat.svelte.ts \
          apps/momo/src/pages/Agent.svelte \
          apps/momo/tests/chat.spec.ts
  git commit -m "feat(desktop-v2): render tool call pills with running → success/failed states"
  ```

---

## Task 5: Interactive Answer Form (askUserQuestion)

**Goal:** When the agent emits a `user-question-request` event, show an inline form in the chat. User picks an answer; we fire `resolve_user_question` to unblock the turn.

**Files (Rust backend — NEW for momo):**
- Modify: `apps/momo/src-tauri/src/{backend.rs, codex_app_server.rs}` — emit `user-question-request` events when the agent runtime calls `ask_user_question`; accept `resolve_user_question` RPC and route the answer back to the waiting turn.
- Reference port from: `apps/puffer-desktop/src-tauri/src/turn.rs:155-160` (event shape) and `:295-315` (RPC handler). Read both before writing; momo's Rust layout differs (`turn.rs` doesn't exist in momo — the equivalent logic lives in `backend.rs` and `codex_app_server.rs`).

**Files (Frontend):**
- Modify: `apps/momo/src/lib/agentClient.ts` (add `resolveUserQuestion` wrapper)
- Modify: `apps/momo/src/lib/chat.svelte.ts` (handle event + add helper)
- Create: `apps/momo/src/components/agent/AnswerForm.svelte`
- Modify: `apps/momo/src/pages/Agent.svelte`

**Files (Tests):**
- Create: `apps/momo/tests/chat/answer-form.spec.ts` (uses harness helpers from Phase 0)

**Pre-step — Rust backend wiring** (before frontend steps):

- [ ] **Step 0a: Read v1 reference**

  Read `apps/puffer-desktop/src-tauri/src/turn.rs` lines 24, 49, 155-169, 194-205, 295-315, 359 to understand the v1 wiring: how `UserQuestionRequest` is constructed, how the `pending_questions` map routes answers back via mpsc, and how `resolve_user_question` finds the pending sender.

- [ ] **Step 0b: Map v1 → momo equivalents**

  Read `apps/momo/src-tauri/src/{backend.rs, codex_app_server.rs, lib.rs}` and identify where to graft each piece:
  - Event emission site (where v1's `EmittedEvent::UserQuestionRequest` would fire)
  - RPC dispatch table (where v1's `resolve_user_question` registers)
  - Per-turn state where pending senders live (analog of v1's `pending_questions` Arc<Mutex<HashMap>>)

  Document the mapping in `apps/momo/src-tauri/src/USER_QUESTION_PORT_NOTES.md` (delete after merge — it's working scratchpad). If the agent runtime in momo doesn't surface `ask_user_question` calls at all yet, surface that explicitly — frontend can still ship dormant, and Step 0 just becomes an event-only stub for tests.

- [ ] **Step 0c: Implement Rust port**

  Inline-style — keep the diff minimal. Cargo dependencies for `tokio::sync::mpsc` etc. are already in momo (check `apps/momo/src-tauri/Cargo.toml`).

  Verify with: `cd apps/momo/src-tauri && cargo build && cargo test`.

### Steps

- [ ] **Step 1: Add `resolveUserQuestion` to `agentClient.ts`**

  ```ts
  // apps/momo/src/lib/agentClient.ts

  export interface AskUserQuestionOption {
    label: string;
    description?: string;
    preview?: string | null;
  }
  export interface AskUserQuestionItem {
    question: string;
    header?: string;
    options: AskUserQuestionOption[];
    multiSelect?: boolean;
  }

  export async function resolveUserQuestion(
    turnId: string,
    requestId: string,
    answers: Record<string, string | string[]>,
  ): Promise<void> {
    await ws.request<unknown>("resolve_user_question", { turnId, requestId, answers });
  }
  ```

  Verify by reading `apps/momo/src-tauri/src/turn.rs:295-315` (`resolve_user_question`) and `apps/momo/src-tauri/src/lib.rs:50` — the params shape is `{ turnId, requestId, answers }` (camelCase).

- [ ] **Step 2: Extend `ChatMessage` with `question` role**

  ```ts
  | {
      id: string;
      role: "question";
      requestId: string;
      turnId: string;
      questions: AskUserQuestionItem[];
      answered: boolean;
      answers?: Record<string, string | string[]>;
      createdAt: number;
    }
  ```

- [ ] **Step 3: Handle `user-question-request`**

  ```ts
  case "user-question-request": {
    const turnId = (payload as { turnId: string }).turnId;
    const requestId = (payload as { requestId: string }).requestId;
    const questions = ((payload as { questions?: unknown }).questions ?? []) as AskUserQuestionItem[];
    // De-dupe: if requestId already present, ignore (defensive).
    if (list.some((m) => m.role === "question" && m.requestId === requestId)) break;
    list.push({
      id: nextMessageId(),
      role: "question",
      requestId,
      turnId,
      questions,
      answered: false,
      createdAt: Date.now()
    });
    break;
  }
  ```

- [ ] **Step 4: Expose answer helper**

  ```ts
  export async function answerQuestion(
    sessionId: string,
    requestId: string,
    answers: Record<string, string | string[]>,
  ): Promise<void> {
    const list = chatSessions[sessionId];
    if (!list) return;
    const target = list.find(
      (m): m is Extract<ChatMessage, { role: "question" }> =>
        m.role === "question" && m.requestId === requestId
    );
    if (!target || target.answered) return;
    target.answered = true;
    target.answers = answers;
    try {
      await agent.resolveUserQuestion(target.turnId, requestId, answers);
    } catch (err) {
      // Roll back so the user can retry.
      target.answered = false;
      const msg = err instanceof Error ? err.message : String(err);
      pushToast(`Could not send answer: ${msg}`, "error");
    }
  }
  ```

- [ ] **Step 5: Build `AnswerForm.svelte` (single-question, single-select MVP)**

  The v1 `QuestionPrompt.svelte` (459 lines) supports multi-question + multi-select + "other" custom input + collapse. For v2's MVP we ship **single-question + single-select only** (the common shape for wallet agents — "Pick a recipient", "Confirm amount"). Multi-question and multi-select can be added later.

  ```svelte
  <script lang="ts">
    import type { AskUserQuestionItem } from "../../lib/agentClient";

    interface Props {
      questions: AskUserQuestionItem[];
      answered: boolean;
      onSubmit: (answers: Record<string, string | string[]>) => void;
    }
    let { questions, answered, onSubmit }: Props = $props();

    let selected = $state<Record<string, string>>({});

    function pick(qIdx: number, label: string) {
      if (answered) return;
      selected = { ...selected, [String(qIdx)]: label };
    }

    function canSubmit(): boolean {
      if (answered) return false;
      return questions.every((_, idx) => selected[String(idx)]);
    }

    function submit() {
      if (!canSubmit()) return;
      onSubmit({ ...selected });
    }
  </script>

  <form class="answer-form" data-answered={answered} onsubmit={(e) => { e.preventDefault(); submit(); }}>
    {#each questions as q, qIdx}
      <div class="answer-form__q">
        <p class="answer-form__heading">{q.question}</p>
        <div class="answer-form__options">
          {#each q.options as opt}
            <button
              type="button"
              class="answer-form__opt"
              data-selected={selected[String(qIdx)] === opt.label}
              disabled={answered}
              onclick={() => pick(qIdx, opt.label)}
            >
              <span class="answer-form__opt-label">{opt.label}</span>
              {#if opt.description}
                <span class="answer-form__opt-desc">{opt.description}</span>
              {/if}
            </button>
          {/each}
        </div>
      </div>
    {/each}
    {#if !answered}
      <button type="submit" class="answer-form__submit" disabled={!canSubmit()}>
        Send answer
      </button>
    {/if}
  </form>

  <style>
    .answer-form {
      max-width: 540px;
      padding: 12px 14px;
      background: var(--color-surface-app);
      border: 1px solid var(--color-input-border);
      border-radius: 4px 16px 16px 16px;
      display: flex;
      flex-direction: column;
      gap: 12px;
    }
    .answer-form__heading {
      margin: 0;
      font-size: 14px;
      font-weight: var(--font-weight-medium);
      color: var(--color-text-primary);
    }
    .answer-form__options {
      display: flex;
      flex-direction: column;
      gap: 6px;
    }
    .answer-form__opt {
      all: unset;
      cursor: pointer;
      padding: 10px 12px;
      border-radius: 10px;
      border: 1px solid var(--color-card-border);
      display: flex;
      flex-direction: column;
      gap: 2px;
      background: var(--color-surface-app);
    }
    .answer-form__opt[data-selected="true"] {
      background: var(--color-selected-fill);
      border-color: var(--color-action-cream-border);
    }
    .answer-form__opt[disabled] {
      cursor: not-allowed;
      opacity: 0.7;
    }
    .answer-form__opt-label {
      font-size: 14px;
      color: var(--color-text-primary);
    }
    .answer-form__opt-desc {
      font-size: 12px;
      color: var(--color-text-secondary);
    }
    .answer-form__submit {
      align-self: flex-end;
      padding: 8px 16px;
      border-radius: 10px;
      background: var(--color-action-cream);
      border: 1px solid var(--color-action-cream-border);
      color: var(--color-action-cream-text);
      cursor: pointer;
      font-weight: var(--font-weight-medium);
    }
    .answer-form__submit:disabled {
      opacity: 0.5;
      cursor: not-allowed;
    }
  </style>
  ```

- [ ] **Step 6: Render in `Agent.svelte`**

  Add the branch:

  ```svelte
  {:else if message.role === "question"}
    <AnswerForm
      questions={message.questions}
      answered={message.answered}
      onSubmit={(answers) => taskId && answerQuestion(taskId, message.requestId, answers)}
    />
  ```

- [ ] **Step 7: Playwright test**

  ```ts
  test("user-question-request renders a form; clicking an option submits resolve_user_question", async ({ page }) => {
    const daemon = new FakeDaemon({ sessions: [] });
    await bootOnboarded(page, daemon);
    await page.getByLabel("Message").fill("which one?");
    await page.getByLabel("Message").press("Enter");
    const sessionId = "session-created-1";
    const turnId = `turn-${sessionId}`;
    const channel = `session:${sessionId}:event`;
    daemon.emit(channel, { type: "turn-start", turnId });
    daemon.emit(channel, {
      type: "user-question-request",
      turnId,
      requestId: "q1",
      questions: [
        {
          question: "Pick a recipient",
          options: [
            { label: "Alice", description: "Mom" },
            { label: "Bob", description: "Roommate" }
          ]
        }
      ]
    });

    await expect(page.locator(".answer-form__heading")).toHaveText("Pick a recipient");
    await page.locator(".answer-form__opt").filter({ hasText: "Alice" }).click();
    await page.locator(".answer-form__submit").click();

    await daemon.waitForRequest(
      "resolve_user_question",
      (req) =>
        req.params.requestId === "q1" &&
        req.params.answers["0"] === "Alice"
    );

    daemon.emit(channel, { type: "turn-complete", turnId });
  });
  ```

- [ ] **Step 8: Verify + commit**

  ```bash
  cd apps/momo && pnpm check && pnpm test:desktop-ui -- chat.spec.ts
  git add apps/momo/src/lib/agentClient.ts \
          apps/momo/src/lib/chat.svelte.ts \
          apps/momo/src/components/agent/AnswerForm.svelte \
          apps/momo/src/pages/Agent.svelte \
          apps/momo/tests/chat.spec.ts
  git commit -m "feat(desktop-v2): inline answer form for askUserQuestion (single-select MVP)"
  ```

---

## Out of Scope (explicitly deferred)

The following v1 features are **not** in this plan. Document them here so a future engineer knows they were considered and intentionally skipped, not forgotten:

| Feature | v1 Location | Why deferred |
|---|---|---|
| Activity折叠组 (collapse N tool calls into one "Agent activity" row) | `ConversationView.svelte:1620-1736` | Not needed for consumer UX |
| Permission approval inline cards | `Approval.svelte` | Not in product MVP — assume agent uses workspace-write or wallet-side gating instead |
| Diff cards (file changes) | `DiffCard.svelte` | Wallet agent doesn't change code |
| Model picker / Fast / Thinking-level / Permission-mode chips | `ModelPicker.svelte` + composer foot | Power-user feature; hidden from consumers |
| Hydration of tool / thinking / question history from persisted timeline | `chat.svelte.ts:268-311` (current) | When user reopens an old session, tool/thinking/question messages are NOT replayed. Document as a follow-up — needs DTO inspection of `SessionTimelineItem.kind === "tool_call"` etc. |
| Multi-question / multi-select / "other" custom input in `AnswerForm` | `QuestionPrompt.svelte` (v1) | Single-question single-select covers the wallet use case; extend later |
| "Thought for Xs" duration label | v1 `thoughtDurationLabel` | Not requested |

---

## Verification Checklist (whole plan)

After all 5 tasks land, run end-to-end:
- [ ] `cd apps/momo && pnpm check` — type-checks clean
- [ ] `pnpm test:desktop-ui` — all chat tests pass
- [ ] Manual smoke (with `pnpm tauri dev`):
  - [ ] Send a message → user bubble shows with timestamp
  - [ ] Bold/italic in agent response renders (Markdown working)
  - [ ] An external URL in agent response opens in OS browser when clicked
  - [ ] During a long turn, the send button turns red; clicking it stops the turn
  - [ ] Thinking deltas show grey italic block (because dev build)
  - [ ] Tool calls show pills that flip from running → success
  - [ ] If a tool call fails, pill goes red
  - [ ] Asking a question shows a form, picking an option + Submit fires `resolve_user_question`
