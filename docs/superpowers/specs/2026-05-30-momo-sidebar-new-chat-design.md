# Momo sidebar new-chat flow + instant session title — design

**Date:** 2026-05-30
**Owner:** sean
**Area:** `apps/momo` (frontend only; no daemon / Rust changes)

## Background

Today the sidebar `+` (Tasks header, `Sidebar.svelte::startNewChat`) calls
`createNewSession()`, which **immediately** mints an empty puffer session via the
daemon and navigates into it. Two problems follow:

1. **Empty sessions pile up.** Clicking `+` always creates a session even if the
   user never sends a message, leaving stray `New task` rows in the rail.
2. **The title lags.** A session's real title is the daemon's `generated_title`,
   produced by a small title model (`daemon_title.rs`: `gpt-5.4-mini`,
   `fast_mode`, `effort=low`, no tools) **synchronously before the first turn's
   agent work begins**. Until that request returns, the rail shows the
   `NEW_SESSION_TITLE` placeholder (`"New task"`). So a freshly-sent chat reads
   "New task" for several seconds.

The daemon already auto-titles with a small model — the perceived slowness is
that the title only appears *after* that request round-trips, and the rail has
nothing better to show meanwhile.

## Goals

- Clicking `+` no longer pre-creates a session. It opens a **welcome page** with
  an input box; the session is created only when the user sends their first
  message.
- The moment a session is created from the user's first message, the rail shows
  the **first 10 characters** of that message as an instant placeholder title —
  never `"New task"`.
- The daemon's small-model `generated_title` continues to be produced and
  **replaces** the placeholder when it lands.
- Net effect: the `"New task"` title is never visible in the normal new-chat
  flow.

## Non-goals

- **No daemon / Rust changes.** The daemon already titles with a small model.
  The known issue that title generation runs *synchronously before* the first
  turn (and therefore delays the first agent token) is **out of scope** —
  changing it means editing `puffer-core` + recompiling the daemon binary, and
  the user only reported title slowness. Tracked as a possible follow-up.
- No model picker, microphone, voice waveform, or prompt-category chips on the
  welcome page (momo doesn't have these features).

## Design

### 1. Welcome page — `NewChat.svelte` at route `/new`

A new standalone page that is **just a centered input box** — no greeting, no
mascot/logo, nothing else. (The Claude-desktop screenshot was the layout
reference, but the user wants it stripped to only the composer.) Submitting goes
straight into the chat view.

```
┌────────────────────────────────────
│
│
│   ┌────────────────────────────────────┐
│   │  Hi, Tomo. How's my luck today?     │   ← the SAME Composer.svelte,
│   │  [+]                          [↑]   │     centered (vertical + horizontal)
│   └────────────────────────────────────┘
│
│
└────────────────────────────────────
```

- **Route:** add `{ pattern: "/new", component: NewChat, hasShell: true,
  fullbleed: true, displayName: "New chat" }` to `routes.ts`. A standalone `/new`
  (one segment) avoids competing with `/agent/:taskId` (two segments) in
  `matchRoute`, so `Agent.svelte` is untouched.
- **Layout:** the composer is centered in the content column (matching the chat
  composer's max-width so it lines up with where it sits in the chat view), both
  vertically and horizontally. No header text, no decorative elements.
- **Input:** reuse `Composer.svelte` verbatim (the chat composer), passing a
  custom `onsubmit`. We must pass `onsubmit` because Composer's *default* branch
  treats any `/agent/...`-ish path as an active session; on `/new` there is no
  session yet, so the page owns submission:

  ```ts
  function onSubmit(text: string): void {
    void createSessionFromText(text).then((sessionId) => {
      navigate(`/agent/${sessionId}`);
    });
    // createSessionFromText surfaces its own errors via the controller;
    // the welcome page does not need a separate catch.
  }
  ```

  `placeholder` stays momo's existing `"Hi, Tomo. How's my luck today?"` for
  consistency with chat/Home. Empty/whitespace input is ignored (Composer
  already guards this).

### 2. Sidebar `+` rewire

`Sidebar.svelte::startNewChat` becomes:

```ts
function startNewChat(): void {
  navigate("/new");
}
```

`createNewSession` is then referenced nowhere (only `Sidebar.svelte` imported
it). **Remove `createNewSession` from `sessionStore.svelte.ts`** and drop the
import + its doc-comment bullet. The optimistic-stub construction it did is
preserved by being moved into the new `registerOptimisticSession` (§4).

### 3. Instant first-10-chars title + daemon small-model replacement

**Hard constraint (the trap):** the first-10-chars title must **never** be
persisted via `rename_session`. The daemon's `should_auto_title` only generates
a title when *both* `display_name` and `generated_title` are empty
(`daemon_title.rs`). Writing the placeholder as `display_name` would permanently
disable the small-model title, freezing the title at the truncated prefix. So
the placeholder is a **purely front-end optimistic layer**.

**Mechanism — `optimisticTitles` map in `sessionStore.svelte.ts`:**

```ts
// sessionId → optimistic placeholder (first ~10 chars of first message).
const optimisticTitles = $state<Record<string, string>>({});

export function optimisticTitle(id: string): string | undefined {
  return optimisticTitles[id];
}
```

**Title resolution order** used by the rail (`Sidebar.svelte::sessionLabel`) and
optionally the chat header:

```
sessionTitle(session)            // daemon display_name || generated_title
  ?? optimisticTitle(id)         // front-end first-10-chars placeholder
  ?? NEW_SESSION_TITLE           // last-resort "New task"
```

So:
- On send → placeholder shows instantly (no `"New task"`).
- `loadSessions()` / `replaceList()` **do not touch** `optimisticTitles`, so a
  refetch that lands while `generated_title` is still null keeps the placeholder
  (does not flash back to `"New task"`).
- When the daemon's `generated_title` lands and broadcasts
  `workspace:sessions:changed` (reason `generated_title`) →
  `subscribeSessionChanges` → `loadSessions()` → now `sessionTitle(session)` is
  non-null and **naturally wins** over the placeholder.

**Placeholder cleanup:** in `replaceList()` (or a small reconcile after it),
delete `optimisticTitles[id]` for any session whose `sessionTitle(session)` is
now non-null. Prevents unbounded growth of the map and keeps the daemon title
authoritative. Deleting a stale entry is harmless even if it races, because the
resolution order already prefers the daemon title.

**`firstChars` helper:**

```ts
function firstChars(text: string, n = 10): string {
  const collapsed = text.trim().replace(/\s+/g, " ");
  const chars = Array.from(collapsed); // Unicode code points (CJK/emoji-safe)
  const head = chars.slice(0, n).join("");
  return chars.length > n ? `${head}…` : head;
}
```

### 4. `createSessionFromText`: register optimistic stub

`createSessionFromText` (`agentChat.svelte.ts`) currently does **not** insert a
sidebar row — Home/task new-sessions only appear after a broadcast-driven
`loadSessions()`. We make it register an optimistic stub so every "send first
message → new session" entry point (welcome page, Home composer, task cards)
shows the row instantly with the first-10-chars title.

Add to `sessionStore.svelte.ts`:

```ts
export function registerOptimisticSession(
  result: CreateSessionResult,
  firstMessage: string,
): void {
  const id = result.sessionId;
  optimisticTitles[id] = firstChars(firstMessage);
  const stub: SessionListItem = { /* same shape createNewSession built */ };
  const existing = sessionList.findIndex((s) => s.sessionId === id);
  if (existing >= 0) sessionList.splice(existing, 1);
  sessionList.unshift(stub);
}
```

`createSessionFromText` calls `registerOptimisticSession(result, trimmed)` right
after `createSession(cwd)` returns, before `submitMessageImpl`. (`createSession`
must return the full `CreateSessionResult`; today it's destructured to
`{ sessionId }` — widen the local binding.)

This makes `createSessionFromText` import from `sessionStore`. Dependency
direction is safe: `sessionStore → daemonChat/projectStore`, and `agentChat →
sessionStore` is new but one-way (sessionStore does not import agentChat), so no
cycle.

## Data flow (welcome page happy path)

```
user on /new types "帮我订一张去东京的机票", presses Enter
  → Composer.onsubmit(text)
  → createSessionFromText(text)
      → loadProjects(); cwd = default project cwd
      → create_session(cwd)            ─┐ daemon mints session
      → registerOptimisticSession(...)  │ rail row appears now:
          optimisticTitles[id]="帮我订一张去东京的…"   title = first-10-chars
          sessionList.unshift(stub)    ─┘
      → submitMessageImpl(id, text)     → run_agent_turn (fire-and-return)
  → navigate(`/agent/${id}`)            chat view opens

(daemon, before first turn's agent work)
  → generate_title_with_model (gpt-5.4-mini)
  → set_generated_title + broadcast workspace:sessions:changed (generated_title)
  → subscribeSessionChanges → loadSessions()
  → rail row title swaps placeholder → daemon generated_title
  → reconcile drops optimisticTitles[id]
```

## Edge cases & error handling

- **`create_session` fails:** `createSessionFromText` rejects; no stub is
  registered (registration happens only after a successful create). The welcome
  page stays put; the controller/store surfaces the error toast as today.
- **Refetch arrives before broadcast:** placeholder retained (see §3) — no flash
  to `"New task"`.
- **User never sends on `/new`:** no session created (the goal). Navigating away
  leaves nothing behind.
- **Direct navigation / reload of `/new`:** renders the welcome page (stateless).
- **Slash-command / vague first message:** daemon still auto-titles per its
  existing rules; placeholder covers the gap regardless.
- **Composer default branch:** unchanged for `/agent/:id` and Home; only the
  welcome page passes a custom `onsubmit`.

## Testing

Playwright (`test:desktop-ui`) against FakeDaemon, plus `npm run check`:

- Sidebar `+` navigates to `/new` and does **not** create a session.
- `/new` renders only the Composer (no greeting, no mascot, no model
  picker/mic/chips).
- Submitting on `/new` creates a session, navigates to `/agent/<id>`, and the
  rail row shows the first-10-chars placeholder (assert it is **not**
  `"New task"`).
- When FakeDaemon emits `generated_title` + `workspace:sessions:changed`, the
  rail row updates to the daemon title and the placeholder no longer governs.
- `firstChars` unit coverage: ASCII, CJK, emoji, whitespace collapse, ≤10 vs
  >10 (ellipsis).

> Known local test gotchas (see memory `momo-playwright-test-pitfalls`): a reused
> 1466 dev server bypasses FakeDaemon, and `.env` domain ≠ hardcoded test domain.
> Rule these out before treating a failure as a regression.

## Copy defaults (confirm at spec review)

- Welcome page: **just a centered Composer** — no greeting, no mascot/logo.
- Composer placeholder: `Hi, Tomo. How's my luck today?` (unchanged).
- Placeholder title length: first **10** characters, `…` when longer.
