# Corbina

Standalone Tauri desktop app for running local coding agents from one workspace UI.

Corbina was forked from the Puffer desktop shell, but it no longer depends on the
Puffer monorepo or an external Puffer daemon. The Tauri host owns local
persistence, git status, file access, filesystem watching, LSP inspection, PTY
terminals, Chrome DevTools Protocol browser streaming, provider process launch,
and event streaming in Rust.

## Supported Providers

- Puffer via the `puffer` CLI, when installed on `PATH`
- Codex via the `codex exec --json` CLI
- Claude via `claude --print --output-format stream-json`

New sessions are provider-scoped. The UI prompts for Puffer, Codex, or Claude
when a session is created, and the selected provider/model is stored on that
session so subsequent turns use the same route.

Provider binaries can be overridden without changing `PATH`:

- `CORBINA_PUFFER_BIN=/absolute/path/to/puffer`
- `CORBINA_CODEX_BIN=/absolute/path/to/codex`
- `CORBINA_CLAUDE_BIN=/absolute/path/to/claude`

Codex and Claude are launched with a built-in Playwright MCP server:

```text
npx --yes @playwright/mcp@latest --headless
```

Codex receives the MCP server through `-c mcp_servers.playwright.*` overrides.
When the Corbina Browser tab can start Chrome, Codex's Playwright MCP server is
pointed at that managed Chrome instance with `--cdp-endpoint=<corbina-cdp-url>`
so browser actions use the same tab surface that the desktop streams. If the
managed browser is unavailable, Codex falls back to Playwright's own headless
browser. Claude receives a generated Playwright MCP config file under
`~/.corbina/playwright-mcp.json`.

## Runtime Services

The copied Puffer panes are backed by standalone Corbina Rust services:

- File manager RPCs validate absolute paths against the current workspace, known
  session roots, and the user's home directory, then return sorted directory
  entries plus UTF-8/base64 file previews.
- Filesystem watches use the native `notify` backend and emit debounced
  `workspace:fs:changed` events for live file-tree refresh.
- LSP inspection launches a matching language server when installed, speaks LSP
  over stdio, and returns hover, definition, and reference summaries for the
  Files pane.
- Terminal tabs use `portable-pty`, replay buffered output, support focus,
  rename, resize, write, close, and prune idle shells.
- Browser tabs launch managed headless Chrome/Chromium profiles, stream
  `Page.screencastFrame` frames over CDP, forward mouse/keyboard/input events,
  expose console/network devtools events, support selection/cursor probing, and
  keep agent-owned browser tabs and short recordings.

These services are session-level capabilities. They are available in sessions
created for Puffer, Codex, and Claude; Codex and Claude browser automation for
agent work continues to use Playwright MCP.

## Runtime Data

Corbina stores app state under `~/.corbina` by default. Set `CORBINA_HOME` to use
a different location.

- `sessions.json` stores Corbina session metadata and transcript summaries
- `config.json` stores the selected provider/model and UI config
- `credentials.json` stores API-key credentials with `0600` permissions on Unix
- `permissions.json`, `pins.json`, and `file-tabs/*` store UI state

Provider CLIs still own their native auth. Corbina detects existing
`~/.codex`, `~/.claude`, and `~/.puffer` credentials and can also pass stored
API keys to the launched provider process.

## Development

```bash
npm install
npm run check
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
npm run test:codex-playwright
npm run tauri dev
```

On Linux, Tauri requires WebKit and appindicator development packages. The
packages used for this environment were:

```bash
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
```

The Playwright regression test requires a browser binary:

```bash
npx playwright install chromium webkit
```

## Verification

Run the production verification set before shipping:

```bash
npm run check
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
npm run test:desktop-ui
npm run test:desktop-ui:webkit
npm run test:codex-playwright
npm audit --audit-level=moderate
npm run tauri build
```

`npm run test:codex-playwright` verifies that Codex resolves the Playwright MCP
server from Corbina's CLI override shape and that Playwright can drive a local
browser page.
