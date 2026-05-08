# /genskill Core GEPA Implementation Plan (Plan 1 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a working `/genskill` slash command in puffer that generates a reusable SKILL.md from the current conversation transcript using a GEPA-style loop (multi-candidate generation, LLM-as-judge scoring, Pareto selection, mutation rounds).

**Architecture:** New Rust crate `puffer-skill-evolution` exposes `run_gepa()` that orchestrates the loop; `puffer-core` registers `/genskill` as a local command that builds an `ExecutionTrace` from `AppState::transcript`, calls `run_gepa()`, and writes the result to `resources/skills/<name>/SKILL.md`. Sub-agents are spawned via the existing `runtime:agent` tool; for testability, the crate uses an `AgentRuntime` trait so tests can mock LLM calls.

**Tech Stack:** Rust 1.95.0, Cargo workspace, `tokio` async, `serde`/`serde_yaml` for frontmatter, `anyhow` for errors, `async-trait` for the runtime abstraction.

**Out of scope (Plans 2 and 3):** Curator system, usage tracking in `puffer-session-store`, benchmark harness in `benchmark/genskill/`. This plan only ships the generation path.

**Spec reference:** `docs/superpowers/specs/2026-05-07-genskill-design.md`

**Branch:** `feat/genskill` (already created; spec already committed at `98193fa`)

**Repo constraints (from `AGENTS.md`):**
- No source file > 1000 lines
- Every public Rust function has a doc comment (`///`)
- ASCII unless existing reason otherwise
- Skill files ≤ 15KB; description ≤ 1024 chars

**Pre-existing master state:** `cargo build --workspace` fails on `matrix-sdk` (query depth overflow in `puffer-connector-matrix`); `cargo test -p puffer-core` has 73 failures from a YAML parse error in `tools/browser.yaml`. **These are pre-existing — do not attempt to fix.** Run tests scoped to crates we touch: `cargo test -p puffer-skill-evolution -p puffer-core -p puffer-resources`.

---

## File Structure

**New crate `crates/puffer-skill-evolution/`** (note: puffer convention is files at crate root, not `src/`):
- `Cargo.toml` — manifest
- `lib.rs` — public API: `run_gepa`, types, `AgentRuntime` trait
- `trace.rs` — extract `ExecutionTrace` from transcript events
- `generate.rs` — spawn N parallel agent calls with `generate.md`
- `judge.rs` — call runtime with `judge.md` for scoring
- `pareto.rs` — non-dominated frontier selection (pure)
- `mutate.rs` — build mutation prompts seeded from survivors

**Edits to existing crates:**
- `Cargo.toml` (workspace root) — add `puffer-skill-evolution` to members
- `crates/puffer-core/Cargo.toml` — add `puffer-skill-evolution` dependency
- `crates/puffer-core/command.rs` — register `/genskill` in `supported_commands()`
- `crates/puffer-core/command_helpers/mod.rs` — add `handle_genskill_command` handler
- `crates/puffer-core/command.rs` (or wherever `dispatch_command` lives) — wire `/genskill` → `handle_genskill_command`

**New resources:**
- `resources/skills/genskill/SKILL.md` — metadata + fallback (so `/skill:genskill` also works)
- `resources/skills/genskill/generate.md` — candidate generation prompt
- `resources/skills/genskill/judge.md` — rubric prompt
- `resources/skills/genskill/mutate.md` — mutation prompt

**New spec docs (per puffer convention):**
- `specs/puffer-skill-evolution/00.md` — component spec
- `specs/puffer-core/02.md` — spec update for `/genskill` registration (only if `01.md` exists; else `02.md`)

---

## Task 1: Bootstrap the new crate

**Files:**
- Create: `crates/puffer-skill-evolution/Cargo.toml`
- Create: `crates/puffer-skill-evolution/lib.rs`
- Modify: `Cargo.toml` (workspace root, add to `members`)

- [ ] **Step 1: Create crate manifest**

Create `crates/puffer-skill-evolution/Cargo.toml`:

```toml
[package]
name = "puffer-skill-evolution"
version.workspace = true
edition.workspace = true
license.workspace = true

[lib]
path = "lib.rs"

[dependencies]
anyhow.workspace = true
async-trait = "0.1"
serde.workspace = true
serde_yaml = "0.9"
tokio = { workspace = true, features = ["sync", "macros"] }
tracing.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

- [ ] **Step 2: Create minimal lib.rs**

Create `crates/puffer-skill-evolution/lib.rs`:

```rust
//! GEPA-style skill generation from conversation traces.
//!
//! Implements the core loop for `/genskill`: multi-candidate generation,
//! LLM-as-judge scoring, Pareto selection, and mutation rounds. The
//! `AgentRuntime` trait abstracts LLM dispatch so tests can mock calls.

#![deny(missing_docs)]
```

- [ ] **Step 3: Add to workspace members**

Read the current root `Cargo.toml` and add `"crates/puffer-skill-evolution"` to the `members` list (alphabetically sorted with the others).

- [ ] **Step 4: Verify it builds**

Run: `source ~/.cargo/env && cargo build -p puffer-skill-evolution`
Expected: `Finished` with no warnings about missing docs (the `#![deny(missing_docs)]` should pass since lib.rs has no public items yet).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/puffer-skill-evolution/
git commit -m "feat(skill-evolution): bootstrap crate with empty lib.rs

Adds puffer-skill-evolution crate to the workspace. Crate will host
the GEPA loop driving /genskill (see specs/puffer-skill-evolution/).
Empty for now; types and modules added in subsequent commits.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 2: Define core types

**Files:**
- Modify: `crates/puffer-skill-evolution/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to bottom of `crates/puffer-skill-evolution/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubric_scores_pareto_dominance() {
        let a = RubricScores { novelty: 0.8, reproducibility: 0.9, structure: 0.7, conciseness: 0.6 };
        let b = RubricScores { novelty: 0.7, reproducibility: 0.8, structure: 0.6, conciseness: 0.5 };
        assert!(a.dominates(&b));
        assert!(!b.dominates(&a));
    }

    #[test]
    fn rubric_scores_pareto_incomparable() {
        let a = RubricScores { novelty: 0.9, reproducibility: 0.5, structure: 0.7, conciseness: 0.6 };
        let b = RubricScores { novelty: 0.5, reproducibility: 0.9, structure: 0.7, conciseness: 0.6 };
        assert!(!a.dominates(&b));
        assert!(!b.dominates(&a));
    }

    #[test]
    fn rubric_scores_total() {
        let s = RubricScores { novelty: 0.5, reproducibility: 0.5, structure: 0.5, conciseness: 0.5 };
        assert!((s.total() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn gepa_options_defaults() {
        let opts = GepaOptions::default();
        assert_eq!(opts.n_candidates, 3);
        assert_eq!(opts.k_rounds, 2);
        assert_eq!(opts.max_size_bytes, 15_000);
    }
}
```

- [ ] **Step 2: Run test (should fail with compile errors)**

Run: `cargo test -p puffer-skill-evolution`
Expected: compile errors — `RubricScores`, `GepaOptions` not found.

- [ ] **Step 3: Implement the types**

Add to `lib.rs` (above the `#[cfg(test)]` block):

```rust
use serde::{Deserialize, Serialize};

/// Per-skill rubric scores produced by the LLM-as-judge pass.
///
/// Each field is in the range `[0.0, 1.0]`. A skill is "Pareto-dominated"
/// by another iff the other's score is `>=` on every dimension and `>` on
/// at least one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RubricScores {
    /// Captures non-obvious knowledge from the trace (0.0-1.0).
    pub novelty: f32,
    /// A fresh agent reading only this skill could reproduce the approach (0.0-1.0).
    pub reproducibility: f32,
    /// Has proper sections (Overview, When to Use, Pitfalls, Checklist) (0.0-1.0).
    pub structure: f32,
    /// Concise and within size budget (0.0-1.0).
    pub conciseness: f32,
}

impl RubricScores {
    /// Returns true if `self` Pareto-dominates `other`.
    pub fn dominates(&self, other: &Self) -> bool {
        let dims = [
            (self.novelty, other.novelty),
            (self.reproducibility, other.reproducibility),
            (self.structure, other.structure),
            (self.conciseness, other.conciseness),
        ];
        let all_ge = dims.iter().all(|(s, o)| s >= o);
        let any_gt = dims.iter().any(|(s, o)| s > o);
        all_ge && any_gt
    }

    /// Returns the sum of all four dimensions (used for tie-breaking).
    pub fn total(&self) -> f32 {
        self.novelty + self.reproducibility + self.structure + self.conciseness
    }
}

/// Configuration for one `/genskill` invocation.
#[derive(Debug, Clone)]
pub struct GepaOptions {
    /// Number of candidates per round.
    pub n_candidates: usize,
    /// Number of evolution rounds (post-Round-0 mutation passes).
    pub k_rounds: usize,
    /// Hard size budget for a candidate skill body.
    pub max_size_bytes: usize,
}

impl Default for GepaOptions {
    fn default() -> Self {
        Self {
            n_candidates: 3,
            k_rounds: 2,
            max_size_bytes: 15_000,
        }
    }
}

/// Structured execution trace extracted from a session transcript.
///
/// The trace is what the generation sub-agent reads as evidence of a
/// non-trivial task. It captures tool calls, outcomes, failures, and
/// the rough shape of the conversation without including raw provider
/// system prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    /// One entry per significant turn.
    pub entries: Vec<TraceEntry>,
    /// Human-readable summary of the task being attempted.
    pub task_summary: String,
}

/// One step of an `ExecutionTrace`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    /// Brief description of what happened in this step.
    pub summary: String,
    /// Names of tools called in this step (may be empty).
    pub tool_calls: Vec<String>,
    /// Whether this step succeeded (best-effort heuristic).
    pub succeeded: bool,
}

/// Frontmatter of a generated skill file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Lowercase-hyphen skill name (≤64 chars).
    pub name: String,
    /// One-line use-case trigger description (≤1024 chars).
    pub description: String,
}

/// A candidate skill produced by the generation or mutation step.
#[derive(Debug, Clone)]
pub struct SkillCandidate {
    /// Parsed frontmatter.
    pub frontmatter: SkillFrontmatter,
    /// Body text (everything after the closing `---`).
    pub body: String,
    /// Scores from the judge pass; `None` until scored.
    pub scores: Option<RubricScores>,
}

impl SkillCandidate {
    /// Total byte length of frontmatter + body when serialized.
    pub fn size_bytes(&self) -> usize {
        // Approximate; exact serialization cost computed at write time.
        self.body.len() + self.frontmatter.name.len() + self.frontmatter.description.len() + 64
    }
}
```

- [ ] **Step 4: Run test (should pass)**

Run: `cargo test -p puffer-skill-evolution`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/puffer-skill-evolution/lib.rs
git commit -m "feat(skill-evolution): add core types

Defines RubricScores (with Pareto dominance), GepaOptions (defaults
N=3, K=2, 15KB), ExecutionTrace, SkillFrontmatter, SkillCandidate.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 3: AgentRuntime trait and MockAgentRuntime

**Files:**
- Modify: `crates/puffer-skill-evolution/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` module in `lib.rs`:

```rust
    use std::sync::Mutex;

    struct CannedRuntime {
        responses: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AgentRuntime for CannedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> anyhow::Result<String> {
            let mut responses = self.responses.lock().unwrap();
            Ok(responses.remove(0))
        }
    }

    #[tokio::test]
    async fn mock_runtime_returns_canned_response() {
        let rt = CannedRuntime {
            responses: Mutex::new(vec!["hello".to_string()]),
        };
        let out = rt.invoke_agent("ignored").await.unwrap();
        assert_eq!(out, "hello");
    }
```

- [ ] **Step 2: Run (compile error: trait not defined)**

Run: `cargo test -p puffer-skill-evolution`
Expected: compile error — `AgentRuntime` not found.

- [ ] **Step 3: Define the trait**

Add to `lib.rs` above the `#[cfg(test)]` block:

```rust
/// Abstraction over LLM dispatch so generation/judge/mutate code is testable.
///
/// In production, this is implemented by a thin wrapper that calls
/// `puffer-core`'s runtime agent dispatch (the `runtime:agent` handler).
/// Tests provide a canned-response implementation.
#[async_trait::async_trait]
pub trait AgentRuntime: Send + Sync {
    /// Invokes a sub-agent with the given prompt and returns its full text response.
    ///
    /// Errors if the sub-agent invocation fails or times out. The response
    /// content is not validated by this trait — callers parse it.
    async fn invoke_agent(&self, prompt: &str) -> anyhow::Result<String>;
}
```

- [ ] **Step 4: Run test (should pass)**

Run: `cargo test -p puffer-skill-evolution`
Expected: 5 tests pass (4 prior + 1 new).

- [ ] **Step 5: Commit**

```bash
git add crates/puffer-skill-evolution/lib.rs
git commit -m "feat(skill-evolution): add AgentRuntime trait

Defines AgentRuntime trait so generation/judge/mutate logic is
testable without hitting a real LLM. Production impl wraps puffer-core's
runtime:agent handler; tests use canned responses.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 4: trace.rs — extract ExecutionTrace from transcript events

**Files:**
- Create: `crates/puffer-skill-evolution/trace.rs`
- Modify: `crates/puffer-skill-evolution/lib.rs` (declare module)

- [ ] **Step 1: Declare module in lib.rs**

Add near the top of `lib.rs`:

```rust
mod trace;
pub use trace::extract_trace;
```

- [ ] **Step 2: Create trace.rs with failing test**

Create `crates/puffer-skill-evolution/trace.rs`:

```rust
//! Extract a structured `ExecutionTrace` from a session transcript.

use crate::{ExecutionTrace, TraceEntry};

/// Minimal transcript event shape this module accepts.
///
/// We don't depend on `puffer-session-store::TranscriptEvent` directly to
/// keep this crate testable in isolation. The caller in `puffer-core`
/// converts session events into this shape.
#[derive(Debug, Clone)]
pub struct TranscriptStep {
    /// Role: "user", "assistant", "tool".
    pub role: String,
    /// Plain text content of the step.
    pub text: String,
    /// Names of tools invoked in this step (empty if none).
    pub tool_calls: Vec<String>,
    /// Whether the step indicates failure (e.g., tool error).
    pub error: bool,
}

/// Builds an `ExecutionTrace` from raw transcript steps.
///
/// Filters out empty user/assistant text turns with no tool calls,
/// keeping only steps that meaningfully advanced the task. The
/// `task_summary` is derived from the first non-empty user turn.
pub fn extract_trace(steps: &[TranscriptStep]) -> ExecutionTrace {
    let task_summary = steps
        .iter()
        .find(|s| s.role == "user" && !s.text.trim().is_empty())
        .map(|s| s.text.lines().next().unwrap_or("").to_string())
        .unwrap_or_else(|| "(no user turn found)".to_string());

    let entries = steps
        .iter()
        .filter(|s| !s.tool_calls.is_empty() || (s.role == "assistant" && !s.text.trim().is_empty()))
        .map(|s| TraceEntry {
            summary: summarize(&s.text),
            tool_calls: s.tool_calls.clone(),
            succeeded: !s.error,
        })
        .collect();

    ExecutionTrace { entries, task_summary }
}

/// One-line summary for trace display: first non-empty line, ≤200 chars.
fn summarize(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.len() > 200 {
        format!("{}…", &line[..197])
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_trace_empty_returns_placeholder() {
        let trace = extract_trace(&[]);
        assert_eq!(trace.task_summary, "(no user turn found)");
        assert!(trace.entries.is_empty());
    }

    #[test]
    fn extract_trace_filters_empty_turns() {
        let steps = vec![
            TranscriptStep {
                role: "user".into(),
                text: "Fix the build error".into(),
                tool_calls: vec![],
                error: false,
            },
            TranscriptStep {
                role: "assistant".into(),
                text: "".into(),
                tool_calls: vec![],
                error: false,
            },
            TranscriptStep {
                role: "assistant".into(),
                text: "Reading file".into(),
                tool_calls: vec!["read".into()],
                error: false,
            },
            TranscriptStep {
                role: "tool".into(),
                text: "error: file not found".into(),
                tool_calls: vec![],
                error: true,
            },
        ];
        let trace = extract_trace(&steps);
        assert_eq!(trace.task_summary, "Fix the build error");
        assert_eq!(trace.entries.len(), 1, "only the assistant turn with tool calls is kept");
        assert_eq!(trace.entries[0].tool_calls, vec!["read".to_string()]);
    }

    #[test]
    fn summarize_truncates_long_text() {
        let s = "a".repeat(300);
        let summary = summarize(&s);
        assert_eq!(summary.len(), 198); // 197 chars + ellipsis
        assert!(summary.ends_with('…'));
    }
}
```

- [ ] **Step 3: Run tests (verify pass)**

Run: `cargo test -p puffer-skill-evolution`
Expected: 8 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/puffer-skill-evolution/
git commit -m "feat(skill-evolution): add trace extraction

Adds trace.rs with TranscriptStep + extract_trace. Filters empty turns,
derives task_summary from first user message, summarizes long text.
Decoupled from puffer-session-store::TranscriptEvent so the crate
remains independently testable.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 5: pareto.rs — non-dominated frontier selection

**Files:**
- Create: `crates/puffer-skill-evolution/pareto.rs`
- Modify: `crates/puffer-skill-evolution/lib.rs` (declare module)

- [ ] **Step 1: Declare module + create pareto.rs with tests**

Add to `lib.rs`:

```rust
mod pareto;
pub use pareto::pareto_frontier;
```

Create `crates/puffer-skill-evolution/pareto.rs`:

```rust
//! Non-dominated (Pareto) frontier selection for scored candidates.

use crate::SkillCandidate;

/// Returns the indices of candidates on the non-dominated frontier.
///
/// A candidate is on the frontier iff no other candidate dominates it.
/// Candidates with `scores: None` are excluded. If no candidates have
/// scores, returns an empty vec.
pub fn pareto_frontier(candidates: &[SkillCandidate]) -> Vec<usize> {
    let mut frontier = Vec::new();
    for (i, cand) in candidates.iter().enumerate() {
        let Some(scores_i) = cand.scores else {
            continue;
        };
        let dominated = candidates.iter().enumerate().any(|(j, other)| {
            if i == j {
                return false;
            }
            other.scores.map_or(false, |s| s.dominates(&scores_i))
        });
        if !dominated {
            frontier.push(i);
        }
    }
    frontier
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RubricScores, SkillFrontmatter};

    fn make_candidate(scores: Option<RubricScores>) -> SkillCandidate {
        SkillCandidate {
            frontmatter: SkillFrontmatter {
                name: "test".into(),
                description: "test".into(),
            },
            body: String::new(),
            scores,
        }
    }

    #[test]
    fn frontier_excludes_unscored() {
        let cands = vec![make_candidate(None), make_candidate(None)];
        assert!(pareto_frontier(&cands).is_empty());
    }

    #[test]
    fn frontier_keeps_dominator_only() {
        let dominator = RubricScores { novelty: 0.9, reproducibility: 0.9, structure: 0.9, conciseness: 0.9 };
        let weak = RubricScores { novelty: 0.5, reproducibility: 0.5, structure: 0.5, conciseness: 0.5 };
        let cands = vec![make_candidate(Some(dominator)), make_candidate(Some(weak))];
        assert_eq!(pareto_frontier(&cands), vec![0]);
    }

    #[test]
    fn frontier_keeps_incomparable() {
        let a = RubricScores { novelty: 0.9, reproducibility: 0.5, structure: 0.7, conciseness: 0.7 };
        let b = RubricScores { novelty: 0.5, reproducibility: 0.9, structure: 0.7, conciseness: 0.7 };
        let cands = vec![make_candidate(Some(a)), make_candidate(Some(b))];
        let frontier = pareto_frontier(&cands);
        assert_eq!(frontier.len(), 2);
        assert!(frontier.contains(&0));
        assert!(frontier.contains(&1));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p puffer-skill-evolution`
Expected: 11 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/puffer-skill-evolution/
git commit -m "feat(skill-evolution): add Pareto frontier selection

Pure function pareto_frontier returns indices of non-dominated
candidates. Excludes unscored candidates. Tested against dominator,
weak, and incomparable cases.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 6: Frontmatter parser

**Files:**
- Create: `crates/puffer-skill-evolution/parse.rs`
- Modify: `crates/puffer-skill-evolution/lib.rs` (declare module, re-export)

- [ ] **Step 1: Declare module in lib.rs**

Add to `lib.rs`:

```rust
mod parse;
pub use parse::parse_skill_md;
```

- [ ] **Step 2: Create parse.rs with tests**

Create `crates/puffer-skill-evolution/parse.rs`:

```rust
//! Parse a `SKILL.md` blob (frontmatter + body) into a `SkillCandidate`.

use crate::{SkillCandidate, SkillFrontmatter};
use anyhow::{anyhow, Context, Result};

/// Parses a SKILL.md document into frontmatter and body.
///
/// The document must start with `---\n`, contain a YAML frontmatter
/// block, then `\n---\n`, then the body. Returns an error if the
/// frontmatter is malformed or required fields are missing.
pub fn parse_skill_md(text: &str) -> Result<SkillCandidate> {
    let trimmed = text.trim_start();
    let rest = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
        .ok_or_else(|| anyhow!("missing opening --- delimiter"))?;
    let end = rest
        .find("\n---\n")
        .or_else(|| rest.find("\n---\r\n"))
        .ok_or_else(|| anyhow!("missing closing --- delimiter"))?;
    let frontmatter_text = &rest[..end];
    let body_offset = end + "\n---\n".len();
    let body = rest.get(body_offset..).unwrap_or("").trim_start().to_string();

    let frontmatter: SkillFrontmatter = serde_yaml::from_str(frontmatter_text)
        .context("parsing skill frontmatter as YAML")?;

    if frontmatter.name.is_empty() {
        return Err(anyhow!("frontmatter `name` is empty"));
    }
    if frontmatter.name.len() > 64 {
        return Err(anyhow!("frontmatter `name` exceeds 64 chars"));
    }
    if frontmatter.description.len() > 1024 {
        return Err(anyhow!("frontmatter `description` exceeds 1024 chars"));
    }

    Ok(SkillCandidate {
        frontmatter,
        body,
        scores: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_skill() {
        let doc = "---\nname: my-skill\ndescription: Use when debugging X\n---\nBody text here.";
        let cand = parse_skill_md(doc).unwrap();
        assert_eq!(cand.frontmatter.name, "my-skill");
        assert_eq!(cand.frontmatter.description, "Use when debugging X");
        assert_eq!(cand.body, "Body text here.");
    }

    #[test]
    fn parse_missing_opening_delimiter() {
        let doc = "name: my-skill\n---\nBody";
        assert!(parse_skill_md(doc).is_err());
    }

    #[test]
    fn parse_missing_closing_delimiter() {
        let doc = "---\nname: my-skill\nBody";
        assert!(parse_skill_md(doc).is_err());
    }

    #[test]
    fn parse_empty_name_rejected() {
        let doc = "---\nname: \"\"\ndescription: x\n---\nBody";
        assert!(parse_skill_md(doc).is_err());
    }

    #[test]
    fn parse_long_name_rejected() {
        let long = "a".repeat(65);
        let doc = format!("---\nname: {}\ndescription: x\n---\nBody", long);
        assert!(parse_skill_md(&doc).is_err());
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p puffer-skill-evolution`
Expected: 16 tests pass (5 new).

- [ ] **Step 4: Commit**

```bash
git add crates/puffer-skill-evolution/
git commit -m "feat(skill-evolution): add SKILL.md frontmatter parser

parse_skill_md returns SkillCandidate from a frontmatter+body string.
Validates name (non-empty, ≤64 chars) and description (≤1024 chars).
Used by generation and mutation pipelines to validate sub-agent output.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 7: judge.rs — score candidates via runtime

**Files:**
- Create: `crates/puffer-skill-evolution/judge.rs`
- Modify: `crates/puffer-skill-evolution/lib.rs`

- [ ] **Step 1: Declare module + write tests**

Add to `lib.rs`:

```rust
mod judge;
pub use judge::{score_candidate, JudgePromptBuilder};
```

Create `crates/puffer-skill-evolution/judge.rs`:

```rust
//! LLM-as-judge scoring of skill candidates.

use crate::{AgentRuntime, RubricScores, SkillCandidate};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

/// Builds the prompt sent to the judge sub-agent.
///
/// Implementations inject the rubric template plus the candidate text;
/// the default impl uses a hard-coded template (later replaced by the
/// `judge.md` resource at runtime in `puffer-core`).
pub trait JudgePromptBuilder: Send + Sync {
    /// Returns a complete prompt for one judging call.
    fn build(&self, candidate: &SkillCandidate) -> String;
}

/// Default judge prompt builder using an inline template.
pub struct DefaultJudgePrompt;

impl JudgePromptBuilder for DefaultJudgePrompt {
    fn build(&self, candidate: &SkillCandidate) -> String {
        format!(
            "You are an LLM judge scoring a generated SKILL.md against four dimensions.\n\
             For each dimension, return a float in [0.0, 1.0]:\n\
             - novelty: captures non-obvious knowledge\n\
             - reproducibility: a fresh agent could reproduce the approach\n\
             - structure: proper sections (overview, when-to-use, pitfalls, checklist)\n\
             - conciseness: stays within budget without fluff\n\n\
             Reply ONLY with a JSON object:\n\
             {{\"novelty\":0.x,\"reproducibility\":0.x,\"structure\":0.x,\"conciseness\":0.x}}\n\n\
             SKILL FRONTMATTER:\nname: {name}\ndescription: {desc}\n\n\
             SKILL BODY:\n{body}\n",
            name = candidate.frontmatter.name,
            desc = candidate.frontmatter.description,
            body = candidate.body,
        )
    }
}

#[derive(Deserialize)]
struct JudgeReply {
    novelty: f32,
    reproducibility: f32,
    structure: f32,
    conciseness: f32,
}

/// Scores one candidate by invoking the runtime with the judge prompt.
///
/// Retries once on malformed JSON. Returns `(0,0,0,0)` if both attempts
/// fail to parse — the candidate stays in the pool but won't make the
/// frontier.
pub async fn score_candidate<R: AgentRuntime + ?Sized>(
    runtime: &R,
    builder: &dyn JudgePromptBuilder,
    candidate: &SkillCandidate,
) -> Result<RubricScores> {
    let prompt = builder.build(candidate);
    let mut last_err = None;
    for _ in 0..2 {
        let raw = runtime.invoke_agent(&prompt).await.context("judge invocation")?;
        match parse_scores(&raw) {
            Ok(s) => return Ok(s),
            Err(e) => last_err = Some(e),
        }
    }
    tracing::warn!(?last_err, "judge produced malformed scores after retry; defaulting to 0");
    Ok(RubricScores { novelty: 0.0, reproducibility: 0.0, structure: 0.0, conciseness: 0.0 })
}

fn parse_scores(raw: &str) -> Result<RubricScores> {
    let start = raw.find('{').ok_or_else(|| anyhow!("no JSON object in judge reply"))?;
    let end = raw.rfind('}').ok_or_else(|| anyhow!("no closing brace in judge reply"))?;
    let json = &raw[start..=end];
    let reply: JudgeReply = serde_json::from_str(json).context("parsing judge JSON")?;
    Ok(RubricScores {
        novelty: clamp01(reply.novelty),
        reproducibility: clamp01(reply.reproducibility),
        structure: clamp01(reply.structure),
        conciseness: clamp01(reply.conciseness),
    })
}

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SkillFrontmatter;
    use std::sync::Mutex;

    struct FixedRuntime(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl AgentRuntime for FixedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> Result<String> {
            Ok(self.0.lock().unwrap().remove(0))
        }
    }

    fn dummy_candidate() -> SkillCandidate {
        SkillCandidate {
            frontmatter: SkillFrontmatter {
                name: "test".into(),
                description: "test".into(),
            },
            body: "body".into(),
            scores: None,
        }
    }

    #[tokio::test]
    async fn score_parses_clean_json() {
        let rt = FixedRuntime(Mutex::new(vec![
            r#"{"novelty":0.8,"reproducibility":0.9,"structure":0.7,"conciseness":0.6}"#.to_string()
        ]));
        let scores = score_candidate(&rt, &DefaultJudgePrompt, &dummy_candidate()).await.unwrap();
        assert!((scores.novelty - 0.8).abs() < 1e-6);
        assert!((scores.reproducibility - 0.9).abs() < 1e-6);
    }

    #[tokio::test]
    async fn score_extracts_json_from_chatter() {
        let rt = FixedRuntime(Mutex::new(vec![
            r#"Here are my scores: {"novelty":0.5,"reproducibility":0.5,"structure":0.5,"conciseness":0.5} done."#.to_string()
        ]));
        let scores = score_candidate(&rt, &DefaultJudgePrompt, &dummy_candidate()).await.unwrap();
        assert!((scores.total() - 2.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn score_defaults_to_zero_after_two_bad_replies() {
        let rt = FixedRuntime(Mutex::new(vec!["garbage".into(), "still garbage".into()]));
        let scores = score_candidate(&rt, &DefaultJudgePrompt, &dummy_candidate()).await.unwrap();
        assert_eq!(scores.total(), 0.0);
    }

    #[tokio::test]
    async fn score_clamps_out_of_range() {
        let rt = FixedRuntime(Mutex::new(vec![
            r#"{"novelty":1.5,"reproducibility":-0.3,"structure":0.5,"conciseness":0.5}"#.to_string()
        ]));
        let scores = score_candidate(&rt, &DefaultJudgePrompt, &dummy_candidate()).await.unwrap();
        assert_eq!(scores.novelty, 1.0);
        assert_eq!(scores.reproducibility, 0.0);
    }
}
```

- [ ] **Step 2: Add serde_json dependency**

Modify `crates/puffer-skill-evolution/Cargo.toml`, add under `[dependencies]`:

```toml
serde_json.workspace = true
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p puffer-skill-evolution`
Expected: 20 tests pass (4 new).

- [ ] **Step 4: Commit**

```bash
git add crates/puffer-skill-evolution/
git commit -m "feat(skill-evolution): add LLM-as-judge scoring

score_candidate invokes the runtime with a 4-dim rubric prompt, parses
JSON from the response (tolerant of surrounding text), retries once,
defaults to (0,0,0,0) on persistent failure. Clamps out-of-range floats.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 8: generate.rs — spawn parallel candidate-generation agents

**Files:**
- Create: `crates/puffer-skill-evolution/generate.rs`
- Modify: `crates/puffer-skill-evolution/lib.rs`

- [ ] **Step 1: Declare module + write tests**

Add to `lib.rs`:

```rust
mod generate;
pub use generate::{generate_candidates, GeneratePromptBuilder, DefaultGeneratePrompt};
```

Create `crates/puffer-skill-evolution/generate.rs`:

```rust
//! Spawn N parallel candidate-generation sub-agents.

use crate::{parse_skill_md, AgentRuntime, ExecutionTrace, SkillCandidate};
use anyhow::Result;

/// Builds the per-candidate generation prompt.
pub trait GeneratePromptBuilder: Send + Sync {
    /// Returns the prompt for one of N concurrent generation calls.
    ///
    /// `index` is the candidate's position in `[0, n)` and may be used
    /// to inject diversity hints (e.g., temperature variation).
    fn build(&self, trace: &ExecutionTrace, index: usize) -> String;
}

/// Default inline-template generation prompt builder.
pub struct DefaultGeneratePrompt;

impl GeneratePromptBuilder for DefaultGeneratePrompt {
    fn build(&self, trace: &ExecutionTrace, index: usize) -> String {
        let trace_yaml = serde_yaml::to_string(trace).unwrap_or_default();
        let style = match index % 3 {
            0 => "Be concise and procedural.",
            1 => "Emphasize edge cases and pitfalls.",
            _ => "Emphasize when-to-use triggers and examples.",
        };
        format!(
            "You are generating a reusable Hermes-style SKILL.md from an execution trace.\n\
             Output ONLY a SKILL.md document with YAML frontmatter (name, description) followed\n\
             by sections: Overview, When to Use, Topic Sections, Common Pitfalls, Verification\n\
             Checklist. Stay under 15000 bytes. Style hint: {style}\n\n\
             EXECUTION TRACE (yaml):\n{trace_yaml}\n",
            style = style,
            trace_yaml = trace_yaml,
        )
    }
}

/// Spawns N parallel generation calls; returns valid candidates.
///
/// Invalid frontmatter, oversize bodies, and runtime failures cause that
/// candidate to be discarded. Returns an error only if zero candidates
/// remain.
pub async fn generate_candidates<R: AgentRuntime + ?Sized>(
    runtime: &R,
    builder: &dyn GeneratePromptBuilder,
    trace: &ExecutionTrace,
    n: usize,
    max_size_bytes: usize,
) -> Result<Vec<SkillCandidate>> {
    let prompts: Vec<String> = (0..n).map(|i| builder.build(trace, i)).collect();
    let mut handles = Vec::new();
    for prompt in &prompts {
        handles.push(runtime.invoke_agent(prompt));
    }
    let mut candidates = Vec::new();
    for (i, fut) in handles.into_iter().enumerate() {
        match fut.await {
            Ok(raw) => match parse_skill_md(&raw) {
                Ok(c) if c.body.len() <= max_size_bytes => candidates.push(c),
                Ok(_) => tracing::warn!(index = i, "candidate exceeded size budget"),
                Err(e) => tracing::warn!(index = i, error = %e, "invalid candidate frontmatter"),
            },
            Err(e) => tracing::warn!(index = i, error = %e, "generation runtime error"),
        }
    }
    if candidates.is_empty() {
        anyhow::bail!("all {} generation candidates failed", n);
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TraceEntry;
    use std::sync::Mutex;

    struct ScriptedRuntime(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl AgentRuntime for ScriptedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> Result<String> {
            Ok(self.0.lock().unwrap().remove(0))
        }
    }

    fn sample_trace() -> ExecutionTrace {
        ExecutionTrace {
            entries: vec![TraceEntry {
                summary: "did a thing".into(),
                tool_calls: vec!["bash".into()],
                succeeded: true,
            }],
            task_summary: "do the thing".into(),
        }
    }

    #[tokio::test]
    async fn generate_returns_valid_candidates() {
        let valid = "---\nname: foo\ndescription: bar\n---\nbody";
        let rt = ScriptedRuntime(Mutex::new(vec![
            valid.into(),
            valid.into(),
            valid.into(),
        ]));
        let cands = generate_candidates(&rt, &DefaultGeneratePrompt, &sample_trace(), 3, 15_000)
            .await
            .unwrap();
        assert_eq!(cands.len(), 3);
    }

    #[tokio::test]
    async fn generate_discards_invalid_keeps_valid() {
        let valid = "---\nname: foo\ndescription: bar\n---\nbody";
        let rt = ScriptedRuntime(Mutex::new(vec![
            "garbage".into(),
            valid.into(),
            "also garbage".into(),
        ]));
        let cands = generate_candidates(&rt, &DefaultGeneratePrompt, &sample_trace(), 3, 15_000)
            .await
            .unwrap();
        assert_eq!(cands.len(), 1);
    }

    #[tokio::test]
    async fn generate_errors_when_all_fail() {
        let rt = ScriptedRuntime(Mutex::new(vec![
            "garbage".into(),
            "garbage".into(),
            "garbage".into(),
        ]));
        let result = generate_candidates(&rt, &DefaultGeneratePrompt, &sample_trace(), 3, 15_000).await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p puffer-skill-evolution`
Expected: 23 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/puffer-skill-evolution/
git commit -m "feat(skill-evolution): add parallel candidate generation

generate_candidates spawns N concurrent runtime calls, parses each
response as SKILL.md, discards invalid/oversize candidates, errors if
all fail. Diversity comes from a 3-cycle style hint per candidate index.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 9: mutate.rs — mutation prompt + mutation generation

**Files:**
- Create: `crates/puffer-skill-evolution/mutate.rs`
- Modify: `crates/puffer-skill-evolution/lib.rs`

- [ ] **Step 1: Declare module + write tests + impl**

Add to `lib.rs`:

```rust
mod mutate;
pub use mutate::{mutate_survivors, MutatePromptBuilder, DefaultMutatePrompt};
```

Create `crates/puffer-skill-evolution/mutate.rs`:

```rust
//! Generate mutated candidates from Pareto survivors.

use crate::{parse_skill_md, AgentRuntime, ExecutionTrace, RubricScores, SkillCandidate};
use anyhow::Result;

/// Builds the mutation prompt: take a survivor, target its weakest dim, regenerate.
pub trait MutatePromptBuilder: Send + Sync {
    /// Builds the mutation prompt for one survivor.
    fn build(&self, trace: &ExecutionTrace, survivor: &SkillCandidate, weakest: &str) -> String;
}

/// Default inline-template mutation prompt builder.
pub struct DefaultMutatePrompt;

impl MutatePromptBuilder for DefaultMutatePrompt {
    fn build(&self, trace: &ExecutionTrace, survivor: &SkillCandidate, weakest: &str) -> String {
        let trace_yaml = serde_yaml::to_string(trace).unwrap_or_default();
        format!(
            "You will refine a SKILL.md draft to improve its weakest dimension: {weakest}.\n\
             Preserve the strengths of the draft (do not regress on other dimensions). Output\n\
             ONLY the revised SKILL.md (frontmatter + body), no commentary. Stay under 15000 bytes.\n\n\
             ORIGINAL TRACE (yaml):\n{trace_yaml}\n\n\
             CURRENT DRAFT:\n---\nname: {name}\ndescription: {desc}\n---\n{body}\n",
            weakest = weakest,
            trace_yaml = trace_yaml,
            name = survivor.frontmatter.name,
            desc = survivor.frontmatter.description,
            body = survivor.body,
        )
    }
}

/// Returns the name of the weakest dimension for a scored candidate.
fn weakest_dimension(scores: &RubricScores) -> &'static str {
    let pairs = [
        ("novelty", scores.novelty),
        ("reproducibility", scores.reproducibility),
        ("structure", scores.structure),
        ("conciseness", scores.conciseness),
    ];
    pairs
        .iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(name, _)| *name)
        .unwrap_or("structure")
}

/// Generates one mutated candidate per survivor.
///
/// Survivors must have `scores: Some(_)`. Invalid mutation outputs are
/// discarded silently (the survivor itself remains in the pool when this
/// function's output is unioned upstream).
pub async fn mutate_survivors<R: AgentRuntime + ?Sized>(
    runtime: &R,
    builder: &dyn MutatePromptBuilder,
    trace: &ExecutionTrace,
    survivors: &[SkillCandidate],
    max_size_bytes: usize,
) -> Result<Vec<SkillCandidate>> {
    let mut mutants = Vec::new();
    for (i, survivor) in survivors.iter().enumerate() {
        let Some(scores) = survivor.scores else { continue };
        let weakest = weakest_dimension(&scores);
        let prompt = builder.build(trace, survivor, weakest);
        match runtime.invoke_agent(&prompt).await {
            Ok(raw) => match parse_skill_md(&raw) {
                Ok(c) if c.body.len() <= max_size_bytes => mutants.push(c),
                Ok(_) => tracing::warn!(index = i, "mutant exceeded size budget"),
                Err(e) => tracing::warn!(index = i, error = %e, "invalid mutant frontmatter"),
            },
            Err(e) => tracing::warn!(index = i, error = %e, "mutation runtime error"),
        }
    }
    Ok(mutants)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SkillFrontmatter, TraceEntry};
    use std::sync::Mutex;

    struct ScriptedRuntime(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl AgentRuntime for ScriptedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> Result<String> {
            Ok(self.0.lock().unwrap().remove(0))
        }
    }

    fn make_scored(scores: RubricScores) -> SkillCandidate {
        SkillCandidate {
            frontmatter: SkillFrontmatter {
                name: "test".into(),
                description: "test".into(),
            },
            body: "body".into(),
            scores: Some(scores),
        }
    }

    fn sample_trace() -> ExecutionTrace {
        ExecutionTrace {
            entries: vec![TraceEntry {
                summary: "x".into(),
                tool_calls: vec![],
                succeeded: true,
            }],
            task_summary: "x".into(),
        }
    }

    #[test]
    fn weakest_picks_lowest_dim() {
        let s = RubricScores { novelty: 0.9, reproducibility: 0.3, structure: 0.7, conciseness: 0.8 };
        assert_eq!(weakest_dimension(&s), "reproducibility");
    }

    #[tokio::test]
    async fn mutate_returns_valid_mutants() {
        let valid = "---\nname: foo\ndescription: bar\n---\nbody";
        let rt = ScriptedRuntime(Mutex::new(vec![valid.into(), valid.into()]));
        let survivors = vec![
            make_scored(RubricScores { novelty: 0.5, reproducibility: 0.5, structure: 0.5, conciseness: 0.5 }),
            make_scored(RubricScores { novelty: 0.6, reproducibility: 0.6, structure: 0.6, conciseness: 0.6 }),
        ];
        let mutants = mutate_survivors(&rt, &DefaultMutatePrompt, &sample_trace(), &survivors, 15_000)
            .await
            .unwrap();
        assert_eq!(mutants.len(), 2);
    }

    #[tokio::test]
    async fn mutate_discards_invalid() {
        let rt = ScriptedRuntime(Mutex::new(vec!["garbage".into()]));
        let survivors = vec![make_scored(RubricScores {
            novelty: 0.5, reproducibility: 0.5, structure: 0.5, conciseness: 0.5,
        })];
        let mutants = mutate_survivors(&rt, &DefaultMutatePrompt, &sample_trace(), &survivors, 15_000)
            .await
            .unwrap();
        assert!(mutants.is_empty());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p puffer-skill-evolution`
Expected: 26 tests pass (3 new).

- [ ] **Step 3: Commit**

```bash
git add crates/puffer-skill-evolution/
git commit -m "feat(skill-evolution): add mutation generation

mutate_survivors invokes the runtime once per Pareto survivor with a
prompt instructing the sub-agent to target the survivor's weakest
dimension. Discards invalid/oversize mutants silently (survivor stays
alive via union in run_gepa).

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 10: lib.rs — run_gepa orchestration

**Files:**
- Modify: `crates/puffer-skill-evolution/lib.rs`

- [ ] **Step 1: Add run_gepa to lib.rs with integration test**

Append to `lib.rs` (above `#[cfg(test)]`):

```rust
use anyhow::Result;

/// Runs the full GEPA loop and returns the Pareto-best candidate.
///
/// 1. Round 0: generate N candidates from the trace.
/// 2. For each round 1..=K: score all unscored candidates; compute
///    Pareto frontier; if more rounds remain, mutate survivors and
///    union them into the pool.
/// 3. Final: from the latest frontier, return the candidate with the
///    highest reproducibility (tie-break: highest total score).
///
/// Errors if Round 0 returns zero candidates.
pub async fn run_gepa<R: AgentRuntime + ?Sized>(
    runtime: &R,
    trace: &ExecutionTrace,
    opts: &GepaOptions,
    generate_builder: &dyn generate::GeneratePromptBuilder,
    judge_builder: &dyn judge::JudgePromptBuilder,
    mutate_builder: &dyn mutate::MutatePromptBuilder,
) -> Result<SkillCandidate> {
    // Round 0
    let mut pool = generate::generate_candidates(
        runtime,
        generate_builder,
        trace,
        opts.n_candidates,
        opts.max_size_bytes,
    )
    .await?;

    let mut frontier_indices: Vec<usize> = Vec::new();
    for round in 0..=opts.k_rounds {
        // Score any unscored candidates
        for cand in pool.iter_mut() {
            if cand.scores.is_none() {
                let scores = judge::score_candidate(runtime, judge_builder, cand).await?;
                cand.scores = Some(scores);
            }
        }
        frontier_indices = pareto::pareto_frontier(&pool);
        tracing::info!(round, pool = pool.len(), frontier = frontier_indices.len(), "GEPA round complete");

        if round < opts.k_rounds {
            let survivors: Vec<SkillCandidate> = frontier_indices.iter().map(|&i| pool[i].clone()).collect();
            let mutants = mutate::mutate_survivors(runtime, mutate_builder, trace, &survivors, opts.max_size_bytes).await?;
            pool.extend(mutants);
        }
    }

    select_best(&pool, &frontier_indices)
}

/// Picks the best candidate from the frontier: highest reproducibility, tiebreak by total.
fn select_best(pool: &[SkillCandidate], frontier: &[usize]) -> Result<SkillCandidate> {
    let mut best: Option<&SkillCandidate> = None;
    for &i in frontier {
        let cand = &pool[i];
        let Some(scores) = cand.scores else { continue };
        match best {
            None => best = Some(cand),
            Some(b) => {
                let bs = b.scores.unwrap();
                let prefer = scores.reproducibility > bs.reproducibility
                    || (scores.reproducibility == bs.reproducibility && scores.total() > bs.total());
                if prefer {
                    best = Some(cand);
                }
            }
        }
    }
    best.cloned().ok_or_else(|| anyhow::anyhow!("frontier empty after GEPA loop"))
}

// Required: clone for SkillCandidate so survivors can be carried forward
impl Clone for SkillCandidate {
    fn clone(&self) -> Self {
        Self {
            frontmatter: self.frontmatter.clone(),
            body: self.body.clone(),
            scores: self.scores,
        }
    }
}
```

Note: also remove the `#[derive(Clone)]` line for `SkillCandidate` if it exists — we're providing a manual impl.

- [ ] **Step 2: Add integration test**

In the `#[cfg(test)]` block at the bottom of `lib.rs`, add:

```rust
    use crate::generate::DefaultGeneratePrompt;
    use crate::judge::DefaultJudgePrompt;
    use crate::mutate::DefaultMutatePrompt;
    use std::sync::Mutex;

    /// Scripts a runtime with: 3 generation responses, 3 score replies,
    /// 1 mutation response, 1 score reply.
    struct OrchestratedRuntime {
        responses: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AgentRuntime for OrchestratedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> anyhow::Result<String> {
            Ok(self.responses.lock().unwrap().remove(0))
        }
    }

    #[tokio::test]
    async fn run_gepa_returns_best_from_frontier() {
        let valid = "---\nname: foo\ndescription: bar\n---\nbody";
        let rt = OrchestratedRuntime {
            responses: Mutex::new(vec![
                // Round 0 generation × 3
                valid.into(), valid.into(), valid.into(),
                // Round 0 judge × 3 (one dominates)
                r#"{"novelty":0.9,"reproducibility":0.9,"structure":0.9,"conciseness":0.9}"#.into(),
                r#"{"novelty":0.5,"reproducibility":0.5,"structure":0.5,"conciseness":0.5}"#.into(),
                r#"{"novelty":0.4,"reproducibility":0.4,"structure":0.4,"conciseness":0.4}"#.into(),
                // Round 1 mutation × 1 (only one survivor on frontier)
                valid.into(),
                // Round 1 judge × 1 mutant (Round 0 winner is already scored)
                r#"{"novelty":0.95,"reproducibility":0.95,"structure":0.95,"conciseness":0.95}"#.into(),
                // Round 2: no judging needed (already scored), no mutation (k=1 above)
            ]),
        };
        let trace = ExecutionTrace {
            entries: vec![TraceEntry { summary: "s".into(), tool_calls: vec![], succeeded: true }],
            task_summary: "t".into(),
        };
        let opts = GepaOptions { n_candidates: 3, k_rounds: 1, max_size_bytes: 15_000 };
        let best = run_gepa(&rt, &trace, &opts, &DefaultGeneratePrompt, &DefaultJudgePrompt, &DefaultMutatePrompt)
            .await
            .unwrap();
        let scores = best.scores.unwrap();
        // Mutant should win: higher total than the 0.9-each Round-0 winner
        assert!((scores.total() - 3.8).abs() < 1e-3);
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p puffer-skill-evolution`
Expected: 27 tests pass (1 new). If `Clone` derive conflicts, remove `#[derive(Clone)]` from `SkillCandidate`'s definition.

- [ ] **Step 4: Commit**

```bash
git add crates/puffer-skill-evolution/lib.rs
git commit -m "feat(skill-evolution): add run_gepa orchestration

Drives the full loop: Round 0 generation, K rounds of (score → Pareto
→ mutate). select_best picks highest reproducibility from the final
frontier, tiebreak by total score. Integration test verifies the
mutant beats Round-0 winner end-to-end with scripted runtime.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 11: Author the four prompt resources

**Files:**
- Create: `resources/skills/genskill/SKILL.md`
- Create: `resources/skills/genskill/generate.md`
- Create: `resources/skills/genskill/judge.md`
- Create: `resources/skills/genskill/mutate.md`

- [ ] **Step 1: Write SKILL.md (also enables /skill:genskill)**

Create `resources/skills/genskill/SKILL.md`:

```markdown
---
name: genskill
description: Generate a reusable skill from the current conversation history using a GEPA-style multi-candidate loop with LLM-as-judge Pareto selection.
disable-model-invocation: false
---

Generate a reusable SKILL.md from the conversation transcript so far.

Treat the transcript as evidence of a non-trivial task. Extract:
- novel knowledge that would surprise a fresh agent
- edge cases hit during the task
- domain knowledge reconstructed during the work
- the approach in a form a fresh agent could reproduce

Output ONLY a SKILL.md document with YAML frontmatter (name, description)
followed by sections: Overview, When to Use, Topic Sections, Common Pitfalls,
Verification Checklist. Stay under 15000 bytes.
```

- [ ] **Step 2: Write generate.md**

Create `resources/skills/genskill/generate.md`:

```markdown
You are generating one of N candidate SKILL.md files from an execution trace.

The trace is provided as YAML at the end of this prompt. Treat it as evidence
of a non-trivial task that a future agent might want to redo. Your job is to
distill it into a reusable skill document.

Output ONLY a SKILL.md document with this structure:

```
---
name: <lowercase-hyphen-name, ≤64 chars>
description: <one-line use-case trigger, ≤1024 chars>
---

# <Title>

## Overview
<2-4 sentences on what this skill does>

## When to Use
<bullet list of triggers>

## <Topic Sections>
<the substantive content — what was learned, how to do it>

## Common Pitfalls
<bullet list of edge cases>

## Verification Checklist
<bullet list to confirm the skill worked>
```

Constraints:
- Stay under 15000 bytes total
- ASCII only
- No commentary outside the SKILL.md document

EXECUTION TRACE:
{{TRACE_YAML}}
```

- [ ] **Step 3: Write judge.md**

Create `resources/skills/genskill/judge.md`:

```markdown
You are an LLM judge scoring a generated SKILL.md against four dimensions.

For each dimension, return a float in [0.0, 1.0]:
- novelty: captures non-obvious knowledge from the original task
- reproducibility: a fresh agent reading only this skill could reproduce the approach
- structure: has Overview, When to Use, Topic Sections, Pitfalls, Verification Checklist
- conciseness: stays within budget without filler

Reply ONLY with a single JSON object on one line:

{"novelty":0.x,"reproducibility":0.x,"structure":0.x,"conciseness":0.x}

No commentary, no markdown fences, no extra text.

SKILL FRONTMATTER:
name: {{NAME}}
description: {{DESCRIPTION}}

SKILL BODY:
{{BODY}}
```

- [ ] **Step 4: Write mutate.md**

Create `resources/skills/genskill/mutate.md`:

```markdown
You are refining a SKILL.md draft to improve its weakest dimension: {{WEAKEST_DIMENSION}}.

Preserve the strengths of the draft. Do not regress on the other dimensions
(novelty, reproducibility, structure, conciseness — whichever is not the
weakest). Output ONLY the revised SKILL.md (frontmatter + body), no commentary.
Stay under 15000 bytes. ASCII only.

ORIGINAL TRACE:
{{TRACE_YAML}}

CURRENT DRAFT:
---
name: {{NAME}}
description: {{DESCRIPTION}}
---
{{BODY}}
```

- [ ] **Step 5: Commit**

```bash
git add resources/skills/genskill/
git commit -m "feat(skill-evolution): add /genskill prompt resources

Authors SKILL.md (skill: discoverable / fallback prompt), generate.md
(candidate generation), judge.md (LLM-as-judge rubric), and mutate.md
(weakest-dimension refinement). Templates use {{TOKEN}} placeholders
the puffer-core handler will substitute at runtime.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 12: Wire `/genskill` into `puffer-core`

**Files:**
- Modify: `crates/puffer-core/Cargo.toml` (add dependency)
- Modify: `crates/puffer-core/command.rs` (register `/genskill`)
- Create: `crates/puffer-core/command_helpers/genskill.rs` (handler)
- Modify: `crates/puffer-core/command_helpers/mod.rs` (export handler)
- Modify: wherever `dispatch_command` matches command names (likely `command.rs`) — wire `"genskill"` → `handle_genskill_command`

**Note:** before this task, run `grep -n "fn dispatch_command\|match cmd\.name" crates/puffer-core/command.rs | head -5` to find the exact dispatch site. The exact wiring code depends on what you find there.

- [ ] **Step 1: Add dependency**

In `crates/puffer-core/Cargo.toml`, add under `[dependencies]`:

```toml
puffer-skill-evolution = { path = "../puffer-skill-evolution" }
```

- [ ] **Step 2: Register the command**

In `crates/puffer-core/command.rs`, inside `supported_commands()`, add (alphabetically):

```rust
        cmd(
            "genskill",
            &[],
            "Generate a reusable skill from the current conversation",
            Some("[--candidates N] [--rounds K]"),
            CommandKind::Local,
        ),
```

- [ ] **Step 3: Create the handler stub + test**

Create `crates/puffer-core/command_helpers/genskill.rs`:

```rust
//! Handler for the `/genskill` slash command.
//!
//! Builds an `ExecutionTrace` from the current `AppState::transcript`,
//! invokes `puffer-skill-evolution::run_gepa`, and writes the resulting
//! SKILL.md to `resources/skills/<name>/SKILL.md`.

use crate::AppState;
use anyhow::Result;
use puffer_skill_evolution::{
    run_gepa, AgentRuntime, ExecutionTrace, GepaOptions, TraceEntry,
};

/// Implementation of the `/genskill` local command.
///
/// Reads optional `--candidates N` and `--rounds K` flags from `args`.
/// On success, returns a user-facing message containing the path to the
/// newly written skill file.
pub async fn handle_genskill_command(
    state: &mut AppState,
    args: &str,
) -> Result<String> {
    let opts = parse_args(args)?;
    let trace = build_trace_from_state(state)?;
    if trace.entries.len() < 5 {
        return Ok(
            "/genskill needs a substantive transcript (at least 5 tool calls). Use it after a non-trivial task.".to_string()
        );
    }
    // TODO(genskill-core): construct PufferAgentRuntime adapter (next step)
    // For now, surface a not-yet-wired message so the command is discoverable.
    Ok("/genskill: registered but runtime adapter not yet wired (see Task 13).".to_string())
}

/// Parses optional `--candidates N --rounds K` flags from raw args.
fn parse_args(args: &str) -> Result<GepaOptions> {
    let mut opts = GepaOptions::default();
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "--candidates" => {
                opts.n_candidates = tokens.get(i + 1).and_then(|t| t.parse().ok()).unwrap_or(3);
                i += 2;
            }
            "--rounds" => {
                opts.k_rounds = tokens.get(i + 1).and_then(|t| t.parse().ok()).unwrap_or(2);
                i += 2;
            }
            _ => i += 1,
        }
    }
    Ok(opts)
}

/// Builds a minimal `ExecutionTrace` from the AppState transcript.
///
/// The transcript model in `puffer-session-store::TranscriptEvent` is
/// rich; this function extracts the subset relevant for skill
/// generation (tool calls + text turns). Detailed mapping is refined
/// in Task 13 when the live runtime is wired.
fn build_trace_from_state(state: &AppState) -> Result<ExecutionTrace> {
    // Best-effort placeholder: derive from state.transcript() once that
    // accessor's exact shape is confirmed during wiring.
    let _ = state;
    Ok(ExecutionTrace {
        entries: Vec::new(),
        task_summary: "(transcript bridging deferred to Task 13)".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults() {
        let opts = parse_args("").unwrap();
        assert_eq!(opts.n_candidates, 3);
        assert_eq!(opts.k_rounds, 2);
    }

    #[test]
    fn parse_args_overrides() {
        let opts = parse_args("--candidates 5 --rounds 3").unwrap();
        assert_eq!(opts.n_candidates, 5);
        assert_eq!(opts.k_rounds, 3);
    }
}
```

- [ ] **Step 4: Wire into command_helpers/mod.rs**

Find `crates/puffer-core/command_helpers/mod.rs` (or `crates/puffer-core/command_helpers.rs` if it's flat — check first with `ls crates/puffer-core/command_helpers/`). Add:

```rust
pub mod genskill;
pub use genskill::handle_genskill_command;
```

Then re-export at the use-site referenced from `command.rs` (the existing `use crate::command_helpers::{...}` block).

- [ ] **Step 5: Wire into dispatch**

Find the dispatch site by running:
```
grep -n "match.*cmd\.name\|match name\|\"review\" =>" crates/puffer-core/command.rs | head -10
```

Add a branch matching `"genskill"` that calls `handle_genskill_command(state, args).await`. Pattern should mirror the existing `/review` or `/security-review` branch.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build -p puffer-core`
Expected: compiles. Pre-existing warnings unchanged.

- [ ] **Step 7: Run targeted tests**

Run: `cargo test -p puffer-skill-evolution -p puffer-core --lib genskill`
Expected: `parse_args_defaults` and `parse_args_overrides` pass. Pre-existing failures unchanged (not in our scope).

- [ ] **Step 8: Commit**

```bash
git add crates/puffer-core/ resources/
git commit -m "feat(core): register /genskill command + stub handler

Adds /genskill to supported_commands as a Local command. Handler
parses --candidates and --rounds flags, validates trace size, and
returns a placeholder message. Live transcript bridging + runtime
adapter wired in next commit (Task 13).

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 13: Wire live runtime adapter + transcript bridge

**Files:**
- Modify: `crates/puffer-core/command_helpers/genskill.rs`

**Note:** This task discovers two things by reading puffer-core's actual code: (1) the live agent dispatch entry point (look for `runtime:agent` handler or `AgentRunner`-like type in `crates/puffer-core/runtime/`) and (2) the actual shape of `AppState::transcript` for bridging to `TranscriptStep`.

- [ ] **Step 1: Find the live agent runtime entry**

Run:
```
grep -rn "runtime:agent\|agent_runtime\|invoke_agent\|spawn_subagent" crates/puffer-core/runtime/ | head -15
```

Identify the function that, given a prompt and current state, dispatches to the `runtime:agent` tool and returns the sub-agent's final text. This is what your adapter will wrap.

- [ ] **Step 2: Implement PufferAgentRuntime adapter**

Add to `crates/puffer-core/command_helpers/genskill.rs`:

```rust
use std::sync::Arc;

/// Production `AgentRuntime` impl that delegates to puffer's live
/// `runtime:agent` handler.
struct PufferAgentRuntime {
    // Holds whatever clonable handle the discovery in Step 1 reveals.
    // Common shape: an Arc<RuntimeContext> or similar.
    inner: Arc<crate::runtime::RuntimeContext>, // adjust based on actual type
}

#[async_trait::async_trait]
impl AgentRuntime for PufferAgentRuntime {
    async fn invoke_agent(&self, prompt: &str) -> anyhow::Result<String> {
        // Replace with the real call discovered in Step 1.
        // Expected shape: something like
        //   self.inner.dispatch_subagent(prompt).await
        // or:
        //   crate::runtime::invoke_agent_tool(&self.inner, prompt).await
        unimplemented!("wire to actual runtime entry — see Task 13 Step 1");
    }
}
```

If the discovered function exists but has a different signature, adapt accordingly. The trait only requires `(&self, &str) -> Result<String>` — anything that produces final agent text fits.

- [ ] **Step 3: Bridge AppState transcript to TranscriptStep**

Replace `build_trace_from_state` body in `genskill.rs`:

```rust
fn build_trace_from_state(state: &AppState) -> Result<ExecutionTrace> {
    let mut steps = Vec::new();
    for event in state.transcript().iter() {
        // Adapt field names to actual TranscriptEvent shape; see
        // crates/puffer-session-store/ for the source of truth.
        let role = match event.role() {
            crate::MessageRole::User => "user",
            crate::MessageRole::Assistant => "assistant",
            crate::MessageRole::Tool => "tool",
            _ => continue,
        }
        .to_string();
        let text = event.text().unwrap_or_default().to_string();
        let tool_calls = event.tool_call_names().unwrap_or_default();
        let error = event.is_error();
        steps.push(puffer_skill_evolution::trace::TranscriptStep {
            role,
            text,
            tool_calls,
            error,
        });
    }
    Ok(puffer_skill_evolution::extract_trace(&steps))
}
```

Re-export `TranscriptStep` from `puffer-skill-evolution::lib.rs`:

```rust
pub use trace::TranscriptStep;
```

If `AppState::transcript()` returns events in a different shape, adjust accessors accordingly. The end goal: a `Vec<TranscriptStep>` to feed `extract_trace`.

- [ ] **Step 4: Wire run_gepa into the handler**

Replace the `TODO(genskill-core)` block in `handle_genskill_command`:

```rust
    let runtime = PufferAgentRuntime { inner: state.runtime_context_handle() };
    let candidate = run_gepa(
        &runtime,
        &trace,
        &opts,
        &puffer_skill_evolution::DefaultGeneratePrompt,
        &puffer_skill_evolution::DefaultJudgePrompt,
        &puffer_skill_evolution::DefaultMutatePrompt,
    )
    .await?;

    let path = write_skill_to_disk(&candidate)?;
    Ok(format!("Skill written to {}", path.display()))
```

(`state.runtime_context_handle()` is illustrative — use whatever accessor the discovery in Step 1 surfaced.)

- [ ] **Step 5: Add write_skill_to_disk**

Append to `genskill.rs`:

```rust
use std::fs;
use std::path::PathBuf;
use puffer_skill_evolution::SkillCandidate;

/// Writes the chosen skill to `resources/skills/<name>/SKILL.md`.
///
/// If a skill with the same name exists, appends `-v2`, `-v3`, etc.
fn write_skill_to_disk(candidate: &SkillCandidate) -> Result<PathBuf> {
    let base = PathBuf::from("resources/skills");
    let mut name = candidate.frontmatter.name.clone();
    let mut counter = 2u32;
    while base.join(&name).exists() {
        name = format!("{}-v{}", candidate.frontmatter.name, counter);
        counter += 1;
    }
    let dir = base.join(&name);
    fs::create_dir_all(&dir)?;
    let path = dir.join("SKILL.md");
    let frontmatter = serde_yaml::to_string(&candidate.frontmatter)?;
    let content = format!("---\n{}---\n{}", frontmatter, candidate.body);
    fs::write(&path, content)?;
    Ok(path)
}
```

- [ ] **Step 6: Run targeted tests**

Run: `cargo test -p puffer-skill-evolution -p puffer-core --lib genskill`
Expected: `parse_args` tests pass; new code compiles. (Live integration test is out of scope here — covered by manual smoke test below.)

- [ ] **Step 7: Manual smoke test**

Build the CLI and run `/genskill` after a short conversation:
```
cargo build -p puffer-cli
# then in a real puffer session:
# 1. have a conversation with at least 5 tool calls
# 2. type /genskill
# 3. verify resources/skills/<generated>/SKILL.md exists
```

If the runtime adapter or transcript bridge surfaced a panic or incorrect mapping, fix the discovered field/method names and re-run.

- [ ] **Step 8: Commit**

```bash
git add crates/puffer-core/command_helpers/genskill.rs crates/puffer-skill-evolution/lib.rs
git commit -m "feat(genskill): wire live runtime + transcript bridge

PufferAgentRuntime adapts puffer-core's runtime:agent dispatcher to
the AgentRuntime trait. build_trace_from_state converts AppState
transcript events to TranscriptStep. write_skill_to_disk writes the
final candidate to resources/skills/<name>/SKILL.md with -vN suffix
on collision.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 14: Component spec and command-spec update

**Files:**
- Create: `specs/puffer-skill-evolution/00.md`
- Create: `specs/puffer-core/02.md` (only if `01.md` exists; otherwise `02.md`)

- [ ] **Step 1: Check existing puffer-core specs**

Run: `ls specs/puffer-core/`
If `00.md` and `01.md` exist, the next is `02.md`. Adjust filename accordingly.

- [ ] **Step 2: Write component spec for puffer-skill-evolution**

Create `specs/puffer-skill-evolution/00.md`:

```markdown
# puffer-skill-evolution

## Purpose

`puffer-skill-evolution` implements the GEPA-style skill generation loop
backing the `/genskill` slash command. It produces reusable SKILL.md
files from execution traces of a non-trivial task.

## Design

The crate has six modules with one clear responsibility each:

- `lib.rs` — public types (`RubricScores`, `GepaOptions`, `ExecutionTrace`,
  `SkillCandidate`), the `AgentRuntime` trait, and `run_gepa` orchestration.
- `trace.rs` — extracts `ExecutionTrace` from a `Vec<TranscriptStep>`. Decoupled
  from `puffer-session-store::TranscriptEvent` for testability.
- `parse.rs` — parses a SKILL.md blob into `SkillCandidate`, validating
  frontmatter constraints (name ≤64 chars, description ≤1024 chars).
- `pareto.rs` — pure non-dominated frontier selection over candidate scores.
- `judge.rs` — LLM-as-judge scoring via `AgentRuntime`. Tolerates JSON in
  surrounding chatter, retries once, defaults to (0,0,0,0) on persistent failure.
- `generate.rs` — spawns N parallel candidate-generation calls; discards
  invalid/oversize results; errors only when zero remain.
- `mutate.rs` — generates one mutant per Pareto survivor, instructed to target
  the survivor's weakest dimension while preserving strengths.

## Logic

`run_gepa` runs Round 0 generation, then K rounds of:

1. Score any unscored candidates.
2. Compute the Pareto frontier.
3. If `r < K`: mutate the frontier and union mutants into the pool.

After the final round, `select_best` picks the frontier candidate with
the highest reproducibility (tiebreak: total score).

## Contract

`AgentRuntime` is the testability seam: production code wraps puffer-core's
live runtime; tests provide canned responses. The trait is intentionally
narrow: `invoke_agent(&self, prompt: &str) -> Result<String>`.

`run_gepa` errors only when Round 0 returns zero candidates (every
generation attempt failed validation). Subsequent rounds tolerate
mutation and judging failures gracefully.

`SkillCandidate::scores` is `Option<RubricScores>` — `None` means
"not yet judged". `pareto_frontier` excludes unscored candidates.

## Architecture Details

The crate follows puffer's flat-file convention (no `src/` subdir).
`Cargo.toml` declares `path = "lib.rs"`. Internal modules are declared
in `lib.rs` and re-exported as needed.

The crate has no I/O dependencies — file writing is the caller's
responsibility (handler in `puffer-core::command_helpers::genskill`).
This keeps the GEPA loop pure and unit-testable.

## Contract Details

- Frontmatter contract: `name` is non-empty lowercase-hyphens ≤64 chars;
  `description` ≤1024 chars. `parse_skill_md` rejects violators.
- Size contract: candidate body ≤ `GepaOptions::max_size_bytes` (default
  15_000). Oversize candidates are discarded by `generate`/`mutate`.
- Pareto contract: a candidate is on the frontier iff no other scored
  candidate has `>=` on all four dimensions and `>` on at least one.
- Tie-break contract: when multiple Pareto candidates remain, prefer
  highest reproducibility, then highest total score.

## Out of Scope (this crate)

- Curator (skill lifecycle/usage tracking) — see `puffer-skill-evolution`
  Plan 2 in `docs/superpowers/specs/2026-05-07-genskill-design.md`.
- Benchmark harness — see Plan 3.
- Live runtime adapter — lives in `puffer-core::command_helpers::genskill`,
  not here, to keep this crate provider-agnostic.
```

- [ ] **Step 3: Write puffer-core update spec**

Create `specs/puffer-core/02.md` (or next-numbered):

```markdown
# puffer-core update 02

## Purpose

Adds the `/genskill` slash command for generating reusable skills from
the current conversation transcript via the `puffer-skill-evolution`
crate.

## Design

`/genskill` is registered in `supported_commands()` as `CommandKind::Local`.
The handler `handle_genskill_command` parses `--candidates N` and
`--rounds K` flags, builds an `ExecutionTrace` from `AppState::transcript`,
and calls `puffer-skill-evolution::run_gepa`. The result is written to
`resources/skills/<name>/SKILL.md` with a `-vN` suffix on name collision.

`PufferAgentRuntime` adapts puffer-core's `runtime:agent` dispatcher to
the `AgentRuntime` trait. It is internal to `command_helpers::genskill`.

## Logic

Trace bridging filters transcript events to user / assistant / tool
roles, extracting text and tool-call names. Empty turns with no tool
calls are dropped. The first non-empty user turn becomes the trace's
`task_summary`.

If the trace has fewer than 5 entries (heuristic for a non-trivial
task), the handler returns a hint message instead of running GEPA —
mirrors hermes-agent's "5+ tool calls" threshold.

## Contract

Command contract: `/genskill` accepts optional `--candidates N` (default 3)
and `--rounds K` (default 2). It always returns a user-facing message;
on success the message contains the path to the new skill file.

Resource contract: `resources/skills/genskill/{SKILL,generate,judge,mutate}.md`
are bundled prompt templates. The handler does not currently template
into them at runtime — `puffer-skill-evolution`'s `Default*Prompt`
builders use inline templates. A future update may swap the resources
in for full templating.

## Architecture Details

The handler uses tokio's async/await; dispatch follows the same pattern
as other prompt-backed and local commands. No new runtime infrastructure
is added — `runtime:agent` handles all sub-agent execution.
```

- [ ] **Step 4: Commit**

```bash
git add specs/puffer-skill-evolution/ specs/puffer-core/
git commit -m "docs(specs): add component spec and core update for /genskill

Adds specs/puffer-skill-evolution/00.md describing the new crate's
modules and contracts. Adds specs/puffer-core/02.md describing the
/genskill command registration, transcript bridging, and runtime
adapter.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Final Verification

- [ ] **Step 1: Full crate test pass**

Run: `cargo test -p puffer-skill-evolution`
Expected: 27 tests pass.

- [ ] **Step 2: Targeted core tests**

Run: `cargo test -p puffer-core --lib genskill`
Expected: `parse_args_defaults`, `parse_args_overrides` pass.

- [ ] **Step 3: Build doesn't introduce new warnings on touched crates**

Run: `cargo build -p puffer-skill-evolution -p puffer-core 2>&1 | grep -i warn | wc -l`
Expected: count is no higher than baseline. (Baseline `puffer-core` has ~69 pre-existing warnings.)

- [ ] **Step 4: Push branch**

```bash
git push -u origin feat/genskill
```

- [ ] **Step 5: Open PR**

```bash
gh pr create --title "feat: /genskill — GEPA-based skill generation (Plan 1 of 3)" --body "$(cat <<'EOF'
## Summary

- Adds `puffer-skill-evolution` crate implementing the GEPA loop (multi-candidate generation, LLM-as-judge scoring, Pareto frontier selection, mutation rounds).
- Registers `/genskill` as a first-class slash command in `puffer-core`.
- Adds 4 prompt resources at `resources/skills/genskill/` (also makes `/skill:genskill` work).
- Spec at `docs/superpowers/specs/2026-05-07-genskill-design.md`; component specs at `specs/puffer-skill-evolution/00.md` and `specs/puffer-core/02.md`.

This is Plan 1 of 3. Plan 2 (Curator) and Plan 3 (Benchmark harness) follow in subsequent PRs — see the design spec for scope.

## Test plan

- [ ] `cargo test -p puffer-skill-evolution` passes (27 unit tests including end-to-end run_gepa with scripted runtime)
- [ ] `cargo test -p puffer-core --lib genskill` passes (parse_args)
- [ ] Manual smoke: `/genskill` after a 5+ tool-call conversation writes a valid SKILL.md
- [ ] Pre-existing master failures (matrix-sdk build, browser.yaml parse) are unchanged

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-Review Notes

- **Spec coverage:** Tasks 1-10 cover §3.2 (puffer-skill-evolution crate). Task 11 covers §3.4 (resources). Tasks 12-13 cover §3.1 (puffer-core integration). Task 14 covers spec docs. Curator (§3.3) and benchmark (§3.5) are explicitly out of scope (Plans 2 and 3).
- **Placeholder scan:** Tasks 12-13 contain code that depends on puffer-core's actual runtime API. The plan instructs the executor to discover exact symbols via grep before writing the adapter — this is a real-codebase discovery step, not a placeholder. The shape of the `AgentRuntime` impl is definitionally narrow (`(&self, &str) → Result<String>`) so once the live entry point is found, wiring is mechanical.
- **Type consistency:** All types referenced in later tasks (`AgentRuntime`, `ExecutionTrace`, `SkillCandidate`, `RubricScores`, `GepaOptions`, `TranscriptStep`) are defined in earlier tasks. Module declarations in `lib.rs` are added incrementally as each module is introduced.
