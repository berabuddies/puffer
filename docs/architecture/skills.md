# Skills: Authoring, Installing, Updating, Removing

This document explains how Puffer skills are loaded, how to ship one as a
built-in, how to install/update one per-user or per-workspace, and the
gotchas that decide whether the model actually *invokes* a skill.

## What a skill is

A skill is a directory containing a `SKILL.md` file:

```
<name>/
  SKILL.md            # YAML frontmatter + prompt body
  references/api.md   # optional sibling files the body can point to
```

`SKILL.md` frontmatter (all optional except that `name` defaults to the
directory name):

| Field | Meaning |
|---|---|
| `name` | Skill name (defaults to dir name); normalized to slash-safe form |
| `description` | What it does — the model reads this to decide whether to call it |
| `user-invocable` | If true (default), also exposed as a `/slash` command |
| `disable-model-invocation` | If true, the model can't auto-call it (manual `/` only) |
| `allowed-tools` | Restrict which tools the skill may use |
| `argument-hint` / `arguments` | Slash-command argument hints / named args |
| `model` / `effort` / `context` | Per-skill overrides |

Unknown frontmatter keys (e.g. `metadata:`) are ignored, not an error.
The body is the prompt injected when the skill runs; it supports
`$ARGUMENTS`, `$1`, `$name`, `${CLAUDE_SKILL_DIR}`, `${CLAUDE_SESSION_ID}`.

## Load layers and precedence

Skills are discovered by scanning `<root>/skills/<name>/SKILL.md` under
three roots. Precedence is **Workspace > User > Builtin** (a same-named
skill in a higher layer overrides a lower one):

| Layer | Path | Scope |
|---|---|---|
| **Builtin** | `<repo>/resources/skills/` | Ships with the product |
| **User** | `~/.puffer/resources/skills/` (`PUFFER_HOME` overrides `~`) | Your machine, all workspaces |
| **Workspace** | `<cwd>/.puffer/resources/skills/` | One workspace only |

**Loading is purely directory-scan based — no registration is required to
load a skill.** (See "Built-in" below for why a built-in *also* gets
registered in a plugin manifest.)

Builtin has two sources that merge: the copy baked into the binary at
compile time via `include_dir!("../../resources")`, plus a filesystem
directory pointed at by the `PUFFER_BUILTIN_RESOURCES_DIR` env var (used
to read built-ins from disk instead of the embedded copy).

## Installing a skill

### A) As a built-in (ships with the repo / product)

1. Put the directory at `resources/skills/<name>/`.
2. Register it in `resources/plugins/puffer-builtins.yaml` under `skills:`.
   This is **plugin attribution + `/plugin` listing only — NOT required
   for loading** (directory scan already loads it; the manifest only lists
   a subset). Register it anyway so the builtins plugin manifest stays
   accurate.

How it reaches each runtime:

- **momo (dev)** — `daemon_launcher.rs` walks up from the puffer binary
  to find `<repo>/resources` and passes it as `PUFFER_BUILTIN_RESOURCES_DIR`
  to the daemon. The daemon reads built-ins from that **disk** directory
  and fresh-loads resources every turn, so a new/edited skill is effective
  on the **next message — no rebuild, no restart**.
- **CLI `puffer` (`~/.cargo/bin/puffer`)** — uses the **embedded** copy
  frozen at compile time. Run `cargo install -p puffer-cli` (or rebuild)
  to re-embed the updated `resources/`.
- **release bundle** — the daemon looks for a `resources/` dir *next to*
  the puffer binary; packaging must ship `resources/` alongside `puffer`.

### B) Per-user (your machine, all workspaces)

Drop the directory at `~/.puffer/resources/skills/<name>/`. Auto-discovered
and hot-reloaded — no registration, no rebuild. Good for trying a skill
without touching the repo.

### C) Per-workspace

Drop the directory at `<cwd>/.puffer/resources/skills/<name>/`. Only that
workspace sees it.

## Updating a skill

Edit the files in place. The daemon fresh-loads resources every turn, so
changes take effect on the next message — **no momo restart needed**.

Exception: the embedded copy inside an already-built CLI binary is frozen
at compile time — rebuild/reinstall to update it.

## Third-party skills that need secrets (env / token)

A skill that calls an external API (e.g. `book-by-phone` →
`WORLDROUTER_API_KEY`) reads the secret from an environment
variable. The puffer process **inherits the parent's environment**:

- **momo dev**: `export THE_TOKEN=... ` in the terminal **before**
  `npm run tauri dev`. The daemon captures its env at spawn time, so to
  change a token you must **restart momo**.
- **packaged app launched from Finder/launchd** does NOT inherit shell
  env → use `launchctl setenv THE_TOKEN ...` (then relaunch) or another
  injection path.
- Never commit the token. Only reference the env var name in `SKILL.md`.

## Invocation is a soft prompt — not a guarantee

Loading and exposing a skill does **not** guarantee the model calls it.
The skill list is injected into the system prompt ("Available
model-invocable skills…") and the `Skill` tool is offered, but the model
still decides.

In particular, worldrouter (OpenAI-Responses) injects a **provider-native
`web_search`** tool (toggle `PUFFER_OPENAI_NATIVE_WEB_SEARCH`, default on;
it won't appear in puffer's own tool list because it's server-side). For a
"find / look up X" request the model takes the one-shot `web_search` and
skips the skill.

To get a skill invoked reliably:

- **Phrase the request around what the alternatives can't do.** For a
  phone-booking skill, "find a sushi place" loses to `web_search`, but
  "**call and book** / cancel / ask the merchant" has no shortcut, so the
  skill wins.
- Or invoke it explicitly via its `/slash` command (note: momo's frontend
  does not yet forward `/` commands to the daemon).

## Verifying

- `puffer doctor` → prints `skills=<N>` (count of loaded skills).
- To mimic the momo daemon's view of built-ins:
  `PUFFER_BUILTIN_RESOURCES_DIR=<repo>/resources puffer doctor`.
- `puffer non-interactive --run-command "/debug"` → dumps the full system
  prompt + tool definitions (local only — no network, no telegram
  subscriber; the process may not self-exit, kill it after). Grep your
  skill name in the "Available model-invocable skills" block and `Skill`
  in the `TOOLS` block.

## Removing a skill

1. Delete its directory: `resources/skills/<name>/` (built-in) or the
   relevant layer's `…/resources/skills/<name>/`.
2. Remove its entry from `resources/plugins/puffer-builtins.yaml`
   `skills:` if it was registered there.

Two gotchas:

- **Removing only the yaml entry is NOT enough** — directory scanning still
  loads the skill. You must delete the directory.
- **For a built-in, deleting the on-disk directory is still not enough for
  runtimes that use the embedded copy.** Built-in resources are the
  compile-time-embedded set *merged* with the on-disk dir (union +
  same-name override), so a deletion on disk does not drop the embedded
  entry — `doctor` will still count it. You must **rebuild** to refresh the
  embedded snapshot: `cargo build -p puffer-cli` (then restart momo so its
  daemon picks up the new binary), or `cargo install -p puffer-cli` for the
  global CLI. (Note the asymmetry: *adding* a built-in works without a
  rebuild because union picks up the new on-disk dir; *removing* one does
  not.)
