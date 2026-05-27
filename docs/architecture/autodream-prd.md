# AutoDream PRD

## Summary

AutoDream is Puffer Code's background memory-consolidation mechanism. It treats
the recent conversation as short-term working memory, extracts durable project
knowledge, updates project memory through the existing Memory tool, and emits a
separate signal when a trace may be worth turning into a reusable GenSkill.

AutoDream is not a replacement for `/genskill`. It can suggest that a workflow
looks skill-worthy, but skill creation remains a separate user-reviewed action.

## Goals

- Consolidate durable project facts without requiring the user to explicitly say
  "remember this".
- Keep project memory useful by adding verified facts, replacing stale facts, and
  removing polluted entries.
- Preserve reusable workflow knowledge from noisy work traces while filtering
  transient task progress.
- Distinguish long-term memory updates from GenSkill suggestions.
- Run automatically in the background when enabled, while still supporting a
  manual command for inspection and one-off consolidation.

## Non-Goals

- AutoDream does not generate skill files directly.
- AutoDream does not edit memory files directly; it must use the Memory tool.
- AutoDream does not store raw transcripts, benchmark artifacts, secrets, run
  ids, or exact noisy paths as durable memory.
- AutoDream does not replace `/compact`; its output is durable memory, not a
  full conversation summary.
- AutoDream does not run when project memory is disabled or unavailable.

## User Problems

Users and agents often discover durable facts during normal work:

- repository constraints
- stable commands
- compatibility rules
- known test blockers
- provider or model-specific behavior
- workflows that prevent repeated exploration

Without AutoDream, these facts are easy to lose unless the user manually updates
memory. AutoDream reduces that burden by periodically reviewing recent work and
keeping only information likely to matter later.

## Current Product Behavior

### Manual Command

Puffer exposes `/autodream`.

- `/autodream` runs one synchronous consolidation pass for the current project.
- `/autodream status` reports whether AutoDream is enabled, the configured
  interval, whether GenSkill suggestions are enabled, turns until the next
  automatic pass, whether project memory is available, and the latest persisted
  background run status when available.

When `/autodream` completes, it reports:

- number of tool calls made by the consolidation pass
- whether the trace was marked as GenSkill-worthy
- the assistant's concise AutoDream result
- an optional suggestion to run `/genskill` when suggestions are enabled and the
  trace is marked skill-worthy

### Visible Background Status

Automatic AutoDream writes `autodream/run_status.json` next to the scheduler
state so users can inspect the latest background pass with `/autodream status`.
The status can be `running`, `completed`, `failed`, or `skipped` when another
process already owns the AutoDream lock.

Completed status includes the number of recent sessions reviewed, Memory/tool
calls made by the side turn, the GenSkill marker, and a short sanitized summary.
Failed status includes a short sanitized error. This file is diagnostic state
only; it is not durable project memory and does not affect scheduler gates.

### Structured Memory Changes

Automatic AutoDream records a sanitized summary of each successful or failed
`Memory` tool call in the run status. Each change includes:

- action: `add`, `replace`, or `remove`
- short new content, when present
- short old text, when present
- Memory tool success
- Memory tool message or error

This summary exists so users can understand what changed without opening raw
transcripts or scanning provider/tool logs. It is deliberately short and
redacted; `MEMORY.md` remains the source of truth.

### GenSkill Suggestion Queue

When a background pass returns a positive GenSkill marker, AutoDream stores a
reviewable pending suggestion in the run status. `/autodream suggestions`
renders the latest pending suggestion with an id, status, timestamp, memory
change count, and rationale.

AutoDream still does not create skills directly. The queue is an inspection and
handoff mechanism for a later user-reviewed `/genskill` pass.

### Automatic Background Review

AutoDream can run automatically after completed assistant turns.

Current behavior:

- Background review is gated by `memory.autodream_enabled`.
- It requires project memory to be enabled and available.
- It uses `memory.autodream_interval` to decide how many completed turns to wait
  between reviews.
- When the interval is reached, the counter resets and AutoDream runs in a
  background thread.

The default interval is documented as 16 completed assistant turns in the core
update spec.

### Restricted Tool Surface

AutoDream runs as a restricted side turn.

Allowed capabilities:

- `Skill`, restricted to the `project-memory` skill
- `Read`, restricted to the exact active project `MEMORY.md`
- `Memory`, for add/replace/remove memory edits

This is intentionally narrower than a normal agent turn. AutoDream should only
load memory, inspect the active memory file, and patch memory through the
approved tool.

### Memory Consolidation Workflow

The current AutoDream prompt uses four phases:

1. Orient
   - Load project memory first.
   - Identify existing durable facts, stale conflicts, workflow bullets, and
     pollution.
   - Treat memory as an explicit patch with keep/add/replace/remove decisions.

2. Gather recent signal
   - Review the current transcript for durable candidates.
   - Prefer facts supported by user corrections, tool output, verification,
     stable repo rules, compatibility constraints, repeated workflows, or
     accepted plans.
   - Extract useful signal even from failed, noisy, external, or benchmark-style
     traces when they contain a reusable method.

3. Consolidate memory
   - Add verified durable facts.
   - Replace stale facts with exact `old_text` copied from loaded memory.
   - Remove pollution.
   - Store reusable workflows as short generalized bullets with 4-6 stable
     actions and a verification condition when available.
   - Keep command-shape details only when they are durable, such as provider,
     model, runner, corpus, and concurrency knobs.

4. Prune and decide GenSkill
   - Exclude transient task progress, shell typos, temporary run ids, abandoned
     hypotheses, network failures, secrets, and local-only artifact paths.
   - Decide GenSkill only after memory edits.
   - Emit exactly one marker: `AUTODREAM_GENSKILL: yes` or
     `AUTODREAM_GENSKILL: no`.

### Conflict Replacement

AutoDream has explicit replacement behavior:

- It must read the existing memory first.
- A replacement must use exact stale entry text as `old_text`.
- It must not keep both sides of a conflict.
- If replace fails because the old text did not match, it should retry with the
  exact loaded stale entry.
- If there is no stale entry, it should add the new durable fact instead.

### Noise Filtering

AutoDream is designed to reject:

- one-off local paths
- transient network/provider failures
- shell typos
- unverified guesses
- abandoned hypotheses
- raw failed probe samples
- exact benchmark run ids
- worker names
- task progress that will not matter later
- explicit "do not remember" or "do not skill" instructions

When it keeps a lesson from noisy work, it should abstract the failure class
rather than copying the exact noisy string.

### External Trace Handling

AutoDream has dedicated behavior for noisy external or benchmark-style traces.

It may write generalized workflow memory when a trace shows a reusable method,
even if the task failed or was unsolved. For these traces, AutoDream should:

- generalize away dataset names, task ids, model names, exact paths, exact
  commands, flags, secrets, payloads, artifact names, and incorrect step ids
- preserve the workflow shape
- name the workflow with the correct domain, such as software engineering,
  file operations, security, machine learning, scientific computing, games, or
  system administration
- avoid treating a clean/control trace as GenSkill-worthy by default

### GenSkill Suggestion Signal

AutoDream produces a GenSkill suggestion only through the explicit final marker.

Current behavior:

- `AUTODREAM_GENSKILL: yes` means AutoDream believes a reusable workflow may
  deserve a future `/genskill` pass.
- `AUTODREAM_GENSKILL: no` means it should not include positive skill language.
- A workflow memory entry alone is not enough for a skill suggestion.
- Clean/control-style external traces default to no suggestion.
- Non-clean, high-noise, unsolved, long-tail, or incorrect-step traces can be
  suggested when AutoDream wrote a durable workflow with at least four reusable
  actions and a verification signal.

Skill creation remains manual and reviewable.

## Configuration

AutoDream is controlled through memory configuration:

- `memory.autodream_enabled`
  Enables automatic background AutoDream review.

- `memory.autodream_interval`
  Number of completed assistant turns between automatic reviews.

- `memory.autodream_min_hours`
  Minimum wall-clock hours since the last successful automatic consolidation.
  Defaults to 24.

- `memory.autodream_min_sessions`
  Minimum number of other updated sessions since the last successful automatic
  consolidation. Defaults to 5.

- `memory.autodream_genskill_suggestions`
  Controls whether command output includes the optional `/genskill` suggestion
  text when the marker is positive.

Project memory must also be enabled and configured.

## Current Implementation

Core implementation:

- `crates/puffer-core/autodream.rs`
  - prompt and side-turn orchestration
  - manual synchronous review
  - background thread spawning
  - interval counter
  - time/session gates for automatic review
  - bounded recent-session context collection for automatic review
  - consolidation lock, persisted scheduler state, and persisted run status
  - structured memory-change summaries
  - reviewable GenSkill suggestion record
  - status rendering
  - GenSkill marker parsing

- `crates/puffer-core/command_helpers/autodream.rs`
  - `/autodream` command handler
  - `/autodream status`
  - `/autodream suggestions`
  - command response formatting

- `resources/skills/autodream/SKILL.md`
  - user-invocable AutoDream skill mirror of the consolidation instructions

- `specs/puffer-core/199.md`
  - initial AutoDream memory consolidation design

- `specs/puffer-core/200.md`
  - external trace and GenSkill boundary refinement

- `specs/puffer-core/201.md`
  - original-mechanism-style automatic scheduling gates

## UX Principles

- AutoDream should feel automatic, conservative, and quiet.
- Manual `/autodream` exists for transparency and debugging, not as the primary
  user workflow.
- Memory edits should be short, durable, and project-specific.
- The user should never need to inspect noisy transcript details to understand
  why a durable memory entry exists.
- A GenSkill suggestion should be phrased as a recommendation, not an automatic
  skill generation event.

## Recent Session Context

Automatic AutoDream runs collect bounded context from recent sessions in the
same project. The collector excludes the current session, excludes sessions from
other working directories, keeps at most five sessions, and summarizes at most
sixteen recent structured transcript events from each session.

The context pack is deliberately lossy. It truncates snippets, redacts
secret-like words, and skips state snapshots and transcript rewrite events. The
pack is appended to the AutoDream prompt as supporting evidence only; durable
memory must still be written through the restricted Memory tool.

## Original-Mechanism Alignment

The public Claude Code mirror models AutoDream as a quiet background memory
consolidation task rather than a frequent per-turn memory patcher. Puffer keeps
the narrower Rust-native tool surface, but aligns the automatic behavior in
three ways:

- Automatic runs are gated by both elapsed time and accumulated session signal.
- A persisted AutoDream state file records the last successful consolidation
  and the last session scan, so restarts do not reset the scheduler.
- A cross-process lock prevents overlapping background consolidations.
- A persisted run-status file makes the latest background pass visible without
  requiring the side thread to mutate live TUI state.
- Structured memory-change summaries show what AutoDream edited without storing
  raw transcript data.
- Positive GenSkill markers become pending suggestions instead of automatic
  skill generation.

Manual `/autodream` intentionally bypasses these gates. It remains the
inspection/debugging path and should not update the automatic scheduler unless
the manual command is explicitly changed to do so later.

## Risks

- Overwriting correct memory with an insufficiently verified replacement.
- Saving transient details that make future turns worse.
- Writing generic workflow memory that consumes context without improving future
  work.
- Suggesting GenSkill too often and creating skill-review fatigue.
- Running background reviews too frequently and increasing provider cost or
  latency.

## Open Product Questions

- Should AutoDream expose a preview mode before applying memory edits?
- Should background AutoDream notify the user only when memory changed?
- Should GenSkill suggestions accumulate beyond the latest run status into a
  longer review queue?
- Should external trace handling be hidden behind a separate mode or metadata
  flag?
- Should the automatic interval adapt to trace length, memory churn, or session
  activity?

## Next Product Work

- Add an optional dry-run path that reports proposed memory edits without
  applying them.
- Add review actions for GenSkill suggestions, such as accept, dismiss, or run
  `/genskill` with the queued rationale.
- Persist a longer suggestion history instead of only the latest run status.
- Support task-specific memory injection for downstream work, using AutoDream
  output rather than a generic memory prefix.
