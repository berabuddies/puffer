# BUG-001: Address bar overwritten by background state events during user editing

## Severity: P1 (blocks main flow — user loses typed URL)

## Screen
Agent Detail → Browser pane address bar

## Starting State
- Browser pane open with a connected tab showing `https://example.com`
- User has clicked the address bar and is typing a new URL

## Steps to Reproduce
1. Open a session with an active browser tab
2. Click the address bar and begin typing a URL (e.g., `https://new-site.com/path`)
3. While typing, the agent navigates the browser (or a redirect occurs), causing a `browser:state` event with a different URL
4. The state event's URL overwrites the user's in-progress input

## Expected Result
The address bar preserves the user's typed text while the input is focused. Background state events update `currentUrl` (the canonical URL) but do not touch `urlDraft` (the displayed/editable value) until the user blurs the input or submits.

## Actual Result
`applyState` and `applyRecordingFrame` unconditionally set `urlDraft = nextUrl`, destroying the user's in-progress text. The user must retype the URL.

## Blocking Impact
- User cannot reliably type a URL while the agent is actively using the browser
- Particularly disruptive during redirects or when the agent navigates rapidly
- Forces the user to race against background events to submit their URL

## Root Cause
Both `applyState` (line ~717) and `applyRecordingFrame` (line ~669) set `urlDraft = nextUrl` without checking whether the address bar is currently focused/being edited.

## Fix
Guard `urlDraft` assignment with `isAddressEditing()` which checks `document.activeElement === addressInput`. Also blur the address input on form submit so that subsequent state events (redirects) can update the displayed URL.

## Files
- `apps/puffer-desktop/src/lib/screens/agent/BrowserPane.svelte` — `applyState`, `applyRecordingFrame`, `submitUrl`
- `apps/puffer-desktop/tests/browser-ui.spec.ts` — regression tests

## Regression Test
`tests/browser-ui.spec.ts`:
- "Address bar preserves user input when a background state event arrives"
- "Address bar updates after user submits a URL"
- "Address bar updates when switching tabs even if previously focused"
