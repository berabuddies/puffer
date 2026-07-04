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

`Cargo.toml` is the source of truth for workspace membership. Keep broad crate
categories in mind instead of relying on a copied list:

- CLI/daemon/UI: `puffer-cli`, `puffer-tui`
- Core runtime: `puffer-core`, `puffer-tools`, `puffer-resources`,
  `puffer-session-store`, `puffer-workflow`, `puffer-config`
- Provider/runner layers: `puffer-transport-anthropic`,
  `puffer-provider-openai`, `puffer-provider-registry`, `puffer-runner-*`,
  `puffer-tool-runner`
- Connector/subscriber stack: `puffer-connector-*`, `puffer-subscriber-*`,
  `puffer-subscriptions`, `puffer-slack`
- Support/security/media: `puffer-test-support`, `puffer-observability`,
  `puffer-logging`, `puffer-media`, `puffer-secrets`,
  `puffer-skill-evolution`, `puffer-mcp-oauth`

See `README.md` and `docs/README.md` for the current repo map and docs index.

## Repo Guardrails

- Use ASCII unless there is an existing reason not to.
- Keep modules small and purpose-specific. Large files exist today, so use
  `scripts/report-large-files.sh` as a risk report and call out touched large
  files in PRs.
- Prefer stable, typed Rust APIs over stringly typed plumbing, especially across
  daemon RPC, resource manifests, permissions, and connector action contracts.
- Keep slash-command docs generated from
  `crates/puffer-core/command/registry.rs`; run
  `scripts/check-slash-commands.sh` after changing the command registry.
- For docs changes, run `scripts/check-doc-links.sh`.

## Supported Slash Commands

The built-in slash-command surface is generated from
`crates/puffer-core/command/registry.rs` and documented in
`docs/reference/slash-commands.md`. Do not maintain command lists by hand in
multiple files.

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

## Monitor / TaskCreate Guardrails

The monitor/triage subsystem (inbound connector events → monitor tasks → human-approved
replies/actions) enforces behavioral rules at runtime. These are correctness/safety boundaries, not
style. Full design: `docs/architecture/monitor-pipeline.md`.

- **Outward actions are human-gated.** A triage/action turn only DRAFTS (`MonitorActionDraft`, or
  legacy `MonitorReplyDraft`) — it writes a `pending_action`/`pending_reply` and stops. A human
  approves the draft in the desktop UI; only then does the daemon send the Telegram reply / create
  the Gmail draft / RSVP the Calendar invite, via the `task_monitor_action_execute` RPC. Never push
  an outward effect from an agent turn (no connector send tool, no `MonitorReplySend`, no
  `TaskUpdate status:completed` to force a send). `task_monitor_complete` refuses human-gated tasks, with one exception: `completed_via: "no_reply_needed"` records the human "reviewed, no reply needed" decision (with an audit entry and optional `reason`) instead of a send receipt (#676).
- **`TaskCreate` skip is success, not failure.** A `{success:true, skipped:true, reason}` result
  means the server gate intentionally suppressed the task (already handled in Telegram, duplicate,
  untrusted source). Do not retry by mutating source metadata.
- **Source provenance is server-owned.** Create a monitor task only from the *current* workflow /
  digest trigger envelope. Do not create tasks from `conversation_context` history, and do not write
  `monitor_connection`, `monitor_connector`, `monitor_envelope_id`, `chat_id`, `source_context`,
  `monitor`, `pending_action`, or other source/delivery metadata — the daemon stamps these from the
  selected current envelope. `TaskUpdate` must not mutate them.
- **Conversation context is for disambiguation only.** Bounded recent history attached to triage is
  to interpret short/referential messages; it is never a task source and never overrides current
  source values.
- **Same chat/contact ≠ duplicate.** A new question, topic change, or separate request from the same
  sender is a new task even when another task from that sender is still pending.
- **Trace/diagnostics are metadata-only** (sender/chat ids, stage ids, decisions, a short text
  preview) — never full message bodies. This is a privacy boundary; do not widen it.

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
- Keep the workspace green against the same gates CI enforces:
  `cargo test --workspace` (or `cargo nextest run --workspace`),
  `cargo fmt --all --check`, and
  `cargo clippy --workspace --all-targets -- -D clippy::correctness -D clippy::suspicious`.
  `scripts/ci-gates.sh` runs every CI gate locally in three parallel lanes
  (root workspace, src-tauri, desktop frontend); `--quick` skips the test
  suites for a fast compile+lint pass.
- After resolving merge/rebase conflicts, run
  `cargo check --workspace --all-targets` BEFORE committing. Git only detects
  textual conflicts; a rename or signature change merged from the other side
  breaks code (including `#[cfg(test)]` code, which plain `cargo check`
  skips) with no conflict marker. The lefthook hooks intentionally skip
  merge/rebase commits, so nothing else catches this before push.
- Hooks can reject a commit after printing unrelated output; confirm HEAD
  actually advanced (`git commit ... && git log --oneline -1`) before
  building on top of it or pushing.
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

## Agent-driven UI testing

Route UI testing work by scenario:

| Scenario | Tool | Deliverable |
|---|---|---|
| New-feature acceptance / flow exploration | agent-browser against the isolated `agent:app` instance | Exploration report + hardened `tests/*-ui.spec.ts` |
| UI/UX visual review | agent-browser screenshots reviewed state by state; component-level via Storybook (`npm run storybook`, :6006) | Issue list + `toHaveScreenshot()` baselines in `tests/visual/*.spec.ts` |
| Bug deep-dive (network/console/daemon protocol) | playwright-mcp (configured in `.mcp.json`) | Root cause + minimal reproduction spec |
| Regression protection | `npm run test:desktop-ui` (CI, no agent) | Pass/fail |

To launch the isolated app (from `apps/puffer-desktop/`):

1. Build the daemon once: `cargo build -p puffer-cli` (the script fails fast if
   `target/debug/puffer` is missing).
2. Run `npm run agent:app`. Append `-- --provider real` to hit a real gateway;
   that mode requires `RELAYDANCE_BASE_URL` and `RELAYDANCE_API_KEY`. The
   default mock provider answers every prompt with a canned reply.
3. Parse stdout: `AGENT_APP_URL=` is the ready-to-open app URL (any browser
   tool works); `AGENT_APP_ROOT=` is the instance's temp directory.
4. Stop with Ctrl-C/SIGTERM. The daemon, temp directory, and any self-started
   Vite are cleaned up automatically; a Vite already running on 1420 is reused
   and left untouched.

Conventions:

- Explore only through the isolated `agent:app` instance — it never touches
  the user's dev data, and sharing the Vite on 1420 is safe.
- Harden findings with role-based selectors (`getByRole` first), styled after
  the existing `tests/*-ui.spec.ts`.
- Count a finding as hardened only once `npm run test:desktop-ui` (functional)
  or `npm run test:desktop-visual` (visual) passes.
- Keep `tests/visual/` out of GitHub CI and run it on macOS only: screenshot
  baselines are macOS-generated and diverge on the ubuntu CI runners.

Spec-rot rules (distilled from the 2026-07 sweep that revived 86 dead specs):

- No pixel or computed-style assertions (heights, radii, rgb colors) in
  functional specs — they die on every restyle. Assert roles, labels, and
  structural attributes; visual intent belongs in `tests/visual/`.
- Match cards/rows by exact accessible heading, not `hasText` substrings: the
  Custom provider card's "OpenAI-compatible" copy silently redirected six
  specs to the wrong dialog for weeks.
- No DOM-identity probes (tagging nodes and asserting the tag survives) —
  row keying/grouping is an implementation detail that legitimately changes.
  Assert user-visible invariants instead (one row per message, no duplicates).
- Any `__TAURI_INTERNALS__` stub must answer `ensure_local_daemon` with the
  FakeDaemon handshake — Tauri-mode startup resolves its daemon through that
  invoke, and a throwing stub hangs every spec at the sidebar.
- Mock daemons should reject unknown RPC methods loudly (the
  StrictSubscriptionDaemon pattern), never answer with silent empties — a new
  daemon method then fails the suite the day it ships instead of rotting.
- Never pipe a test run through `tail`/`head` without `set -o pipefail`: the
  pipeline's exit code masked 77 failures as a green run once already.

When the desktop-ui CI job goes red (root-cause it — never add retries,
timeouts, or `.skip` to get back to green):

1. Download the `playwright-test-results` artifact; each failure's
   `error-context.md` carries the error, call log, and a page snapshot, and
   CI-only failures also include a trace (`npx playwright show-trace <zip>`).
2. Reproduce locally by title: `npx playwright test -g "<test name>"`.
3. Decide which side broke: if the UI change is intentional, migrate the
   spec's assertions to the new contract (and say so in the commit); if not,
   the spec just caught a regression — fix the app.
