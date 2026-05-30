# Momo chat test harness

This folder is shared by the daemon chat specs under `apps/momo/tests/agent/`
(reducer + interaction e2e) and the `chat-smoke.spec.ts` regression. The
large `fakeDaemon.ts` is *reference-only* — nobody patches it, helpers wrap it.

## When to reach for which helper

| Want to... | Use |
|---|---|
| Stand up an onboarded page with the fake WS daemon | `bootOnboarded(page, daemon)` (bootHelpers.ts) |
| Open `/agent/<id>` directly (no sidebar click) | `openSession(page, id)` (bootHelpers.ts) |
| Fire turn-start / text-delta / turn-complete from a happy path | `emitTurnStart`, `emitTextDelta`, `emitTurnComplete` (chatEmit.ts) |
| Same, but in one call for the *happy path only* | `emitTurnLifecycle({ deltas })` (chatEmit.ts) |
| Drive a thinking / tool / question event flow | `emitThinkingDelta`, `emitToolRequest`, `emitToolInvocation`, `emitQuestion` |
| Hold an RPC open until you say go | `const p = deferRpc(daemon, "run_agent_turn"); /* assert… */ p.resolve();` (chatTiming.ts; backed by `FakeDaemon.deferRpc`; see the harness self-test in `chat-smoke.spec.ts`) |
| Make an RPC take a fixed ms | `delayRpc(daemon, "load_session_detail", 800)` (chatTiming.ts) |
| Target a chat surface in the DOM | locate the `BubbleConversation` markup directly (messages: `.mb-row[data-role] .mb-bubble` / `.pf-msg-text`; cards: `.pf-tool` / `.pf-approval` / `.pf-question`); the legacy `chatLocators.ts` helpers were removed with the old chat UI |
| **Eyeball the whole chat UI** (every state at once, no live agent) | open the dev-only gallery at `#/dev/chat-gallery` (`src/pages/dev/ChatGallery.svelte`) — real `BubbleConversation` + a fixture covering user/assistant bubbles, tool pill collapse/expand, failed tool, diff, approval card, options-card question + answered echo, system note, typing. Run on any free port: `cd apps/momo && npx vite --host 127.0.0.1 --port 1456` then open `http://localhost:1456/#/dev/chat-gallery` (1466 collides with the dev/test server). Dev-only: route + `PUBLIC_PATHS` entry are gated by `import.meta.env.DEV`, so Vite drops it from prod |
| Type + Enter + IME / state | drive the shell `Composer` directly (`page.getByLabel("Message")`); the legacy `composerHelpers.ts` were removed with the old chat UI |
| Build a fixture session in 1 line | `makeSession({ id, timeline })` (sessionFixtures.ts) |

## When NOT to use a helper — use raw `daemon.emit(channel, payload)`

Race conditions, lifecycle anomalies, and "what if event B arrives between
event A and event C" tests should stay close to the wire. Hiding them
behind sugar is exactly how those bugs slip back in. Specifically:

- Stop / cancel timing (delta-before-cancel, cancel-without-turnId).
- Cross-session leak (event for session A while session B is open).
- Reconnect mid-turn (drop + reconnect, event replay).
- Transcript reload during a live turn.

For these, write the payload by hand and call
`daemon.emit("session:<id>:event", payload)` directly. The point of those
tests is the timing — make it obvious in the diff.

## Locators vs bundled waits

Locator helpers return a `Locator`. They do not await. Test author writes
the `await expect(locator).to...` so the same `locator` can be observed
across multiple states ("this bubble was pending; this same bubble is
now resolved").

If you find yourself wanting `expectAssistantText(page, text)`, push back
— that folds typing-dot-clearance + text-match into one wait, and the
identity-preservation tests then can't observe the bubble across phases.

## Adding new helpers

When a new helper is needed:

1. Add the function with a 1-line JSDoc + example.
2. Add a row to the table above.
3. If it wraps a fakeDaemon API, link the line number it depends on so
   future fakeDaemon maintainers know.
4. Return `Locator` from anything DOM-facing; only `await` inside helpers
   that have no useful timing variants.

## File map

```
fakeDaemon.ts        — websocket fake. Already supports cancel_turn and
                       resolve_user_question. Extend with new public methods
                       when a race needs more wiring (see deferRpc, added
                       2026-05-27 for promise-based RPC pinning).
bootHelpers.ts       — bootOnboarded(), openSession() (stub)
chatEmit.ts          — emit* primitives + emitTurnLifecycle convenience
chatTiming.ts        — deferRpc(), delayRpc()
sessionFixtures.ts   — makeSession(), hydrateMidFlow()
(chatLocators.ts / composerHelpers.ts were removed with the legacy chat UI;
 target the BubbleConversation markup / shell Composer directly.)
```
