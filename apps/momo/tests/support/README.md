# Momo chat test harness

This folder is shared by `apps/momo/tests/chat/**.spec.ts` and the
`chat-smoke.spec.ts` regression. The 2043-line `fakeDaemon.ts` is
*reference-only* — nobody patches it, helpers wrap it.

## When to reach for which helper

| Want to... | Use |
|---|---|
| Stand up an onboarded page with the fake WS daemon | `bootOnboarded(page, daemon)` (bootHelpers.ts) |
| Open `/agent/<id>` directly (no sidebar click) | `openSession(page, id)` (bootHelpers.ts) |
| Fire turn-start / text-delta / turn-complete from a happy path | `emitTurnStart`, `emitTextDelta`, `emitTurnComplete` (chatEmit.ts) |
| Same, but in one call for the *happy path only* | `emitTurnLifecycle({ deltas })` (chatEmit.ts) |
| Drive a thinking / tool / question event flow | `emitThinkingDelta`, `emitToolRequest`, `emitToolInvocation`, `emitQuestion` |
| Hold an RPC open until you say go | `deferRpc(daemon, "load_session_detail")` (chatTiming.ts) |
| Make an RPC take a fixed ms | `delayRpc(daemon, "load_session_detail", 800)` (chatTiming.ts) |
| Target a chat surface in the DOM | `locate*` from chatLocators.ts (returns `Locator`, you await) |
| Type + Enter + IME / state | `composer*` from composerHelpers.ts |
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
fakeDaemon.ts        — DO NOT EDIT. 2043 lines, already supports cancel_turn
                       and resolve_user_question.
bootHelpers.ts       — bootOnboarded(), openSession() (stub)
chatEmit.ts          — emit* primitives + emitTurnLifecycle convenience
chatTiming.ts        — deferRpc(), delayRpc()
chatLocators.ts      — locate{User,Assistant}Bubble, locateThinkingBlock,
                       locateToolPill, locateQuestionForm
composerHelpers.ts   — composer{Type,Submit,IME,ExpectState,ExpectDraft}
sessionFixtures.ts   — makeSession(), hydrateMidFlow()
```
