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
visible UI, clicking controls, filling fields, pressing keys, uploading files,
capturing screenshots, evaluating page JavaScript, or checking page text.
Browser is not a model tool and must not be requested as a provider tool call.
Run Browser commands as `browser ...` inside Bash.

Target: $target

Workflow:

1. Open or find the tab.
   - `browser list` lists tabs for the current agent session.
   - `browser open https://example.com --label docs --width 1280 --height 900` opens or reuses a managed Chrome tab.
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
   - `browser upload @e9 ./file.pdf --tab-id t1` attaches one or more files to a file input.
   - `browser check @e7 --tab-id t1` and `browser uncheck @e7 --tab-id t1` toggle checkbox-like controls.
   - `browser press Enter --tab-id t1` sends a key.

4. Verify with another snapshot.
   Use a new snapshot after each action that could change the page. Prefer refs over brittle coordinates and prefer the current tab id or label over positional assumptions.

Global options:

- `browser --json ...` prints machine-readable JSON.
- `browser --session-id <id> ...` uses another root browser session.
- Most page and ref commands accept `--tab-id t1`, `--width 1280`, and `--height 900`.

Tab and session commands:

- `browser list`
- `browser open [url] [--tab-id t1] [--label docs] [--width 1280] [--height 900]`
- `browser tab list`
- `browser tab new [url] [--tab-id t2] [--label scratch] [--width 1280] [--height 900]`
- `browser tab focus t1` or `browser tab select t1`
- `browser tab close [t1]`
- `browser close --tab-id t1`
- `browser close --group`
- `browser quit` or `browser exit`

Navigation commands:

- `browser navigate https://example.com --tab-id t1`
- `browser goto https://example.com --tab-id t1`
- `browser reload --tab-id t1`
- `browser back --tab-id t1`
- `browser forward --tab-id t1`

Inspection commands:

- `browser snapshot --tab-id t1`
- `browser screenshot --tab-id t1`
- `browser screenshot ./page.png --annotate --tab-id t1`
- `browser screenshot --screenshot-dir .puffer/screenshots --screenshot-format jpeg --screenshot-quality 90 --tab-id t1`
- `browser eval "document.title" --tab-id t1` or `browser evaluate "document.title" --tab-id t1`

Interaction commands:

- `browser click @e3 --tab-id t1`
- `browser dblclick @e3 --tab-id t1`
- `browser hover @e3 --tab-id t1`
- `browser focus @e3 --tab-id t1` or `browser focus-ref @e3 --tab-id t1`
- `browser fill @e5 "hello" --tab-id t1`
- `browser select @e6 "New York" --tab-id t1`
- `browser upload @e9 ./one.png ./two.png --tab-id t1`
- `browser check @e7 --tab-id t1`
- `browser uncheck @e7 --tab-id t1`
- `browser type "hello" --tab-id t1`
- `browser type "hello" --ref @e5 --tab-id t1`
- `browser press Enter --tab-id t1` or `browser key Enter --tab-id t1`
- `browser keydown Shift --tab-id t1`
- `browser keyup Shift --tab-id t1`
- `browser keyboard type "hello" --tab-id t1`
- `browser keyboard insert-text "hello" --tab-id t1`
- `browser scroll down 800 --tab-id t1`
- `browser scroll up 600 --tab-id t1`
- `browser scroll left 400 --tab-id t1`
- `browser scroll right 400 --tab-id t1`
- `browser scroll-into-view @e5 --tab-id t1` or `browser scrollinto @e5 --tab-id t1`

The Browser CLI controls the same daemon-managed Chrome sessions used by the
Browser tab. v1 tabs are stable Puffer handles over managed Chrome sessions;
do not assume cookies or storage are shared between tabs unless verified. Use
only commands documented here; if a command is missing, inspect the CLI help
instead of inventing a command shape.
