# Burbot Architecture

Snapshot of `crates/puffer-burbot/` as of branch `feature/burbot-pesa-runtime` (HEAD = `55cbe35`, plus 56 uncommitted file changes including the harness refactor, forced goal-check, yolo mode, and verifier-context fix). Binary entrypoint: `src/main.rs` → `cli::run()`. ~26.8 kloc Rust across `src/`, `src/llm/`, `src/runtime/`.

## Purpose

Burbot is an experimental Meta-PESA agent runtime kept deliberately separate from Puffer's conversational TUI. It uses Puffer's provider crates (`puffer-provider-openai`, `puffer-provider-registry`) for OpenAI-compatible LLM transport but owns its control loop end-to-end. Where Puffer's TUI is a classical chat-with-tools agent (model decides each turn, runtime executes), Burbot is a contract-driven, deterministic graph-rewrite runtime in which the LLM is **one component among many** — a proposer and a verifier, not the conductor.

The trade is intentional: Burbot sacrifices conversational autonomy for **verifiability** — every action is recorded in a typed plan graph, contract-validated, scheduler-scored, safety-gated, and post-action verified. Every run produces a JSONL trace that can be replayed and audited.

## Design principles

1. **Plan graph is the source of truth.** The runtime never relies on conversation memory. State lives in `PlanGraph` + `BeliefGraph`, persisted to JSONL traces.
2. **Contracts gate everything.** Every action is bound to an `ActionContract` declaring side-effect class, preconditions, postconditions, idempotency, risk level, and structured-arg safety rules.
3. **The LLM proposes; the runtime decides.** LLM outputs are candidate suggestions. The runtime saturates, scores, and schedules.
4. **Determinism where possible.** Rewrite rules, saturation, scheduler scoring, and safety gates are pure functions of the graph + contracts.
5. **Replayable.** Every event is appended to a JSONL trace; runs can be inspected and re-derived.

## High-level architecture

```
                       ┌──────────────────┐
                       │   CLI / RunGoal  │
                       └────────┬─────────┘
                                │ goal, options
                                ▼
   ┌────────────────────────────────────────────────────────────┐
   │                      run_goal loop                          │
   │  (runtime.rs:113-313)                                       │
   │                                                             │
   │   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌──────────┐    │
   │   │ Rewrite │ → │ Saturate│ → │Scheduler│ → │  Safety  │    │
   │   │ Engine  │   │         │   │  pick   │   │   Gate   │    │
   │   └────┬────┘   └─────────┘   └─────────┘   └─────┬────┘    │
   │        │            ▲                              │         │
   │        │            │ CanReplace                   ▼         │
   │        │       ┌────┴────┐                  ┌──────────┐    │
   │        │       │   egg   │                  │ Executor │    │
   │        │       │(opt-in) │                  └─────┬────┘    │
   │        │       └─────────┘                        │         │
   │        │                                          ▼         │
   │   ┌────┴────────────────────────────────────────────────┐   │
   │   │  LLM (propose / observe-act / verify / review)      │   │
   │   │  llm.rs + llm/schema.rs + llm/chat.rs               │   │
   │   └─────────────────────────────────────────────────────┘   │
   │                                          │                  │
   │                                          ▼                  │
   │                              process_observation            │
   │                              (verify, repair, complete?)    │
   │                                          │                  │
   │                                          ▼                  │
   │                              RunResult::{Completed,Stalled} │
   └────────────────────────────────────────────────────────────┘
                                │
                                ▼
                         JSONL trace + snapshot
```

## Module map

### Top-level `src/`

| File | Lines | Role |
|---|---|---|
| `lib.rs` | 41 | Crate root, module declarations |
| `main.rs` | small | Binary entry → `run_cli()` |
| `cli.rs` | ~370 | Clap CLI: `contract`, `run`, `trace`, `eval`, `evolve`, `llm` |
| `contract.rs` | large | `ActionContract`, `CapabilityContract`, `ContractRegistry`, `SideEffectClass`, structured arg safety specs |
| `graph.rs` | large | `PlanGraph`, `PlanNode`, `PlanEdge`, `ActionRef`, `ActionScores`, kinds + statuses |
| `belief.rs` | small | `BeliefGraph`: precondition tracking |
| `ids.rs` | small | `NodeId`, `RunId`, `TraceId` newtypes |
| `planner.rs` | ~660 | Initial seed candidates from goal, rule-driven candidate plan generation, `CompletionRole` |
| `plan_synthesis.rs` | medium | Synthesise plan from goal + initial candidates |
| `puffer_tools.rs` | medium | Bridge to `puffer-tools` crate; converts tool YAML resources into `CapabilityContract`s |
| `rules.rs` | ~700 | `RewriteEngine` + 6 universal rules: `UnknownSideEffectsIncreaseRisk`, `ObserveBeforeRiskyAction`, `RequireApprovalForExternalWrite`, `VerifyBeforeCompletion`, `BlockRepeatedFailure`, `InsertRepairCandidatesFromContract` |
| `saturation.rs` | ~140 | Hand-written substitution: builds equivalence classes by intent + side-effect, emits `CanReplace` edges, prunes dominated open actions |
| `egg_optimizer.rs` | optional, behind `egg-optimizer` feature | E-graph-based read-only equivalence discovery via `egg::SymbolLang` |
| `scheduler.rs` | ~340 | Multi-factor weighted scoring: `expected_progress, success_probability, success_uncertainty_penalty, information_gain, verification_value, reversibility_bonus, risk_penalty, cost_penalty, latency_penalty, uncertainty_penalty, repeated_failure_penalty` |
| `executor.rs` | medium | `ExecutorDispatcher`, `PufferToolExecutor`, `Observation`. Resolves contract+args → invokes tool. |
| `verification.rs` | ~340 | Per-action structural verification (postconditions, required-before-completion gating) |
| `failure.rs` | small | `FailureKind` enum (`CommandNotFound`, `MissingPath`, `PermissionDenied`, `TimedOut`, `VerificationFailed`, `GoalUnsatisfied`, `NoProgress`, `NonZeroExit`, `ToolExecutionError`, `Contract(...)`) |
| `semantics.rs` | medium | Intent normalization, action-intent matching, side-effect classification helpers |
| `symbolic.rs` | small | Symbolic worker executor (proposal/critic/verifier roles) |
| `trace.rs` | ~200 | `TraceEvent`, `TraceEventType`, `JsonlTraceStore` (append-only) |
| `graph_store.rs` | small | Persists per-run graph snapshot to JSON |
| `stats.rs` | small | `ActionTraceStats`: action-level success/failure counts replayed from trace |
| `calibration.rs` | small | Adjusts scheduler weights from trace stats |
| `model_policy.rs` | medium | Schema validation of LLM-proposed args; allowed completion roles per action |
| `eval.rs` | medium | Eval-suite runner (`burbot eval ...`) |
| `evolve.rs` | medium | Mutates contract resources from trace failures; proposes new tool variants |
| `promotion.rs` | small | Mutation candidate evaluation |
| `llm.rs` | ~1000 | OpenAI/DeepSeek/Codex transport: credential resolution, request build, retry, wall-timeout, response parse |

### `src/llm/`

| File | Role |
|---|---|
| `schema.rs` | All four prompt builders: `observe_act_prompt`, `goal_verification_prompt`, `generated_artifact_review_prompt`, plus chat-tool-call request builders |
| `chat.rs` | Chat-completions tool-call path (preferred for non-Codex backends) |
| `parse.rs` | Parsers for tool-call JSON, candidate list JSON, goal-verification JSON; structural violation extraction |
| `openai_error.rs` | Retryable error classification, status extraction, custom error types (`OpenAIStatusError`, `OpenAIWallTimeoutError`, `OpenAINoStructuralAssistantContent`) |
| `policy_tests.rs` | Validation policy tests |
| `tests.rs` | Schema/parse tests |

### `src/runtime/`

| File | Lines | Role |
|---|---|---|
| `runtime.rs` (parent) | ~1000 | `BurbotRuntime` struct, `run_goal` outer loop, `process_observation`, all the bookkeeping |
| `model.rs` | 39 | `RunOptions` + `RunResult::{Completed, Stalled}` |
| `model_loop.rs` | ~1630 | LLM-side proposal logic, single-tool fallback, multi-candidate observe-act, recovery candidates, **`goal_verified_or_expand`** |
| `model_loop_support.rs` | ~460 | Context builders: `frontier_summary`, `verified_target_summary`, `action_history_summary`, etc. |
| `model_candidates.rs` | ~540 | `add_model_candidate_proposals`, dedup, dependency check, structural validation, `CandidateOutcome` |
| `model_feedback.rs` | small | Feedback channel from validation errors back into next LLM call |
| `model_retry.rs` | small | Retry-budget tracking for transient LLM errors |
| `artifact_context.rs` | small | Build generated-artifact ledger for prompts |
| `artifact_review.rs` | medium | Generated-artifact review LLM call (parallel to goal-verification) |
| `progress.rs` | 159 | `ProgressEvidence` (`changes_state`, `has_state_witness`, `has_structural_witness`, `has_output_witness`); `model_unknown_terminal_without_progress` |
| `parallel.rs` | ~270 | Contract-proven read-only batch executor; safety gate per node |
| `safety.rs` | 153 | `SafetyGate::blocks` — 8 block reasons; `ApprovalStore` |
| `repair.rs` | 170 | Failure repair candidate insertion |
| `dependencies.rs` | small | Block-dead-deps cleanup |
| `filesystem_witness.rs` | medium | Workspace snapshot diff (rooted at `workspace_root`); attaches `filesystem_witness` to observation output |
| `write_preconditions.rs` | ~730 | Read-before-write enforcement: ensure file is read at current epoch before writing |
| `stale.rs` | ~600 | Stale-evidence detection |
| `liveness_tests.rs` | large | Proves the loop can never silently spin |
| `support.rs` | 210 | Helper utilities: `success_artifact`, `failure_completion_artifact`, etc. |
| `observation.rs` | 48 | Observation→graph wiring |
| `snapshot.rs` | 17 | Per-run graph snapshot persistence |
| `progress_tests.rs`, `tests.rs`, `retry_tests.rs`, `model_feedback_tests.rs` | 1900+ | Test suites |

## The PESA loop (`runtime::run_goal`)

```
run_goal(goal, options):
  graph    ← PlanGraph::from_goal(goal)        # one Goal node, no actions yet
  beliefs  ← BeliefGraph::new()
  trace    ← append RunStarted, GoalParsed
  seed_actions(...)                            # initial candidates from rule-derived plans
  if observe_act_llm: add_initial_model_candidates(...)

  loop:
    apply_rewrites(graph)                      # 6 rules: risk, observe-before, etc.
    apply_saturation(graph)                    # CanReplace edges + prune dominated
    selected ← scheduler.choose_next_action(graph)
    if selected is None:
      block_actions_with_dead_dependencies()   # cleanup
      if observe_act_llm: add_recovery_candidates() ; continue
      return RunResult::Stalled                # no executable frontier

    if write_precondition_missing(selected):
      ensure_existing_file_read_precondition() ; continue

    trace ← append ActionSelected
    if safety_gate.blocks(selected):
      if options.yolo:
        trace ← append SafetyBlocked{bypassed:true} ; fall through to execute
      else:
        node.status = Blocked
        add_recovery_candidates() ; continue

    if enable_parallel_read_only and parallel_proof:
      execute_many_parallel(batch)
      for obs in observations: process_observation(...)

    else:
      observation ← executor.execute(invocation)
      attach_workspace_change_witness(...)
      maybe_artifact ← process_observation(...)
      if maybe_artifact: return RunResult::Completed

  end loop
```

### `process_observation` decision tree

After every successful execution:

1. Update node status (`Executed` / `Failed`).
2. Record belief observation; advance `state_epoch` if `progress.changes_state`.
3. **Increment `state_advancing_since_goal_check`** if `observation.success` (NEW: forced goal-check counter).
4. Append `ActionExecuted` trace event.
5. If success → satisfy dependent preconditions; attach observation node.
6. Run per-action `verify_observation` (postconditions check) → `VerificationOutcome { required, passed }`.
7. **Generated-artifact review** (`review_generated_artifact_if_needed`) for model-created artifacts.
8. **Verification-action verifies-terminal path**: if this node is a verification-of-Terminal and frontier empty, call `goal_verified_or_expand` → maybe declare completion.
9. **Verification scheduling**: if action's contract has postconditions and not yet verified, add verification candidates.
10. **No-progress trap**: if model proposed an Unknown-side-effects Terminal and produced no progress witness, mark Failed and add recovery.
11. **Terminal completion path**: if `verification.passed` AND `completion_role == Terminal` AND `should_verify_terminal_completion` (state witness present), call `goal_verified_or_expand` → declare completion if satisfied.
12. **Forced goal-check (NEW)**: if counter ≥ `FORCED_GOAL_CHECK_INTERVAL=4` AND verification passed AND not Terminal-classified, force a goal-check; if satisfied, declare completion. Counter resets to 0 either way.
13. Verification cleanup, failure recording, repair-anchor resolution.
14. **Failed-terminal completion**: if `expect_failure` mode and a terminal action failed, declare failure-as-completion.
15. Append model observe-act candidates for next round.

Returns `Some(artifact)` on completion (loop terminates with `RunResult::Completed`), `None` to continue.

## Plan graph

`PlanGraph` is a typed DAG (`graph.rs`).

### Node kinds (`PlanNodeKind`)

`Goal`, `Subgoal`, `Action`, `Observation`, `Failure`, `Repair`, `Verification`, `Artifact`, `Constraint`, `Approval`.

### Node statuses (`PlanStatus`)

`Open` (executable), `Blocked` (safety/dep), `Executed`, `Failed`, `Satisfied`, `Pruned`.

### Edge kinds (`PlanEdgeKind`)

`DecomposesTo`, `Requires`, `Produces`, `Supports`, `Contradicts`, `FailedWith`, `Repairs`, `Verifies`, `Blocks`, `CanReplace`.

`Supports` edges are how the LLM marks completion roles: `edge.payload.completion_role ∈ {terminal, support, verification, repair}`.

### Action scoring (`ActionScores` per node)

Each action node carries an `ActionScores` struct populated by rules and used by the scheduler. The scheduler combines them with `SchedulerWeights` (defaults below) into a single `ScoreBreakdown.total`:

```
default weights:
  expected_progress         1.00
  success_probability       0.60
  success_uncertainty_pen   0.35
  information_gain          0.45
  verification_value        0.40
  reversibility_bonus       0.25
  risk_penalty              1.25
  cost_penalty              0.35
  latency_penalty           0.20
  uncertainty_penalty       0.50
  repeated_failure_penalty  1.00
```

`Scheduler::choose_next_action(graph)` picks the highest-`total` action node with status `Open` whose preconditions are satisfied (`belief.preconditions_satisfied`).

## Contracts and tools

`CapabilityContract` is loaded from YAML resource directories (`resources/tools/*.yaml`) via `puffer-tools`. Each contract has many `ActionContract`s; each action declares:

- `name`, `description`, `input_schema` (JSON Schema)
- `side_effect_class` (`SideEffectClass::{PureObservation, LocalRead, ExternalRead, LocalWrite, ExternalWrite, Communication, DestructiveWrite, FinancialOrLegalEffect, Unknown}`)
- `risk_level`, `idempotency`, `reversibility`
- `preconditions`, `postconditions`
- `approval.required` (boolean)
- `structured_argument_safety` (per-arg block/approval rules)

The default contracts loaded for benchmarks: `puffer.tools` (the bundled puffer-tools resources). Optional contracts: `burbot.symbolic` (symbolic workers).

## Rewrite engine

`RewriteEngine::universal()` (`rules.rs:40`) returns 6 rules applied in order each loop tick:

1. **`UnknownSideEffectsIncreaseRisk`** — bumps `uncertainty_penalty` and `risk_penalty` for actions whose contract has `Unknown` side-effect class.
2. **`ObserveBeforeRiskyAction`** — adds `Requires` edges from risky actions to a cheap observation step.
3. **`RequireApprovalForExternalWrite`** — inserts an `Approval` node for ExternalWrite/Communication/financial actions.
4. **`VerifyBeforeCompletion`** — ensures Terminal actions have a `Verifies` follow-up.
5. **`BlockRepeatedFailure`** — marks repeatedly failing action shapes as `Blocked`.
6. **`InsertRepairCandidatesFromContract`** — when a failure has a contract-declared repair recipe, instantiate the repair candidate.

## Saturation

`saturation.rs::saturate_guarded_substitutions` (default, no `egg`):

1. Group open action candidates into equivalence classes by `(normalized_intent, side_effect_class)` plus the contract-derived dominance ordering.
2. Emit `CanReplace` edges from each non-dominated member to dominated ones.
3. Prune dominated open actions (status → `Pruned`).

Optional `egg-optimizer` feature additionally runs `add_read_only_equivalence_edges` (`egg_optimizer.rs`):

1. Lower each open read-only candidate to a `RecExpr<SymbolLang>`.
2. Insert into an `EGraph`, run `Runner` with rewrite rules (iter limit 6, node limit 10000).
3. For each resulting e-class with multiple node-IDs, emit `CanReplace` edges across the class.

The egg path is opt-in; the deterministic hand-written path always runs.

## Safety gate (`runtime/safety.rs`)

`SafetyGate::blocks(node, ctx)` returns `Option<BlockReason>` checked in priority order:

1. `ExternalWriteApprovalRequired` — `SideEffectClass::ExternalWrite` without approval.
2. `CommunicationApprovalRequired` — `SideEffectClass::Communication` without approval.
3. `DestructiveActionBlocked` — `DestructiveWrite` (hard veto, no approval path).
4. `FinancialOrLegalActionBlocked` — `FinancialOrLegalEffect` (hard veto).
5. `ArgumentBlocked` — payload arg matches `BlockPathPrefix` or `BlockParentTraversal` rule (hard veto).
6. `ArgumentApprovalRequired` — payload arg matches `RequireApprovalPathComponent` rule.
7. `ApprovalRequired` — action contract `approval.required = true` without approval.
8. `UnknownSideEffectsNeedPreconditions` — `Unknown` side-effects without satisfied preconditions.

When blocked, the runtime sets node status `Blocked`, emits a `SafetyBlocked` trace event, and asks the LLM for recovery candidates. `--yolo` (NEW) skips the block: emits `SafetyBlocked { bypassed: true }` and falls through to execute.

## LLM integration

Burbot uses `puffer-provider-openai` for OpenAI-compatible HTTP transport (Codex Responses API, OpenAI Chat Completions, DeepSeek Chat Completions, etc.). It owns its own `reqwest::blocking::Client` with wall-clock timeout, retry/backoff, and OAuth refresh.

### Four LLM call sites

| Function | When called | Returns |
|---|---|---|
| `propose_puffer_tool_call` | Single-tool path (free-text goal, no observation context yet) | One `PufferToolCallProposal { tool_id, args }` |
| `propose_observe_act_candidates` | Multi-candidate proposal each loop tick | `Vec<ModelCandidateProposal>` |
| `verify_goal_satisfied` | Forced goal-check + Terminal completion path | `GoalVerificationResult { satisfied, confidence, missing_evidence, suggested_candidates, rejected_suggested_candidates }` |
| `review_generated_artifact` | Verifying a model-created artifact | Same shape as `verify_goal_satisfied` |

### Endpoint selection (`llm.rs`)

- `is_codex_backend(base_url)` → `/responses` POST with Codex-specific headers (`version: 0.125.0`).
- `supports_responses_api && images empty` → `/v1/responses`.
- `supports_responses_api && images present` → `/v1/responses` with multimodal content.
- Else (DeepSeek, OpenRouter, etc.) → `/v1/chat/completions`. `BURBOT_OPENAI_USE_CHAT_TOOL_CALLS` env var controls whether to attempt tool-call format vs JSON object response_format.

Defaults: 300s wall-clock per call (`BURBOT_OPENAI_TIMEOUT_SECS`), 3 retries (`BURBOT_OPENAI_RETRY_ATTEMPTS`), exponential-ish backoff.

### Prompt structure (`llm/schema.rs`)

All prompts share a stable prefix structure: `[~3KB instructions] + [tools catalog JSON] + [Goal: <text>] + [Structural context JSON]`. Only the trailing structural context varies per call; the prefix is constant within a run.

`observe_act_prompt` includes: instructions, tool catalog, goal, then `observation_context` (varies — has `frontier`, `history`, `generated_artifacts`).

`goal_verification_prompt` includes: instructions, tool catalog, goal, then `verification_context`. As of the current snapshot, `frontier` is **omitted** from `verification_context` (a deliberate fix — the verifier was reading frontier as a TODO list and rejecting completion based on speculative future work).

### Credential resolution (`llm.rs::resolve_openai_credential`)

In order:
1. `BURBOT_OPENAI_AUTH_SOURCE=codex|local-codex|codex-oauth|local-codex-oauth` → forces local Codex credential discovery.
2. `OPENAI_API_KEY` env var → API key auth, default base URL `https://api.openai.com` (overridable via `OPENAI_BASE_URL`).
3. `~/.config/puffer/auth.json` (or platform equivalent) → Puffer's OpenAI credential.
4. Local Codex credentials at `~/.codex/auth.json` (auto-detect via `puffer-provider-registry::detect_import_candidates`).

## Forced goal-check (NEW, runtime.rs)

The original Terminal-completion path required the LLM to mark a candidate `completion_role: Terminal`. On orchestration tasks (configure-git-webserver, etc.) the LLM rarely or never tags anything Terminal — so `goal_verified_or_expand` was never called and the loop spun.

The forced check fires periodically:

```rust
const FORCED_GOAL_CHECK_INTERVAL: u64 = 4;

// In process_observation, after Terminal completion path didn't fire:
if observation.success
    && verification.passed
    && options.enable_observe_act_llm
    && completion_role != Some(CompletionRole::Terminal)
    && self.state_advancing_since_goal_check >= FORCED_GOAL_CHECK_INTERVAL
{
    self.state_advancing_since_goal_check = 0;
    if self.goal_verified_or_expand(...)? {
        // emit CompletionDeclared{ forced_goal_check: true }
        return Ok(Some(success_artifact(...)));
    }
}
```

The counter increments on every successful observation (not gated on `has_state_witness`, which would never fire for actions modifying paths outside `workspace_root`).

## Yolo mode (NEW, cli.rs + runtime.rs + safety.rs)

`burbot run --yolo` bypasses the safety gate. When `options.yolo` is true and `SafetyGate::blocks` returns `Some(reason)`, the runtime emits `SafetyBlocked { reason, bypassed: true }` and falls through to execute the action. Without yolo, a blocked node has its status set to `Blocked` and the runtime asks the LLM for recovery candidates.

Yolo is intended for sandboxed benchmarks (Docker containers, ephemeral VMs). It does **not** disable contract validation, scheduler scoring, or rewrite rules — only the safety gate.

Plumbed through `benchmark/burbot_harbor_agent.py` and `benchmark/run_tb2.py` as `--burbot-yolo`.

## Trace format

`trace.rs` defines `TraceEvent`:

```rust
pub struct TraceEvent {
    trace_id: TraceId,
    run_id: RunId,
    timestamp_ms: u64,
    event_type: TraceEventType,
    plan_node_id: Option<NodeId>,
    contract_id: Option<String>,
    action_name: Option<String>,
    input: Value,
    output: Value,
    success: Option<bool>,
    failure_mode: Option<...>,
    cost: Option<f64>,
    latency_ms: Option<u64>,
    metadata: Value,
}
```

### Event types

`run_started`, `goal_parsed`, `contract_loaded`, `node_added`, `model_candidates_proposed`, `rewrite_applied`, `action_scored`, `action_selected`, `parallel_batch_selected`, `safety_blocked`, `action_executed`, `observation_attached`, `failure_classified`, `repair_added`, `verification_performed`, `goal_verification_performed`, `artifact_review_performed`, `completion_declared`, `run_finished`, `mutation_proposed`, `mutation_evaluated`, `mutation_promoted`.

Traces are written as JSONL to `<workspace>/.puffer/burbot/traces/<run_id>.jsonl` (one line per event) plus a per-run graph snapshot at `<workspace>/.puffer/burbot/graphs/<run_id>.json`.

## CLI

```
burbot
├── contract     # validate / inspect tool contracts
├── run          # run one goal end-to-end
│   ├── --goal <text>
│   ├── --tools <dir>
│   ├── --model <name>
│   ├── --llm-tool-call / --no-llm-tool-call
│   ├── --puffer-tool / --puffer-args
│   ├── --expect-failure
│   ├── --symbolic-workers
│   ├── --parallel-read-only
│   └── --yolo                       # NEW
├── trace
│   ├── list
│   └── show <run_id>
├── eval
│   └── (suite runner, see evals/suites/*.yaml)
├── evolve       # propose contract mutations from traces
└── llm
    └── probe    # smoke-test credentials
```

`RunOptions` (`runtime/model.rs`):

```rust
pub struct RunOptions {
    puffer_tool: Option<String>,
    puffer_args: Option<Value>,
    puffer_tool_source: CandidateSource,
    allow_failed_terminal_completion: bool,
    enable_symbolic_workers: bool,
    enable_parallel_read_only: bool,
    enable_observe_act_llm: bool,
    model: Option<String>,
    goal_verification_min_confidence: f64,  // CLI default 0.4 (was 0.75)
    yolo: bool,                              // NEW
}
```

## Eval / benchmark integration

- `evals/suites/*.yaml` define multi-task suites consumed by `burbot eval`.
- `benchmark/run_tb2.py` runs Terminal Bench 2.0 tasks via Harbor, mounting the local `target/debug/burbot` binary into Docker containers.
- `benchmark/burbot_harbor_agent.py` is the Harbor `BaseInstalledAgent` adapter. As of this snapshot, the in-container exec is **pure bash** (was Python heredoc; the rewrite eliminated dependency on `python3` being present in the task container). Result/trajectory JSON is emitted host-side from `populate_context_post_run` reading `burbot.txt` + `burbot.rc`.

## Known limitations and observed failure modes

These are honest observations from running `configure-git-webserver` and `chess-best-move` against `deepseek-v4-pro`.

### 1. Single-attempt model-conservative tasks fail to declare completion

Multi-step orchestration goals (no single creating action) often run to functional completion but never trigger `CompletionDeclared`. The forced goal-check (NEW) addresses this partly, but the verifier still over-emphasizes missing evidence with non-frontier models.

### 2. Workspace witness is rooted at `workspace_root`

`progress.has_state_witness` requires `filesystem_witness` to show changes inside the workspace snapshot. Tasks that modify `/etc`, `/var/www`, `/usr` get no state witness, so `progress.changes_state` stays false, the state epoch doesn't advance, and many heuristics that branch on state-witness behave as if no progress happened.

### 3. Vision-required tasks need vision-capable endpoints

`chess-best-move` requires reading a PNG. Burbot's LLM layer forwards image data URLs into observation context but does **not** check whether the configured base URL supports multimodal. DeepSeek's chat-completions rejects `image_url` content parts → all subsequent propose calls 400 → stall.

### 4. Schema-rejection density grows with run length

Late-run propose rounds increasingly return `added: 0` due to:
- Duplicate `(tool, args)` shapes (LLM re-proposes the same Write).
- Hallucinated `depends_on: [...]` IDs (LLM cites IDs from previously-pruned candidates).
- `Support` candidates after the support phase is exhausted (`non_state_advancing_candidate_after_probe_saturated`).
- Empty proposals (LLM fails schema or gives up).

The runtime rejects each correctly, but has no inline-retry to repair the LLM's output, so each rejection costs a full LLM call. Over a 174-event trace we observed 25 of 68 propose rounds returning zero.

### 5. Stateless re-prompting

Each propose call rebuilds the full context (instructions + tools + history + frontier + artifacts) from the graph. Provider auto-prefix-caching helps with the stable head, but the call is still self-contained — there is no per-run conversation thread. Over long runs the LLM's outputs degrade (context decay), increasing hallucinated dependency IDs and duplicate proposals.

### 6. Goal-verifier thresholds couple loosely

`goal_verified_or_expand` requires `satisfied=true && confidence ≥ threshold && missing_evidence.is_empty()` — three AND'd conditions. With smaller models, `satisfied=false` fires whenever the LLM names *anything* missing, regardless of whether it's actually required. Lowering the confidence threshold alone doesn't help.

### 7. Harness 900s timeout vs verifier-pleasing polish

In Harbor, the agent execution timeout is 900s. Burbot frequently runs functional task completion in under 14 minutes but keeps polishing (perms, follow-up checks) past the wall and gets killed before the verifier runs. The forced goal-check fix is meant to short-circuit polish loops, but only fires when its preconditions are met (counter ≥ 4 *successful state-advancing* actions since the last GV).

## Recent changes in this snapshot (uncommitted)

Compared to `HEAD = 55cbe35` ("Add Burbot OpenAI-compatible chat fallback"):

| Change | Files | Purpose |
|---|---|---|
| Forced goal-check counter + branch | `runtime.rs` | Trigger `goal_verified_or_expand` periodically when no Terminal candidate has appeared |
| Constant `FORCED_GOAL_CHECK_INTERVAL = 4` | `runtime.rs` | Tunable threshold |
| Counter increments on `observation.success` (not `has_state_witness`) | `runtime.rs` | Workspace-rooted witness misses out-of-workspace edits |
| `--yolo` CLI flag | `cli.rs` | Bypass safety gate for sandboxed bench |
| `RunOptions::yolo: bool` | `runtime/model.rs` + all construction sites | Plumbing |
| Safety gate yolo bypass with trace | `runtime.rs` | Emit `SafetyBlocked { bypassed: true }` and fall through |
| Goal-confidence default lowered to 0.4 | `cli.rs` | (was 0.75 — found to not be the bottleneck) |
| Verifier context: `frontier` removed | `runtime/model_loop.rs:805-814` | Verifier was treating frontier as TODO and rejecting completion |
| Goal-verifier prompt updated | `llm/schema.rs:282` | Tells verifier to judge from `history`/`verified_target`/`generated_artifacts` only |
| Observe-act prompt: Terminal=demonstration nudge | `llm/schema.rs:230` | Encourages tagging the e2e demonstration as Terminal in orchestration tasks |
| Harness rewrite: pure bash exec, host-side JSON | `benchmark/burbot_harbor_agent.py` | Eliminates `python3` requirement in task container |
| `--burbot-yolo` flag | `benchmark/run_tb2.py` | Wires `--yolo` through the harness |

## Pointers for future work

The redesign discussed during this session (cache-aware PESA) proposes:

1. **Cache observability**: surface `usage.prompt_cache_hit_tokens` per LLM call.
2. **Deterministic JSON ordering**: sort all map keys serialized into prompt prefixes.
3. **Constraint surface protocol**: add `available_dependency_ids`, `forbidden_completion_roles`, `existing_action_signatures` to propose context so the LLM can avoid the rejection patterns mechanically.
4. **Inline validate-and-retry**: when LLM returns invalid candidates, retry within the same propose call with violation feedback (bounded ≤ 2 retries).
5. **Liveness backstop**: after N consecutive zero-add propose rounds, force a goal-check, then a single safe Bash, then stall with a labeled reason.
6. **Per-run propose-thread**: maintain a single chat-completions conversation per run; turn N is "EVENT: action X executed, output Y. Frontier delta: ..." appended to the transcript. Replaces stateless rebuild.
7. **Bounded context with rolling summary**: cap history at 8, summarize the rest every K turns.

These are layered: 1+2 are enabling/diagnostic, 3+4 attack the rejection density, 5 prevents silent stalls, 6 is the architectural shift, 7 caps prompt growth.

## File layout

```
crates/puffer-burbot/
├── Cargo.toml
├── burbot.md            (this file)
└── src/
    ├── lib.rs                 main.rs
    ├── cli.rs                 contract.rs       graph.rs        belief.rs
    ├── ids.rs                 planner.rs        plan_synthesis.rs
    ├── puffer_tools.rs        rules.rs          saturation.rs   egg_optimizer.rs
    ├── scheduler.rs           executor.rs       verification.rs failure.rs
    ├── semantics.rs           symbolic.rs       trace.rs        graph_store.rs
    ├── stats.rs               calibration.rs    model_policy.rs
    ├── eval.rs                evolve.rs         promotion.rs    llm.rs
    ├── llm/
    │   ├── chat.rs            schema.rs         parse.rs
    │   ├── openai_error.rs    policy_tests.rs   tests.rs
    └── runtime/
        ├── runtime.rs (parent)            model.rs
        ├── model_loop.rs                  model_loop_support.rs
        ├── model_candidates.rs            model_feedback.rs     model_retry.rs
        ├── artifact_context.rs            artifact_review.rs
        ├── progress.rs                    parallel.rs
        ├── safety.rs                      repair.rs              dependencies.rs
        ├── filesystem_witness.rs          write_preconditions.rs
        ├── stale.rs                       liveness_tests.rs      support.rs
        ├── observation.rs                 snapshot.rs
        └── (test files)
```

---

*Snapshot taken 2026-04-30 on branch `feature/burbot-pesa-runtime`. HEAD: `55cbe35`. 56 modified/added files in working tree, ~4844 insertions / 1394 deletions vs HEAD. Generated alongside benchmark eval against `deepseek-v4-pro` on Terminal Bench 2.0 `configure-git-webserver`.*
