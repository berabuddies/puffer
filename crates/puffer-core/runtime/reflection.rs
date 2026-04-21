use super::openai::conversation::ConversationItem;
use super::ToolInvocation;
use crate::AppState;
mod judge;
mod llm;
mod support;
mod trace;

use self::llm::{
    parse_llm_judge_decision, parse_llm_judge_response, render_judge_lines, render_relevant_paths,
    select_final_signal, LlmJudgeDecision,
};
use self::support::{
    build_prompt, classify_edit_progress, classify_validation, classify_write_progress,
    extract_artifact_candidates, extract_path_candidates, is_runtime_path, language_label,
    observe_invocation, path_matches_targets, render_action_preview, summarize_goal, unix_time_ms,
    validation_improved,
};
use self::trace::{
    batch_observed_event, code_judge_decision_event, final_decision_event, llm_judge_error_event,
    llm_judge_request_event, llm_judge_response_event, llm_judge_skipped_event,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use std::collections::{BTreeSet, HashMap, VecDeque};

#[cfg(test)]
mod tests;

const MIN_TOOL_CALLS_BEFORE_EVALUATION: usize = 4;
const MIN_BATCHES_BETWEEN_EVALUATIONS: usize = 2;
const RECENT_ACTION_WINDOW: usize = 10;
const RECENT_ACTION_PREVIEW: usize = 4;
const EVALUATION_TRIGGER_SCORE: u8 = 3;
const DEFAULT_LLM_JUDGE_MODEL_SELECTOR: &str = "openai/gpt-5.4";
const DEFAULT_LLM_JUDGE_EFFORT_LEVEL: &str = "low";

pub use self::trace::ReflectionTraceEvent;

/// Selects the natural language used for reflection checkpoints and LLM judging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectionLanguage {
    English,
    Chinese,
}

impl Default for ReflectionLanguage {
    fn default() -> Self {
        Self::Chinese
    }
}

/// Configures the heuristic code judge that detects unproductive loops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeJudgeConfig {
    pub soft_stall_ms: u128,
    pub hard_stall_ms: u128,
    pub min_score: u8,
    pub repeated_fingerprint_threshold: usize,
    pub repeated_error_threshold: usize,
    pub repeated_read_threshold: usize,
    pub repeated_write_threshold: usize,
}

impl Default for CodeJudgeConfig {
    fn default() -> Self {
        Self {
            soft_stall_ms: 5 * 60 * 1000,
            hard_stall_ms: 10 * 60 * 1000,
            min_score: 4,
            repeated_fingerprint_threshold: 3,
            repeated_error_threshold: 2,
            repeated_read_threshold: 3,
            repeated_write_threshold: 4,
        }
    }
}

/// Controls how the LLM judge collaborates with the code judge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmJudgeMode {
    Independent,
    ConfirmCodeJudge,
}

impl Default for LlmJudgeMode {
    fn default() -> Self {
        Self::ConfirmCodeJudge
    }
}

/// Selects how much conversation context is passed to the LLM judge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmJudgeContextScope {
    CurrentWindow,
    RecentWindow,
    SummaryAndRecent,
}

impl Default for LlmJudgeContextScope {
    fn default() -> Self {
        Self::CurrentWindow
    }
}

/// Controls whether the LLM judge reuses the main agent prompt cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmJudgePromptCacheMode {
    InheritMainAgent,
    Dedicated,
}

impl Default for LlmJudgePromptCacheMode {
    fn default() -> Self {
        Self::InheritMainAgent
    }
}

/// Configures the optional LLM-based reflection judge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmJudgeConfig {
    pub mode: LlmJudgeMode,
    pub model_selector: Option<String>,
    pub effort_level: Option<String>,
    pub prompt_cache_mode: LlmJudgePromptCacheMode,
    pub context_scope: LlmJudgeContextScope,
    pub recent_item_count: usize,
    pub max_context_chars: usize,
    pub max_tool_output_chars: usize,
}

impl Default for LlmJudgeConfig {
    fn default() -> Self {
        Self {
            mode: LlmJudgeMode::default(),
            model_selector: Some(DEFAULT_LLM_JUDGE_MODEL_SELECTOR.to_string()),
            effort_level: Some(DEFAULT_LLM_JUDGE_EFFORT_LEVEL.to_string()),
            prompt_cache_mode: LlmJudgePromptCacheMode::default(),
            context_scope: LlmJudgeContextScope::default(),
            recent_item_count: 12,
            max_context_chars: 12_000,
            max_tool_output_chars: 1_200,
        }
    }
}

/// Configures the runtime reflection stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectionConfig {
    pub language: ReflectionLanguage,
    pub code_judge: Option<CodeJudgeConfig>,
    pub llm_judge: Option<LlmJudgeConfig>,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            language: ReflectionLanguage::default(),
            code_judge: Some(CodeJudgeConfig::default()),
            llm_judge: Some(LlmJudgeConfig::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReflectionCheckpoint {
    pub prompt: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionKind {
    Read,
    Write,
    Edit,
    Bash,
    Other,
}

#[derive(Debug, Clone)]
struct ActionObservation {
    kind: ActionKind,
    fingerprint: String,
    error_signature: Option<String>,
    primary_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValidationSnapshot {
    success: bool,
    failed: Option<u32>,
    passed: Option<u32>,
    error_count: Option<u32>,
}

#[derive(Debug, Clone)]
struct BatchAssessment {
    validation_progress: bool,
    artifact_progress: bool,
    edit_progress: bool,
    loopiness_score: u8,
    focus_bad: bool,
    time_since_progress_ms: u128,
    signal_notes: Vec<String>,
    recent_actions: Vec<String>,
    /// When the most recent assistant message contains a "Done / All set /
    /// verified" style completion claim, the agent is about to hand the task
    /// back to Harbor. Reflection should force-evaluate regardless of the
    /// normal stall / progress gates so the code / LLM judge has a chance to
    /// catch false-completes before the verifier rejects the submission.
    completion_claim: Option<String>,
}

#[derive(Debug, Clone)]
struct JudgeSignal {
    source: &'static str,
    summary: String,
    reason: String,
    next_action: Option<String>,
}

#[derive(Debug, Clone)]
struct EvaluationGate {
    should_evaluate: bool,
    skip_reason: Option<String>,
    score: u8,
    threshold: u8,
}

#[derive(Debug, Clone)]
struct BatchObservation {
    assessment: BatchAssessment,
    evaluation: EvaluationGate,
}

#[derive(Debug, Clone)]
pub(super) struct ReflectionObservation {
    pub(super) trace_events: Vec<ReflectionTraceEvent>,
    pub(super) checkpoint: Option<ReflectionCheckpoint>,
}

#[derive(Debug, Clone)]
pub(super) struct ReflectionTracker {
    config: ReflectionConfig,
    goal: String,
    target_paths: BTreeSet<String>,
    artifact_paths: BTreeSet<String>,
    relevant_paths: BTreeSet<String>,
    recent_actions: VecDeque<ActionObservation>,
    total_tool_calls: usize,
    batch_count: usize,
    last_progress_at_ms: u128,
    last_evaluation_batch: usize,
    last_validation: Option<ValidationSnapshot>,
}

impl ReflectionTracker {
    pub(super) fn new(goal: &str, config: ReflectionConfig) -> Self {
        let now_ms = unix_time_ms();
        let target_paths = extract_path_candidates(goal);
        let artifact_paths = extract_artifact_candidates(goal);
        Self {
            config,
            goal: summarize_goal(goal),
            target_paths: target_paths.clone(),
            artifact_paths,
            relevant_paths: target_paths,
            recent_actions: VecDeque::with_capacity(RECENT_ACTION_WINDOW),
            total_tool_calls: 0,
            batch_count: 0,
            last_progress_at_ms: now_ms,
            last_evaluation_batch: 0,
            last_validation: None,
        }
    }

    #[cfg(test)]
    pub(super) fn relevant_paths_for_test(&self) -> &BTreeSet<String> {
        &self.relevant_paths
    }

    pub(super) fn observe_batch(
        &mut self,
        invocations: &[ToolInvocation],
    ) -> Option<ReflectionCheckpoint> {
        self.observe_batch_with_trace_at(invocations, unix_time_ms())
            .and_then(|observation| observation.checkpoint)
    }

    pub(super) fn observe_batch_at(
        &mut self,
        invocations: &[ToolInvocation],
        now_ms: u128,
    ) -> Option<ReflectionCheckpoint> {
        self.observe_batch_with_trace_at(invocations, now_ms)
            .and_then(|observation| observation.checkpoint)
    }

    pub(super) fn observe_batch_with_trace(
        &mut self,
        invocations: &[ToolInvocation],
    ) -> Option<ReflectionObservation> {
        self.observe_batch_with_trace_at(invocations, unix_time_ms())
    }

    pub(super) fn observe_batch_with_trace_at(
        &mut self,
        invocations: &[ToolInvocation],
        now_ms: u128,
    ) -> Option<ReflectionObservation> {
        let observation = self.observe_batch_internal(invocations, now_ms)?;
        let mut trace_events = vec![batch_observed_event(
            &observation.assessment,
            self.batch_count,
            self.total_tool_calls,
            observation.evaluation.should_evaluate,
            observation.evaluation.skip_reason.clone(),
            observation.evaluation.score,
            observation.evaluation.threshold,
            &self.relevant_paths,
        )];
        if !observation.evaluation.should_evaluate {
            return Some(ReflectionObservation {
                trace_events,
                checkpoint: None,
            });
        }

        let score = self.code_judge_score(&observation.assessment);
        let signal = self.code_judge_signal(&observation.assessment);
        let threshold = self
            .config
            .code_judge
            .as_ref()
            .map(|config| config.min_score)
            .unwrap_or_default();
        trace_events.push(code_judge_decision_event(score, threshold, signal.as_ref()));
        self.last_evaluation_batch = self.batch_count;
        let checkpoint = signal
            .as_ref()
            .map(|value| self.build_checkpoint(&observation.assessment, value));
        trace_events.push(final_decision_event(signal.as_ref(), checkpoint.as_ref()));
        Some(ReflectionObservation {
            trace_events,
            checkpoint,
        })
    }

    pub(super) fn observe_openai_batch(
        &mut self,
        invocations: &[ToolInvocation],
        items: &[ConversationItem],
        state: &AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        auth_store: &mut AuthStore,
    ) -> Option<ReflectionObservation> {
        // Agents usually emit their "Done." completion claim in a batch with
        // NO new tool calls — the last turn before the conversation loop
        // exits. `observe_batch_internal` early-returns None on empty
        // invocations, so without this branch our completion-claim trigger
        // would never actually fire.  When we see a completion claim and
        // the run has done enough prior work to be worth judging, synthesize
        // a minimal observation so the code/LLM judges get a chance to
        // challenge the agent's self-assessment.
        let mut observation = match self.observe_batch_internal(invocations, unix_time_ms()) {
            Some(observation) => observation,
            None => {
                let claim = latest_assistant_completion_claim(items);
                if claim.is_some()
                    && self.total_tool_calls >= MIN_TOOL_CALLS_BEFORE_EVALUATION
                    && self
                        .batch_count
                        .saturating_sub(self.last_evaluation_batch)
                        >= MIN_BATCHES_BETWEEN_EVALUATIONS
                {
                    self.synthesize_claim_only_observation()
                } else {
                    return None;
                }
            }
        };
        // Detect "I'm done" style completion claims in the latest assistant
        // message. When the agent declares success, force an evaluation even
        // if the heuristic gate would otherwise skip — false-completes were
        // the dominant failure mode on TB2 (9 out of 36 fails in the Kimi
        // OAuth run), and every one of them had a plausible "Done." sign-off
        // that the normal stall-based trigger couldn't see.
        observation.assessment.completion_claim = latest_assistant_completion_claim(items);
        let completion_forced = observation.assessment.completion_claim.is_some()
            && !observation.evaluation.should_evaluate
            // Respect the minimum-tool-calls and rate-limit gates even on a
            // completion claim: firing a judge after 1 turn would just cost
            // tokens without meaningful signal.
            && self.total_tool_calls >= MIN_TOOL_CALLS_BEFORE_EVALUATION
            && self.batch_count.saturating_sub(self.last_evaluation_batch)
                >= MIN_BATCHES_BETWEEN_EVALUATIONS;
        if completion_forced {
            observation.evaluation.should_evaluate = true;
            observation.evaluation.skip_reason = None;
            observation
                .assessment
                .signal_notes
                .push("completion_claim_forced_evaluation".to_string());
        }
        let mut trace_events = vec![batch_observed_event(
            &observation.assessment,
            self.batch_count,
            self.total_tool_calls,
            observation.evaluation.should_evaluate,
            observation.evaluation.skip_reason.clone(),
            observation.evaluation.score,
            observation.evaluation.threshold,
            &self.relevant_paths,
        )];
        if !observation.evaluation.should_evaluate {
            return Some(ReflectionObservation {
                trace_events,
                checkpoint: None,
            });
        }

        let code_score = self.code_judge_score(&observation.assessment);
        let code_signal = self.code_judge_signal(&observation.assessment);
        let code_threshold = self
            .config
            .code_judge
            .as_ref()
            .map(|config| config.min_score)
            .unwrap_or_default();
        trace_events.push(code_judge_decision_event(
            code_score,
            code_threshold,
            code_signal.as_ref(),
        ));
        let llm_signal = self.llm_judge_signal(
            &observation.assessment,
            code_signal.as_ref(),
            items,
            state,
            resources,
            providers,
            auth_store,
            &mut trace_events,
        );
        let final_signal = select_final_signal(
            self.config.llm_judge.as_ref().map(|config| config.mode),
            code_signal,
            llm_signal,
        );
        self.last_evaluation_batch = self.batch_count;
        let checkpoint = final_signal
            .as_ref()
            .map(|signal| self.build_checkpoint(&observation.assessment, signal));
        trace_events.push(final_decision_event(
            final_signal.as_ref(),
            checkpoint.as_ref(),
        ));
        Some(ReflectionObservation {
            trace_events,
            checkpoint,
        })
    }

    fn observe_batch_internal(
        &mut self,
        invocations: &[ToolInvocation],
        now_ms: u128,
    ) -> Option<BatchObservation> {
        if invocations.is_empty() {
            return None;
        }

        self.batch_count += 1;
        self.total_tool_calls += invocations.len();

        let mut assessment = BatchAssessment {
            validation_progress: false,
            artifact_progress: false,
            edit_progress: false,
            loopiness_score: 0,
            focus_bad: false,
            time_since_progress_ms: 0,
            signal_notes: Vec::new(),
            recent_actions: Vec::new(),
            completion_claim: None,
        };
        let mut saw_progress = false;

        for invocation in invocations {
            let observed = observe_invocation(invocation);
            if let Some(path) = &observed.primary_path {
                if !is_runtime_path(path) {
                    self.relevant_paths.insert(path.clone());
                }
            }
            self.push_recent_action(observed.clone());
            assessment
                .recent_actions
                .push(render_action_preview(&observed));

            match invocation.tool_id.as_str() {
                "Write" => {
                    if let Some(write_progress) =
                        classify_write_progress(invocation, &self.artifact_paths)
                    {
                        if write_progress.meaningful && !is_runtime_path(&write_progress.path) {
                            assessment.edit_progress = true;
                            saw_progress = true;
                            if write_progress.artifact {
                                assessment.artifact_progress = true;
                            }
                        }
                    }
                }
                "Edit" => {
                    if let Some(edit_progress) =
                        classify_edit_progress(invocation, &self.target_paths)
                    {
                        if edit_progress.meaningful && !is_runtime_path(&edit_progress.path) {
                            assessment.edit_progress = true;
                            saw_progress = true;
                        }
                    }
                }
                "Bash" => {
                    if let Some(snapshot) = classify_validation(invocation) {
                        if validation_improved(self.last_validation, snapshot) {
                            assessment.validation_progress = true;
                            saw_progress = true;
                        }
                        self.last_validation = Some(snapshot);
                    }
                }
                _ => {}
            }
        }

        assessment.loopiness_score = self.loopiness_score();
        assessment.focus_bad = self.focus_bad();

        if saw_progress {
            self.last_progress_at_ms = now_ms;
        }
        assessment.time_since_progress_ms = now_ms.saturating_sub(self.last_progress_at_ms);
        assessment.signal_notes = self.signal_notes(&assessment);

        Some(BatchObservation {
            evaluation: self.evaluation_gate(&assessment),
            assessment,
        })
    }

    fn push_recent_action(&mut self, action: ActionObservation) {
        if self.recent_actions.len() == RECENT_ACTION_WINDOW {
            self.recent_actions.pop_front();
        }
        self.recent_actions.push_back(action);
    }

    fn loopiness_score(&self) -> u8 {
        let thresholds = self.config.code_judge.as_ref().cloned().unwrap_or_default();
        let mut score = 0u8;
        let mut fingerprints: HashMap<&str, usize> = HashMap::new();
        let mut errors: HashMap<&str, usize> = HashMap::new();
        let mut read_paths: HashMap<&str, usize> = HashMap::new();
        let mut write_paths: HashMap<&str, usize> = HashMap::new();

        for action in &self.recent_actions {
            *fingerprints.entry(action.fingerprint.as_str()).or_default() += 1;
            if let Some(error) = &action.error_signature {
                *errors.entry(error.as_str()).or_default() += 1;
            }
            if let Some(path) = &action.primary_path {
                match action.kind {
                    ActionKind::Read => *read_paths.entry(path.as_str()).or_default() += 1,
                    ActionKind::Write | ActionKind::Edit => {
                        *write_paths.entry(path.as_str()).or_default() += 1;
                    }
                    _ => {}
                }
            }
        }

        if fingerprints
            .values()
            .any(|count| *count >= thresholds.repeated_fingerprint_threshold)
        {
            score += 2;
        }
        if errors
            .values()
            .any(|count| *count >= thresholds.repeated_error_threshold)
        {
            score += 2;
        }
        if read_paths
            .values()
            .any(|count| *count >= thresholds.repeated_read_threshold)
        {
            score += 1;
        }
        if write_paths
            .values()
            .any(|count| *count >= thresholds.repeated_write_threshold)
        {
            score += 1;
        }
        score
    }

    fn focus_bad(&self) -> bool {
        let touched_paths = self
            .recent_actions
            .iter()
            .filter_map(|action| action.primary_path.as_deref())
            .filter(|path| !is_runtime_path(path))
            .collect::<Vec<_>>();
        if touched_paths.len() < 4 {
            return false;
        }

        if !self.target_paths.is_empty() {
            let on_target = touched_paths
                .iter()
                .filter(|path| path_matches_targets(path, &self.target_paths))
                .count();
            return on_target * 2 < touched_paths.len();
        }

        touched_paths.into_iter().collect::<BTreeSet<_>>().len() > 6
    }

    fn signal_notes(&self, assessment: &BatchAssessment) -> Vec<String> {
        let mut notes = Vec::new();
        notes.push(if assessment.validation_progress {
            "validation_progress: positive".to_string()
        } else {
            "validation_progress: stalled".to_string()
        });
        notes.push(if assessment.artifact_progress {
            "artifact_progress: meaningful artifact update".to_string()
        } else {
            "artifact_progress: no meaningful artifact gain".to_string()
        });
        notes.push(if assessment.edit_progress {
            "edit_progress: relevant files changed".to_string()
        } else {
            "edit_progress: mostly exploratory".to_string()
        });
        notes.push(format!("loopiness: score {}", assessment.loopiness_score));
        notes.push(if assessment.focus_bad {
            "focus: wandering away from relevant files".to_string()
        } else {
            "focus: concentrated enough".to_string()
        });
        notes.push(format!(
            "time_since_last_progress: {}s",
            (assessment.time_since_progress_ms / 1000) as u64
        ));
        notes
    }

    fn evaluation_gate(&self, assessment: &BatchAssessment) -> EvaluationGate {
        if self.total_tool_calls < MIN_TOOL_CALLS_BEFORE_EVALUATION {
            return EvaluationGate {
                should_evaluate: false,
                skip_reason: Some(format!(
                    "total_tool_calls {} below minimum {}",
                    self.total_tool_calls, MIN_TOOL_CALLS_BEFORE_EVALUATION
                )),
                score: 0,
                threshold: EVALUATION_TRIGGER_SCORE,
            };
        }
        if self.batch_count.saturating_sub(self.last_evaluation_batch)
            < MIN_BATCHES_BETWEEN_EVALUATIONS
        {
            return EvaluationGate {
                should_evaluate: false,
                skip_reason: Some(format!(
                    "only {} batches since last evaluation; minimum is {}",
                    self.batch_count.saturating_sub(self.last_evaluation_batch),
                    MIN_BATCHES_BETWEEN_EVALUATIONS
                )),
                score: 0,
                threshold: EVALUATION_TRIGGER_SCORE,
            };
        }
        if assessment.validation_progress
            || assessment.artifact_progress
            || assessment.edit_progress
        {
            return EvaluationGate {
                should_evaluate: false,
                skip_reason: Some("recent real progress detected".to_string()),
                score: 0,
                threshold: EVALUATION_TRIGGER_SCORE,
            };
        }

        let score = self.code_judge_score(assessment);
        if score >= EVALUATION_TRIGGER_SCORE {
            EvaluationGate {
                should_evaluate: true,
                skip_reason: None,
                score,
                threshold: EVALUATION_TRIGGER_SCORE,
            }
        } else {
            EvaluationGate {
                should_evaluate: false,
                skip_reason: Some(format!(
                    "stall score {score} below evaluation threshold {EVALUATION_TRIGGER_SCORE}"
                )),
                score,
                threshold: EVALUATION_TRIGGER_SCORE,
            }
        }
    }

    fn code_judge_score(&self, assessment: &BatchAssessment) -> u8 {
        let config = self.config.code_judge.as_ref().cloned().unwrap_or_default();
        let mut score = 0u8;
        if assessment.time_since_progress_ms >= config.soft_stall_ms {
            score += 2;
        }
        if assessment.time_since_progress_ms >= config.hard_stall_ms {
            score += 2;
        }
        score += assessment.loopiness_score.min(3);
        if assessment.focus_bad {
            score += 1;
        }
        score
    }

    fn code_judge_signal(&self, assessment: &BatchAssessment) -> Option<JudgeSignal> {
        let config = self.config.code_judge.as_ref()?;
        let score = self.code_judge_score(assessment);
        if score < config.min_score {
            return None;
        }

        Some(JudgeSignal {
            source: "code_judge",
            summary: format!(
                "code judge triggered after {}s without real progress; loopiness={}, focus={}",
                (assessment.time_since_progress_ms / 1000) as u64,
                assessment.loopiness_score,
                if assessment.focus_bad {
                    "wandering"
                } else {
                    "focused"
                }
            ),
            reason: format!(
                "heuristic stall score {} reached the configured threshold {}",
                score, config.min_score
            ),
            next_action: None,
        })
    }

    fn llm_judge_signal(
        &self,
        assessment: &BatchAssessment,
        code_signal: Option<&JudgeSignal>,
        items: &[ConversationItem],
        state: &AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        auth_store: &mut AuthStore,
        trace_events: &mut Vec<ReflectionTraceEvent>,
    ) -> Option<Option<JudgeSignal>> {
        let Some(config) = self.config.llm_judge.as_ref() else {
            trace_events.push(ReflectionTraceEvent::LlmJudgeSkipped {
                mode: "disabled".to_string(),
                reason: "llm judge disabled in reflection config".to_string(),
            });
            return None;
        };
        if matches!(config.mode, LlmJudgeMode::ConfirmCodeJudge) && code_signal.is_none() {
            trace_events.push(llm_judge_skipped_event(
                config.mode,
                "confirm_code_judge mode requires a code-judge trigger first",
            ));
            return None;
        }

        let attempt = judge::run_llm_judge(
            &self.goal,
            &self.relevant_paths,
            self.config.language,
            config,
            assessment,
            code_signal,
            items,
            state,
            resources,
            providers,
            auth_store,
        );
        trace_events.push(llm_judge_request_event(config, &attempt));
        if let Some(error) = &attempt.error {
            trace_events.push(llm_judge_error_event(
                "execution_failed",
                error,
                &attempt,
                code_signal.is_some(),
            ));
            return None;
        }
        let raw_response_text = attempt.raw_response_text.clone().unwrap_or_default();
        let Some(response) = parse_llm_judge_response(&raw_response_text) else {
            trace_events.push(llm_judge_error_event(
                "parse_failed",
                "llm judge response did not contain a valid JSON object",
                &attempt.with_raw_response_text(Some(raw_response_text)),
                code_signal.is_some(),
            ));
            return None;
        };
        let decision = match parse_llm_judge_decision(&response.decision) {
            Some(value) => value,
            None => {
                trace_events.push(llm_judge_error_event(
                    "invalid_decision",
                    format!("unsupported llm judge decision {:?}", response.decision),
                    &attempt.with_raw_response_text(Some(raw_response_text)),
                    code_signal.is_some(),
                ));
                return None;
            }
        };
        trace_events.push(llm_judge_response_event(
            &attempt,
            &response.decision,
            response.confidence.map(|value| format!("{value:.2}")),
            &response.reason,
            &response.next_action,
        ));
        if matches!(decision, LlmJudgeDecision::Continue) {
            return Some(None);
        }

        let confidence = response
            .confidence
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "n/a".to_string());
        Some(Some(JudgeSignal {
            source: "llm_judge",
            summary: format!(
                "llm judge {} with confidence {} using {:?}",
                response.decision.to_ascii_lowercase(),
                confidence,
                config.context_scope
            ),
            reason: response.reason,
            next_action: Some(response.next_action),
        }))
    }

    /// Builds a minimal `BatchObservation` for the case where the model
    /// returned a completion-claim text turn WITHOUT any tool calls (so
    /// `observe_batch_internal` skipped). The synthesized observation has no
    /// progress signals and passes through a `should_evaluate=true` gate so
    /// downstream code/LLM judges can run. See `observe_openai_batch` for
    /// the call-site guard.
    fn synthesize_claim_only_observation(&mut self) -> BatchObservation {
        self.batch_count += 1;
        // Inflate `time_since_progress_ms` past both code-judge stall
        // thresholds so `code_judge_score` yields ≥ 4 (the default
        // `min_score`). This lets the code judge emit a signal even though
        // no real stall occurred — we *want* the completion-claim path to
        // reach the judges, and under the default `ConfirmCodeJudge` llm
        // mode the llm judge only runs when the code judge triggered.
        let stall_ms = self
            .config
            .code_judge
            .as_ref()
            .map(|config| config.hard_stall_ms.saturating_add(60_000))
            .unwrap_or(600_000);
        BatchObservation {
            assessment: BatchAssessment {
                validation_progress: false,
                artifact_progress: false,
                edit_progress: false,
                loopiness_score: 0,
                focus_bad: false,
                time_since_progress_ms: stall_ms,
                signal_notes: vec!["synthesized_from_completion_claim".to_string()],
                recent_actions: Vec::new(),
                completion_claim: None,
            },
            evaluation: EvaluationGate {
                should_evaluate: true,
                skip_reason: None,
                score: 4,
                threshold: EVALUATION_TRIGGER_SCORE,
            },
        }
    }

    fn build_checkpoint(
        &self,
        assessment: &BatchAssessment,
        signal: &JudgeSignal,
    ) -> ReflectionCheckpoint {
        let signal_lines = assessment
            .signal_notes
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let recent_actions = assessment
            .recent_actions
            .iter()
            .rev()
            .take(RECENT_ACTION_PREVIEW)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let relevant_paths = render_relevant_paths(&self.relevant_paths);
        let judge_lines = render_judge_lines(signal);
        let summary = format!(
            "reflection checkpoint ({}) via {} after {}s without real progress",
            language_label(self.config.language),
            signal.source,
            (assessment.time_since_progress_ms / 1000) as u64
        );
        let mut prompt = build_prompt(
            self.config.language,
            &self.goal,
            &summary,
            &signal_lines,
            &recent_actions,
            &relevant_paths,
            &judge_lines,
        );
        // When the checkpoint fired because the agent declared the task done,
        // append a claim-specific nudge. Data from the 89-task Kimi OAuth run
        // showed that solved tasks are 2.3× more likely to have executed a
        // test than unsolved ones; 21 of 27 unsolved long-trajectory tasks
        // declared "Done" without ever running `pytest`, `/app/test_*.py`, or
        // equivalent. We ask the agent to close that gap BEFORE re-submitting
        // — this is prompt guidance, not verifier tampering.
        if assessment.completion_claim.is_some() {
            let addendum = match self.config.language {
                ReflectionLanguage::Chinese => {
                    "\n\n<system-reminder>\n你刚刚声明任务已完成。但注意：**TB2 的 verifier 测试文件位于任务目录内部、不会挂载到 /app**，所以你看不见也无法直接跑它。你能做的是**自己构造一条对照任务要求的独立检查**，跑一遍看自己的方案是否真的满足：\n\n1. 回到最初的 prompt，逐条列出 explicit requirement（输出文件路径、函数签名、数值精度、bool flag、输出格式约束等）。\n2. 针对每条 requirement 写一段 `/tmp/check.py` 或 bash 检查：**读你产出的 artifact，验证它是否满足这条 requirement**。不是运行你自己写的函数确认 happy-path，而是**模拟 verifier 会怎么检查你**。\n   - 数值任务：sanity-check 数值是否落在物理/合理区间（raman G 峰应 ~1580，不是 19196）。\n   - 文件产出：用 `jq` / `python -m json.tool` 校验结构；用 `head`/`wc` 校验规模。\n   - 代码任务：import 并调用 API，造 corner case 输入，断言输出。\n3. 检查失败就继续修；全部通过再声明完成。\n\n不要再次跳过验证就汇报『已完成』。\n</system-reminder>"
                }
                ReflectionLanguage::English => {
                    "\n\n<system-reminder>\nYou just declared the task complete. But: **TB2's verifier tests live inside the task directory and are not mounted into /app** — you cannot see or run them directly. What you CAN do is construct an **independent check against the task requirements** and run it:\n\n1. Re-read the original prompt. List every explicit requirement (output file path, function signature, numeric tolerance, bool flag, output-format constraint).\n2. Write a `/tmp/check.py` or bash one-liner per requirement that **reads your produced artifact and verifies it meets that requirement**. Not a smoke-test of your own code's happy path — a simulation of what the verifier will check.\n   - Numeric tasks: sanity-check that values land in physically / logically plausible ranges (graphene Raman G peak is ~1580, not 19196).\n   - File outputs: validate structure with `jq` / `python -m json.tool`; check scale with `head` / `wc`.\n   - Code tasks: import + call the API, feed corner-case inputs, assert outputs.\n3. If any check fails, keep working. Only declare done once all pass.\n\nDo not skip verification and claim \"completed\" again.\n</system-reminder>"
                }
            };
            prompt.push_str(addendum);
        }
        ReflectionCheckpoint { prompt, summary }
    }
}

/// Walks conversation items back-to-front and returns the text of the most
/// recent assistant message that contains a "completion claim" — words the
/// model typically uses when it thinks it has finished the task (and is
/// about to hand control back to Harbor's verifier). Returns `None` if the
/// latest assistant message is either a pure tool-call or doesn't contain
/// any such phrasing.
fn latest_assistant_completion_claim(items: &[ConversationItem]) -> Option<String> {
    for item in items.iter().rev() {
        match item {
            ConversationItem::FunctionCall { .. } | ConversationItem::FunctionCallOutput { .. } => {
                // Tool activity after the claim — model resumed work, not
                // claiming done yet. Skip past these to find the claim-text.
                continue;
            }
            ConversationItem::Message { role, content } if role == "assistant" => {
                let text = content
                    .iter()
                    .filter_map(|part| match part {
                        crate::runtime::openai::conversation::ContentPart::Text { text } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if is_completion_claim(&text) {
                    return Some(text);
                }
                return None;
            }
            _ => continue,
        }
    }
    None
}

/// Heuristic classifier for whether an assistant message looks like a
/// "I finished the task" signal. Tuned against the 25 verifier-reject
/// trajectories from the Kimi OAuth TB2 run — every one of them ended
/// with one of these phrasings.
fn is_completion_claim(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Only consider the first ~200 chars of the message; the claim is
    // almost always a leading sentence before any bullet-list walk-through.
    let prefix_len = trimmed.char_indices().nth(200).map(|(i, _)| i).unwrap_or(trimmed.len());
    let head = trimmed[..prefix_len].to_ascii_lowercase();
    // Markers derived from the Kimi OAuth TB2 run's 25 verifier-reject
    // trajectories — every one of them ended with one of these phrasings.
    let markers: &[&str] = &[
        "done",
        "complete",
        "completed",
        "finished",
        "all set",
        "all done",
        "ready",
        "verified",
        "task is complete",
        "task complete",
        "successfully",
        "has been written",
        "have been written",
        "has been saved",
        "saved to `",
        "saved to /",
        "implemented",
        "i have implemented",
        "i have created",
        "i have written",
        "i have successfully",
        "i've implemented",
        "i've created",
        "i've written",
        "✅",
    ];
    markers.iter().any(|m| head.contains(m))
}

#[cfg(test)]
mod completion_claim_tests {
    use super::*;
    use crate::runtime::openai::conversation::{ContentPart, ConversationItem};

    fn msg(role: &str, text: &str) -> ConversationItem {
        ConversationItem::Message {
            role: role.to_string(),
            content: vec![ContentPart::Text {
                text: text.to_string(),
            }],
        }
    }

    #[test]
    fn detects_done_claims() {
        assert!(is_completion_claim("Done! The function is in /app/run.py"));
        assert!(is_completion_claim("The task is complete."));
        assert!(is_completion_claim("All set! Here's what was configured"));
        assert!(is_completion_claim("I've created the polyglot file"));
        assert!(is_completion_claim("The implementation is complete."));
        assert!(is_completion_claim(
            "The text in the G-code spells out: MAKER. This has been written to /app/out.txt."
        ));
    }

    #[test]
    fn ignores_non_claims() {
        assert!(!is_completion_claim(""));
        assert!(!is_completion_claim("Let me start by listing /app"));
        assert!(!is_completion_claim("I need to understand the task first"));
    }

    #[test]
    fn walks_past_trailing_tool_calls() {
        let items = vec![
            msg("user", "do the thing"),
            msg("assistant", "Done! The task is complete."),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Bash".into(),
                arguments: "{}".into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: crate::runtime::openai::conversation::ToolOutputPayload::success(
                    "ok".into(),
                ),
            },
        ];
        assert!(latest_assistant_completion_claim(&items).is_some());
    }

    #[test]
    fn returns_none_when_last_assistant_is_not_a_claim() {
        let items = vec![
            msg("user", "do the thing"),
            msg("assistant", "Let me start investigating"),
        ];
        assert!(latest_assistant_completion_claim(&items).is_none());
    }
}
