# Playwright Adapter Notes

This document maps generated fuzz actions to the existing desktop Playwright
test harness. It is a contract for the next implementation layer; agents can
use it immediately when converting generated cases into deterministic specs.
The machine-readable action support map is
`apps/puffer-desktop/tests/fuzz/adapters/playwright-actions.json`.

## Current Harness

Use:

- `apps/puffer-desktop/tests/support/fakeDaemon.ts`
- `new FakeDaemon(...)`
- `await daemon.install(page)`
- `await daemon.open(page)`
- `daemon.delayResponse(method, predicate, ms)`
- `daemon.delayFailure(method, predicate, error, ms)`
- `daemon.failNext(method, error)`
- `daemon.emit(event, payload)`
- `await daemon.waitForRequest(method, predicate)`

Generated replay specs install a local turn-id shim around `FakeDaemon` so
each `run_agent_turn` response returns a unique turn id. This keeps multi-turn
chat replay aligned with real daemon behavior and prevents fake-daemon turn id
reuse from creating false stop/permission/question failures.

## Common Setup

```ts
const daemon = new FakeDaemon();
await daemon.install(page);
await daemon.open(page);
await page.locator(".pf-sidebar-agents-list").getByRole("button", { name: /^Browser regression\b/ }).click();
```

## Action Mapping

| Fuzz action | Playwright / fake daemon mapping |
| --- | --- |
| `open-workspace` | `await daemon.open(page)` |
| `open-agent-detail` | click `.pf-sidebar-agents-list` agent button or workspace card |
| `open-browser-pane` | click `getByRole("button", { name: "Browser", exact: true })` and wait for `browser_open` |
| `type-url` | `await page.locator(".pf-browser-address").fill(url)` |
| `press-address-enter` | `await page.locator(".pf-browser-address").press("Enter")` and wait for `browser_navigate` |
| `click-new-tab` | click `.pf-browser-tab-add` and wait for `browser_agent` action `open` |
| `close-active-tab` | click active browser tab close button and wait for `browser_agent` action `close` |
| `switch-tab` | click the indexed browser tab button |
| `reload` | click `button[title='Reload']` and wait for `browser_reload` |
| `history` | click `button[title='Back']` or `button[title='Forward']` and wait for `browser_history` |
| `type-page-input` | click browser canvas/page target, then `page.keyboard.type(text)` |
| `special-key` | click browser canvas/page target, then `page.keyboard.press(key)` |
| `emit-state-for-old-tab` | `daemon.emit("browser:<old-id>:state", payload)` |
| `emit-frame-burst` | loop `daemon.emit("browser:<tab-id>:frame", framePayload)` |
| `type-composer` | fill the chat composer textarea |
| `send-prompt` | click send button and wait for turn-start daemon method |
| `stop-turn` | click stop button and wait for stop/cancel daemon method |
| `emit-old-session-stream` | `daemon.emit(...)` using the previous session id |
| `emit-permission` | push pending permission event in the active session timeline |
| `answer-permission` | click approve/deny in the permission prompt |
| `emit-question` | push pending question event in the active session timeline |
| `answer-question` | fill/click question response UI |
| `import-credential` | open Settings > Providers and trigger import for provider |
| `refresh-credential` | trigger provider refresh button |
| `save-default-model` | change default provider/model and click save |
| `open-file` | open Files pane and select/open path |
| `edit-file` | type into file editor |
| `save-file` | click file save and wait for file write daemon request |
| `open-terminal` | open Terminal pane and create a pty tab |
| `type-terminal` | focus terminal and type text |
| `close-terminal` | close active terminal tab |
| `add-mcp-server` | open Settings > MCP and submit add form |
| `save-permissions` | open Settings > Permissions and click save |
| `disconnect-reconnect` | close/reinstall fake daemon websocket or reload with same backend URL |
| `resize-narrow` | `await page.setViewportSize({ width, height })` |

`stop-turn` leaves the replay in a canceling state until a terminal turn event
is emitted. Chat seeds should settle that state with `settle-canceled-turn`
before generating model changes or normal session-switch follow-up actions.

## Invariant Mapping

| Invariant | Suggested assertion |
| --- | --- |
| `app-no-crash` | assert page is not closed; no fatal console/pageerror; root app visible |
| `no-permanent-loading` | pending indicator clears after response or remains scoped to a real pending request |
| `active-session-stable` | selected session id/title remains the newer selection |
| `active-tab-stable` | active browser tab label/url remains the newer selection |
| `draft-preserved-on-failure` | input value remains after failed request |
| `one-request-per-intent` | count matching `daemon.requests` entries |
| `stale-error-scoped` | old error does not appear on active object |
| `controls-disabled-while-pending` | effectful button disabled after first submit until settled |
| `no-cross-provider-model` | selected model/provider pair remains compatible |
| `focus-restored` | `document.activeElement` is reachable after modal close |
| `no-data-loss` | edited file/settings value survives refresh/failure |

## Next Implementation Step

Add a typed Playwright adapter that loads generated JSON cases and dispatches
known action ids to these handlers. Unknown action ids should fail fast so agent
tasks do not silently skip coverage.
