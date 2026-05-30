# Repo Guide — read this first

This is the **Puffer Code** Rust workspace (a Rust rebuild of Claude Code).
Crate/runtime/backend details live in **`AGENTS.md`** — read it before touching any `crates/*`.

## `apps/` are standalone front-end subprojects — NOT Next.js / React

Both desktop apps are **Tauri 2 + Svelte 5** (Vite + TypeScript). There is **no**
React, **no** Next.js, and **no** `hooks/use-*.ts` anywhere in them. The
"stores/hooks" equivalent is **Svelte 5 runes in `*.svelte.ts` files**
(e.g. `src/router.svelte.ts`, `src/lib/agent/agentChat.svelte.ts`).

| Path | Stack | Read before editing |
|---|---|---|
| `apps/momo` | Tauri 2 + Svelte 5 | `apps/momo/CLAUDE.md` |
| `apps/puffer-desktop` | Tauri 2 + Svelte 5 | momo's fork source |
| `crates/*` | Rust | `AGENTS.md` |

## How the three relate

- **`crates/puffer-cli`** builds the `puffer` binary — the agent runtime. Its
  `puffer daemon` subcommand is a WebSocket/NDJSON server (`daemon.rs`) exposing
  puffer-core's session/chat/workflow/task/file/lsp/pty/browser RPCs. **Both
  desktop apps connect to it.**
- **`apps/puffer-desktop`** (product name **Corbina**) is a full coding-agent
  **IDE**. It is **multi-provider** (Puffer via the `puffer` CLI, Codex via
  `codex exec`, Claude via `claude --print`) and owns its IDE services
  (git/lsp/pty/browser) in Rust. It can launch a local **or SSH-remote**
  `puffer daemon` against any workspace cwd.
- **`apps/momo`** is a slimmed, **puffer-only consumer fork of Corbina** (chat +
  wallet). It connects only to a local `puffer daemon` rooted at `$HOME`
  (`~/.puffer`), drops the IDE panes, and adds connectors / OAuth login / wallet.

Both apps `cargo build -p puffer-cli` and spawn `puffer daemon` via
`src-tauri/src/daemon_launcher.rs`, then connect from the frontend's
`daemonClient.ts`. They are **Tauri + Svelte, never React** — see each app's
`CLAUDE.md` for details.

**Before working on any subproject, open its own `CLAUDE.md` first.** Do not infer
a stack from the app's name — verify against its `package.json`.
