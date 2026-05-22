---
name: browser
description: Use the managed Chrome Browser tab through the internal CLI to inspect pages, open tabs, and interact with UI through snapshots and refs.
allowed-tools:
  - Bash
argument-hint: "[url or browser task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use Bash to run the Browser internal CLI when a task requires a real page in
Puffer's managed Chrome Browser tab: opening a page, switching tabs, inspecting
visible UI, clicking controls, filling fields, pressing keys, or checking page
text. Browser is not a model tool; invoke it as `browser ...` when the shell
alias is installed, or as `puffer internal-tool browser ...` everywhere.

Target: $target

Alias setup:

- `puffer internal-tool aliases` prints shell aliases such as
  `alias browser='puffer internal-tool browser'`.
- If the alias is missing, use the full `puffer internal-tool browser` command.

Workflow:

1. Open or find the tab.
   - `browser list` lists tabs for the current agent session.
   - `browser open https://example.com --label docs` opens a managed Chrome tab.
   - `browser tab new https://example.com --label scratch` forces a fresh tab.
   - `browser tab focus t1` switches the active agent-facing tab handle.

2. Snapshot before interacting.
   - `browser snapshot --tab-id t1` returns visible text and fresh refs like `@e1`.
   - Refs are scoped to the tab and the latest snapshot. Re-snapshot after navigation, form submits, reloads, or dynamic page updates.

3. Act on refs.
   - `browser click @e3 --tab-id t1` clicks an element from the latest snapshot.
   - `browser focus @e3 --tab-id t1` focuses an element without clicking it.
   - `browser fill @e5 "hello" --tab-id t1` replaces text in an editable control.
   - `browser type "hello" --ref @e5 --tab-id t1` focuses a ref and inserts text.
   - `browser scroll-into-view @e5 --tab-id t1` centers a ref before interacting when needed.
   - `browser select @e6 "New York" --tab-id t1` chooses one native `<select>` option by exact value or label.
   - `browser check @e7 --tab-id t1` and `browser uncheck @e7 --tab-id t1` toggle checkbox-like controls.
   - `browser press Enter --tab-id t1` sends a key.

4. Verify with another snapshot.
   Use a new snapshot after each action that could change the page. Prefer refs over brittle coordinates and prefer the current tab id or label over positional assumptions.

Navigation helpers:

- `browser navigate https://example.com --tab-id t1`
- `browser reload --tab-id t1`
- `browser back --tab-id t1`
- `browser forward --tab-id t1`
- `browser close --tab-id t1`
- `browser quit`

Additional helpers:

- `browser hover @e3 --tab-id t1`
- `browser dblclick @e3 --tab-id t1`
- `browser keyboard insert-text "hello" --tab-id t1`
- `browser keydown Shift --tab-id t1` and `browser keyup Shift --tab-id t1`
- `browser scroll down --px 800 --tab-id t1`

The Browser CLI controls the same daemon-managed Chrome sessions used by the
Browser tab. v1 tabs are stable Puffer handles over managed Chrome sessions;
do not assume cookies or storage are shared between tabs unless verified.
