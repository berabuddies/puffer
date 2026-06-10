//! AutoDream background memory consolidation.
//!
//! AutoDream is an orchestration layer over project memory: it periodically
//! runs a restricted side turn that can load the active project memory, update
//! it through the `Memory` tool, and report whether the recent trace looks
//! valuable enough to become a generated skill.

#[path = "autodream/status.rs"]
mod status;

use self::status::{
    autodream_dir, read_autodream_run_status, read_autodream_state, write_autodream_run_status,
    write_autodream_state, AutoDreamGenskillSuggestion, AutoDreamMemoryChange,
    AutoDreamRunStatusFile,
};
use crate::runtime::{RequestToolFilter, ToolInvocation};
use crate::AppState;
use anyhow::{anyhow, Context, Result};
use fslock::LockFile;
use glob::Pattern;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, SessionSummary, TranscriptEvent};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

const MIN_GENSKILL_SUGGESTION_MESSAGES: usize = 4;

const AUTODREAM_PROMPT: &str = r#"Run AutoDream for the active project. Treat the conversation above as short-term working memory and consolidate only durable project knowledge.

Phase 1: Orient.
- Load the active project memory with the project-memory skill before deciding what to change.
- Use only the exact MEMORY.md path exposed by the project-memory skill or current memory context. Do not guess alternate memory paths; if a read is denied, retry once with the exact project-memory path before concluding you are blocked.
- Identify existing durable facts, workflow bullets, stale conflicts, and possible polluted entries.
- Treat memory as an explicit patch. For every candidate, choose exactly one action: keep, add, replace, or remove.

Phase 2: Gather recent signal.
- Review the current transcript for durable candidates. Do not wait for the user to say "remember this".
- Prefer candidates confirmed by user correction, tool output, passing verification, stable repository rules, compatibility constraints, repeated workflow shape, or an accepted next-step plan.
- Real traces often contain useful facts mixed with chatter. Extract short durable facts from verified eval results, reusable commands, frozen baselines, known unrelated blockers that affect future test interpretation, stable repo constraints, and user-approved workflows.
- External or benchmark-style noisy trajectories are still valid signal. Do not skip memory merely because a trace is external, failed, unsolved, synthetic-looking, or full of tool noise; if it shows a reusable diagnose-filter-verify method, write one generalized workflow memory entry.
- For external noisy trajectories, generalize away dataset names, task ids, agent/model names, exact file paths, exact commands, temporary paths, secrets, flags, payloads, benchmark artifacts, and incorrect step ids. Preserve the workflow shape: how to inspect, avoid false starts, verify the final state, and decide what not to carry forward.
- For external trajectories, respect the trajectory category when naming workflow memory. Do not default to software-engineering for system-administration, machine-learning, scientific-computing, games, debugging, security, or file-operation traces.
- For external noisy bug-fix traces, do not stop at a single exact bug fact when the trace also shows a reusable method. Pair the fact with one generalized workflow entry that captures how to localize the failing behavior, inspect the relevant implementation and tests, patch minimally, verify with the focused regression, and discard incidental paths, task ids, and failed probes.
- Keep baseline and blocker memories as facts, not workflows. A baseline, metric checkpoint, known test blocker, or "next step" plan is not GenSkill-worthy by itself.
- Extract reusable workflows when the trace shows a stable method with multiple actions, such as corpus expansion, eval hardening, classifier tuning, prompt tuning, or real-trace rollout.
- For failed or unsolved traces, write memory only when there is a durable recovery lesson or anti-imitation method, such as how to classify false starts, avoid repeating bad tool steps, or verify that a final artifact/result is real.
- When the transcript explicitly labels a verified "Durable workflow:" with four or more reusable actions and a validation signal, write that workflow memory unless the user negates it.
- Suspect first, then narrow: list likely durable candidates mentally, then keep only the ones supported by concrete evidence in the transcript.

Phase 3: Consolidate memory.
- Prefer short imperative entries that preserve exact commands, crate names, file paths, API names, model names, gates, and compatibility constraints needed for future work.
- For durable command-shape memories, preserve reusable flags and knobs that future runs must choose intentionally, including provider/model selectors, runner paths, corpus paths, and explicit concurrency such as `--jobs` or jobs concurrency.
- For workflow memory, store the stable method rather than checkpoint telemetry. Omit version-by-version scores, temporary run ids, job counts, and exploratory run parameters unless they are the durable command or frozen baseline name.
- If no project-specific fact is durable but a reusable external-trace method is evident, prefer one generalized workflow memory entry over writing nothing. The entry should name the domain, include 4-6 stable actions, include a verification condition, and explicitly exclude the noise class without copying the noisy string.
- For external noisy traces, the fallback memory entry should begin with a clear domain phrase such as "Noisy external software-engineering workflow", "Noisy external file-operation workflow", or "Noisy external security workflow" so later scoring and humans can recognize it as durable workflow memory.
- When both an exact durable bug fact and a reusable external workflow are supported, write both only if the exact fact is likely to recur; otherwise prefer the workflow entry. A workflow-worthy external trace should not end with only a narrow file/API fact.
- If any verified durable candidate exists, write at least one memory entry unless the transcript is purely noise or the user explicitly says not to remember it.
- When replacing, `old_text` must be copied from the existing MEMORY.md entry that was loaded by the project-memory skill. Do not use transcript wording such as "the old note is stale" as `old_text`.
- Replacement is complete only when the stale MEMORY.md entry is gone and the new standalone memory entry captures the verified replacement fact.
- If a replace call fails because no entry matched, immediately retry with the exact stale entry text from MEMORY.md. If there is no stale memory entry, use add instead of replace.
- Never keep both sides of a conflict. Prefer the latest verified workflow over old notes, but preserve unrelated memory entries.
- For reusable multi-step workflows, write one durable workflow memory bullet with the Memory tool before deciding on GenSkill. The bullet must start with the workflow domain and include 4-6 stable actions plus the verification command or success signal when available.
- Do not count the workflow as captured if it only appears in the final response. If you will say `AUTODREAM_GENSKILL: yes`, first ensure the workflow bullet is present in MEMORY.md.

Phase 4: Prune, normalize, and decide GenSkill.
- Do not save temporary task progress, one-off local paths, rate limits, transient network failures, shell typos, unverified guesses, abandoned hypotheses, raw selector/API samples from a failed probe, exact run ids unless they are a named frozen baseline, worker names, or details the user said not to remember.
- Do not store meta-instructions such as "do not skill this", "do not remember this", "no GenSkill", or "not a workflow". Use those phrases only to suppress the suggested skill or exclude the named detail from memory.
- Do not invent a durable workflow from a failed tool call, a prompt-tuning attempt, or a rejected Memory edit unless the user explicitly asks to keep the recovered method and a later tool result verifies it.
- When a transcript contains a useful recovery method mixed with local failures, abstract the failure class and omit exact local error strings, bad paths, run ids, worker counts, machine limits, or stale-binary messages.
- GenSkill is separate from memory. Even when `AUTODREAM_GENSKILL: no` is correct, still write durable workflow memory if the generalized method would help future work.
- Decide GenSkill after memory edits. A trace can be GenSkill-worthy even when it is mixed with noise, failed hypotheses, or one-off tool errors; ignore those and judge the reusable workflow that remains.
- For project-native Puffer workflows, use the saved memory entry as the main GenSkill signal. If this pass wrote or preserved a durable workflow entry with a named domain, four or more reusable actions, and a validation or acceptance condition, say `AUTODREAM_GENSKILL: yes` unless a negative rule below applies. This includes eval label-audit workflows, real-trace rollout workflows, classifier/prompt tuning workflows, subagent merge/consolidation workflows, and regression triage workflows.
- Do not require repeated historical examples for project-native workflow memory that already describes a reusable procedure. The fact that the workflow was durable enough to write to MEMORY.md is evidence that it should be considered for GenSkill.
- For external benchmark traces, writing a generalized workflow memory entry is not enough by itself to suggest GenSkill. Suggest GenSkill only when the trace also shows nontrivial transfer value: repeated false starts, incorrect-step filtering, multi-stage recovery, cross-task methodology, a long-tail/unsolved recovery workflow, or a validation pattern that future agents would otherwise rediscover.
- Treat clean/control-style external traces as negative for GenSkill. If trajectory metadata says `noise_band=clean`, `clean_control=true`, or `selection=full clean control`, say `AUTODREAM_GENSKILL: no` even if you wrote durable workflow memory; this clean-control signal overrides `solved=false`, long-tail wording, and generic reusable workflow shape unless the transcript explicitly labels the workflow as skill-worthy or shows repeated exploration failures.
- For non-clean external traces with `incorrect_error_stage_count>0`, `solved=false`, long-tail step counts, or `noise_band=high|medium|unsolved`, say `AUTODREAM_GENSKILL: yes` when you wrote a durable workflow entry with four or more actions plus a verification signal. Do not require broader transfer evidence beyond that saved workflow for high-noise or unsolved external traces.
- Say `AUTODREAM_GENSKILL: yes` only when a durable workflow memory entry was actually written in this pass or already exists in MEMORY.md, and at least four of these are true: the memory entry describes four or more reusable steps; tool output or verification confirmed the workflow; the workflow applies to future tasks beyond this exact run; the workflow would reduce future search or repeated exploration; it is more than one command or one project convention; the trace shows at least two false starts, incorrect steps, recovery pivots, cross-file/tool phases, or an unsolved/long-tail recovery pattern.
- Say `AUTODREAM_GENSKILL: no` for single commands, one-off conventions, ordinary durable facts, known blocker notes, frozen baseline notes, clean/control-style external tasks, or workflows lacking verification.
- If the marker is `AUTODREAM_GENSKILL: no`, do not say the trace is skill-worthy, do not mention `/genskill`, and do not include any other positive skill suggestion language.
- Before the final response, verify MEMORY.md after edits. If replacement new facts or workflow memory are missing, fix MEMORY.md before answering.
- Keep the final response under 120 words and put `AUTODREAM_GENSKILL: yes` or `AUTODREAM_GENSKILL: no` on its own final line exactly once."#;

const PROJECT_MEMORY_TOOL_ID: &str = "Memory";
const PROJECT_MEMORY_READ_TOOL_ID: &str = "Read";
const PROJECT_MEMORY_SKILL_TOOL_ID: &str = "Skill";
const PROJECT_MEMORY_SKILL_NAME: &str = "project-memory";
const SESSION_SCAN_INTERVAL_MS: u64 = 10 * 60 * 1000;
const MAX_RECENT_SESSIONS: usize = 5;
const MAX_EVENTS_PER_SESSION: usize = 16;
const MAX_SNIPPET_CHARS: usize = 260;
const MAX_SESSION_CONTEXT_CHARS: usize = 8_000;
const MAX_RUN_SUMMARY_CHARS: usize = 360;
const MAX_MEMORY_CHANGE_CHARS: usize = 220;

struct AutoDreamLock {
    _file: LockFile,
}

struct AutoDreamSideTurn {
    state: AppState,
    resources: LoadedResources,
    filter: Option<RequestToolFilter>,
}

struct RecentSessionContext {
    text: String,
    session_count: usize,
}

/// Result of one AutoDream consolidation pass.
#[derive(Debug, Clone)]
pub struct AutoDreamOutcome {
    /// Final text returned by the AutoDream side turn.
    pub assistant_text: String,
    /// Tool calls made while consolidating memory.
    pub tool_invocations: Vec<ToolInvocation>,
    /// Whether the side turn marked the trace as worthy of genskill.
    pub genskill_suggested: bool,
}

/// Describes project-memory initialization performed for a manual AutoDream run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManualAutoDreamBootstrap {
    /// User-visible initialization message to prepend to the AutoDream result.
    pub message: String,
    /// True when this invocation created project memory for the current cwd.
    pub initialized_project_memory: bool,
}

/// Runs one AutoDream consolidation pass synchronously.
pub fn run_autodream_review(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
) -> Result<AutoDreamOutcome> {
    run_autodream_review_with_context(state, resources, providers, auth_store, None)
}

/// Initializes project memory for an explicit manual AutoDream run.
pub fn ensure_manual_autodream_project_memory(
    state: &mut AppState,
) -> Result<ManualAutoDreamBootstrap> {
    if !state.memory_enabled() || state.project_memory.is_some() {
        return Ok(ManualAutoDreamBootstrap::default());
    }
    let Some(context) = crate::memory::activate_project_memory(state)? else {
        return Ok(ManualAutoDreamBootstrap::default());
    };
    Ok(ManualAutoDreamBootstrap {
        message: format!(
            "Initialized project memory at {}.\n\n",
            context.memory_file.display()
        ),
        initialized_project_memory: true,
    })
}

/// Returns AutoDream assistant text without internal machine-readable markers.
pub fn visible_autodream_assistant_text(text: &str) -> String {
    text.lines()
        .filter(|line| {
            !line
                .trim_start()
                .to_ascii_uppercase()
                .starts_with("AUTODREAM_GENSKILL:")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Returns true when a manual AutoDream result should prompt the user to run GenSkill.
pub fn should_show_manual_autodream_genskill_suggestion(
    state: &AppState,
    bootstrap: &ManualAutoDreamBootstrap,
    genskill_detected: bool,
) -> bool {
    let enough_context = state
        .transcript
        .iter()
        .filter(|message| {
            matches!(
                message.role,
                crate::MessageRole::User | crate::MessageRole::Assistant
            )
        })
        .count()
        >= MIN_GENSKILL_SUGGESTION_MESSAGES;
    !bootstrap.initialized_project_memory
        && enough_context
        && state.autodream_genskill_suggestions_enabled()
        && genskill_detected
}

/// Renders the user-visible result for a manual AutoDream run.
pub fn render_manual_autodream_result(
    bootstrap: &ManualAutoDreamBootstrap,
    outcome: &AutoDreamOutcome,
    should_suggest_genskill: bool,
) -> String {
    let mut sections = Vec::new();
    let bootstrap_message = bootstrap.message.trim();
    if !bootstrap_message.is_empty() {
        sections.push(bootstrap_message.to_string());
    }
    if bootstrap.initialized_project_memory {
        sections.push("Project memory ready.".to_string());
    }
    sections.push("AutoDream complete.".to_string());
    sections.push(concise_autodream_summary(&outcome.assistant_text));
    if should_suggest_genskill {
        sections.push(
            "Reusable workflow found. Review the suggestion menu to create a skill draft."
                .to_string(),
        );
    }
    sections.join("\n")
}

fn concise_autodream_summary(text: &str) -> String {
    let visible = visible_autodream_assistant_text(text);
    let normalized = visible.to_ascii_lowercase();
    if visible.trim().is_empty() {
        return "Memory checked.".to_string();
    }
    let reports_no_changes = normalized.contains("no stale")
        || normalized.contains("no new durable")
        || normalized.contains("no durable")
        || normalized.contains("no changes")
        || normalized.contains("no updates")
        || normalized.contains("kept the existing")
        || normalized.contains("memory verified");
    if reports_no_changes {
        return "Memory checked. No durable changes needed.".to_string();
    }
    let reports_update = normalized.contains("updated")
        || normalized.contains("added")
        || normalized.contains("replaced")
        || normalized.contains("removed")
        || normalized.contains("saved");
    if reports_update {
        return "Project memory updated.".to_string();
    }
    "Memory checked.".to_string()
}

fn run_autodream_review_with_context(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    session_context: Option<&str>,
) -> Result<AutoDreamOutcome> {
    if !state.memory_enabled() {
        return Err(anyhow!("AutoDream requires project memory to be enabled."));
    }
    let Some(mut side_turn) = prepare_autodream_side_turn(state, resources)? else {
        return Err(anyhow!(
            "AutoDream requires an active configured project memory file."
        ));
    };
    let prompt = autodream_prompt_with_session_context(session_context);
    let turn = crate::runtime::execute_user_prompt_with_tool_filter(
        &mut side_turn.state,
        &side_turn.resources,
        providers,
        auth_store,
        &prompt,
        side_turn.filter.as_ref(),
    )?;
    let genskill_suggested = parse_genskill_marker(&turn.assistant_text);
    Ok(AutoDreamOutcome {
        assistant_text: turn.assistant_text,
        tool_invocations: turn.tool_invocations,
        genskill_suggested,
    })
}

/// Spawns one AutoDream consolidation pass on a background thread.
pub fn spawn_autodream_review(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) {
    spawn_autodream_review_inner(state, resources, providers, auth_store, None);
}

/// Spawns one gated AutoDream consolidation pass on a background thread.
pub fn spawn_autodream_review_with_store(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
) {
    spawn_autodream_review_inner(state, resources, providers, auth_store, Some(session_store));
}

/// Records a completed turn and reports whether AutoDream should run.
pub fn autodream_turn_completed(state: &mut AppState) -> bool {
    if !state.memory_enabled() || !state.autodream_enabled() || state.project_memory.is_none() {
        return false;
    }
    state.autodream_review_turns += 1;
    if state.autodream_review_turns >= state.autodream_interval() {
        state.autodream_review_turns = 0;
        true
    } else {
        false
    }
}

/// Records a completed turn and reports whether gated AutoDream should run.
pub fn autodream_turn_completed_with_store(
    state: &mut AppState,
    session_store: &SessionStore,
) -> bool {
    if !state.memory_enabled() || !state.autodream_enabled() || state.project_memory.is_none() {
        return false;
    }
    state.autodream_review_turns += 1;
    if state.autodream_review_turns < state.autodream_interval() {
        false
    } else {
        state.autodream_review_turns = 0;
        match autodream_gates_open(state, session_store) {
            Ok(decision) => {
                state.autodream_last_skip_reason = decision.skip_reason;
                decision.should_run
            }
            Err(error) => {
                state.autodream_last_skip_reason = Some(format!("gate_error: {error}"));
                false
            }
        }
    }
}

/// Renders a short status block for `/autodream status`.
pub fn autodream_status(state: &AppState) -> String {
    format!(
        "enabled={}\ninterval={}\nmin_hours={}\nmin_sessions={}\ngenskill_suggestions={}\nturns_until_next={}\nproject_memory={}\nlast_skip_reason={}",
        state.autodream_enabled(),
        state.autodream_interval(),
        state.autodream_min_hours(),
        state.autodream_min_sessions(),
        state.autodream_genskill_suggestions_enabled(),
        state
            .autodream_interval()
            .saturating_sub(state.autodream_review_turns),
        if state.project_memory.is_some() {
            "available"
        } else {
            "unavailable"
        },
        state.autodream_last_skip_reason.as_deref().unwrap_or("none"),
    )
}

/// Renders AutoDream configuration plus the last persisted background run status.
pub fn autodream_status_with_store(state: &AppState, session_store: &SessionStore) -> String {
    let base = autodream_status(state);
    match read_autodream_run_status(session_store.root()) {
        Ok(Some(status)) => format!(
            "{base}\nbackground_status={}\nbackground_updated_at_ms={}\nbackground_sessions_reviewed={}\nbackground_tool_calls={}\nbackground_genskill_suggested={}\nbackground_summary={}",
            status.status,
            status.updated_at_ms,
            status.sessions_reviewed,
            status.tool_calls,
            status.genskill_suggested,
            status.summary,
        ) + &render_status_memory_changes(&status)
            + &render_status_genskill_suggestion(&status),
        Ok(None) => format!("{base}\nbackground_status=none"),
        Err(error) => format!("{base}\nbackground_status=unavailable\nbackground_error={error}"),
    }
}

/// Renders queued AutoDream GenSkill suggestions for user review.
pub fn autodream_suggestions_with_store(session_store: &SessionStore) -> String {
    match read_autodream_run_status(session_store.root()) {
        Ok(Some(status)) => match status.genskill_suggestion {
            Some(suggestion) => format!(
                "AutoDream GenSkill suggestions:\n- id={}\n  status={}\n  created_at_ms={}\n  memory_changes={}\n  rationale={}",
                suggestion.id,
                suggestion.status,
                suggestion.created_at_ms,
                suggestion.memory_changes,
                suggestion.rationale,
            ),
            None => "AutoDream GenSkill suggestions: none".to_string(),
        },
        Ok(None) => "AutoDream GenSkill suggestions: none".to_string(),
        Err(error) => format!("AutoDream GenSkill suggestions unavailable: {error}"),
    }
}

fn spawn_autodream_review_inner(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    session_store: Option<&SessionStore>,
) {
    if !state.memory_enabled() || !state.autodream_enabled() || state.project_memory.is_none() {
        return;
    }
    let state = state.clone();
    let resources = resources.clone();
    let providers = providers.clone();
    let mut auth_store = auth_store.clone();
    let session_store = session_store.cloned();
    let session_root = session_store
        .as_ref()
        .map(|store| store.root().to_path_buf());
    thread::spawn(move || {
        let lock = session_root
            .as_deref()
            .and_then(|root| acquire_autodream_lock(root).ok().flatten());
        if session_root.is_some() && lock.is_none() {
            if let Some(root) = session_root.as_deref() {
                let now = unix_timestamp_ms();
                let _ = write_autodream_run_status(
                    root,
                    &AutoDreamRunStatusFile {
                        status: "skipped".to_string(),
                        started_at_ms: now,
                        updated_at_ms: now,
                        sessions_reviewed: 0,
                        tool_calls: 0,
                        genskill_suggested: false,
                        summary: "another AutoDream consolidation is already running".to_string(),
                        error: None,
                        memory_changes: Vec::new(),
                        genskill_suggestion: None,
                    },
                );
            }
            return;
        }
        let context = session_store
            .as_ref()
            .and_then(|store| build_recent_session_context_pack(&state, store).ok());
        let sessions_reviewed = context
            .as_ref()
            .map(|context| context.session_count)
            .unwrap_or_default();
        if let Some(root) = session_root.as_deref() {
            let now = unix_timestamp_ms();
            let _ = write_autodream_run_status(
                root,
                &AutoDreamRunStatusFile {
                    status: "running".to_string(),
                    started_at_ms: now,
                    updated_at_ms: now,
                    sessions_reviewed,
                    tool_calls: 0,
                    genskill_suggested: false,
                    summary: "AutoDream background consolidation is running".to_string(),
                    error: None,
                    memory_changes: Vec::new(),
                    genskill_suggestion: None,
                },
            );
        }
        match run_autodream_review_with_context(
            &state,
            &resources,
            &providers,
            &mut auth_store,
            context.as_ref().map(|context| context.text.as_str()),
        ) {
            Ok(outcome) => {
                if let Some(root) = session_root.as_deref() {
                    let _ = mark_autodream_consolidated(root);
                    let now = unix_timestamp_ms();
                    let memory_changes = extract_memory_changes(&outcome.tool_invocations);
                    let genskill_suggestion =
                        build_genskill_suggestion(now, &outcome, memory_changes.len());
                    let _ = write_autodream_run_status(
                        root,
                        &AutoDreamRunStatusFile {
                            status: if memory_changes.is_empty() {
                                "completed_no_changes".to_string()
                            } else {
                                "completed".to_string()
                            },
                            started_at_ms: now,
                            updated_at_ms: now,
                            sessions_reviewed,
                            tool_calls: outcome.tool_invocations.len(),
                            genskill_suggested: outcome.genskill_suggested,
                            summary: summarize_run_text(&outcome.assistant_text),
                            error: None,
                            memory_changes,
                            genskill_suggestion,
                        },
                    );
                }
            }
            Err(error) => {
                if let Some(root) = session_root.as_deref() {
                    let now = unix_timestamp_ms();
                    let _ = write_autodream_run_status(
                        root,
                        &AutoDreamRunStatusFile {
                            status: "failed".to_string(),
                            started_at_ms: now,
                            updated_at_ms: now,
                            sessions_reviewed,
                            tool_calls: 0,
                            genskill_suggested: false,
                            summary: "AutoDream background consolidation failed".to_string(),
                            error: Some(summarize_run_text(&error.to_string())),
                            memory_changes: Vec::new(),
                            genskill_suggestion: None,
                        },
                    );
                }
            }
        }
        drop(lock);
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AutoDreamGateDecision {
    should_run: bool,
    skip_reason: Option<String>,
}

fn autodream_gates_open(
    state: &AppState,
    session_store: &SessionStore,
) -> Result<AutoDreamGateDecision> {
    let root = session_store.root();
    let mut gate_state = read_autodream_state(root)?;
    let now = unix_timestamp_ms();
    let elapsed_ms = now.saturating_sub(gate_state.last_consolidated_at_ms);
    let min_ms = state.autodream_min_hours().saturating_mul(3_600_000);
    if gate_state.last_consolidated_at_ms > 0 && elapsed_ms < min_ms {
        return Ok(AutoDreamGateDecision {
            should_run: false,
            skip_reason: Some(format!(
                "time_gate: {}h remaining",
                ((min_ms - elapsed_ms) + 3_599_999) / 3_600_000
            )),
        });
    }
    if now.saturating_sub(gate_state.last_session_scan_at_ms) < SESSION_SCAN_INTERVAL_MS {
        return Ok(AutoDreamGateDecision {
            should_run: false,
            skip_reason: Some("scan_throttle".to_string()),
        });
    }
    gate_state.last_session_scan_at_ms = now;
    write_autodream_state(root, &gate_state)?;
    let sessions = sessions_touched_since(session_store, gate_state.last_consolidated_at_ms)?;
    let session_count = sessions
        .into_iter()
        .filter(|session| session.id != state.session.id)
        .count();
    if session_count < state.autodream_min_sessions() {
        return Ok(AutoDreamGateDecision {
            should_run: false,
            skip_reason: Some(format!(
                "session_gate: {session_count}/{}",
                state.autodream_min_sessions()
            )),
        });
    }
    Ok(AutoDreamGateDecision {
        should_run: true,
        skip_reason: None,
    })
}

fn sessions_touched_since(
    session_store: &SessionStore,
    since_ms: u64,
) -> Result<Vec<SessionSummary>> {
    Ok(session_store
        .list_sessions()?
        .into_iter()
        .filter(|session| session.updated_at_ms > since_ms)
        .collect())
}

fn build_recent_session_context_pack(
    state: &AppState,
    session_store: &SessionStore,
) -> Result<RecentSessionContext> {
    let gate_state = read_autodream_state(session_store.root())?;
    let sessions =
        select_recent_autodream_sessions(state, session_store, gate_state.last_consolidated_at_ms)?;
    if sessions.is_empty() {
        return Ok(RecentSessionContext {
            text: String::new(),
            session_count: 0,
        });
    }
    let session_count = sessions.len();
    let mut blocks = Vec::new();
    for session in sessions {
        let record = session_store.load_session(session.id)?;
        let snippets = summarize_transcript_events(&record.events);
        if snippets.is_empty() {
            continue;
        }
        blocks.push(format_session_context_block(&session, &snippets));
    }
    Ok(RecentSessionContext {
        text: truncate_chars(&blocks.join("\n\n"), MAX_SESSION_CONTEXT_CHARS),
        session_count,
    })
}

fn select_recent_autodream_sessions(
    state: &AppState,
    session_store: &SessionStore,
    since_ms: u64,
) -> Result<Vec<SessionSummary>> {
    let mut sessions = sessions_touched_since(session_store, since_ms)?;
    sessions.retain(|session| session.id != state.session.id);
    sessions.retain(|session| session.cwd == state.cwd);
    sessions.truncate(MAX_RECENT_SESSIONS);
    Ok(sessions)
}

fn summarize_transcript_events(events: &[TranscriptEvent]) -> Vec<String> {
    let start = events.len().saturating_sub(MAX_EVENTS_PER_SESSION);
    events[start..]
        .iter()
        .filter_map(event_snippet)
        .filter(|snippet| !snippet.is_empty())
        .collect()
}

fn event_snippet(event: &TranscriptEvent) -> Option<String> {
    match event {
        TranscriptEvent::UserMessage { text, .. } => Some(format!("user: {}", safe_snippet(text))),
        TranscriptEvent::AssistantMessage { text, .. } => {
            Some(format!("assistant: {}", safe_snippet(text)))
        }
        TranscriptEvent::SystemMessage { text, .. } => {
            Some(format!("system: {}", safe_snippet(text)))
        }
        TranscriptEvent::ToolInvocation {
            tool_id,
            input,
            output,
            success,
            ..
        } => Some(format!(
            "tool {} success={}: input={} output={}",
            tool_id,
            success,
            safe_snippet(input),
            safe_snippet(output)
        )),
        TranscriptEvent::CommandInvoked { name, args, .. } => {
            Some(format!("command: /{} {}", name, safe_snippet(args)))
        }
        TranscriptEvent::GitDiffSnapshot { snapshot } => Some(format!(
            "git: command={} status={} diffstat={}",
            safe_snippet(&snapshot.command),
            safe_snippet(&snapshot.status),
            safe_snippet(&snapshot.unstaged_diffstat)
        )),
        TranscriptEvent::SessionRenamed { name } => {
            Some(format!("renamed: {}", safe_snippet(name)))
        }
        TranscriptEvent::TurnBoundary { .. }
        | TranscriptEvent::TranscriptRewritten { .. }
        | TranscriptEvent::StateSnapshot { .. } => None,
    }
}

fn format_session_context_block(session: &SessionSummary, snippets: &[String]) -> String {
    let title = session
        .generated_title
        .as_deref()
        .or(session.display_name.as_deref())
        .unwrap_or("untitled");
    let mut lines = vec![format!(
        "- session {} title={} updated_at_ms={} events={}",
        session.id,
        safe_snippet(title),
        session.updated_at_ms,
        session.event_count
    )];
    for snippet in snippets {
        lines.push(format!("  - {snippet}"));
    }
    lines.join("\n")
}

fn autodream_prompt_with_session_context(session_context: Option<&str>) -> String {
    let Some(context) = session_context
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return AUTODREAM_PROMPT.to_string();
    };
    format!(
        "{AUTODREAM_PROMPT}\n\nRecent session context since the last successful AutoDream:\n{context}\n\nUse this recent session context only as supporting evidence. Keep durable facts only when they are project-specific, verified, and useful after this run."
    )
}

fn safe_snippet(text: &str) -> String {
    let sanitized = redact_secret_like(text.replace(['\n', '\r', '\t'], " ").trim());
    truncate_chars(&sanitized, MAX_SNIPPET_CHARS)
}

fn redact_secret_like(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            if is_secret_like(word) {
                "<redacted>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_secret_like(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    lower.contains("password")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("authorization")
        || lower.contains("bearer")
        || lower.contains("secret")
        || lower.contains("token=")
        || lower.contains("sk-")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    output.push_str("...");
    output
}

fn summarize_run_text(text: &str) -> String {
    safe_snippet(&truncate_chars(text.trim(), MAX_RUN_SUMMARY_CHARS))
}

fn extract_memory_changes(invocations: &[ToolInvocation]) -> Vec<AutoDreamMemoryChange> {
    invocations
        .iter()
        .filter(|invocation| invocation.tool_id == PROJECT_MEMORY_TOOL_ID)
        .filter_map(memory_change_from_invocation)
        .collect()
}

fn memory_change_from_invocation(invocation: &ToolInvocation) -> Option<AutoDreamMemoryChange> {
    let input = serde_json::from_str::<serde_json::Value>(&invocation.input).ok()?;
    let action = input.get("action")?.as_str()?.to_string();
    let content = input
        .get("content")
        .and_then(|value| value.as_str())
        .map(memory_change_snippet);
    let old_text = input
        .get("old_text")
        .and_then(|value| value.as_str())
        .map(memory_change_snippet);
    Some(AutoDreamMemoryChange {
        action,
        content,
        old_text,
        success: invocation.success,
        message: memory_change_snippet(
            memory_tool_message(&invocation.output)
                .as_deref()
                .unwrap_or(if invocation.success {
                    "memory tool succeeded"
                } else {
                    "memory tool failed"
                }),
        ),
    })
}

fn memory_tool_message(output: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(output).ok()?;
    value
        .get("message")
        .or_else(|| value.get("error"))
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn build_genskill_suggestion(
    now: u64,
    outcome: &AutoDreamOutcome,
    memory_changes: usize,
) -> Option<AutoDreamGenskillSuggestion> {
    if !outcome.genskill_suggested {
        return None;
    }
    Some(AutoDreamGenskillSuggestion {
        id: format!("autodream-{now}"),
        created_at_ms: now,
        rationale: summarize_run_text(&outcome.assistant_text),
        memory_changes,
        status: "pending_review".to_string(),
    })
}

fn render_status_memory_changes(status: &AutoDreamRunStatusFile) -> String {
    if status.memory_changes.is_empty() {
        return "\nbackground_memory_changes=0".to_string();
    }
    let mut lines = vec![format!(
        "\nbackground_memory_changes={}",
        status.memory_changes.len()
    )];
    for change in &status.memory_changes {
        let mut detail = format!("{} success={}", change.action, change.success);
        if let Some(content) = change.content.as_deref() {
            detail.push_str(&format!(" content={content}"));
        }
        if let Some(old_text) = change.old_text.as_deref() {
            detail.push_str(&format!(" old_text={old_text}"));
        }
        detail.push_str(&format!(" message={}", change.message));
        lines.push(format!("background_memory_change={detail}"));
    }
    lines.join("\n")
}

fn render_status_genskill_suggestion(status: &AutoDreamRunStatusFile) -> String {
    match status.genskill_suggestion.as_ref() {
        Some(suggestion) => format!(
            "\nbackground_genskill_suggestion_id={}\nbackground_genskill_suggestion_status={}",
            suggestion.id, suggestion.status
        ),
        None => "\nbackground_genskill_suggestion_id=none".to_string(),
    }
}

fn memory_change_snippet(text: &str) -> String {
    safe_snippet(&truncate_chars(text, MAX_MEMORY_CHANGE_CHARS))
}

fn acquire_autodream_lock(session_root: &Path) -> Result<Option<AutoDreamLock>> {
    let lock_path = autodream_dir(session_root).join("consolidation.lock");
    fs::create_dir_all(
        lock_path
            .parent()
            .ok_or_else(|| anyhow!("AutoDream lock path has no parent"))?,
    )?;
    let mut file = LockFile::open(&lock_path)
        .map_err(anyhow::Error::new)
        .with_context(|| format!("failed to open AutoDream lock {}", lock_path.display()))?;
    if file.try_lock().map_err(anyhow::Error::new)? {
        Ok(Some(AutoDreamLock { _file: file }))
    } else {
        Ok(None)
    }
}

fn mark_autodream_consolidated(session_root: &Path) -> Result<()> {
    let mut state = read_autodream_state(session_root)?;
    state.last_consolidated_at_ms = unix_timestamp_ms();
    write_autodream_state(session_root, &state)
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn prepare_autodream_side_turn(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Option<AutoDreamSideTurn>> {
    let Some(context) = state.project_memory.as_ref() else {
        return Ok(None);
    };
    let memory_parent = context
        .memory_file
        .parent()
        .ok_or_else(|| anyhow!("project memory path has no parent directory"))?
        .to_path_buf();
    let read_scope = Pattern::escape(&context.memory_file.to_string_lossy());
    let mut side_state = state.clone();
    if !side_state
        .working_dirs
        .iter()
        .any(|path| path == &memory_parent)
    {
        side_state.working_dirs.push(memory_parent);
    }
    let mut side_resources = resources.clone();
    side_resources.tools.retain(|tool| {
        matches!(
            tool.value.id.as_str(),
            PROJECT_MEMORY_TOOL_ID | PROJECT_MEMORY_READ_TOOL_ID | PROJECT_MEMORY_SKILL_TOOL_ID
        )
    });
    side_resources
        .skills
        .retain(|skill| skill.value.name == PROJECT_MEMORY_SKILL_NAME);
    let filter = crate::runtime::build_request_tool_filter(&[
        PROJECT_MEMORY_SKILL_TOOL_ID.to_string(),
        format!("{PROJECT_MEMORY_READ_TOOL_ID}({read_scope})"),
        PROJECT_MEMORY_TOOL_ID.to_string(),
    ])?;
    Ok(Some(AutoDreamSideTurn {
        state: side_state,
        resources: side_resources,
        filter,
    }))
}

fn parse_genskill_marker(text: &str) -> bool {
    text.lines().any(|line| {
        let normalized = line.trim().to_ascii_lowercase();
        normalized.contains("autodream_genskill: yes")
            || normalized.contains("autodream_genskill: true")
    })
}

#[cfg(test)]
#[path = "autodream/tests.rs"]
mod tests;
