# BUG-002: No loading feedback on reload and back/forward navigation

## Severity: P1 (blocks main flow — user has no feedback that action was received)

## Screen
Agent Detail → Browser pane status bar and controls

## Starting State
- Browser pane open with a connected tab showing a loaded page
- Status bar shows "Connected"

## Steps to Reproduce
1. Open a session with an active browser tab that has finished loading
2. Click the Reload button (or Back/Forward)
3. Observe the status bar — it remains "Connected" until the daemon responds with a state event

## Expected Result
The status bar immediately shows "Loading" when the user clicks Reload, Back, or Forward, providing instant feedback that the action was received.

## Actual Result
The status bar stays on "Connected" until the daemon's next `browser:state` event arrives (which can take seconds on slow pages). The user has no indication their click was registered.

## Blocking Impact
- User may click Reload/Back/Forward multiple times thinking the first click didn't register
- No visual distinction between "idle" and "waiting for navigation" states
- Particularly confusing on slow-loading pages where the delay is noticeable

## Root Cause
`runHistory` and `reloadActiveTab` dispatch the RPC call but do not update the local `loading` or `status` state synchronously. The same issue existed in `submitUrl` for URL bar submissions.

## Fix
Set `loading = true` and `status = "Loading"` synchronously before dispatching the RPC in `runHistory`, `reloadActiveTab`, and `submitUrl`.

## Files
- `apps/puffer-desktop/src/lib/screens/agent/BrowserPane.svelte` — `runHistory`, `reloadActiveTab`, `submitUrl`
- `apps/puffer-desktop/tests/browser-ui.spec.ts` — regression tests

## Regression Test
`tests/browser-ui.spec.ts`:
- "Status bar shows loading state on reload"
- "Status bar shows loading state on back/forward navigation"
