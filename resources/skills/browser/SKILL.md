---
name: browser
description: Use the managed Chrome Browser tab to inspect pages, open tabs, and interact with UI through snapshots and refs.
allowed-tools:
  - Browser
argument-hint: "[url or browser task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use the Browser tool when a task requires a real page in Puffer's managed
Chrome Browser tab: opening a page, switching tabs, inspecting visible UI,
clicking controls, filling fields, pressing keys, or checking page text.

Target: $target

Workflow:

1. Open or find the tab.
   - `{"action":"list"}` lists tabs for the current agent session.
   - `{"action":"open","url":"https://example.com","label":"docs"}` opens a managed Chrome tab.
   - `{"action":"new","url":"https://example.com","label":"scratch"}` forces a fresh managed tab instead of reusing one.
   - `{"action":"focus","tabId":"t1"}` switches the active agent-facing tab handle.

2. Snapshot before interacting.
   - `{"action":"snapshot","tabId":"t1"}` returns visible text and fresh element refs like `@e1`.
   - Refs are scoped to the tab and the latest snapshot. Re-snapshot after navigation, form submits, reloads, or dynamic page updates.

3. Act on refs.
   - `{"action":"click","tabId":"t1","ref":"@e3"}` clicks an element from the latest snapshot.
   - `{"action":"focus_ref","tabId":"t1","ref":"@e3"}` focuses an element from the latest snapshot without clicking it.
   - `{"action":"fill","tabId":"t1","ref":"@e5","text":"hello"}` replaces text in an editable control.
   - `{"action":"type","tabId":"t1","ref":"@e5","text":"hello"}` focuses a ref and inserts text.
   - `{"action":"scrollIntoView","tabId":"t1","ref":"@e5"}` centers a ref before interacting when needed.
   - `{"action":"select","tabId":"t1","ref":"@e6","value":"New York"}` chooses one native `<select>` option by exact value or label.
   - `{"action":"check","tabId":"t1","ref":"@e7"}` and `{"action":"uncheck","tabId":"t1","ref":"@e7"}` toggle checkbox-like controls.
   - `{"action":"press","tabId":"t1","key":"Enter"}` sends a key.

4. Verify with another snapshot.
   Use a new snapshot after each action that could change the page. Prefer refs over brittle coordinates and prefer the current tab id or label over positional assumptions.

Navigation helpers:

- `{"action":"navigate","tabId":"t1","url":"https://example.com"}`
- `{"action":"reload","tabId":"t1"}`
- `{"action":"back","tabId":"t1"}`
- `{"action":"forward","tabId":"t1"}`
- `{"action":"close","tabId":"t1"}`
- `{"action":"quit"}` closes every managed tab in the current browser session.

Additional helpers:

- `{"action":"hover","tabId":"t1","ref":"@e3"}`
- `{"action":"dblclick","tabId":"t1","ref":"@e3"}`
- `{"action":"insertText","tabId":"t1","text":"hello"}`
- `{"action":"keydown","tabId":"t1","key":"Shift"}` and `{"action":"keyup","tabId":"t1","key":"Shift"}`
- `{"action":"scroll","tabId":"t1","direction":"down","px":800}`

The Browser tool controls the same daemon-managed Chrome sessions used by the
Browser tab. v1 tabs are stable Puffer handles over managed Chrome sessions;
do not assume cookies or storage are shared between tabs unless verified.
