# Project Memory Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Bring Hermes-style persistent project memory into Puffer so configured projects in `~/.puffer` each get an independent `MEMORY.md` outside the repo tree.

**Architecture:** Keep Puffer's existing `CLAUDE.md` behavior, but add a user-level project registry in `~/.puffer/projects.toml`. At runtime, resolve the current `cwd` against that registry using longest-prefix matching, then inject the matched project's `MEMORY.md` from `~/.puffer/projects/<slug>/MEMORY.md` into the system prompt and expose the same path via `/memory project`.

**Tech Stack:** Rust, `puffer-config`, `puffer-core`, TOML config, file-backed Markdown memory files.

---

## Hermes design notes used

1. Hermes keeps persistent memory file-backed and injected at session start.
2. Memory is separated from ephemeral session state.
3. Stable on-disk files are preferred over hidden DB-only storage for user-editable memory.
4. Memory lookup is scoped before prompt assembly, not bolted on after generation.

## Applied plan

### Task 1: Add a project registry model in `puffer-config`
- Added `ProjectRegistry`, `ProjectEntry`, and `ResolvedProjectMemory`.
- Added `ConfigPaths::projects_file()` and `ConfigPaths::projects_memory_dir()`.
- Added `load_project_registry()` and `resolve_project_memory()`.
- Resolution uses longest matching configured root.

### Task 2: Store per-project memory under `~/.puffer/projects/`
- Derived per-project storage paths from configured project `name` plus a stable hash of `path`.
- Final file location is `~/.puffer/projects/<slug>/MEMORY.md`.

### Task 3: Inject resolved project memory into runtime system prompt
- Updated `puffer-core/runtime/system_prompt.rs` to prepend configured project `MEMORY.md`.
- Preserved fallback support for repo `CLAUDE.md` and user `~/.claude/CLAUDE.md` / `~/.puffer/CLAUDE.md`.
- Project memory and Claude fallback can coexist in the rendered system prompt.
- Renamed the injected heading to generic `# Project Context` for project memory, while legacy Claude fallback keeps its own CLAUDE-specific heading.

### Task 4: Point `/memory project` at the resolved project file
- Updated `/memory` helpers so project scope opens the resolved `MEMORY.md` when configured.
- Falls back to repo `CLAUDE.md` if no project registry entry matches.
- Ensures parent directories exist before opening the editor.

### Task 5: Verify with tests
- Added `puffer-config` tests for path resolution and longest-prefix selection.
- Added `puffer-core` tests for system prompt injection and `/memory project` path routing.

## Config format

```toml
[[projects]]
name = "puffer"
path = "/absolute/path/to/puffer"

[[projects]]
name = "daily-stock-analysis"
path = "/absolute/path/to/daily_stock_analysis"
```

## Expected behavior

- Opening Puffer inside a configured project path loads that project's dedicated `MEMORY.md`.
- `/memory path project` returns the resolved file under `~/.puffer/projects/.../MEMORY.md`.
- `/memory open project` creates parent directories if needed and opens that file.
- Projects not listed in `projects.toml` continue using repo-local `CLAUDE.md` behavior.
