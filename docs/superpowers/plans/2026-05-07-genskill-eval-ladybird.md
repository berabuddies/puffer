# Ladybird PR Replay Eval — Implementation Plan (Plan 3 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Ladybird PR replay benchmark for `/genskill` — a self-contained eval harness that runs three replay arms (no-skill, direct, gepa) across 10 hand-curated Ladybird PRs, computes four metrics (pass rate, token efficiency, duplicate work, info retention), and produces a comparison report.

**Architecture:** New benchmark crate `puffer-genskill-eval` at `benchmark/genskill/ladybird/src/`. Outside the main puffer runtime — consumes `/genskill` outputs as a black box. Docker sandbox per replay. PR corpus is committed data; replays produce gitignored artifacts that are published per benchmark run.

**Tech Stack:** Rust 1.95.0, `tokio` async, `bollard` for Docker, `serde`/`serde_json`, `git2` or shell-out to git, `anyhow` for errors. Python is NOT used — all eval logic is Rust.

**Spec reference:** `docs/superpowers/specs/2026-05-07-genskill-eval-ladybird.md`

**Branch:** assume execution happens on the `feat/genskill` branch (or a successor branch after Plan 1 lands). The eval harness must keep working regardless of Plan 1's merge state — it depends only on the existence of a built `puffer` binary that supports `/genskill`.

**Repo constraints (from `AGENTS.md`):**
- No source file > 1000 lines
- Every public Rust function has a `///` doc comment
- ASCII unless existing reason otherwise

**Pre-existing master state:** `cargo build --workspace` fails on `matrix-sdk` query depth overflow (in `puffer-connector-matrix`). Run scoped tests/builds for our new crate: `cargo build -p puffer-genskill-eval`, `cargo test -p puffer-genskill-eval`. Do not attempt to fix `matrix-sdk`.

**Out of scope for this plan:**
- Curator (Plan 2)
- Modifications to `/genskill` itself or `puffer-skill-evolution` (Plan 1)
- Cross-codebase generalization tests (e.g., SQLite corpus) — listed in spec §12 future work
- Continuous-integration / nightly automation — manual `cargo run --bin puffer-genskill-eval` for now

---

## File Structure

**New benchmark tree:**
```
benchmark/genskill/ladybird/
  Cargo.toml                     # bin crate "puffer-genskill-eval"
  Dockerfile.ladybird-eval       # reproducible sandbox image
  README.md                      # quick-start
  pr_corpus/                     # 10 PRs, all data committed
    pr-NNNNN/
      meta.json
      pre.diff                   # base_commit relative to ladybird master
      reference_fix.patch        # the merged fix diff
      tests/                     # test files added/modified by the PR
      expert_run.md              # recorded expert agent transcript
      skills/
        gepa/SKILL.md            # produced by /genskill on expert_run.md
        direct/SKILL.md          # produced by direct prompt on expert_run.md
  scripts/
    select_prs.sh                # gh CLI helper to find candidate PRs
    record_expert_run.sh         # wraps the expert-run recording flow
    generate_skills.sh           # invokes /genskill and direct prompt for one PR
    run_replay.sh                # wraps `cargo run -- replay ...`
    aggregate.sh                 # wraps `cargo run -- aggregate ...`
  src/
    main.rs                      # CLI: corpus / generate / replay / aggregate / report
    cli.rs                       # arg parsing
    pr_corpus.rs                 # load/validate corpus on disk
    sandbox.rs                   # Docker spawn + lifecycle
    replay.rs                    # one (pr, arm) replay execution
    metrics.rs                   # info_retention, tokens, duplicates
    report.rs                    # markdown rendering
    fs_helpers.rs                # safe path joins, atomic writes
  reports/
    .gitignore                   # ignore everything except published/
    published/
      .gitkeep
```

**Workspace integration:**
- Add `benchmark/genskill/ladybird` to root `Cargo.toml`'s `members` (so `cargo build -p puffer-genskill-eval` works).
- The bin crate has zero dependency on `puffer-core`, `puffer-skill-evolution`, etc. It calls the `puffer` binary as a subprocess.

---

## Phase A — PR Corpus Curation

This phase produces the frozen dataset. It's mostly tool-driven research with manual confirmation.

## Task 1: Bootstrap the eval crate skeleton

**Files:**
- Create: `benchmark/genskill/ladybird/Cargo.toml`
- Create: `benchmark/genskill/ladybird/src/main.rs`
- Create: `benchmark/genskill/ladybird/README.md`
- Modify: `Cargo.toml` (workspace root, add to `members`)

- [ ] **Step 1: Create the crate manifest**

`benchmark/genskill/ladybird/Cargo.toml`:

```toml
[package]
name = "puffer-genskill-eval"
version.workspace = true
edition.workspace = true
license.workspace = true

[[bin]]
name = "puffer-genskill-eval"
path = "src/main.rs"

[dependencies]
anyhow.workspace = true
clap = { version = "4", features = ["derive"] }
serde.workspace = true
serde_json.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "process", "fs", "time"] }
tracing.workspace = true
tracing-subscriber = "0.3"
walkdir = "2"
similar = "2"
```

- [ ] **Step 2: Create main.rs with CLI scaffolding**

`benchmark/genskill/ladybird/src/main.rs`:

```rust
//! Ladybird PR replay benchmark for /genskill.
//!
//! See spec at docs/superpowers/specs/2026-05-07-genskill-eval-ladybird.md

#![deny(missing_docs)]

use anyhow::Result;
use clap::{Parser, Subcommand};

/// CLI entry point.
#[derive(Parser)]
#[command(name = "puffer-genskill-eval", about = "Ladybird PR replay benchmark for /genskill")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

/// Top-level subcommands.
#[derive(Subcommand)]
enum Cmd {
    /// Validate the on-disk corpus structure.
    Validate,
    /// Run a single replay: one PR, one arm.
    Replay {
        /// PR id (matches pr_corpus/<id>/).
        pr: String,
        /// Replay arm: no-skill | direct | gepa.
        arm: String,
    },
    /// Aggregate completed replays into a single report.
    Aggregate {
        /// Run date directory under reports/ (e.g., 2026-05-20).
        run_date: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Validate => {
            println!("Corpus validation not yet implemented");
            Ok(())
        }
        Cmd::Replay { pr, arm } => {
            println!("Replay {pr} {arm} not yet implemented");
            Ok(())
        }
        Cmd::Aggregate { run_date } => {
            println!("Aggregate {run_date} not yet implemented");
            Ok(())
        }
    }
}
```

- [ ] **Step 3: Create README**

`benchmark/genskill/ladybird/README.md`:

```markdown
# /genskill Ladybird PR Replay Benchmark

This is the implementation of Plan 3 from
`docs/superpowers/specs/2026-05-07-genskill-design.md`. The full design
is in `docs/superpowers/specs/2026-05-07-genskill-eval-ladybird.md`.

## Quick start

```
# Validate corpus on disk
cargo run -p puffer-genskill-eval -- validate

# Run a single replay
cargo run -p puffer-genskill-eval -- replay pr-12345 gepa

# Aggregate finished replays into a report
cargo run -p puffer-genskill-eval -- aggregate 2026-05-20
```

Cost per full run (30 replays): ~$35-70 with Sonnet.
```

- [ ] **Step 4: Add to workspace**

Add `"benchmark/genskill/ladybird"` to the `members` array in the root `Cargo.toml`, alphabetically grouped with the existing crate paths.

- [ ] **Step 5: Verify it builds**

Run: `source ~/.cargo/env && cargo build -p puffer-genskill-eval`
Expected: builds clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml benchmark/genskill/ladybird/
git commit -m "feat(eval): bootstrap puffer-genskill-eval crate

Bin crate scaffold for Plan 3's Ladybird PR replay benchmark. CLI
defines validate/replay/aggregate subcommands with stubs; subsequent
tasks fill them in. No dependency on puffer-core or puffer-skill-
evolution -- this crate is a pure black-box eval harness.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 2: Write `select_prs.sh` PR-discovery helper

**Files:**
- Create: `benchmark/genskill/ladybird/scripts/select_prs.sh`

- [ ] **Step 1: Create the discovery script**

`benchmark/genskill/ladybird/scripts/select_prs.sh`:

```bash
#!/usr/bin/env bash
# Lists candidate Ladybird PRs that match Plan 3's selection criteria.
#
# Output: one PR number per line. Pipe into `tee` and review by hand;
# this script does not commit anything to the corpus.
#
# Criteria enforced by this script (subset of spec §3.1):
#   - Merged into LadybirdBrowser/ladybird master
#   - At least 6 months old (>= 180 days since merge)
#   - Modifies at most 5 source files (test files don't count toward this)
#   - At least one new/modified file under Tests/
#
# Manual filters that this script CANNOT enforce (review by hand):
#   - Clear root cause described in the PR/issue
#   - Builds in standard Ladybird Docker image (no special hardware deps)
#   - Domain distribution per spec §3.2

set -euo pipefail

REPO="LadybirdBrowser/ladybird"
SINCE_DAYS="${SINCE_DAYS:-180}"
LIMIT="${LIMIT:-200}"

cutoff=$(date -u -v -"${SINCE_DAYS}d" +"%Y-%m-%dT%H:%M:%SZ" 2>/dev/null \
       || date -u -d "${SINCE_DAYS} days ago" +"%Y-%m-%dT%H:%M:%SZ")

# Pull recent merged PRs in JSON. Field selection keeps the call cheap.
gh pr list \
  --repo "$REPO" \
  --state merged \
  --limit "$LIMIT" \
  --search "merged:<${cutoff}" \
  --json number,title,mergedAt,files,url \
  | jq -r '.[]
      | select(
          (.files | map(select(.path | test("^Tests/"))) | length) >= 1
          and
          (.files | map(select(.path | test("^Tests/") | not)) | length) <= 5
        )
      | "\(.number)\t\(.title)\t\(.url)"'
```

- [ ] **Step 2: Make it executable**

Run: `chmod +x benchmark/genskill/ladybird/scripts/select_prs.sh`

- [ ] **Step 3: Smoke test**

Run: `bash benchmark/genskill/ladybird/scripts/select_prs.sh | head -5`
Expected: a tab-separated list of PR number / title / URL. (May be empty if `gh` is not authenticated; that's fine for the commit.)

- [ ] **Step 4: Commit**

```bash
git add benchmark/genskill/ladybird/scripts/select_prs.sh
git commit -m "feat(eval): add PR discovery script

select_prs.sh wraps gh CLI to list merged Ladybird PRs that meet the
mechanical selection criteria from spec §3.1 (>=180 days old, <=5
non-test files modified, >=1 test file touched). Manual review still
required for criteria the script can't enforce: root-cause clarity,
build-in-standard-Docker, domain distribution.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 3: Hand-curate 10 PRs and commit metadata

This is a manual research step. Pick 10 PRs from the candidates produced by Task 2, satisfying the §3.1 criteria. Distribute across the 6 areas listed in spec §3.2: 3 LibWeb, 2 LibJS, 1 LibHTTP, 2 LibCore, 1 LibCrypto, 1 build/tooling.

**Files:**
- Create: `benchmark/genskill/ladybird/pr_corpus/pr-<N>/meta.json` × 10
- Create: `benchmark/genskill/ladybird/pr_corpus/pr-<N>/pre.diff` × 10
- Create: `benchmark/genskill/ladybird/pr_corpus/pr-<N>/reference_fix.patch` × 10
- Create: `benchmark/genskill/ladybird/pr_corpus/pr-<N>/tests/<test files>` × 10

- [ ] **Step 1: Run discovery and shortlist**

Run: `bash benchmark/genskill/ladybird/scripts/select_prs.sh > /tmp/ladybird-candidates.tsv`

Open `/tmp/ladybird-candidates.tsv` and pick ~25 candidates spread across the 6 areas. For each, briefly read the PR description on github.com to confirm:
- Clear root cause
- No external dependencies (special hardware, large datasets)
- Test runs in standard Docker

Narrow to 10 PRs.

- [ ] **Step 2: For each chosen PR, gather artifacts**

For PR `<N>` with merge commit `<merge_sha>` and pre-fix base commit `<base_sha>`:

```bash
PR=<N>; BASE_SHA=<base_sha>; MERGE_SHA=<merge_sha>; AREA=<libweb-css|libjs|...>; TASK_PROMPT=<one paragraph>

DIR="benchmark/genskill/ladybird/pr_corpus/pr-$PR"
mkdir -p "$DIR/tests"

# Get the merged fix as a patch
gh api "repos/LadybirdBrowser/ladybird/pulls/$PR" -q '.diff_url' \
  | xargs curl -sL > "$DIR/reference_fix.patch"

# Compute pre.diff (master → base_sha) — needed if the eval harness will
# rebase against the current master before running. For a frozen base
# replay we just record the base_sha and rely on `git checkout` later;
# pre.diff stays empty for now and may be backfilled later.
: > "$DIR/pre.diff"

# Identify the test files modified by this PR and copy their post-merge
# content into tests/. The eval applies these on top of base_sha so the
# failing test exists in the replay sandbox.
gh pr view "$PR" --repo LadybirdBrowser/ladybird --json files \
  -q '.files[].path' \
  | grep '^Tests/' \
  | while read -r p; do
      mkdir -p "$DIR/tests/$(dirname "$p")"
      gh api "repos/LadybirdBrowser/ladybird/contents/$p?ref=$MERGE_SHA" \
        -q '.content' | base64 -d > "$DIR/tests/$p"
    done
```

- [ ] **Step 3: Write meta.json**

For each PR:

```json
{
  "pr_number": 12345,
  "url": "https://github.com/LadybirdBrowser/ladybird/pull/12345",
  "title": "Fix CSS grid line resolution off-by-one",
  "merged_at": "2025-09-14T10:23:00Z",
  "base_commit": "<base_sha>",
  "merge_commit": "<merge_sha>",
  "area": "libweb-css",
  "files_changed": [
    "Userland/Libraries/LibWeb/CSS/Grid.cpp"
  ],
  "task_prompt": "Fix the failing test Tests/LibWeb/Css/Grid/grid-line-off-by-one.html. The test expects ... but currently gets ..."
}
```

The `task_prompt` is what fresh agents see in replay. Write it like a real bug report — describe the symptom and the failing test, but don't reveal the fix.

- [ ] **Step 4: Validate the corpus**

After completing all 10, run a quick sanity check:

```bash
for d in benchmark/genskill/ladybird/pr_corpus/pr-*/; do
  for f in meta.json reference_fix.patch; do
    [ -f "$d$f" ] || echo "MISSING: $d$f"
  done
  [ -d "$d/tests" ] || echo "MISSING: $d/tests"
done
```

Expected: no `MISSING:` output.

- [ ] **Step 5: Commit**

```bash
git add benchmark/genskill/ladybird/pr_corpus/
git commit -m "feat(eval): curate 10-PR Ladybird corpus

Hand-picked PRs satisfying spec §3.1 criteria: merged >=180 days ago,
<=5 source files modified, >=1 test file touched, clear root cause.
Distribution per spec §3.2: 3 LibWeb, 2 LibJS, 1 LibHTTP, 2 LibCore,
1 LibCrypto, 1 build/tooling.

Frozen at this commit; benchmark re-runs use the same PRs for
comparability.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 4: Implement corpus validation in the eval binary

Replace the `validate` stub with a real check.

**Files:**
- Create: `benchmark/genskill/ladybird/src/pr_corpus.rs`
- Modify: `benchmark/genskill/ladybird/src/main.rs`

- [ ] **Step 1: Write tests for the loader**

Create `benchmark/genskill/ladybird/src/pr_corpus.rs`:

```rust
//! On-disk corpus loader and validator.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One PR's metadata as stored in pr_corpus/<id>/meta.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrMeta {
    /// Numeric PR id, matches the directory name suffix.
    pub pr_number: u64,
    /// Full GitHub URL to the PR.
    pub url: String,
    /// Title from the PR.
    pub title: String,
    /// Merge timestamp (RFC3339).
    pub merged_at: String,
    /// SHA before the fix (rebase target for replays).
    pub base_commit: String,
    /// SHA after merge.
    pub merge_commit: String,
    /// Area tag from spec §3.2.
    pub area: String,
    /// Source files modified by the merge (test files excluded).
    pub files_changed: Vec<String>,
    /// User-facing task prompt for the replay.
    pub task_prompt: String,
}

/// One loaded corpus entry.
#[derive(Debug)]
pub struct CorpusEntry {
    /// Directory id, e.g., "pr-12345".
    pub id: String,
    /// Parsed metadata.
    pub meta: PrMeta,
    /// Path of the directory holding all artifacts.
    pub dir: PathBuf,
}

/// Loads and validates every pr_corpus/pr-* subdirectory.
///
/// Returns Err if any entry is malformed; returns Ok with the loaded
/// entries otherwise. Use this from the `validate` CLI subcommand to
/// catch missing files before any replay starts.
pub fn load_corpus(corpus_dir: &Path) -> Result<Vec<CorpusEntry>> {
    let mut entries = Vec::new();
    for dent in std::fs::read_dir(corpus_dir).with_context(|| {
        format!("reading {}", corpus_dir.display())
    })? {
        let dent = dent?;
        let path = dent.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("pr-") => n.to_string(),
            _ => continue,
        };
        let meta_path = path.join("meta.json");
        let meta_text = std::fs::read_to_string(&meta_path)
            .with_context(|| format!("reading {}", meta_path.display()))?;
        let meta: PrMeta = serde_json::from_str(&meta_text)
            .with_context(|| format!("parsing {}", meta_path.display()))?;

        for required in ["reference_fix.patch", "tests"] {
            if !path.join(required).exists() {
                return Err(anyhow!("{}/{} missing", path.display(), required));
            }
        }

        entries.push(CorpusEntry {
            id: name,
            meta,
            dir: path,
        });
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    if entries.is_empty() {
        return Err(anyhow!("no pr-* entries under {}", corpus_dir.display()));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_pr(root: &Path, id: &str, body: &str) {
        let dir = root.join(id);
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(dir.join("meta.json"), body).unwrap();
        fs::write(dir.join("reference_fix.patch"), "").unwrap();
    }

    #[test]
    fn load_valid_corpus() {
        let tmp = TempDir::new().unwrap();
        write_pr(
            tmp.path(),
            "pr-1",
            r#"{
                "pr_number": 1,
                "url": "https://example.com/1",
                "title": "x",
                "merged_at": "2025-01-01T00:00:00Z",
                "base_commit": "abc",
                "merge_commit": "def",
                "area": "libweb-css",
                "files_changed": [],
                "task_prompt": "do x"
            }"#,
        );
        let entries = load_corpus(tmp.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "pr-1");
        assert_eq!(entries[0].meta.pr_number, 1);
    }

    #[test]
    fn load_rejects_missing_reference_fix() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("pr-1");
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(
            dir.join("meta.json"),
            r#"{
                "pr_number": 1, "url":"x","title":"x","merged_at":"x",
                "base_commit":"x","merge_commit":"x","area":"x",
                "files_changed":[],"task_prompt":"x"
            }"#,
        )
        .unwrap();
        assert!(load_corpus(tmp.path()).is_err());
    }

    #[test]
    fn load_rejects_empty_dir() {
        let tmp = TempDir::new().unwrap();
        assert!(load_corpus(tmp.path()).is_err());
    }
}
```

- [ ] **Step 2: Add `tempfile` dev-dependency**

Modify `benchmark/genskill/ladybird/Cargo.toml`, add under `[dev-dependencies]`:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Wire validate into main.rs**

Replace `Cmd::Validate` arm in `main.rs`:

```rust
Cmd::Validate => {
    let entries = pr_corpus::load_corpus(std::path::Path::new(
        "benchmark/genskill/ladybird/pr_corpus",
    ))?;
    println!("OK: {} entries", entries.len());
    for e in &entries {
        println!("  {} ({}, {})", e.id, e.meta.area, e.meta.title);
    }
    Ok(())
}
```

Add at the top of `main.rs`:

```rust
mod pr_corpus;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p puffer-genskill-eval`
Expected: 3 tests pass.

- [ ] **Step 5: Run validate against real corpus**

Run: `cargo run -p puffer-genskill-eval -- validate`
Expected: `OK: 10 entries` with one line per PR.

- [ ] **Step 6: Commit**

```bash
git add benchmark/genskill/ladybird/
git commit -m "feat(eval): implement corpus validation

pr_corpus.rs loads and validates every pr_corpus/pr-* directory:
parses meta.json, ensures reference_fix.patch and tests/ exist, sorts
by id. validate subcommand prints a summary or errors with the first
missing/malformed entry.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 5: Record expert runs (one per PR)

This task uses the puffer binary that has `/genskill` registered. The output (`expert_run.md`) is the **input** for both skill-generation arms in Task 6.

**Files:**
- Create: `benchmark/genskill/ladybird/scripts/record_expert_run.sh`
- Create: `benchmark/genskill/ladybird/pr_corpus/pr-<N>/expert_run.md` × 10

- [ ] **Step 1: Build the puffer binary**

Run: `cargo build -p puffer-cli --release`
Expected: produces `target/release/puffer` (or platform equivalent).

Confirm `/genskill` is registered: run `target/release/puffer --help` and look for it in the slash-command list.

- [ ] **Step 2: Write the recording wrapper**

Create `benchmark/genskill/ladybird/scripts/record_expert_run.sh`:

```bash
#!/usr/bin/env bash
# Records an expert run for one PR. Spins up Ladybird in a sandbox,
# applies the test files from the corpus on top of base_commit, then
# launches puffer with the task prompt. The user (or a scripted agent)
# solves it; on exit we save the transcript to expert_run.md.
#
# Usage: record_expert_run.sh pr-12345
#
# Requires: docker, puffer binary on PATH, ladybird build image.

set -euo pipefail

PR="${1:?usage: record_expert_run.sh <pr-id>}"
CORPUS_DIR="benchmark/genskill/ladybird/pr_corpus/$PR"
META="$CORPUS_DIR/meta.json"

[ -f "$META" ] || { echo "no meta.json at $META"; exit 1; }

BASE_SHA=$(jq -r '.base_commit' "$META")
TASK_PROMPT=$(jq -r '.task_prompt' "$META")

# Spin up a transient Docker container with ladybird checked out at base_sha.
# IMAGE_TAG is the eval Docker image built by Dockerfile.ladybird-eval (Task 8).
IMAGE_TAG="${IMAGE_TAG:-puffer-genskill-eval-ladybird}"
WORKDIR="/work/ladybird"

CONTAINER=$(docker run -d --rm \
  -v "$PWD:/host:ro" \
  -e BASE_SHA="$BASE_SHA" \
  --workdir "$WORKDIR" \
  "$IMAGE_TAG" \
  sleep infinity)

trap "docker rm -f $CONTAINER >/dev/null" EXIT

docker exec "$CONTAINER" git -C "$WORKDIR" reset --hard "$BASE_SHA"
docker exec "$CONTAINER" cp -r "/host/$CORPUS_DIR/tests/." "$WORKDIR/"

echo "=== Expert run for $PR ==="
echo "Task: $TASK_PROMPT"
echo "Container: $CONTAINER"
echo
echo "Run puffer inside the container, attach to it via docker exec,"
echo "and solve the task. When done, exit puffer; the transcript will"
echo "be at /tmp/puffer-session.jsonl inside the container."
echo
read -rp "Press enter once you've completed the run..."

docker cp "$CONTAINER:/tmp/puffer-session.jsonl" "$CORPUS_DIR/expert_run.jsonl"
echo "Saved transcript to $CORPUS_DIR/expert_run.jsonl"

# Convert JSONL to markdown for human readability and as input to /genskill.
# transcript_to_markdown is a small helper added in Task 7.
cargo run -p puffer-genskill-eval -- transcript-to-md \
  --in "$CORPUS_DIR/expert_run.jsonl" \
  --out "$CORPUS_DIR/expert_run.md"
echo "Saved markdown to $CORPUS_DIR/expert_run.md"
```

- [ ] **Step 3: Add the transcript-to-md subcommand**

This is a small utility that converts a puffer session JSONL transcript to a flat markdown document. Used by `record_expert_run.sh` (above) and again as input for the skill generators in Task 6.

In `main.rs`, add to `Cmd`:

```rust
    /// Convert a puffer session JSONL transcript to flat markdown.
    TranscriptToMd {
        /// Input JSONL transcript path.
        #[arg(long = "in")]
        input: std::path::PathBuf,
        /// Output markdown path.
        #[arg(long = "out")]
        output: std::path::PathBuf,
    },
```

Create `benchmark/genskill/ladybird/src/transcript.rs`:

```rust
//! Convert puffer session JSONL transcripts to flat markdown.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::Path;

/// A single line of a puffer JSONL transcript.
///
/// We accept extra fields and only deserialize the ones we render.
#[derive(Debug, Deserialize)]
struct TranscriptLine {
    role: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool_call: Option<ToolCall>,
}

#[derive(Debug, Deserialize)]
struct ToolCall {
    name: String,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

/// Reads a JSONL transcript and writes a flat markdown rendering.
///
/// Each line becomes a section header with the role; tool calls are
/// rendered as fenced code blocks. Output is human-readable AND a
/// reasonable input for /genskill (it's just markdown).
pub fn transcript_to_md(input: &Path, output: &Path) -> Result<()> {
    let content = fs::read_to_string(input)
        .with_context(|| format!("reading {}", input.display()))?;
    let mut out = fs::File::create(output)
        .with_context(|| format!("creating {}", output.display()))?;

    writeln!(out, "# Expert run transcript")?;
    writeln!(out)?;

    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: TranscriptLine = serde_json::from_str(line)
            .with_context(|| format!("parsing line {}", i + 1))?;
        writeln!(out, "## {} (line {})", parsed.role, i + 1)?;
        if let Some(t) = &parsed.text {
            writeln!(out)?;
            writeln!(out, "{}", t.trim())?;
            writeln!(out)?;
        }
        if let Some(tc) = &parsed.tool_call {
            writeln!(out, "**tool_call:** `{}`", tc.name)?;
            if let Some(input_val) = &tc.input {
                writeln!(out, "```json")?;
                writeln!(out, "{}", serde_json::to_string_pretty(input_val)?)?;
                writeln!(out, "```")?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn renders_simple_transcript() {
        let tmp = TempDir::new().unwrap();
        let input = tmp.path().join("in.jsonl");
        let output = tmp.path().join("out.md");
        fs::write(
            &input,
            r#"{"role":"user","text":"hi"}
{"role":"assistant","text":"hello","tool_call":{"name":"Read","input":{"path":"/x"}}}
"#,
        )
        .unwrap();
        transcript_to_md(&input, &output).unwrap();
        let result = fs::read_to_string(&output).unwrap();
        assert!(result.contains("## user"));
        assert!(result.contains("hi"));
        assert!(result.contains("**tool_call:** `Read`"));
    }
}
```

Add `mod transcript;` to `main.rs` and wire the subcommand:

```rust
Cmd::TranscriptToMd { input, output } => {
    transcript::transcript_to_md(&input, &output)?;
    println!("Wrote {}", output.display());
    Ok(())
}
```

- [ ] **Step 4: Smoke test the converter**

Create a tiny test transcript and run the converter to confirm output is sensible:

```bash
cat > /tmp/test.jsonl <<'EOF'
{"role":"user","text":"Fix the failing test"}
{"role":"assistant","text":"Let me read the file","tool_call":{"name":"Read","input":{"path":"foo.cpp"}}}
EOF
cargo run -p puffer-genskill-eval -- transcript-to-md --in /tmp/test.jsonl --out /tmp/test.md
cat /tmp/test.md
```

Expected: a readable markdown rendering with role headers and tool-call sections.

- [ ] **Step 5: Run expert recording for all 10 PRs**

For each PR `pr-NNNNN` in the corpus, run:

```bash
bash benchmark/genskill/ladybird/scripts/record_expert_run.sh pr-NNNNN
```

This is interactive: you (or a high-quality agent) will solve each task inside the sandbox. Each run is expected to take 10-30 minutes. Plan for several hours total.

- [ ] **Step 6: Verify all 10 expert runs exist**

```bash
for d in benchmark/genskill/ladybird/pr_corpus/pr-*/; do
  [ -f "$d/expert_run.md" ] && [ -s "$d/expert_run.md" ] || echo "MISSING: $d/expert_run.md"
done
```

Expected: no `MISSING:` output.

- [ ] **Step 7: Commit**

```bash
git add benchmark/genskill/ladybird/
git commit -m "feat(eval): record 10 expert runs and add transcript converter

record_expert_run.sh wraps the recording flow: spawn sandbox, reset to
base_commit, apply test files, prompt for puffer session, capture
transcript. transcript-to-md converts puffer JSONL session files to
flat markdown for /genskill consumption.

Each expert_run.md is the source-of-truth task description that both
skill-generation arms see. Frozen at this commit.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase B — Skill Generation

## Task 6: Generate gepa and direct skills for each PR

**Files:**
- Create: `benchmark/genskill/ladybird/scripts/generate_skills.sh`
- Create: `benchmark/genskill/ladybird/pr_corpus/pr-<N>/skills/{gepa,direct}/SKILL.md` × 10

- [ ] **Step 1: Write the script**

Create `benchmark/genskill/ladybird/scripts/generate_skills.sh`:

```bash
#!/usr/bin/env bash
# Generates two skills (gepa, direct) for one PR's expert_run.md.
#
# Usage: generate_skills.sh pr-12345

set -euo pipefail

PR="${1:?usage: generate_skills.sh <pr-id>}"
CORPUS_DIR="benchmark/genskill/ladybird/pr_corpus/$PR"
EXPERT="$CORPUS_DIR/expert_run.md"

[ -f "$EXPERT" ] || { echo "no expert_run.md at $EXPERT"; exit 1; }

# GEPA arm: invoke /genskill against the expert run. We use a non-
# interactive puffer session that loads expert_run.md as conversation
# context, then runs /genskill.
#
# The exact flag for "load this transcript and run a single command non-
# interactively" depends on puffer's CLI surface (see `puffer --help`).
# Fill in the actual flag during execution.
mkdir -p "$CORPUS_DIR/skills/gepa" "$CORPUS_DIR/skills/direct"

puffer non-interactive \
  --load-transcript "$EXPERT" \
  --run-command '/genskill' \
  --output "$CORPUS_DIR/skills/gepa/SKILL.md"

# Direct arm: a single LLM call with a one-line prompt. Reads
# expert_run.md, asks the model to "Generate a reusable skill based on
# the conversation history above", saves the response.
#
# We shell out to the same provider puffer uses, via puffer's
# non-interactive mode with no skills/tools loaded.
puffer non-interactive \
  --load-transcript "$EXPERT" \
  --user-message "Generate a reusable skill based on the conversation history above. Output ONLY a SKILL.md document with YAML frontmatter (name, description) followed by sections. Stay under 15000 bytes." \
  --output "$CORPUS_DIR/skills/direct/SKILL.md"

echo "Generated $CORPUS_DIR/skills/{gepa,direct}/SKILL.md"
```

The `puffer non-interactive` flags above are placeholders — the script's executor must adapt to whatever puffer's actual non-interactive surface is. If puffer has no non-interactive transcript-load mode yet, the executor should add a small `--script` mode in a separate (small) Plan-1.5 PR before this script can run, OR temporarily script the flow with `expect`/`tmux`. Document the choice in the commit message.

- [ ] **Step 2: Run for all 10 PRs**

```bash
for d in benchmark/genskill/ladybird/pr_corpus/pr-*/; do
  pr=$(basename "$d")
  bash benchmark/genskill/ladybird/scripts/generate_skills.sh "$pr"
done
```

Each generation is non-interactive but each `/genskill` call takes a few minutes (multiple sub-agent calls + judging). Plan for ~30 minutes total.

- [ ] **Step 3: Verify**

```bash
for d in benchmark/genskill/ladybird/pr_corpus/pr-*/skills/; do
  for arm in gepa direct; do
    [ -s "$d$arm/SKILL.md" ] || echo "MISSING/EMPTY: $d$arm/SKILL.md"
  done
done
```

Expected: no `MISSING/EMPTY:` output.

- [ ] **Step 4: Commit**

```bash
git add benchmark/genskill/ladybird/
git commit -m "feat(eval): generate gepa and direct skills for all 10 PRs

generate_skills.sh runs /genskill (gepa arm) and a one-line direct
prompt (direct arm) against each expert_run.md. Resulting SKILL.md
files live under pr_corpus/<pr>/skills/{gepa,direct}/. Frozen at this
commit; replay always loads these exact files.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase C — Replay Infrastructure

## Task 7: Build the eval Docker image

**Files:**
- Create: `benchmark/genskill/ladybird/Dockerfile.ladybird-eval`

- [ ] **Step 1: Write the Dockerfile**

Create `benchmark/genskill/ladybird/Dockerfile.ladybird-eval`:

```dockerfile
# Sandbox image for puffer-genskill-eval replays.
#
# Includes:
#   - Ladybird build dependencies
#   - A pre-fetched ladybird/master checkout at /work/ladybird (warm cache)
#   - The puffer binary at /usr/local/bin/puffer
#
# Each replay container does:
#   git -C /work/ladybird reset --hard <base_sha>
#   cp <test files from corpus> /work/ladybird/
#   exec puffer non-interactive ...

FROM ubuntu:24.04

ARG LADYBIRD_REPO=https://github.com/LadybirdBrowser/ladybird.git
ARG LADYBIRD_REF=master

# Ladybird build deps. Mirrors LadybirdBrowser/ladybird's official build
# image. Pin specific package versions where possible for reproducibility.
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y \
    build-essential \
    cmake \
    ninja-build \
    git \
    pkg-config \
    libgl1-mesa-dev \
    libssl-dev \
    libavcodec-dev \
    libavformat-dev \
    libavutil-dev \
    libavfilter-dev \
    libopenal-dev \
    libxkbcommon-dev \
    libpulse-dev \
    libwayland-dev \
    libwebp-dev \
    qt6-base-dev \
    qt6-tools-dev \
    qt6-multimedia-dev \
    qt6-wayland \
    ca-certificates \
    curl \
    jq \
    python3 \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /work
RUN git clone --depth 1 --branch "${LADYBIRD_REF}" "${LADYBIRD_REPO}" ladybird

# Warm the build cache. If this fails, the image still works but
# replays pay the full build cost on first invocation.
RUN cd ladybird && (./Meta/ladybird.sh build || true)

# Puffer binary is mounted in at runtime via -v rather than baked in,
# so changes to puffer don't invalidate the image cache.
# Place a stub for clarity:
RUN mkdir -p /opt/puffer && echo "Mount /usr/local/bin/puffer at runtime" > /opt/puffer/README

CMD ["/bin/bash"]
```

- [ ] **Step 2: Build the image**

Run:

```bash
docker build \
  -f benchmark/genskill/ladybird/Dockerfile.ladybird-eval \
  -t puffer-genskill-eval-ladybird \
  benchmark/genskill/ladybird/
```

Expected: image builds (may take 30-60 minutes for the warm-cache step). If `Meta/ladybird.sh build` fails inside the container, that's tolerated — the image still works, replays will rebuild on first run.

- [ ] **Step 3: Smoke test**

```bash
docker run --rm puffer-genskill-eval-ladybird \
  bash -c 'cd /work/ladybird && git log -1 --oneline'
```

Expected: prints a recent commit hash.

- [ ] **Step 4: Commit**

```bash
git add benchmark/genskill/ladybird/Dockerfile.ladybird-eval
git commit -m "feat(eval): add Ladybird sandbox Dockerfile

Reproducible sandbox image based on Ubuntu 24.04 with Ladybird build
deps, pre-cloned ladybird/master, and a pre-warmed build cache. Puffer
binary is mounted at runtime so puffer changes don't invalidate the
image cache.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 8: Implement the sandbox module

**Files:**
- Create: `benchmark/genskill/ladybird/src/sandbox.rs`
- Modify: `benchmark/genskill/ladybird/Cargo.toml` (add bollard or shell out)

For simplicity and zero new dependencies beyond what we already have, this implementation **shells out to `docker`** rather than using `bollard` (the async Docker client). Docker CLI is the same surface as the bash scripts and easier to debug.

- [ ] **Step 1: Write tests + impl**

Create `benchmark/genskill/ladybird/src/sandbox.rs`:

```rust
//! Spawns and tears down replay sandboxes via the docker CLI.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Image tag built by Dockerfile.ladybird-eval.
pub const DEFAULT_IMAGE: &str = "puffer-genskill-eval-ladybird";

/// Working directory inside the container where ladybird is checked out.
pub const CONTAINER_WORKDIR: &str = "/work/ladybird";

/// One running replay sandbox. Drop releases the container.
pub struct Sandbox {
    container_id: String,
}

impl Sandbox {
    /// Spawns a fresh container, checks out `base_commit`, copies test
    /// files in. The puffer binary at `puffer_bin_host_path` is mounted
    /// at /usr/local/bin/puffer.
    pub async fn start(
        image: &str,
        puffer_bin_host_path: &Path,
        base_commit: &str,
        test_files_host_dir: &Path,
    ) -> Result<Self> {
        let puffer_bin_abs = puffer_bin_host_path.canonicalize().with_context(|| {
            format!("canonicalizing puffer binary path {}", puffer_bin_host_path.display())
        })?;
        let test_files_abs = test_files_host_dir.canonicalize().with_context(|| {
            format!("canonicalizing test files dir {}", test_files_host_dir.display())
        })?;

        let out = Command::new("docker")
            .arg("run")
            .arg("-d")
            .arg("--rm")
            .args([
                "-v",
                &format!("{}:/usr/local/bin/puffer:ro", puffer_bin_abs.display()),
            ])
            .args([
                "-v",
                &format!("{}:/work/test_files:ro", test_files_abs.display()),
            ])
            .args(["--workdir", CONTAINER_WORKDIR])
            .arg(image)
            .args(["sleep", "infinity"])
            .stdout(Stdio::piped())
            .output()
            .await
            .context("spawning docker run")?;
        if !out.status.success() {
            return Err(anyhow!(
                "docker run failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let container_id = String::from_utf8(out.stdout)?.trim().to_string();
        if container_id.is_empty() {
            return Err(anyhow!("empty container id from docker run"));
        }
        let sandbox = Self { container_id };

        sandbox
            .exec(&["git", "reset", "--hard", base_commit])
            .await?;
        sandbox
            .exec(&["bash", "-c", "cp -r /work/test_files/. /work/ladybird/"])
            .await?;
        Ok(sandbox)
    }

    /// Runs a command inside the container, returning (stdout, stderr).
    pub async fn exec(&self, argv: &[&str]) -> Result<(String, String)> {
        let mut cmd = Command::new("docker");
        cmd.arg("exec").arg(&self.container_id);
        for a in argv {
            cmd.arg(a);
        }
        let out = cmd.output().await.context("docker exec")?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if !out.status.success() {
            return Err(anyhow!(
                "docker exec failed (status {:?}): {}",
                out.status.code(),
                stderr
            ));
        }
        Ok((stdout, stderr))
    }

    /// Container id (for diagnostics; do not rely on this in scripts).
    pub fn container_id(&self) -> &str {
        &self.container_id
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Best-effort teardown. Synchronous std::process here because
        // tokio::process::Command requires a runtime context that
        // Drop can't guarantee.
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.container_id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
```

- [ ] **Step 2: Wire mod into main.rs**

Add to `main.rs`:

```rust
mod sandbox;
```

- [ ] **Step 3: Manual smoke test**

Skipped — sandbox is exercised in Task 9's replay implementation. No standalone test here because it requires a live Docker daemon.

- [ ] **Step 4: Build verification**

Run: `cargo build -p puffer-genskill-eval`
Expected: builds clean.

- [ ] **Step 5: Commit**

```bash
git add benchmark/genskill/ladybird/
git commit -m "feat(eval): add Docker sandbox helper

sandbox.rs spawns a transient ladybird-eval container per replay,
mounts the puffer binary read-only, copies test files in, and resets
the working tree to base_commit. Drop releases the container best-
effort. Shells out to docker CLI rather than bollard to keep
dependencies minimal.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 9: Implement the replay module

**Files:**
- Create: `benchmark/genskill/ladybird/src/replay.rs`
- Modify: `benchmark/genskill/ladybird/src/main.rs`

This is the largest task — it ties together sandbox + puffer + stop conditions + artifact capture.

- [ ] **Step 1: Define the replay artifact shape**

Create `benchmark/genskill/ladybird/src/replay.rs`:

```rust
//! One-replay execution: spawn sandbox, run puffer, capture artifact.

use crate::pr_corpus::CorpusEntry;
use crate::sandbox::{Sandbox, DEFAULT_IMAGE};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::time::{timeout, Duration};

/// A replay arm: which skill (if any) was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Arm {
    /// No skill loaded (baseline).
    NoSkill,
    /// Direct-prompt-generated skill loaded.
    Direct,
    /// /genskill GEPA-generated skill loaded.
    Gepa,
}

impl Arm {
    /// Parses a CLI string into an Arm.
    pub fn parse(s: &str) -> Result<Arm> {
        match s {
            "no-skill" => Ok(Arm::NoSkill),
            "direct" => Ok(Arm::Direct),
            "gepa" => Ok(Arm::Gepa),
            _ => Err(anyhow!("unknown arm {s}; expected no-skill | direct | gepa")),
        }
    }
}

/// How a replay terminated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Target test passed.
    Pass,
    /// Test still failed after agent's claim of completion.
    WrongFix,
    /// Agent gave up.
    GaveUp,
    /// Wall-clock budget exceeded.
    WallTimeout,
    /// Tool-call cap exceeded.
    ToolBudget,
    /// Token budget exceeded.
    TokenBudget,
}

/// Token usage breakdown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub tool_results: u64,
    pub total: u64,
}

/// One tool call recorded during the replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub input: serde_json::Value,
    pub output_size: u64,
    pub ts: String,
}

/// Outcome of running the target test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestOutcome {
    pub command: String,
    pub exit_code: i32,
    pub stdout_tail: String,
}

/// Full replay artifact stored at reports/<run_date>/<pr>-<arm>.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayArtifact {
    pub pr: String,
    pub arm: Arm,
    pub outcome: Outcome,
    pub wall_seconds: u64,
    pub tool_calls: u64,
    pub tokens: Tokens,
    pub tool_call_log: Vec<ToolCall>,
    pub final_diff: String,
    pub test_outcome: Option<TestOutcome>,
}

/// Configuration for one replay.
pub struct ReplayConfig<'a> {
    pub corpus_entry: &'a CorpusEntry,
    pub arm: Arm,
    pub puffer_bin_host_path: PathBuf,
    pub image: String,
    pub wall_budget: Duration,
    pub tool_budget: u64,
    pub token_budget: u64,
    pub run_date_dir: PathBuf,
}

/// Runs one replay end-to-end. Writes the artifact JSON to
/// `<run_date_dir>/<pr>-<arm>.json` and returns the artifact in memory.
pub async fn run_one(cfg: ReplayConfig<'_>) -> Result<ReplayArtifact> {
    let started = std::time::Instant::now();

    let test_files_dir = cfg.corpus_entry.dir.join("tests");
    let sandbox = Sandbox::start(
        &cfg.image,
        &cfg.puffer_bin_host_path,
        &cfg.corpus_entry.meta.base_commit,
        &test_files_dir,
    )
    .await
    .context("starting sandbox")?;

    // Confirm test fails before agent starts (sanity check).
    let test_filter = test_filter_for(cfg.corpus_entry);
    let pre_check = sandbox
        .exec(&["bash", "-c", &format!("ladybird-test --filter={test_filter} ; echo $?")])
        .await
        .ok();
    tracing::info!(?pre_check, "pre-replay test status");

    // Compose the puffer non-interactive invocation.
    let mut puffer_args = vec![
        "non-interactive".to_string(),
        "--user-message".to_string(),
        cfg.corpus_entry.meta.task_prompt.clone(),
        "--max-tool-calls".to_string(),
        cfg.tool_budget.to_string(),
        "--max-tokens".to_string(),
        cfg.token_budget.to_string(),
        "--emit-artifact".to_string(),
        "/tmp/replay-artifact.json".to_string(),
    ];
    if let Some(skill_path) = skill_path_for(&cfg) {
        puffer_args.push("--load-skill".to_string());
        puffer_args.push(skill_path);
    }

    let exec_args: Vec<&str> = std::iter::once("puffer")
        .chain(puffer_args.iter().map(String::as_str))
        .collect();

    // Wall-clock timeout wraps the puffer process. If the process
    // finishes inside the budget, the artifact has the precise outcome
    // (incl. tool/token budget hits as detected by puffer itself).
    let exec_result = timeout(cfg.wall_budget, sandbox.exec(&exec_args)).await;
    let outcome_kind = match exec_result {
        Err(_) => Outcome::WallTimeout,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "puffer exec failed");
            Outcome::GaveUp
        }
        Ok(Ok(_)) => Outcome::Pass, // refined below by reading the artifact
    };

    // Pull the puffer-emitted artifact out of the container.
    let pulled = sandbox
        .exec(&["bash", "-c", "cat /tmp/replay-artifact.json"])
        .await;
    let mut artifact = if let Ok((stdout, _)) = pulled {
        serde_json::from_str::<ReplayArtifact>(&stdout).unwrap_or_else(|_| empty_artifact(&cfg))
    } else {
        empty_artifact(&cfg)
    };

    // Reconcile outcome (wall-timeout dominates whatever puffer claimed).
    if matches!(outcome_kind, Outcome::WallTimeout) {
        artifact.outcome = Outcome::WallTimeout;
    }

    // Run the target test inside the sandbox to produce the binary signal.
    let test_run = sandbox
        .exec(&["bash", "-c", &format!("ladybird-test --filter={test_filter}; echo EXIT=$?")])
        .await;
    if let Ok((stdout, _)) = test_run {
        let exit_code = parse_exit_code(&stdout).unwrap_or(-1);
        artifact.test_outcome = Some(TestOutcome {
            command: format!("ladybird-test --filter={test_filter}"),
            exit_code,
            stdout_tail: tail(&stdout, 4_000),
        });
        if exit_code == 0 && !matches!(artifact.outcome, Outcome::WallTimeout) {
            artifact.outcome = Outcome::Pass;
        } else if exit_code != 0 && matches!(artifact.outcome, Outcome::Pass) {
            artifact.outcome = Outcome::WrongFix;
        }
    }

    artifact.wall_seconds = started.elapsed().as_secs();

    // Persist artifact.
    std::fs::create_dir_all(&cfg.run_date_dir)
        .with_context(|| format!("creating {}", cfg.run_date_dir.display()))?;
    let artifact_path = cfg
        .run_date_dir
        .join(format!("{}-{:?}.json", cfg.corpus_entry.id, cfg.arm))
        .with_extension("json");
    std::fs::write(
        &artifact_path,
        serde_json::to_string_pretty(&artifact)?,
    )?;
    tracing::info!(path = %artifact_path.display(), "wrote replay artifact");

    Ok(artifact)
}

fn empty_artifact(cfg: &ReplayConfig<'_>) -> ReplayArtifact {
    ReplayArtifact {
        pr: cfg.corpus_entry.id.clone(),
        arm: cfg.arm,
        outcome: Outcome::GaveUp,
        wall_seconds: 0,
        tool_calls: 0,
        tokens: Tokens::default(),
        tool_call_log: Vec::new(),
        final_diff: String::new(),
        test_outcome: None,
    }
}

fn skill_path_for(cfg: &ReplayConfig<'_>) -> Option<String> {
    match cfg.arm {
        Arm::NoSkill => None,
        Arm::Direct => Some(format!(
            "/host/{}/skills/direct/SKILL.md",
            cfg.corpus_entry.dir.display()
        )),
        Arm::Gepa => Some(format!(
            "/host/{}/skills/gepa/SKILL.md",
            cfg.corpus_entry.dir.display()
        )),
    }
}

fn test_filter_for(entry: &CorpusEntry) -> String {
    // Heuristic: the first .html or .cpp under tests/ is the target.
    // Better: meta.json gets a `test_filter` field. For now, derive.
    entry
        .meta
        .files_changed
        .iter()
        .find(|p| p.starts_with("Tests/"))
        .cloned()
        .unwrap_or_else(|| entry.id.clone())
}

fn parse_exit_code(s: &str) -> Option<i32> {
    s.lines()
        .rev()
        .find_map(|l| l.strip_prefix("EXIT=").and_then(|n| n.parse().ok()))
}

fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        s[s.len() - n..].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_parses() {
        assert_eq!(Arm::parse("no-skill").unwrap(), Arm::NoSkill);
        assert_eq!(Arm::parse("direct").unwrap(), Arm::Direct);
        assert_eq!(Arm::parse("gepa").unwrap(), Arm::Gepa);
        assert!(Arm::parse("garbage").is_err());
    }

    #[test]
    fn parse_exit_extracts() {
        let s = "some output\nmore output\nEXIT=0\n";
        assert_eq!(parse_exit_code(s), Some(0));
    }

    #[test]
    fn parse_exit_finds_last() {
        let s = "EXIT=1\nlater\nEXIT=0\n";
        assert_eq!(parse_exit_code(s), Some(0));
    }

    #[test]
    fn tail_short_string_is_unchanged() {
        assert_eq!(tail("abc", 10), "abc");
    }

    #[test]
    fn tail_truncates_long() {
        let s: String = "x".repeat(50);
        let t = tail(&s, 10);
        assert_eq!(t.len(), 10);
    }
}
```

- [ ] **Step 2: Wire into main.rs**

Add `mod replay;` to `main.rs`. Replace the `Cmd::Replay` arm:

```rust
Cmd::Replay { pr, arm } => {
    let arm = replay::Arm::parse(&arm)?;
    let entries = pr_corpus::load_corpus(std::path::Path::new(
        "benchmark/genskill/ladybird/pr_corpus",
    ))?;
    let entry = entries.iter().find(|e| e.id == pr).ok_or_else(|| {
        anyhow::anyhow!("pr {pr} not in corpus")
    })?;
    let run_date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let cfg = replay::ReplayConfig {
        corpus_entry: entry,
        arm,
        puffer_bin_host_path: std::path::PathBuf::from("target/release/puffer"),
        image: replay::sandbox::DEFAULT_IMAGE.to_string(),
        wall_budget: std::time::Duration::from_secs(30 * 60),
        tool_budget: 50,
        token_budget: 250_000,
        run_date_dir: std::path::PathBuf::from(format!(
            "benchmark/genskill/ladybird/reports/{run_date}"
        )),
    };
    let artifact = replay::run_one(cfg).await?;
    println!("{}", serde_json::to_string_pretty(&artifact)?);
    Ok(())
}
```

Add `chrono` to Cargo.toml under `[dependencies]`:

```toml
chrono = { version = "0.4", default-features = false, features = ["clock"] }
```

- [ ] **Step 3: Run unit tests**

Run: `cargo test -p puffer-genskill-eval`
Expected: 8 tests pass (3 from pr_corpus, 1 from transcript, 5 from replay).

- [ ] **Step 4: Smoke test against one PR**

Run (after Tasks 1-7 are complete):
```
cargo run -p puffer-genskill-eval -- replay pr-NNNNN no-skill
```
where `pr-NNNNN` is the first corpus entry. Expected: artifact JSON printed and saved under `benchmark/genskill/ladybird/reports/<today>/`.

If puffer's `non-interactive --emit-artifact` doesn't exist yet, this is the moment to add it (small additive change in puffer-cli — the executor scopes that as part of this task or as a tiny pre-req).

- [ ] **Step 5: Commit**

```bash
git add benchmark/genskill/ladybird/
git commit -m "feat(eval): implement single-replay execution

replay::run_one wraps sandbox + puffer non-interactive + test
verification + artifact capture into one async function. Stop
conditions: 30 min wall, 50 tool calls, 250K tokens. Outcomes:
pass | wrong-fix | gave-up | wall-timeout | tool-budget | token-budget.
Artifact stored at reports/<run_date>/<pr>-<arm>.json.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase D — Metrics and Reporting

## Task 10: Implement metrics

**Files:**
- Create: `benchmark/genskill/ladybird/src/metrics.rs`

- [ ] **Step 1: Write tests + impl**

Create `benchmark/genskill/ladybird/src/metrics.rs`:

```rust
//! Metric calculators for replay artifacts.

use crate::replay::ReplayArtifact;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Computed metrics for a single replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayMetrics {
    pub passed: bool,
    pub total_tokens: u64,
    pub tool_calls: u64,
    pub duplicate_score: u64,
    pub file_overlap: f32,
    pub symbol_overlap: f32,
}

/// Computes metrics from one artifact + the merged-fix reference.
///
/// `reference_fix` is the unified diff text from
/// pr_corpus/<pr>/reference_fix.patch.
pub fn compute(artifact: &ReplayArtifact, reference_fix: &str) -> ReplayMetrics {
    let passed = matches!(artifact.outcome, crate::replay::Outcome::Pass);
    let total_tokens = artifact.tokens.total;
    let tool_calls = artifact.tool_calls;
    let duplicate_score = compute_duplicate_score(&artifact.tool_call_log);
    let agent_files = files_in_diff(&artifact.final_diff);
    let ref_files = files_in_diff(reference_fix);
    let file_overlap = jaccard(&agent_files, &ref_files);
    let agent_symbols = symbols_in_diff(&artifact.final_diff);
    let ref_symbols = symbols_in_diff(reference_fix);
    let symbol_overlap = if ref_symbols.is_empty() {
        0.0
    } else {
        agent_symbols.intersection(&ref_symbols).count() as f32
            / ref_symbols.len() as f32
    };
    ReplayMetrics {
        passed,
        total_tokens,
        tool_calls,
        duplicate_score,
        file_overlap,
        symbol_overlap,
    }
}

fn compute_duplicate_score(log: &[crate::replay::ToolCall]) -> u64 {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for call in log {
        let key = normalize_key(call);
        *counts.entry(key).or_default() += 1;
    }
    counts.values().map(|n| n.saturating_sub(1)).sum()
}

fn normalize_key(call: &crate::replay::ToolCall) -> String {
    match call.name.as_str() {
        "Read" => format!("Read::{}", call.input.get("path").and_then(|v| v.as_str()).unwrap_or("?")),
        "Bash" => {
            let cmd = call.input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let normalized = cmd
                .trim()
                .to_ascii_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            format!("Bash::{normalized}")
        }
        "Grep" => {
            let pat = call.input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = call.input.get("path").and_then(|v| v.as_str()).unwrap_or("");
            format!("Grep::{pat}::{path}")
        }
        "Edit" | "Write" => format!(
            "{}::{}",
            call.name,
            call.input.get("file_path").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        other => format!("{other}::{}", call.input),
    }
}

fn files_in_diff(diff: &str) -> HashSet<String> {
    diff.lines()
        .filter_map(|l| l.strip_prefix("+++ b/").or_else(|| l.strip_prefix("--- a/")))
        .map(|s| s.trim().to_string())
        .collect()
}

fn symbols_in_diff(diff: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for line in diff.lines() {
        if !line.starts_with('+') && !line.starts_with('-') {
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        for token in line[1..].split(|c: char| !c.is_alphanumeric() && c != '_') {
            if token.len() < 4 || token.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                continue;
            }
            out.insert(token.to_string());
        }
    }
    out
}

fn jaccard<T: Eq + std::hash::Hash>(a: &HashSet<T>, b: &HashSet<T>) -> f32 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f32 / union as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jaccard_empty_is_zero() {
        let a: HashSet<&str> = HashSet::new();
        let b: HashSet<&str> = HashSet::new();
        assert_eq!(jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_full_overlap() {
        let a: HashSet<&str> = ["x"].into_iter().collect();
        let b: HashSet<&str> = ["x"].into_iter().collect();
        assert_eq!(jaccard(&a, &b), 1.0);
    }

    #[test]
    fn files_in_diff_extracts() {
        let d = "diff --git a/foo.cpp b/foo.cpp\n--- a/foo.cpp\n+++ b/foo.cpp\n@@ ...\n";
        let files = files_in_diff(d);
        assert!(files.contains("foo.cpp"));
    }

    #[test]
    fn duplicate_score_simple() {
        let log = vec![
            crate::replay::ToolCall {
                name: "Read".into(),
                input: serde_json::json!({"path":"x"}),
                output_size: 0,
                ts: "".into(),
            },
            crate::replay::ToolCall {
                name: "Read".into(),
                input: serde_json::json!({"path":"x"}),
                output_size: 0,
                ts: "".into(),
            },
            crate::replay::ToolCall {
                name: "Read".into(),
                input: serde_json::json!({"path":"y"}),
                output_size: 0,
                ts: "".into(),
            },
        ];
        assert_eq!(compute_duplicate_score(&log), 1);
    }

    #[test]
    fn symbols_in_diff_extracts_identifiers() {
        let d = "@@ ...\n+ ResolveGridLine(line);\n-  oldFunc(x);\n";
        let syms = symbols_in_diff(d);
        assert!(syms.contains("ResolveGridLine"));
        assert!(syms.contains("oldFunc"));
        assert!(!syms.contains("x"));
    }
}
```

- [ ] **Step 2: Wire into main.rs**

Add `mod metrics;` to `main.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p puffer-genskill-eval`
Expected: 13 tests pass (8 prior + 5 new).

- [ ] **Step 4: Commit**

```bash
git add benchmark/genskill/ladybird/
git commit -m "feat(eval): implement replay metrics

metrics::compute computes per-replay metrics:
- passed (binary)
- total_tokens
- tool_calls
- duplicate_score (sum of (count - 1) over normalized tool calls)
- file_overlap (Jaccard over diff file lists)
- symbol_overlap (intersection / reference cardinality)

Diff parsing extracts file paths from --- a/ / +++ b/ headers and
identifiers from added/removed lines.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 11: Implement aggregation and reporting

**Files:**
- Create: `benchmark/genskill/ladybird/src/report.rs`
- Modify: `benchmark/genskill/ladybird/src/main.rs`

- [ ] **Step 1: Write the report renderer**

Create `benchmark/genskill/ladybird/src/report.rs`:

```rust
//! Aggregates per-replay metrics across the corpus and renders markdown.

use crate::metrics::ReplayMetrics;
use crate::replay::Arm;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One row in the aggregated report.
#[derive(Debug, Clone)]
pub struct ArmSummary {
    pub passed: u64,
    pub total: u64,
    pub mean_tokens: f64,
    pub mean_tool_calls: f64,
    pub mean_duplicate: f64,
    pub mean_file_overlap: f64,
    pub mean_symbol_overlap: f64,
}

/// One PR's three-arm comparison.
pub type PrTriple = BTreeMap<Arm, ReplayMetrics>;

/// Aggregates metrics across all PRs for one arm.
pub fn aggregate(metrics_by_pr: &BTreeMap<String, PrTriple>, arm: Arm) -> ArmSummary {
    let mut s = ArmSummary {
        passed: 0,
        total: 0,
        mean_tokens: 0.0,
        mean_tool_calls: 0.0,
        mean_duplicate: 0.0,
        mean_file_overlap: 0.0,
        mean_symbol_overlap: 0.0,
    };
    for triple in metrics_by_pr.values() {
        if let Some(m) = triple.get(&arm) {
            s.total += 1;
            if m.passed {
                s.passed += 1;
                s.mean_tokens += m.total_tokens as f64;
                s.mean_tool_calls += m.tool_calls as f64;
                s.mean_duplicate += m.duplicate_score as f64;
                s.mean_file_overlap += m.file_overlap as f64;
                s.mean_symbol_overlap += m.symbol_overlap as f64;
            }
        }
    }
    let n = s.passed.max(1) as f64;
    s.mean_tokens /= n;
    s.mean_tool_calls /= n;
    s.mean_duplicate /= n;
    s.mean_file_overlap /= n;
    s.mean_symbol_overlap /= n;
    s
}

/// Renders the aggregated summary as markdown.
pub fn render_summary(
    run_date: &str,
    metrics_by_pr: &BTreeMap<String, PrTriple>,
) -> String {
    let no_skill = aggregate(metrics_by_pr, Arm::NoSkill);
    let direct = aggregate(metrics_by_pr, Arm::Direct);
    let gepa = aggregate(metrics_by_pr, Arm::Gepa);

    let mut out = String::new();
    let _ = writeln!(out, "# /genskill Evaluation — Ladybird PR Replay\n");
    let _ = writeln!(out, "Run date: {run_date}");
    let _ = writeln!(out, "PRs: {}\n", metrics_by_pr.len());
    let _ = writeln!(out, "## Headline\n");
    let _ = writeln!(out, "| Metric                        | no-skill | direct | gepa  |");
    let _ = writeln!(out, "|-------------------------------|----------|--------|-------|");
    let _ = writeln!(
        out,
        "| Pass rate                     | {} / {} | {} / {} | {} / {} |",
        no_skill.passed, no_skill.total, direct.passed, direct.total, gepa.passed, gepa.total
    );
    let _ = writeln!(
        out,
        "| Mean tokens (passed runs)     | {:.0} | {:.0} | {:.0} |",
        no_skill.mean_tokens, direct.mean_tokens, gepa.mean_tokens
    );
    let _ = writeln!(
        out,
        "| Mean tool calls (passed)      | {:.1} | {:.1} | {:.1} |",
        no_skill.mean_tool_calls, direct.mean_tool_calls, gepa.mean_tool_calls
    );
    let _ = writeln!(
        out,
        "| Mean duplicate_score (passed) | {:.1} | {:.1} | {:.1} |",
        no_skill.mean_duplicate, direct.mean_duplicate, gepa.mean_duplicate
    );
    let _ = writeln!(
        out,
        "| Mean file_overlap (passed)    | {:.2} | {:.2} | {:.2} |",
        no_skill.mean_file_overlap, direct.mean_file_overlap, gepa.mean_file_overlap
    );
    let _ = writeln!(
        out,
        "| Mean symbol_overlap (passed)  | {:.2} | {:.2} | {:.2} |",
        no_skill.mean_symbol_overlap, direct.mean_symbol_overlap, gepa.mean_symbol_overlap
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## Per-PR breakdown\n");
    for (pr, triple) in metrics_by_pr {
        let _ = writeln!(out, "### {pr}\n");
        let _ = writeln!(out, "| Arm      | Outcome | Tokens | Tools | Dup | FileOvl | SymOvl |");
        let _ = writeln!(out, "|----------|---------|--------|-------|-----|---------|--------|");
        for arm in [Arm::NoSkill, Arm::Direct, Arm::Gepa] {
            if let Some(m) = triple.get(&arm) {
                let _ = writeln!(
                    out,
                    "| {:?} | {} | {} | {} | {} | {:.2} | {:.2} |",
                    arm,
                    if m.passed { "pass" } else { "fail" },
                    m.total_tokens,
                    m.tool_calls,
                    m.duplicate_score,
                    m.file_overlap,
                    m.symbol_overlap
                );
            }
        }
        let _ = writeln!(out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_empty_is_zeroed() {
        let by_pr: BTreeMap<String, PrTriple> = BTreeMap::new();
        let s = aggregate(&by_pr, Arm::Gepa);
        assert_eq!(s.total, 0);
        assert_eq!(s.passed, 0);
    }
}
```

- [ ] **Step 2: Implement Cmd::Aggregate**

Add `mod report;` to `main.rs`. Replace the `Cmd::Aggregate` arm:

```rust
Cmd::Aggregate { run_date } => {
    let dir = std::path::PathBuf::from(format!(
        "benchmark/genskill/ladybird/reports/{run_date}"
    ));
    let entries = pr_corpus::load_corpus(std::path::Path::new(
        "benchmark/genskill/ladybird/pr_corpus",
    ))?;
    let mut by_pr: std::collections::BTreeMap<String, report::PrTriple> =
        std::collections::BTreeMap::new();
    for entry in &entries {
        let mut triple: report::PrTriple = std::collections::BTreeMap::new();
        for arm in [replay::Arm::NoSkill, replay::Arm::Direct, replay::Arm::Gepa] {
            let path = dir.join(format!("{}-{:?}.json", entry.id, arm));
            if !path.exists() {
                continue;
            }
            let json = std::fs::read_to_string(&path)?;
            let artifact: replay::ReplayArtifact = serde_json::from_str(&json)?;
            let reference_fix = std::fs::read_to_string(entry.dir.join("reference_fix.patch"))
                .unwrap_or_default();
            let m = metrics::compute(&artifact, &reference_fix);
            triple.insert(arm, m);
        }
        if !triple.is_empty() {
            by_pr.insert(entry.id.clone(), triple);
        }
    }
    let md = report::render_summary(&run_date, &by_pr);
    let out_path = dir.join("summary.md");
    std::fs::write(&out_path, &md)?;
    println!("{md}");
    println!("\n(saved to {})", out_path.display());
    Ok(())
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p puffer-genskill-eval`
Expected: 14 tests pass.

- [ ] **Step 4: Commit**

```bash
git add benchmark/genskill/ladybird/
git commit -m "feat(eval): implement aggregation and markdown report

report::render_summary produces the headline + per-PR comparison
table. aggregate() rolls per-PR ReplayMetrics into ArmSummary across
the corpus. Cmd::Aggregate loads every artifact in a run_date dir,
joins with reference_fix.patch from the corpus to compute metrics,
and writes summary.md alongside the artifacts.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase E — End-to-End Run

## Task 12: Drive a full benchmark run + publish first report

**Files:**
- Create: `benchmark/genskill/ladybird/scripts/run_all.sh`
- Create: `benchmark/genskill/ladybird/reports/published/<date>/summary.md`

- [ ] **Step 1: Write the orchestration script**

Create `benchmark/genskill/ladybird/scripts/run_all.sh`:

```bash
#!/usr/bin/env bash
# Runs all 30 replays (10 PRs × 3 arms) sequentially and aggregates.
#
# Usage: run_all.sh [run_date]
#
# Sequential, not parallel: keeps Docker resource usage predictable
# and makes log inspection easy. Each replay is self-contained.

set -euo pipefail

RUN_DATE="${1:-$(date -u +%Y-%m-%d)}"
ROOT="benchmark/genskill/ladybird"

cargo build -p puffer-genskill-eval --release

cargo run --release -p puffer-genskill-eval -- validate

for pr_dir in "$ROOT"/pr_corpus/pr-*/; do
  pr=$(basename "$pr_dir")
  for arm in no-skill direct gepa; do
    echo "=== $pr / $arm ==="
    cargo run --release -p puffer-genskill-eval -- replay "$pr" "$arm" \
      || echo "(replay failed; continuing)"
  done
done

cargo run --release -p puffer-genskill-eval -- aggregate "$RUN_DATE"
```

- [ ] **Step 2: Run it**

```bash
bash benchmark/genskill/ladybird/scripts/run_all.sh
```

This is the full benchmark run: 30 replays × ≤30 min each. Plan for ~5-10 hours wall time depending on PR difficulty.

Watch the per-replay output for unexpected failures (Docker issues, puffer crashes). If a single replay fails repeatedly, investigate and re-run only that one.

- [ ] **Step 3: Inspect the report**

```bash
cat benchmark/genskill/ladybird/reports/<date>/summary.md
```

Sanity check:
- All 10 PRs appear in the per-PR section
- Pass counts add up
- GEPA ideally ≥ direct on most metrics (this is the hypothesis under test; if not, that's also a valid result and the spec calls for honest reporting)

- [ ] **Step 4: Promote the report to published/**

```bash
date_dir=$(date -u +%Y-%m-%d)
mkdir -p benchmark/genskill/ladybird/reports/published/$date_dir
cp benchmark/genskill/ladybird/reports/$date_dir/summary.md \
   benchmark/genskill/ladybird/reports/published/$date_dir/summary.md
```

- [ ] **Step 5: Commit the published report**

```bash
git add benchmark/genskill/ladybird/scripts/run_all.sh \
        benchmark/genskill/ladybird/reports/published/
git commit -m "feat(eval): publish first /genskill benchmark report

run_all.sh orchestrates the full 30-replay benchmark sequentially.
First report at reports/published/<date>/summary.md compares no-skill
vs direct vs gepa across the 10-PR corpus on pass rate, tokens,
tool calls, duplicate work, file overlap, and symbol overlap.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Final Verification

- [ ] **Step 1: All tests pass**

Run: `cargo test -p puffer-genskill-eval`
Expected: 14 tests pass.

- [ ] **Step 2: Eval crate builds clean**

Run: `cargo build -p puffer-genskill-eval --release`
Expected: builds with no new warnings.

- [ ] **Step 3: Corpus validates**

Run: `cargo run -p puffer-genskill-eval -- validate`
Expected: `OK: 10 entries`.

- [ ] **Step 4: First report published**

Run: `ls benchmark/genskill/ladybird/reports/published/`
Expected: at least one date directory with `summary.md`.

- [ ] **Step 5: Push branch and (optionally) open PR**

```bash
git push -u origin feat/genskill   # or appropriate branch
```

Open PR via:

```bash
gh pr create --title "feat: /genskill Ladybird PR replay eval (Plan 3)" --body "$(cat <<'EOF'
## Summary

Implements Plan 3 from `docs/superpowers/specs/2026-05-07-genskill-design.md`:
the Ladybird PR replay benchmark for evaluating /genskill against direct prompting.

- New crate `puffer-genskill-eval` at `benchmark/genskill/ladybird/`
- 10 hand-curated Ladybird PRs as the frozen corpus (`pr_corpus/`)
- Docker sandbox per replay, three arms (no-skill / direct / gepa)
- Six metrics: pass rate, tokens, tool calls, duplicate work, file overlap, symbol overlap
- First published report at `reports/published/<date>/summary.md`

Black-box harness: zero dependency on `puffer-core` or `puffer-skill-evolution`.
Calls the `puffer` binary as a subprocess.

## Test plan

- [ ] `cargo test -p puffer-genskill-eval` passes (14 tests)
- [ ] `cargo run -p puffer-genskill-eval -- validate` reports 10 corpus entries
- [ ] First published `summary.md` shows GEPA-vs-direct comparison

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-Review Notes

- **Spec coverage:** Tasks 1-4 cover §3 (corpus). Task 5 covers §3.4 (expert runs). Task 6 covers §4 (skill generation). Tasks 7-9 cover §5 (replay mechanics). Task 10 covers §6 (metrics). Task 11 covers §7 (reporting). Task 12 closes §13 (end-to-end first run).
- **Placeholder scan:** Task 6 has placeholder flag names for puffer's non-interactive mode (`--load-transcript`, `--run-command`, `--output`). The plan instructs the executor to adapt to whatever puffer's actual surface is (or scope a tiny pre-req to add the missing flags). This is real-codebase discovery, not a placeholder.
- **Type consistency:** `Arm`, `Outcome`, `ReplayArtifact`, `ReplayMetrics`, `PrTriple` are all defined before being used in later tasks. The `Arm::Pretty` printing via `{:?}` produces `NoSkill`/`Direct`/`Gepa` — fine for filenames but if a more readable name is needed, add a `Display` impl in Task 9.
- **Cost reality check:** First end-to-end run (Task 12) is the most expensive single step. Estimate ~$35-70 in API tokens plus several hours wall time. If the user wants to pilot at lower cost, run only 3 PRs first by editing `run_all.sh` to limit the loop.
