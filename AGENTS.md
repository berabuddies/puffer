# Puffer Code Agents

This repository is a production-facing Rust rebuild of Claude Code under the
name `Puffer Code`.

## Primary Goal

Match Claude Code behavior where it matters for coding workflows, while:

- removing telemetry and feedback/reporting infrastructure
- preserving Claude-compatible Anthropic request behavior where required
- supporting Anthropic and OpenAI with API key and OAuth flows
- using a native Rust TUI instead of Ink
- keeping prompts and tool metadata editable through declarative resource files

## Current Workspace

The repo is a Cargo workspace with these main crates and component crates:

- `puffer-cli`
  Top-level CLI, auth commands, session commands, and interactive entrypoint.
- `puffer-config`
  Config discovery and layered config loading.
- `puffer-connector-core`
  Shared connector runtime, conversation/session mapping, commands, splitting,
  and retry helpers.
- `puffer-connector-discord`
  Discord bot gateway connector.
- `puffer-connector-email`
  IMAP/SMTP email connector.
- `puffer-connector-matrix`
  Matrix room connector.
- `puffer-connector-slack`
  Slack Socket Mode connector.
- `puffer-connector-telegram`
  Telegram bot connector.
- `puffer-connector-webhook`
  HTTP webhook connector.
- `puffer-core`
  Command registry, app state, slash-command dispatch, and provider execution.
- `puffer-observability`
  Optional OTLP/HTTP tracing with conservative redaction defaults.
- `puffer-provider-openai`
  OpenAI auth and request helpers.
- `puffer-provider-registry`
  Provider descriptors, auth storage, and provider provenance.
- `puffer-resources`
  Bundled/user/workspace resources: prompts, tools, skills, plugins, MCP, IDE, mascot metadata.
- `puffer-session-store`
  Session metadata, transcript events, listing, load, and fork support.
- `puffer-subscriber-email`
  Out-of-process email subscriber.
- `puffer-subscriber-runtime`
  Subscriber manifest, process supervision, command, and event bus runtime.
- `puffer-subscriber-telegram-user`
  Out-of-process Telegram personal-account subscriber.
- `puffer-subscriptions`
  Subscription specs, event routing, classifiers, and action dispatch.
- `puffer-test-support`
  Test helpers for commands, terminal output, temp workspaces, and tmux probing.
- `puffer-tools`
  Tool schemas, registry, built-in tools, and execution backends.
- `puffer-transport-anthropic`
  Anthropic auth, fingerprinting, attribution block generation, and request building.
- `puffer-tui`
  Ratatui/Crossterm interactive UI.
- `puffer-workflow`
  Native workflow definitions, validation, cron scheduling, DAG execution, and
  run storage.

## Hard Repo Constraints

- Every public Rust function must have a docstring.
- No Rust source file may exceed 1000 lines.
- Use ASCII unless there is an existing reason not to.
- Keep modules small and purpose-specific.
- Prefer stable, typed Rust APIs over stringly-typed plumbing.

## Supported Slash Commands

The current supported command surface is intended to include:

- `/add-dir`
- `/agents`
- `/branch`
- `/btw`
- `/buddy`
- `/clear`
- `/color`
- `/compact`
- `/config`
- `/context`
- `/copy`
- `/cost`
- `/diff`
- `/doctor`
- `/effort`
- `/exit`
- `/export`
- `/fast`
- `/help`
- `/hooks`
- `/ide`
- `/init`
- `/keybindings`
- `/login`
- `/logout`
- `/mcp`
- `/memory`
- `/model`
- `/permissions`
- `/plan`
- `/plugin`
- `/pr-comments`
- `/reload-plugins`
- `/remote-control`
- `/remote-env`
- `/rename`
- `/resume`
- `/rewind`
- `/review`
- `/sandbox`
- `/security-review`
- `/session`
- `/skills`
- `/status`
- `/statusline`
- `/tasks`
- `/terminal-setup`
- `/theme`
- `/usage`
- `/vim`
- `/skill:<name>`

Aliases live in `puffer-core`.

## Out of Scope

Do not reintroduce:

- telemetry
- analytics
- feedback upload/reporting flows
- privacy/telemetry settings flows
- Claude-branded mobile/desktop/product marketing commands

## Auth Expectations

Current auth command surface in `puffer-cli`:

- `puffer auth status`
- `puffer auth set-api-key <provider> [--stdin]`
- `puffer auth clear <provider>`
- `puffer auth oauth-url <provider>`
- `puffer auth oauth-start <provider>`
- `puffer auth oauth-exchange <provider> --verifier ... [--state ...] [--stdin]`
- `puffer auth oauth-refresh <provider>`
- `puffer auth login <provider> [value] [--stdin]`

The intended provider coverage is:

- Anthropic API key
- Anthropic OAuth
- OpenAI API key
- OpenAI/Codex OAuth

## Anthropic Compatibility Notes

Anthropic compatibility is stricter than other providers.

Preserve:

- header order where the repo models it
- Claude-style `claude-cli/...` user agent
- attribution block as a standalone first system block
- fingerprinting and CCH placeholder logic
- session-ingress auth behavior

Do not simplify the Anthropic path into generic provider code if that would
erase these details.

## Resource Model

Bundled resources live under `resources/` and currently include:

- `providers/`
- `prompts/`
- `tools/`
- `skills/`
- `plugins/`
- `mcp_servers/`
- `ides/`
- `mascots/`

Resource provenance matters. Preserve source metadata when loading resources.

## Session Model

Session state is append-only and should stay migration-friendly.

Current metadata includes:

- `id`
- `display_name`
- `cwd`
- `created_at_ms`
- `updated_at_ms`
- `parent_session_id`
- `slug`
- `tags`
- `note`

Do not replace this with opaque storage.

## TUI Direction

The TUI should keep moving toward Claude Code parity, but within current repo
constraints:

- Ratatui/Crossterm
- split modules for rendering, popup logic, markdown, and execution helpers
- transcript-first layout
- slash-command popup
- eventually tmux-aware parity testing

## Working Style

- Prefer incremental commits for small, coherent steps.
- Create any additional git worktrees under the repo-local `.worktree/`
  directory.
- Keep the workspace green with `cargo test --workspace`.
- When adding new features, wire tests in the same step where practical.
- When updating a component, write a new update spec in that component's
  `specs/<component>/` folder. Do not overwrite prior numbered specs; use the
  next unused two-digit Markdown file such as `03.md` when `00.md`, `01.md`,
  and `02.md` already exist.
- Component update specs must be concise, up-to-date, and exhaustive about the
  design, architecture, logic, contracts, and compatibility implications of the
  change.
- If there is a conflict between fidelity and maintainability, document the
  gap in code comments or commit messages rather than silently diverging.
