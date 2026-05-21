# Ladybird PR Replay — /genskill Evaluation Spec (Plan 3)

**Status:** Draft
**Date:** 2026-05-07
**Owner:** shawn
**Repo:** berabuddies/puffer
**Depends on:** Plan 1 (`/genskill` core) — must be merged before this can run

---

## 1. Motivation

The core question for `/genskill` is whether GEPA-style multi-candidate generation produces measurably better skills than direct prompting. The previous design proposed three evaluation strategies (rubric, functional replay, longitudinal usage). This spec replaces the synthetic functional replay (Strategy B) with a **real-world dataset**: merged pull requests from the Ladybird browser project, each with associated unit tests that provide ground-truth pass/fail signal.

**Why Ladybird:**
- Real C++ codebase with non-trivial complexity (a web browser)
- Open-source, all PRs and tests publicly available
- Cross-language: skills generated from puffer (Rust) sessions get tested on Ladybird (C++) tasks → measures **general coding skill quality**, not project-specific patterns
- Unit tests = automatic ground truth, no LLM judge needed for the binary signal
- PR descriptions provide natural task prompts

**Hypothesis:** A fresh agent loaded with a GEPA-generated skill, given the pre-fix codebase and the failing test, will:
1. Produce a fix that lands closer to the merged solution (information retention)
2. Use fewer tokens overall (token efficiency)
3. Repeat fewer redundant tool calls (less duplicate work)

...than the same agent loaded with a direct-prompted skill from the same source conversation.

---

## 2. Architecture Overview

This evaluation runs **after** Plan 1 ships. It does not modify any production code in `puffer-skill-evolution` or `puffer-core` — it is a self-contained benchmark harness that consumes `/genskill`'s output as a black box.

```
benchmark/genskill/ladybird/
   │
   ├── pr_corpus/             # 10 hand-curated PRs (data, version-controlled)
   │     pr-NNNNN/
   │       ├── meta.json
   │       ├── pre.diff
   │       ├── reference_fix.patch
   │       ├── tests/
   │       ├── expert_run.md
   │       └── skills/
   │            ├── gepa/SKILL.md
   │            └── direct/SKILL.md
   │
   ├── replay/                # Rust binary that runs one PR with one skill
   │     run_pr.rs
   │     metrics.rs
   │     sandbox.rs
   │
   ├── aggregate/             # roll up across PRs
   │     aggregate.rs
   │
   └── reports/               # generated, gitignored except for committed published runs
         YYYY-MM-DD/
           summary.md
           per_pr/
             pr-NNNNN.md
```

The harness has three independent phases that can be run separately:

1. **Generate phase** — for each PR, run `/genskill` and direct-prompt against the recorded expert run, store the two resulting SKILL.md files
2. **Replay phase** — for each `(PR, skill_arm)` pair, spin up a sandboxed agent and let it attempt the fix
3. **Report phase** — aggregate replay outputs into a comparison report

---

## 3. The PR Corpus

### 3.1 Selection criteria

Each PR must satisfy **all** of:

| Criterion | Why |
|---|---|
| Merged into Ladybird `master` | Ground-truth solution exists |
| Adds or modifies at least one unit test | Automatic pass/fail signal |
| Modifies ≤ 5 source files (excluding tests) | Bounds replay complexity |
| Has a clear root cause described in the PR or linked issue | Provides natural task prompt |
| Builds and tests run in standard Ladybird Docker image | Reproducible without special hardware |
| At least 6 months old (so it's stable, not contested) | Avoids regression-churn cases |

### 3.2 Domain distribution

To prevent overfitting to a single subsystem, the 10 PRs are spread across these areas (target distribution):

| Area | Count |
|---|---|
| LibWeb (CSS / Layout / DOM) | 3 |
| LibJS (engine / parser) | 2 |
| LibHTTP / networking | 1 |
| LibCore / utilities | 2 |
| LibCrypto / LibTLS | 1 |
| Build system / tooling | 1 |

The exact 10 PRs are committed to `pr_corpus/` and frozen — re-runs of the benchmark always use the same PRs for comparability.

### 3.3 Per-PR artifacts

```
pr_corpus/pr-12345/
  meta.json
    {
      "pr_number": 12345,
      "url": "https://github.com/LadybirdBrowser/ladybird/pull/12345",
      "title": "Fix CSS grid line resolution off-by-one",
      "merged_at": "2025-09-14T10:23:00Z",
      "base_commit": "<sha-before-fix>",
      "merge_commit": "<sha-after-fix>",
      "area": "libweb-css",
      "files_changed": ["Userland/Libraries/LibWeb/CSS/Grid.cpp", ...],
      "task_prompt": "Fix the failing test Tests/LibWeb/Css/Grid/grid-line-off-by-one.html. The test expects ... but currently gets ..."
    }

  pre.diff             # patch from main → base_commit (so we can recreate state)
  reference_fix.patch  # the merged fix diff (ground truth)
  tests/               # the test files added/modified by the PR
  expert_run.md        # full transcript of an expert agent fixing it (input to /genskill)

  skills/
    gepa/SKILL.md      # produced by /genskill on expert_run.md
    direct/SKILL.md    # produced by direct prompt on expert_run.md
```

### 3.4 How the expert run is generated (one-time)

For each PR:
1. Check out `base_commit` of Ladybird in a fresh sandbox
2. Apply the test files from `tests/` so the failing test exists
3. Run an instance of puffer (with full tool access) in that sandbox with `meta.task_prompt` as the initial user message
4. Let it run until the test passes or it gives up (≤ 60 min)
5. Save the entire transcript as `expert_run.md`

The expert run is the **input** for both skill-generation arms (GEPA and direct). Both arms see the exact same conversation.

---

## 4. The Three Replay Arms

For each PR, three separate replays are run for comparison:

| Arm | Skill loaded | Purpose |
|---|---|---|
| `no-skill` | (none) | Baseline — what does a fresh agent do without any guidance? |
| `direct` | `pr-NNNNN/skills/direct/SKILL.md` | Direct-prompt control |
| `gepa` | `pr-NNNNN/skills/gepa/SKILL.md` | GEPA treatment |

Three arms × 10 PRs = **30 replays per benchmark run**.

The `no-skill` arm is critical: it shows the floor (what does the agent do unaided?) so the GEPA-vs-direct delta can be normalized against it. A skill that helps less than no skill at all is actively harmful.

---

## 5. Replay Mechanics

### 5.1 Sandbox

**Choice: Docker.** Matches hermes TBLite's pattern, gives reproducible builds, and isolates Ladybird's heavy native dependencies from the host.

A single `Dockerfile.ladybird-eval` based on Ladybird's official build image:
- Pre-installs all Ladybird build dependencies
- Has a fixed Ladybird `master` checkout pre-built (warmed cache)
- Each replay spawns a fresh container, checks out `meta.base_commit`, applies `tests/`, runs the agent

### 5.2 Agent setup per replay

Inside the container:
1. Reset working tree to `base_commit`
2. Apply files from `pr-NNNNN/tests/` (so the failing test exists)
3. Confirm test fails: `ladybird-test --filter=<test-name>` → expect non-zero exit
4. Start a fresh puffer instance with:
   - The PR's `meta.task_prompt` as the initial user message
   - For `direct` and `gepa` arms: load the corresponding SKILL.md as a resource (mirrors how `/skill:<name>` works)
   - For `no-skill` arm: load no skills
5. Run until **stop condition** (see §5.3)
6. Capture replay artifact (see §5.4)

### 5.3 Stop conditions

A replay ends when **any** of:

| Condition | Outcome | Recorded as |
|---|---|---|
| Target test passes | Success | `pass` |
| Target test still fails after agent claims completion | Failure | `wrong-fix` |
| Agent emits "I cannot solve this" or equivalent | Failure | `gave-up` |
| Wall-clock 30 min exceeded | Failure | `wall-timeout` |
| Tool calls exceed 50 | Failure | `tool-budget` |
| Total tokens exceed 250K | Failure | `token-budget` |

Why these caps: 30 min and 50 tool calls comfortably bound the expected expert run (typically 10-25 tool calls in 10-20 min); 250K tokens prevents runaway loops. A skill that requires breaking these caps is worse than one that succeeds inside them.

### 5.4 Replay artifact (`replays/YYYY-MM-DD/<arm>.json`)

```json
{
  "pr": "pr-12345",
  "arm": "gepa",
  "outcome": "pass",
  "wall_seconds": 412,
  "tool_calls": 18,
  "tokens": {
    "input": 62100,
    "output": 8400,
    "tool_results": 31200,
    "total": 101700
  },
  "tool_call_log": [
    { "name": "Read", "input": "Userland/Libraries/LibWeb/CSS/Grid.cpp", "output_size": 3120, "ts": "..." },
    { "name": "Bash", "input": "grep -rn ResolveGridLine ...", "output_size": 480, "ts": "..." },
    ...
  ],
  "final_diff": "...",          // unified diff of agent's changes vs base_commit
  "test_outcome": {
    "command": "ladybird-test --filter=grid-line-off-by-one",
    "exit_code": 0,
    "stdout_tail": "PASS"
  }
}
```

Everything needed to compute metrics offline is in this artifact — no metrics are computed during the replay itself, keeping the agent loop clean.

---

## 6. Metrics

All metrics computed in `metrics.rs` from the replay artifact, after the run completes.

### 6.1 Information retention

**Question:** Did the skill carry the right knowledge from the expert run to the fresh agent?

**Three measurements, combined:**

**(a) File overlap (Jaccard):**
```
files_changed_by_agent ∩ files_in_reference_fix.patch
─────────────────────────────────────────────────────  ∈ [0, 1]
files_changed_by_agent ∪ files_in_reference_fix.patch
```

Agent touching the same files = at least the agent looked in the right place.

**(b) Symbol overlap:**
Identifiers (function names, struct names) in `final_diff` that also appear in `reference_fix.patch`. Normalized: count(intersection) / count(reference_fix symbols).

This tells us whether the agent reached for the same APIs as the reference solution.

**(c) Approach fidelity (LLM-judged):**
A separate LLM call gets `reference_fix.patch` and `final_diff` and answers "are these solving the problem in the same way?" on a 0–1 scale. Used as a sanity check on (a) and (b).

**Composite info retention score:** `0.4 × file_overlap + 0.4 × symbol_overlap + 0.2 × approach_fidelity` ∈ [0, 1].

### 6.2 Token efficiency

```
total_tokens = tokens.input + tokens.output + tokens.tool_results
```

Lower is better. Reported as raw value and as ratio against `no-skill` baseline:
```
efficiency_ratio = total_tokens(arm) / total_tokens(no-skill arm, same PR)
```

Ratio < 1.0 means the skill saved tokens; > 1.0 means it cost more than nothing.

### 6.3 Duplicate work

A tool call is a `(tool_name, normalized_input)` tuple. Normalization per tool:
- `Read`: full file path
- `Bash`: drop trailing whitespace, lowercase, collapse runs of whitespace
- `Grep`: pattern + path glob
- `Edit` / `Write`: file path only (count multiple edits to same file as duplicates of "intent to edit X")

```
duplicate_score = Σ over unique (tool, input) calls: max(0, occurrences - 1)
```

A score of 0 means every tool call was unique. Higher means more redundant exploration. Reported as raw count.

### 6.4 Ground-truth outcome

Binary: `pass` / `fail` (any non-pass outcome). The single most important number.

### 6.5 Per-PR metric record

```json
{
  "pr": "pr-12345",
  "outcomes": {
    "no-skill":  { "outcome": "fail",  "info_retention": 0.32, "tokens": 142000, "duplicate_score": 11 },
    "direct":    { "outcome": "pass",  "info_retention": 0.61, "tokens": 98000,  "duplicate_score": 6  },
    "gepa":      { "outcome": "pass",  "info_retention": 0.81, "tokens": 64000,  "duplicate_score": 2  }
  }
}
```

---

## 7. Reporting

### 7.1 Aggregated summary

```markdown
# /genskill Evaluation — Ladybird PR Replay
Run date: 2026-05-20
Corpus: 10 PRs, frozen at commit <sha>

## Headline numbers

|                       | no-skill | direct | gepa  | Δ (gepa − direct) |
|-----------------------|----------|--------|-------|--------------------|
| Pass rate             | 4 / 10   | 6 / 10 | 8 / 10| **+2** ✅          |
| Mean tokens (passed)  | 138K     | 96K    | 71K   | **−26%** ✅        |
| Mean duplicate_score  | 9.4      | 5.8    | 2.1   | **−64%** ✅        |
| Mean info_retention   | 0.41     | 0.62   | 0.79  | **+0.17** ✅       |

## Headline interpretation

- **GEPA wins on all four headline metrics** in this run
- The largest gain is in duplicate_score: GEPA-skill agents repeat less
- Token efficiency improvement is non-trivial (−26%); skills are paying for themselves
```

### 7.2 Per-PR breakdown

For each PR, a section like:

```markdown
### PR #12345 — Fix CSS grid line resolution off-by-one

| Arm        | Outcome | Tokens | Tools | Dup | InfoRet |
|------------|---------|--------|-------|-----|---------|
| no-skill   | ❌ fail (token-budget) | 250K | 47 | 14 | 0.28 |
| direct     | ✅ pass | 89K  | 21 | 5  | 0.59 |
| gepa       | ✅ pass | 52K  | 13 | 1  | 0.83 |

GEPA used 42% fewer tokens than direct on this PR. The skill correctly pointed
the agent at LibWeb/CSS/Grid.cpp:ResolveGridLine on its second tool call,
where direct's skill led to four file reads before finding the right one.
```

### 7.3 Negative-result transparency

If GEPA loses on any metric, the report **highlights it** rather than burying it. The point is honest measurement, not advocacy. Example:

> "GEPA's pass rate (8/10) beats direct (6/10), but on PR-67890 GEPA hit a wall-timeout while direct passed. Inspection shows GEPA's skill was overly verbose for this domain — see §Appendix B."

---

## 8. New Files & Implementation

### 8.1 New paths
```
benchmark/genskill/ladybird/
  pr_corpus/                         # data — committed
    pr-NNNNN/                        # × 10
      meta.json
      pre.diff
      reference_fix.patch
      tests/...
      expert_run.md
      skills/{gepa,direct}/SKILL.md
  Dockerfile.ladybird-eval           # reproducible env
  scripts/
    select_prs.sh                    # gh CLI helper for finding candidate PRs
    record_expert_run.sh             # spins up sandbox, runs expert pass
    generate_skills.sh               # invokes /genskill and direct prompt
    run_replay.sh                    # one (pr, arm) replay
    aggregate.sh                     # rolls up across all replays
  src/
    Cargo.toml                       # new bin crate puffer-genskill-eval
    main.rs                          # CLI entry: orchestrates all phases
    pr_corpus.rs                     # load/validate corpus on disk
    replay.rs                        # one-replay execution
    sandbox.rs                       # docker spawn + lifecycle
    metrics.rs                       # info_retention, tokens, duplicates
    report.rs                        # markdown rendering
  reports/
    .gitignore                       # ignore everything except published/
    published/
      2026-05-20/                    # one folder per published run
        summary.md
        per_pr/...
```

### 8.2 New crate

`benchmark/genskill/ladybird/src/` is a new bin crate `puffer-genskill-eval`. It is **not** part of the main workspace's runtime path — added to workspace `Cargo.toml`'s `members` only when explicitly building benchmark tooling, or kept as a separate workspace.

### 8.3 No production code changes

The Plan 1 outputs (`puffer-skill-evolution`, `/genskill` command) are consumed as black boxes. The eval harness only:
- Reads the SKILL.md files they produce
- Invokes `puffer` CLI in sandboxes (no library dependency)

This keeps the eval independently shippable and means experiments don't risk breaking the main binary.

---

## 9. Task Plan (Plan 3 itself)

13 tasks total:

**Phase A — Corpus (one-time, mostly manual)**
1. Write `select_prs.sh` (gh CLI: list merged PRs, filter by mergecommit + test files + size)
2. Hand-curate 10 PRs satisfying §3 criteria; commit `meta.json` + `tests/` + `pre.diff` + `reference_fix.patch`
3. Build `Dockerfile.ladybird-eval` with prebuilt Ladybird tree
4. Write `record_expert_run.sh` and run it for all 10 PRs; commit `expert_run.md`s

**Phase B — Skill generation**
5. Write `generate_skills.sh` (invokes `/genskill` and direct prompt against each `expert_run.md`); commit resulting `skills/{gepa,direct}/SKILL.md`s

**Phase C — Replay infrastructure**
6. Bootstrap `puffer-genskill-eval` bin crate
7. Implement `sandbox.rs` (docker spawn, working-tree reset, test invocation)
8. Implement `replay.rs` (orchestrates one `(pr, arm)` run, captures artifact JSON)
9. Implement stop-condition checks (timeout, budget, tool-call cap)

**Phase D — Metrics and reporting**
10. Implement `metrics.rs` (file overlap, symbol overlap, info retention composite, token sums, duplicate score)
11. Implement `report.rs` (markdown rendering)
12. Implement `aggregate.rs` (roll up across PRs and arms)

**Phase E — End-to-end run**
13. Run all 30 replays end-to-end, publish first report at `reports/published/<date>/`

---

## 10. Open Questions / Decisions Needed

- **Sandbox choice confirmed:** Docker (this spec assumes it; revisit if the user prefers worktree)
- **Stop conditions confirmed:** 30 min wall, 50 tool calls, 250K tokens (defaults; revisit after first run)
- **Three arms confirmed:** `no-skill`, `direct`, `gepa` (the `no-skill` baseline normalizes the comparison)
- **PR list:** to be curated in Task 2; criteria in §3.1 are the gate
- **Approach-fidelity LLM judge:** uses Claude Sonnet 4.6 by default; configurable
- **Re-run cadence:** ad-hoc initially. After Plan 1+2 ship, propose nightly with a stable `pr_corpus@vN` tag

---

## 11. Cost Estimate

Per benchmark run (one full pass through 10 PRs × 3 arms = 30 replays):

| Cost component | Estimate |
|---|---|
| Replay tokens (30 × ~100K avg) | ~3M tokens, ~$30-60 with Sonnet |
| Skill generation (10 × /genskill + 10 × direct) | ~150K tokens, ~$2-4 |
| Approach-fidelity judging (30 × ~5K) | ~150K tokens, ~$1-2 |
| **Total** | **~$35-70 per full run** |

Cost scales linearly with the number of PRs in the corpus. Doubling to 20 PRs roughly doubles cost.

The expert-run recording is a **one-time** cost (~$30-60 across 10 PRs) — not paid on subsequent benchmark runs.

---

## 12. Future Work

- **Cross-codebase generalization:** add a second corpus (e.g., 10 SQLite PRs) and check whether GEPA skills generated from Ladybird sessions help with SQLite tasks (true generalization test)
- **Skill staleness:** re-run the same corpus against skills generated 6 months apart to measure how `/genskill` improves as the underlying model improves
- **Curator integration (Plan 2):** run replays with curated skills (consolidated/pruned by Curator) to see whether Curator's intervention helps or hurts
- **TBLite porting:** the infrastructure here (Docker sandbox, replay binary, stop conditions) is exactly what would be needed to port hermes' TBLite as a regression gate. See `docs/superpowers/specs/2026-05-07-genskill-design.md` §9 for that future work item.

---

## 13. References

- [LadybirdBrowser/ladybird](https://github.com/LadybirdBrowser/ladybird) — target codebase
- [berabuddies/puffer](https://github.com/berabuddies/puffer) — eval harness lives here under `benchmark/`
- `docs/superpowers/specs/2026-05-07-genskill-design.md` — parent spec (this is Plan 3)
- `docs/superpowers/specs/2026-05-07-genskill-design.md` §7 — original three-strategy eval (Strategy B is replaced by this spec)
- [hermes-agent TBLite](https://github.com/NousResearch/hermes-agent/tree/main/environments/benchmarks/tblite) — same philosophy: skills judged by what they enable
