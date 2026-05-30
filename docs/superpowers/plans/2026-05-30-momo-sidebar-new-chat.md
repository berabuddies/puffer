# Momo sidebar new-chat page + instant session title — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sidebar `+` opens a `/new` page that is just a centered input box; the session is minted only on first send, and the rail shows the first 10 chars of that message instantly until the daemon's small-model title replaces it.

**Architecture:** Front-end only (no daemon/Rust). A new standalone `/new` route renders a centered `Composer` whose `onsubmit` calls the existing `createSessionFromText` → `navigate('/agent/<id>')`. A purely front-end `optimisticTitles` map in `sessionStore` holds the first-10-chars placeholder (never persisted via `rename_session`, which would disable the daemon's auto-title); the rail's title resolution falls back to it, and the daemon's `generated_title` broadcast naturally wins and clears it.

**Tech Stack:** Svelte 5 (runes), TypeScript, hash router (`router.svelte.ts`), Playwright (`@playwright/test`) against `FakeDaemon`.

Spec: `docs/superpowers/specs/2026-05-30-momo-sidebar-new-chat-design.md`

---

## File Structure

**Create:**
- `apps/momo/src/pages/NewChat.svelte` — the `/new` page: a centered `Composer`, custom `onsubmit` → `createSessionFromText` → navigate. No greeting/mascot.
- `apps/momo/tests/new-chat.spec.ts` — e2e for the `/new` page, instant placeholder title, daemon replacement + cleanup.

**Modify:**
- `apps/momo/src/lib/sessionTitle.ts` — add exported `firstChars()` pure helper.
- `apps/momo/src/routes.ts` — register the `/new` route.
- `apps/momo/src/components/shell/Sidebar.svelte` — `startNewChat` → `navigate('/new')`; drop `createNewSession` import; rail label falls back to `optimisticTitle`.
- `apps/momo/src/lib/sessionStore.svelte.ts` — add `optimisticTitles` map, `optimisticTitle()`, `registerOptimisticSession()`; drop `createNewSession`; clear placeholder in `replaceList`.
- `apps/momo/src/lib/agent/agentChat.svelte.ts` — `createSessionFromText` registers the optimistic stub.
- `apps/momo/tests/sessions.spec.ts` — rewrite the two `+`-creates-a-session tests.
- `apps/momo/tests/chat-smoke.spec.ts` — update/remove the two tests that drove `create_session` via the sidebar `+`.

All commands below run from `apps/momo/`.

---

## Task 1: `firstChars` helper (pure function, TDD)

**Files:**
- Modify: `apps/momo/src/lib/sessionTitle.ts`
- Test: `apps/momo/tests/new-chat.spec.ts` (new)

- [ ] **Step 1: Write the failing test**

Create `apps/momo/tests/new-chat.spec.ts`. This first test exercises `firstChars` in the page context by importing the source module (same pattern as `tests/agent/agent-chat-reducer.spec.ts`, which `page.evaluate(async () => await import("/src/lib/..."))`).

```ts
import { expect, test, type Locator, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";

/** The flat Tasks session list in the rail. */
function taskList(page: Page): Locator {
  return page.locator(".spaces .space-items");
}

test("firstChars: collapses whitespace, code-point-safe slice, ellipsis when truncated", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  const results = await page.evaluate(async () => {
    const mod = await import("/src/lib/sessionTitle.ts");
    const f = mod.firstChars as (t: string, n?: number) => string;
    return {
      short: f("hello"),
      exactly10: f("0123456789"),
      eleven: f("0123456789a"),
      whitespace: f("  a   b\nc  "),
      cjk: f("帮我订一张去东京的机票"),
      emoji: f("👍👍👍👍👍👍👍👍👍👍👍")
    };
  });

  expect(results.short).toBe("hello");
  expect(results.exactly10).toBe("0123456789");
  expect(results.eleven).toBe("0123456789…");
  expect(results.whitespace).toBe("a b c");
  // 11 CJK code points → first 10 + ellipsis.
  expect(results.cjk).toBe("帮我订一张去东京的机…");
  // 11 single-scalar emoji → first 10 + ellipsis (Array.from keeps code points whole).
  expect(results.emoji).toBe("👍👍👍👍👍👍👍👍👍👍…");
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx playwright test tests/new-chat.spec.ts -g "firstChars"`
Expected: FAIL — `mod.firstChars` is `undefined` (helper not defined yet), so the `evaluate` throws / assertions fail.

- [ ] **Step 3: Implement `firstChars` in `sessionTitle.ts`**

Append to `apps/momo/src/lib/sessionTitle.ts` (after the existing `sessionTitle` function):

```ts
/**
 * First `n` Unicode code points of the user's first message, with collapsed
 * whitespace and a trailing ellipsis when truncated. Used as an *instant*
 * optimistic rail title the moment a session is created from a message, until
 * the daemon's small-model `generatedTitle` lands and replaces it.
 *
 * `Array.from` iterates by code point, so CJK characters and single-scalar
 * emoji are never split mid-character the way `String.prototype.slice` would.
 */
export function firstChars(text: string, n = 10): string {
  const collapsed = text.trim().replace(/\s+/g, " ");
  const chars = Array.from(collapsed);
  const head = chars.slice(0, n).join("");
  return chars.length > n ? `${head}…` : head;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npx playwright test tests/new-chat.spec.ts -g "firstChars"`
Expected: PASS.

- [ ] **Step 5: Type-check**

Run: `npm run check`
Expected: no new errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/sessionTitle.ts tests/new-chat.spec.ts
git commit -m "feat(momo): add firstChars helper for optimistic rail titles"
```

---

## Task 2: `/new` page + sidebar `+` rewire (drop pre-create), update stale tests

This task delivers the core behavior: `+` no longer creates a session — it opens `/new`, and only sending the first message mints the session. `createNewSession` is removed; the tests that drove it are rewritten. (Optimistic placeholder title comes in Task 3; after this task the rail still shows "New task" briefly, which the rewritten tests account for.)

**Files:**
- Create: `apps/momo/src/pages/NewChat.svelte`
- Modify: `apps/momo/src/routes.ts`
- Modify: `apps/momo/src/components/shell/Sidebar.svelte`
- Modify: `apps/momo/src/lib/sessionStore.svelte.ts`
- Modify: `apps/momo/tests/sessions.spec.ts`
- Modify: `apps/momo/tests/chat-smoke.spec.ts`

- [ ] **Step 1: Rewrite the two `+` tests in `tests/sessions.spec.ts` (failing)**

Replace the first two tests (`'Tasks "+" creates a session that appears in the rail list'` and `'Tasks "+" New chat does not send an unknown "puffer" provider to the daemon'`, lines 20–69) with:

```ts
test('Tasks "+" opens the /new page without creating a session', async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();

  // Opening the new-chat page must NOT mint a session (the old behavior did).
  await expect(page).toHaveURL(/#\/new$/);
  expect(daemon.requests.some((r) => r.method === "create_session")).toBe(false);

  // The page is just the chat composer input.
  await expect(page.getByLabel("Message")).toBeVisible();
});

// Regression: the new-session path must NOT pin a providerId on create_session.
// It used to send `providerId: "puffer"`, which the real daemon rejects with
// `unknown provider \`puffer\`` (canonical providers are openai / anthropic).
// The trigger moved from the sidebar "+" to sending the first message on /new.
test('Sending from /new creates a session without an unknown "puffer" provider', async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();
  await expect(page).toHaveURL(/#\/new$/);

  const createPromise = daemon.waitForRequest("create_session");
  await page.getByLabel("Message").fill("hello there");
  await page.getByLabel("Message").press("Enter");
  const create = await createPromise;

  expect(create.params.providerId).not.toBe("puffer");
  expect(create.params).not.toHaveProperty("providerId");

  // FakeDaemon.createSession mints `session-created-1` with no seeded sessions.
  await page.waitForURL(/#\/agent\/session-created-1$/);
  await expect(page.locator(".toast.toast--error")).toHaveCount(0);
});
```

- [ ] **Step 2: Update the two `+`-driven tests in `tests/chat-smoke.spec.ts` (failing)**

(a) Replace the test `'sidebar "+" New chat navigates to a real session id, not [object Promise]'` (lines 167–189) — the Promise-stringification regression no longer applies because `startNewChat` stops awaiting a Promise. Replace it with a guard that `+` just opens `/new`:

```ts
// The sidebar "+" now opens the /new page instead of minting a session inline,
// so the old [object Promise] stringification bug is structurally gone. Guard
// that the click navigates to /new and creates nothing.
test('sidebar "+" opens the /new page and creates no session on click', async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();
  await expect(page).toHaveURL(/#\/new$/);
  expect(daemon.requests.some((r) => r.method === "create_session")).toBe(false);
});
```

(b) Update the harness test `'harness: deferRpc pins create_session until resolve()'` (lines 194–214) — its trigger was the sidebar `+`, which no longer fires `create_session`. Switch the trigger to the Home composer (which still creates a session on send):

```ts
test("harness: deferRpc pins create_session until resolve() is called", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  // Pin the next create_session response. The Home composer send fires the RPC
  // (the sidebar "+" no longer does), but won't get a result until resolve().
  const pending = deferRpc(daemon, "create_session");

  await page.getByLabel("Message").fill("pin me");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");

  // URL stays put — navigation only fires after the sessionId resolves.
  await page.waitForTimeout(200);
  expect(page.url()).not.toMatch(/#\/agent\//);

  pending.resolve();
  await page.waitForURL(/#\/agent\//, { timeout: 5_000 });
});
```

- [ ] **Step 3: Run the updated tests to verify they fail**

Run: `npx playwright test tests/sessions.spec.ts tests/chat-smoke.spec.ts -g "/new|opens the /new|deferRpc|without an unknown"`
Expected: FAIL — there is no `/new` route yet and `startNewChat` still calls `createNewSession`, so the `+` click still creates a session / never lands on `/new`.

- [ ] **Step 4: Create `apps/momo/src/pages/NewChat.svelte`**

```svelte
<!--
  NewChat — the "/new" page. Intentionally minimal: just the chat Composer,
  centered. No greeting, no mascot, no model picker — the user wanted a bare
  input box (see spec 2026-05-30-momo-sidebar-new-chat).

  Clicking the sidebar "+" lands here WITHOUT creating a session. The session
  is minted only when the user sends their first message: we pass a custom
  `onsubmit` so Composer's default branch (which would treat the "/new" path as
  an active /agent/<id> session) is bypassed. `createSessionFromText` creates
  the session, registers the optimistic rail row + first-10-chars title (Task 3),
  fires the first turn, and returns the new id to navigate into.
-->
<script lang="ts">
  import Composer from "../components/shell/Composer.svelte";
  import { createSessionFromText } from "../lib/agent/agentChat.svelte";
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  function onSubmit(text: string): void {
    void createSessionFromText(text)
      .then((sessionId) => navigate(`/agent/${sessionId}`))
      .catch(() => {
        // Stay on /new so the user can retry; surface the failure.
        pushToast("Couldn't start a new chat — try again.", "error");
      });
  }
</script>

<div class="new-chat">
  <div class="new-chat__inner">
    <Composer placeholder="Hi, Tomo. How's my luck today?" onsubmit={onSubmit} />
  </div>
</div>

<style>
  /* Fullbleed page (see routes.ts): the column is locked to viewport height
     with no padding, so we center the composer ourselves both axes. */
  .new-chat {
    flex: 1;
    min-height: 0;
    height: 100%;
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 var(--shell-page-padding);
  }
  .new-chat__inner {
    width: 100%;
    max-width: var(--shell-page-max);
  }
  /* Composer ships its own top divider; there's nothing above it here. */
  .new-chat :global(.composer) {
    border-top: 0;
  }
</style>
```

- [ ] **Step 5: Register the `/new` route in `apps/momo/src/routes.ts`**

Add the import alongside the other page imports (near `import Agent from "./pages/Agent.svelte";`):

```ts
import NewChat from "./pages/NewChat.svelte";
```

Add the route entry immediately before the `/agent/:taskId` entry (a one-segment `/new` can't collide with the two-segment `/agent/:taskId` in `matchRoute`, but keeping them adjacent groups the chat-entry routes):

```ts
  { pattern: "/new", component: NewChat as Component<Record<string, unknown>>, hasShell: true, fullbleed: true, displayName: "New chat" },
```

- [ ] **Step 6: Rewire `startNewChat` in `apps/momo/src/components/shell/Sidebar.svelte`**

Replace the `startNewChat` function (lines 322–335) and its doc comment with:

```ts
  /**
   * Open the bare "/new" page. We deliberately do NOT create a session here —
   * the session is minted only when the user sends their first message on that
   * page (via the composer's createSessionFromText). This avoids stray empty
   * "New task" rows from clicks that never lead to a message.
   */
  function startNewChat(): void {
    navigate("/new");
  }
```

Then drop `createNewSession` from the `sessionStore.svelte` import (lines 62–67), leaving:

```ts
  import {
    projectSessions,
    renameSession,
    deleteSession
  } from "../../lib/sessionStore.svelte";
```

- [ ] **Step 7: Remove `createNewSession` from `apps/momo/src/lib/sessionStore.svelte.ts`**

Delete the entire `createNewSession` function (lines 184–231). Update the two comments that named it so they don't dangle:

- File-header docstring bullet (line 15) — change:
  ```
   *   - `createNewSession()` — mint a new session via the puffer daemon under
   *     the default project's fixed cwd.
  ```
  to:
  ```
   *   - `registerOptimisticSession()` — insert an optimistic rail row (+ instant
   *     placeholder title) for a session just created from a first message.
  ```
  (The function arrives in Task 3; naming it now keeps the header truthful for the final state.)

- `projectSessions` docstring (line 77) — change `\`createNewSession()\` only ever stubs rows under that same cwd` to `optimistic stubs are only ever created under that same cwd`.

- `replaceList` comment (lines 89–91) — change `an optimistic stub from\n  // createNewSession whose create_session response landed before` to `an optimistic stub from a freshly created session whose create_session\n  // response landed before`.

- [ ] **Step 8: Run the updated tests to verify they pass**

Run: `npx playwright test tests/sessions.spec.ts tests/chat-smoke.spec.ts`
Expected: PASS (all sidebar + chat-smoke tests green, including the rewritten ones).

- [ ] **Step 9: Type-check**

Run: `npm run check`
Expected: no errors (no remaining references to `createNewSession`).

- [ ] **Step 10: Commit**

```bash
git add src/pages/NewChat.svelte src/routes.ts src/components/shell/Sidebar.svelte src/lib/sessionStore.svelte.ts tests/sessions.spec.ts tests/chat-smoke.spec.ts
git commit -m "feat(momo): sidebar + opens /new input page instead of pre-creating a session"
```

---

## Task 3: Instant optimistic title + daemon replacement & cleanup

**Files:**
- Modify: `apps/momo/src/lib/sessionStore.svelte.ts`
- Modify: `apps/momo/src/lib/agent/agentChat.svelte.ts`
- Modify: `apps/momo/src/components/shell/Sidebar.svelte`
- Test: `apps/momo/tests/new-chat.spec.ts`

- [ ] **Step 1: Write the failing e2e tests**

Append to `apps/momo/tests/new-chat.spec.ts`:

```ts
test("first send shows the first-10-chars title instantly, never \"New task\"", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();
  await expect(page).toHaveURL(/#\/new$/);

  // 11 CJK code points → optimistic title is the first 10 + ellipsis.
  const prompt = "帮我订一张去东京的机票";
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  await page.waitForURL(/#\/agent\/session-created-1$/);

  await expect(taskList(page).getByText("帮我订一张去东京的机…", { exact: true })).toBeVisible();
  await expect(taskList(page).getByText("New task", { exact: true })).toHaveCount(0);
});

test("daemon generated title replaces the optimistic placeholder", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();
  const prompt = "帮我订一张去东京的机票";
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await page.waitForURL(new RegExp(`#/agent/${sessionId}$`));
  await expect(taskList(page).getByText("帮我订一张去东京的机…", { exact: true })).toBeVisible();

  // Wait until the store has wired the workspace listener before broadcasting,
  // or the one-shot event races the subscription.
  await daemon.waitForRequest(
    "subscribe_event",
    (req) => req.params.event === "workspace:sessions:changed"
  );
  daemon.updateSessionMetadata(sessionId, {
    displayName: null,
    generatedTitle: "预订东京机票",
    title: "预订东京机票"
  });
  daemon.emit("workspace:sessions:changed", { reason: "generated_title", sessionId });

  await expect(taskList(page).getByText("预订东京机票", { exact: true })).toBeVisible();
  await expect(taskList(page).getByText("帮我订一张去东京的机…", { exact: true })).toHaveCount(0);
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `npx playwright test tests/new-chat.spec.ts -g "first-10-chars|replaces the optimistic"`
Expected: FAIL — without the optimistic map the rail shows "New task" (no `createNewSession` stub anymore), so the placeholder assertions fail.

- [ ] **Step 3: Add the optimistic map + helpers to `sessionStore.svelte.ts`**

Widen the `sessionTitle` import (currently only `NEW_SESSION_TITLE`) and add the `CreateSessionResult` type import:

```ts
import { NEW_SESSION_TITLE, sessionTitle, firstChars } from "./sessionTitle";
```

```ts
import {
  createSession as daemonCreateSession,
  listGroupedSessions,
  renameSession as daemonRenameSession,
  deleteSession as daemonDeleteSession,
  subscribeSessionsChanged,
  type CreateSessionResult,
} from "./agent/daemonChat";
```

After the `sessionList` declaration (line 65), add:

```ts
/**
 * Front-end-only optimistic titles: sessionId → first-10-chars of the first
 * message. Shown in the rail the instant a session is created, until the
 * daemon's `generatedTitle` lands. NEVER persisted via rename_session — that
 * would set `display_name` and permanently disable the daemon's small-model
 * auto-title (see daemon_title.rs::should_auto_title).
 */
const optimisticTitles = $state<Record<string, string>>({});

/** The optimistic placeholder for a session, or undefined when none. */
export function optimisticTitle(id: string): string | undefined {
  return optimisticTitles[id];
}
```

Add the registration function (this replaces the deleted `createNewSession`'s stub logic; place it near the other exported actions):

```ts
/**
 * Insert an optimistic rail row for a session just created from a first
 * message, and record its first-10-chars placeholder title. Called by
 * `createSessionFromText` right after `create_session` returns, so the sidebar
 * shows the new chat (with a meaningful title, never "New task") before the
 * next `loadSessions()` reconciles. The daemon's generated title replaces the
 * placeholder when `workspace:sessions:changed` fires (see `replaceList`).
 */
export function registerOptimisticSession(
  result: CreateSessionResult,
  firstMessage: string
): void {
  const id = result.sessionId;
  const placeholder = firstChars(firstMessage);
  if (placeholder) optimisticTitles[id] = placeholder;
  const stub: SessionListItem = {
    sessionId: id,
    displayName: result.displayName,
    generatedTitle: result.generatedTitle,
    title: result.displayName ?? result.generatedTitle ?? NEW_SESSION_TITLE,
    cwd: result.cwd,
    folderPath: result.cwd,
    updatedAtMs: result.updatedAtMs ?? result.createdAtMs,
    createdAtMs: result.createdAtMs,
    eventCount: 0,
    activityStatus: "idle",
    slug: result.slug,
    tags: [],
    note: null,
    parentSessionId: null,
    providerId: result.providerId ?? null,
    modelId: result.modelId ?? null,
  };
  const existing = sessionList.findIndex((s) => s.sessionId === id);
  if (existing >= 0) sessionList.splice(existing, 1);
  sessionList.unshift(stub);
}
```

- [ ] **Step 4: Clear the placeholder in `replaceList` once a real title exists**

In `replaceList` (lines 88–96), after computing `merged` and before the `splice`, drop optimistic entries whose daemon title has landed:

```ts
function replaceList(next: SessionListItem[]): void {
  const nextIds = new Set(next.map((s) => s.sessionId));
  const localOnly = sessionList.filter((s) => !nextIds.has(s.sessionId));
  const merged = sortByUpdatedDesc([...next, ...localOnly]);
  // Once the daemon has an authoritative title (display name / generated
  // title), the optimistic placeholder is no longer needed — drop it so the
  // map doesn't grow unbounded. The title resolution order already prefers the
  // daemon title, so this is just cleanup.
  for (const s of merged) {
    if (optimisticTitles[s.sessionId] && sessionTitle(s)) {
      delete optimisticTitles[s.sessionId];
    }
  }
  sessionList.splice(0, sessionList.length, ...merged);
}
```

- [ ] **Step 5: Register the optimistic stub in `createSessionFromText`**

In `apps/momo/src/lib/agent/agentChat.svelte.ts`, add the import (alongside the existing `projectStore` import group):

```ts
import { registerOptimisticSession } from "../sessionStore.svelte";
```

Change the body of `createSessionFromText` (lines 1386–1392) from destructuring just `sessionId` to keeping the full result and registering the stub:

```ts
  const result = await createSession(cwd);
  const sessionId = result.sessionId;
  registerOptimisticSession(result, trimmed);
  ensureState(sessionId);
  ensureSubscription(sessionId);
  if (trimmed) {
    await submitMessageImpl(sessionId, trimmed);
  }
  return sessionId;
```

- [ ] **Step 6: Fall back to the optimistic title in the rail label**

In `apps/momo/src/components/shell/Sidebar.svelte`, add `optimisticTitle` to the `sessionStore.svelte` import:

```ts
  import {
    projectSessions,
    renameSession,
    deleteSession,
    optimisticTitle
  } from "../../lib/sessionStore.svelte";
```

Change `sessionLabel` (lines 190–192) to insert the optimistic placeholder between the daemon title and the "New task" fallback:

```ts
  function sessionLabel(session: SessionListItem): string {
    return sessionTitle(session) ?? optimisticTitle(session.sessionId) ?? NEW_SESSION_TITLE;
  }
```

- [ ] **Step 7: Run the Task 3 tests to verify they pass**

Run: `npx playwright test tests/new-chat.spec.ts`
Expected: PASS (firstChars + both placeholder tests).

- [ ] **Step 8: Type-check**

Run: `npm run check`
Expected: no errors (no import cycle: `agentChat → sessionStore` is one-way; `sessionStore` imports `daemonChat`/`sessionTitle`, not `agentChat`).

- [ ] **Step 9: Full desktop-ui suite**

Run: `npm run test:desktop-ui`
Expected: PASS. (Watch for the known local gotchas — a reused 1466 dev server bypassing FakeDaemon, and `.env` domain mismatches — before treating any failure as a regression.)

- [ ] **Step 10: Commit**

```bash
git add src/lib/sessionStore.svelte.ts src/lib/agent/agentChat.svelte.ts src/components/shell/Sidebar.svelte tests/new-chat.spec.ts
git commit -m "feat(momo): instant first-10-chars rail title, replaced by daemon generated title"
```

---

## Self-Review

**Spec coverage:**
- Goal "`+` opens welcome page, no pre-create" → Task 2 (route, NewChat, startNewChat, removed createNewSession). ✓
- Goal "welcome page = just a centered input box" → Task 2 Step 4 (NewChat.svelte, no greeting/mascot). ✓
- Goal "first 10 chars instant" → Task 3 (firstChars + registerOptimisticSession + sessionLabel fallback). ✓
- Goal "daemon small-model title replaces it" → Task 3 Step 1 second test + existing `subscribeSessionChanges`/`replaceList`. ✓
- Goal "never persist via rename_session" → respected: optimistic map is local; no rename_session call added. ✓
- Goal "no daemon/Rust changes" → all files are under `apps/momo/src` + tests. ✓
- Spec §1 custom `onsubmit` (avoid Composer treating `/new` as a session) → Task 2 Step 4. ✓
- Spec §3 placeholder retention across `loadSessions` (no flash to "New task") → `replaceList` leaves `optimisticTitles` intact except when a real title exists. ✓
- Spec §4 stub fields → Task 3 Step 3 matches the `agentClient.SessionListItem` shape. ✓
- Spec testing list → firstChars unit (Task 1), `+`→`/new` no-create (Task 2), instant placeholder + daemon replacement (Task 3). ✓

**Placeholder scan:** No "TBD"/"TODO"/"handle edge cases"; every code step shows full code. ✓

**Type consistency:** `registerOptimisticSession(result, firstMessage)` defined in Task 3 Step 3, called identically in Task 3 Step 5. `optimisticTitle(id)` defined Step 3, used Step 6. `firstChars(text, n=10)` defined Task 1, used in `registerOptimisticSession`. `CreateSessionResult` imported from `./agent/daemonChat` (matches its export). `SessionListItem` from `./agentClient` (already imported in sessionStore). ✓
