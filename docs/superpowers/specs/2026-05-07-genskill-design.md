# /genskill — Design Spec

**Status:** Draft v2
**Date:** 2026-05-07
**Owner:** shawn
**Repo:** berabuddies/puffer

---

## 1. Motivation

Puffer is a Rust rebuild of Claude Code. Today, skills at `resources/skills/<name>/SKILL.md` are hand-authored (e.g. `reviewer`, `browser`). We want puffer to **generate skills from its own conversation history** — a parallel sub-agent reads the session's execution trace and produces a reusable SKILL.md capturing what was learned.

The approach mirrors NousResearch's two-repo pattern:
- **hermes-agent** writes a skill after complex tasks (5+ tool calls), capturing approach, edge cases, reconstructed domain knowledge — and uses a **Curator** background system to track real-world usage of agent-created skills
- **hermes-agent-self-evolution** (ICLR 2026 Oral) applies DSPy + GEPA (Genetic-Pareto Prompt Evolution) to refine skills against execution traces

`/genskill` brings both patterns into puffer, re-implemented natively in Rust:
- **Thinking pattern** — reflective trace-based skill authoring
- **Tooling pattern** — multi-candidate generation, LLM-as-judge scoring, Pareto selection, post-deployment usage tracking via a Curator

**Hypothesis to test:** GEPA-style multi-candidate + Pareto selection + reflection produces better skills than a one-shot "generate a skill from this conversation" prompt.

**Important caveat:** This comparison (GEPA-from-scratch vs direct-prompt-from-scratch) is **not something hermes has answered**. hermes-self-evolution compares GEPA-evolved skills against the **previous version of the same skill**, not against direct prompting. The evaluation methodology in §7 is therefore designed from scratch for puffer.

---

## 2. Architecture

`/genskill` is a **first-class slash command** registered in `puffer-core::supported_commands` as a **local command**. It dispatches to a new crate, `puffer-skill-evolution`, which runs the GEPA loop. Sub-agents are spawned via the existing `Agent` runtime tool (`resources/tools/agent.yaml`) — no new sub-agent infrastructure needed.

```
/genskill
   │
   ▼
puffer-core (local command handler)
   │  - extracts transcript from AppState
   │  - formats execution trace
   ▼
puffer-skill-evolution (new crate)
   │  - GEPA loop: generate → score → Pareto → mutate → repeat
   │  - Curator: post-deployment usage tracking
   │  - uses Agent tool for candidate generation
   │  - uses provider directly for LLM-as-judge
   ▼
resources/skills/<generated-name>/SKILL.md
```

---

## 3. Components

### 3.1 `puffer-core` change

Register `/genskill` in `supported_commands` as a local command that:
1. Reads transcript from `AppState`
2. Builds an `ExecutionTrace`
3. Calls `puffer-skill-evolution::run_gepa(trace, opts)`
4. Writes returned skill to `resources/skills/<name>/SKILL.md`
5. Records usage via Curator
6. Reports path to user

CLI: `/genskill [--candidates N] [--rounds K]` — defaults `N=3`, `K=2`.

Also register `/curator status`, `/curator run`, `/curator run --dry-run` (mirrors `hermes curator` commands).

### 3.2 New crate: `puffer-skill-evolution`

Workspace-level new crate at `crates/puffer-skill-evolution/`. Public surface:

```rust
pub struct GepaOptions {
    pub n_candidates: usize,    // default 3
    pub k_rounds: usize,         // default 2
    pub max_size_bytes: usize,   // default 15_000
}

pub struct ExecutionTrace { /* structured trace */ }
pub struct SkillCandidate {
    pub frontmatter: SkillFrontmatter,
    pub body: String,
    pub scores: Option<RubricScores>,
}
pub struct RubricScores {
    pub novelty: f32,            // 0.0-1.0: captures non-obvious knowledge?
    pub reproducibility: f32,    // 0.0-1.0: would a fresh agent reproduce the approach?
    pub structure: f32,          // 0.0-1.0: proper sections, pitfalls, checklist?
    pub conciseness: f32,        // 0.0-1.0: within budget, no fluff?
}

pub async fn run_gepa(
    trace: ExecutionTrace,
    opts: GepaOptions,
    runtime: &dyn AgentRuntime,
) -> Result<SkillCandidate>;

// Curator surface
pub struct CuratorState {
    pub stale_after_days: u32,    // default 30
    pub archive_after_days: u32,   // default 90
    pub min_idle_hours: u32,       // default 2
    pub interval_hours: u32,       // default 168 (7 days)
}

pub async fn run_curator(state: &CuratorState) -> Result<CuratorReport>;
pub fn record_skill_view(name: &str);
pub fn record_skill_load(name: &str);
pub fn record_skill_patch(name: &str);
```

Internal modules:
- `trace.rs` — extract/format execution trace from a transcript
- `generate.rs` — spawn parallel `Agent` calls with `generate.md`
- `judge.rs` — call provider with `judge.md` to score candidates
- `pareto.rs` — non-dominated frontier selection
- `mutate.rs` — round-K mutations seeded from Pareto survivors
- `curator.rs` — post-deployment lifecycle (mirrors hermes-agent Curator)

Constraints (per `puffer/AGENTS.md`): every public function has a docstring; no source file > 1000 lines.

### 3.3 Curator module

Mirrors hermes-agent's Curator. Two-phase pattern:

**Phase 1 — Deterministic state transitions (no LLM):**
- Skill unused for `stale_after_days` (default 30) → marked stale
- Skill unused for `archive_after_days` (default 90) → moved to `resources/skills/.archive/`
- Bundled, hub-installed, and pinned skills are **never touched**
- Only `/genskill`-generated (agent-created) skills participate

**Phase 2 — LLM review pass:**
- Spawned via `Agent` tool with `curator.md` prompt
- Surveys agent-created skills (max 8 iterations)
- Per-skill decision: keep / patch / consolidate overlapping pair / archive
- All proposed changes are written to a review report; mutations require human approval before applying

**Trigger:**
- On CLI session start
- On recurring tick inside `puffer-core` event loop
- Conditions: `interval_hours` elapsed since last run AND agent idle for `min_idle_hours`

**Storage of usage stats:** `puffer-session-store` extension — track per-skill `view_count`, `load_count`, `patch_count`, `last_used_at`.

### 3.4 Resources

```
resources/skills/genskill/
  SKILL.md       — metadata + fallback prompt (so /skill:genskill also works)
  generate.md    — candidate generation prompt
  judge.md       — LLM-as-judge rubric prompt
  mutate.md      — mutation prompt (round > 0)
  curator.md     — Curator LLM review prompt
```

`generate.md` instructs sub-agents to:
1. Treat the execution trace as evidence of a non-trivial task
2. Extract: novel knowledge, edge cases hit, reconstructed domain knowledge, what would surprise a fresh agent
3. Produce Hermes-rich SKILL.md (frontmatter, Overview, When to Use, Topic Sections, Pitfalls, Verification Checklist) under 15KB

`judge.md` instructs the judge to score each candidate on the four rubric dimensions (each 0.0–1.0) with brief justification.

`mutate.md` instructs the sub-agent to take a Pareto survivor and target its weakest dimension while preserving its strong ones.

`curator.md` mirrors hermes-agent's curator review prompt: per-skill keep/patch/consolidate/archive decisions.

### 3.5 Benchmark harness

```
benchmark/genskill/
  tasks/                 — synthetic task scenarios (one .md per task)
  seeds/                 — synthetic seed conversations (one .md per seed)
  run_comparison/        — Rust binary: GEPA vs direct, scores, reports
  judge.md               — shared rubric (symlink/copy)
  reports/               — generated comparison outputs
```

---

## 4. GEPA Loop (detailed)

```
Input: ExecutionTrace, GepaOptions { N, K }

Round 0 (generation):
  Spawn N parallel Agent tool calls — generate.md + execution_trace + temp variation
  Collect N candidate SKILL.md texts; validate frontmatter; discard invalid

Round r (1..K):
  For each candidate, invoke LLM-as-judge with judge.md
    Score on [novelty, reproducibility, structure, conciseness]
  Compute Pareto frontier (non-dominated set across 4 dims)
  Survivors = frontier
  If r < K:
    Spawn one mutation Agent call per survivor with mutate.md
    Each mutant targets the survivor's weakest dimension
    New pool = survivors + mutants

Final:
  Return Pareto-best
    Tie-break: highest reproducibility, then highest sum
```

Defaults: N=3, K=2, max 15KB.

---

## 5. Data Flow

1. User invokes `/genskill` mid-conversation
2. `puffer-core` reads `AppState::transcript`
3. `trace.rs` produces `ExecutionTrace` (ordered tool calls, outcomes, summaries)
4. `generate.rs` spawns N concurrent `Agent` calls — returns N drafts
5. `judge.rs` evaluates each via one provider call → `RubricScores`
6. `pareto.rs` filters to non-dominated frontier
7. If rounds remain, `mutate.rs` produces new candidates → loop to 5
8. Final survivor written to `resources/skills/<name>/SKILL.md`
9. `curator.rs` initializes usage stats for the new skill
10. Path returned to user

---

## 6. Error Handling & Constraints

| Failure mode | Handling |
|---|---|
| Sub-agent returns invalid frontmatter | Discard; if all N fail, abort with error |
| Sub-agent exceeds 15KB | Penalize conciseness to 0; still eligible |
| Judge returns malformed scores | Retry once; else `(0,0,0,0)` |
| Pareto frontier collapses to one | Skip mutation, accept survivor |
| Pareto tie at final | Highest reproducibility, then highest sum |
| Generated `name` collides | Append `-v2`, `-v3` suffix |
| Transcript too short (<5 tool calls) | Refuse with hint |
| Curator wants to delete bundled skill | Always refuse — only agent-created skills are eligible |

Hard constraints (per `puffer/AGENTS.md`):
- No source file > 1000 lines
- Every public Rust function has a docstring
- ASCII unless existing reason otherwise
- Skill files ≤ 15KB; description ≤ 1024 chars (mirrors Hermes)

---

## 7. Evaluation Plan

### 7.1 Hypothesis

`/genskill` (GEPA path) produces skills that, when loaded into a fresh agent, lead to higher task completion rates and fewer tool calls than skills produced by direct prompting.

### 7.2 Why we can't reuse hermes' evaluation directly

| Hermes uses... | For... | Can we reuse? |
|---|---|---|
| GEPA-vs-prior-version comparison | Refining existing skills | **No** — we're generating from scratch, hermes never evaluated this |
| TBLite (100 Docker terminal tasks) | Regression gate during skill evolution | **Not directly** — Hermes-specific harness; portable in principle (1-2 weeks) but tests general capability, not skill-specific fitness. See §9. |
| TerminalBench2, YC-Bench | Long-horizon validation | **No** — same portability issues, wrong question for our use case |
| LLM-as-judge rubric | Quality scoring | **Yes** — adapt the multi-dimensional 0-1 pattern |
| Curator usage tracking | Post-deployment skill quality | **Yes** — mirror as Strategy C |
| DSPy framework | GEPA implementation | **No** — Python; we re-implement the algorithm in Rust |

So our evaluation is **designed from scratch** with three triangulating strategies: rubric, functional replay, longitudinal usage.

### 7.3 Strategy A — LLM-as-judge rubric (automated, daily)

For each seed in `benchmark/genskill/seeds/`:
- Generate skill via `/genskill` (GEPA)
- Generate skill via direct prompt: *"Generate a reusable skill based on the conversation history above"*
- Score both with same `judge.md` rubric on 4 dimensions
- Compare mean scores across seeds

**Caveat:** judge and GEPA both LLM-based — risk of structural-bias toward verbose/structured output. Use as fast signal, not proof.

### 7.4 Strategy B — Functional replay (primary signal, weekly)

Mirrors hermes-self-evolution's TBLite philosophy: **skills are judged by what they enable, not how they look** — but applied to **task-matched** scenarios so the comparison is meaningful.

For each task in `benchmark/genskill/tasks/`:
1. **Setup**: record an "expert run" of an agent solving the task (becomes the seed conversation)
2. **Generate**: produce two skills from the expert run — one GEPA, one direct
3. **Replay**: give a fresh agent only the generated skill (no original conversation) and the task
4. **Measure**:
   - Task completion (binary)
   - Tool call count (lower = skill saved exploration)
   - Approach fidelity (LLM-judged 0–1)
5. Aggregate across tasks; compare GEPA vs direct

GEPA wins if completion rate ↑ and/or tool calls ↓.

### 7.5 Strategy C — Longitudinal usage tracking (post-deployment, ground truth)

Mirrors hermes-agent's Curator. After `/genskill` ships and is used by real puffer users:

1. Each generation run records `generation_method = "gepa" | "direct"` in skill metadata
2. Curator tracks per-skill `view_count`, `load_count`, `patch_count`, `last_used_at`
3. After 30+ days of usage data, aggregate by generation method:
   - Mean load count
   - Mean patch count (high = skill needed correction → lower quality)
   - Stale rate (% archived after 30 days unused)
4. **A good skill is one users keep using** — this is ground truth, not LLM-judged

The "direct" group needs comparison data, so the comparison binary in `benchmark/genskill/run_comparison/` also generates direct-prompted skills under the same conditions and registers them in the same store.

### 7.6 Initial task suite (target 10–15)

- "Add a new resource-backed slash command to puffer-core"
- "Debug a failing `cargo test` in `puffer-tools`"
- "Write a tool spec yaml for a new built-in tool"
- "Trace why a sub-agent isn't receiving the expected prompt"
- "Add a new connector skeleton crate following puffer-connector-* pattern"
- "Investigate a permission denial in tool execution"
- ... (rest TBD during implementation)

Each task ships with `task.md`, `expected.md`, `expert_run.md`.

### 7.7 Report format

```
# /genskill GEPA vs Direct — 2026-05-07

## Strategy A: Rubric (mean across N seeds)
| Dimension       | GEPA  | Direct | Δ     |
|-----------------|-------|--------|-------|
| Novelty         | 0.84  | 0.71   | +0.13 |
| Reproducibility | 0.79  | 0.62   | +0.17 |
| Structure       | 0.91  | 0.73   | +0.18 |
| Conciseness     | 0.66  | 0.81   | -0.15 |

## Strategy B: Functional replay (M tasks)
| Metric              | GEPA  | Direct |
|---------------------|-------|--------|
| Completion rate     | 80%   | 60%    |
| Mean tool calls     | 12.3  | 18.7   |
| Approach fidelity   | 0.78  | 0.55   |

## Strategy C: Longitudinal (30-day window, P skills/method)
| Metric           | GEPA  | Direct |
|------------------|-------|--------|
| Mean load count  | 14.2  | 6.5    |
| Mean patch count | 1.8   | 4.1    |
| Stale rate       | 22%   | 51%    |
```

### 7.8 Cost & cadence

- **Strategy A**: O(seeds × (N×K + 1)) provider calls — nightly automated
- **Strategy B**: O(tasks × 2) full agent runs — weekly / pre-merge
- **Strategy C**: zero marginal cost (telemetry only) — runs continuously, reports monthly

---

## 8. New Files & Crates Summary

| Path | Type | Purpose |
|---|---|---|
| `crates/puffer-skill-evolution/Cargo.toml` | new | crate manifest |
| `crates/puffer-skill-evolution/src/lib.rs` | new | public API (`run_gepa`, `run_curator`) |
| `crates/puffer-skill-evolution/src/trace.rs` | new | trace extraction |
| `crates/puffer-skill-evolution/src/generate.rs` | new | candidate generation |
| `crates/puffer-skill-evolution/src/judge.rs` | new | LLM-as-judge scoring |
| `crates/puffer-skill-evolution/src/pareto.rs` | new | non-dominated selection |
| `crates/puffer-skill-evolution/src/mutate.rs` | new | mutation generation |
| `crates/puffer-skill-evolution/src/curator.rs` | new | post-deployment lifecycle |
| `crates/puffer-core/src/...` | edit | register `/genskill`, `/curator` commands |
| `crates/puffer-session-store/src/...` | edit | per-skill usage stats |
| `resources/skills/genskill/SKILL.md` | new | metadata + fallback prompt |
| `resources/skills/genskill/generate.md` | new | candidate generation prompt |
| `resources/skills/genskill/judge.md` | new | rubric prompt |
| `resources/skills/genskill/mutate.md` | new | mutation prompt |
| `resources/skills/genskill/curator.md` | new | curator review prompt |
| `benchmark/genskill/tasks/*.md` | new | functional task suite |
| `benchmark/genskill/seeds/*.md` | new | rubric eval seeds |
| `benchmark/genskill/run_comparison/` | new | comparison runner binary |
| `specs/puffer-skill-evolution/00.md` | new | component spec |
| `specs/puffer-core/02.md` | new | spec update for command registration |
| `specs/puffer-session-store/01.md` | new | spec update for usage stats |

---

## 9. Open Questions / Future Work

- **Format mismatch with existing puffer skills**: existing skills (`reviewer`, `browser`) are 1–5 lines; Hermes-rich are 8–14KB. Future "compaction" pass to produce a puffer-minimal version alongside the rich version.
- **Provider cost**: N×K candidate calls + N×K judge calls per `/genskill`. Need telemetry to validate defaults.
- **Iterative skill improvement**: future `/genskill --refine <skill>` to evolve existing skills against new traces (this would actually mirror hermes-self-evolution's GEPA-vs-prior-version flow).
- **TBLite portability**: building `puffer-tblite` as a regression gate (hermes-style) is a reasonable follow-up project. ~1-2 weeks of work to port the Docker-based test harness. **Not in scope here** because TBLite tests general capability; Strategy B already gives skill-specific fitness signal which is what `/genskill`'s comparison needs.
- **Multi-provider sub-agents**: confirm `Agent` parallel dispatch works across all configured providers (Anthropic / OpenAI / Codex paths).
- **Atropos RL integration**: out of scope, but trace format should remain RL-friendly to keep door open.

---

## 10. References

- [berabuddies/puffer](https://github.com/berabuddies/puffer) — target repo
- [nousresearch/hermes-agent](https://github.com/nousresearch/hermes-agent) — skill authoring philosophy and Curator design
- [hermes-agent skill-authoring SKILL.md](https://github.com/nousresearch/hermes-agent/blob/main/skills/software-development/hermes-agent-skill-authoring/SKILL.md) — Hermes-rich skill format
- [Curator | Hermes Agent docs](https://hermes-agent.nousresearch.com/docs/user-guide/features/curator) — usage tracking & lifecycle
- [NousResearch/hermes-agent-self-evolution](https://github.com/NousResearch/hermes-agent-self-evolution) — GEPA loop reference (`PLAN.md`)
- [Hermes benchmarks (TBLite/TB2/YC-Bench)](https://github.com/NousResearch/hermes-agent/tree/main/environments/benchmarks) — functional eval philosophy
- "Reflective Prompt Evolution Can Outperform Reinforcement Learning" — Lakshya Agrawal et al., ICLR 2026 Oral
