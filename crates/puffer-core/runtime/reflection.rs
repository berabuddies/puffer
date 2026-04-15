use super::openai::conversation::ConversationItem;
use super::{execute_user_prompt_with_options, ToolInvocation, TurnRequestOptions};
use crate::AppState;
mod llm;
mod support;

use self::llm::{
    build_llm_judge_prompt, parse_llm_judge_decision, parse_llm_judge_response, render_judge_lines,
    render_llm_judge_context, render_relevant_paths, select_final_signal, LlmJudgeDecision,
    LlmJudgeResponse,
};
use self::support::{
    content_is_meaningful, count_case_insensitive, extract_artifact_candidates, extract_count,
    extract_path_candidates, first_non_empty_line, is_runtime_path, looks_like_validation_command,
    normalize_text, path_matches_targets, summarize_goal,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
mod tests;

const MIN_TOOL_CALLS_BEFORE_EVALUATION: usize = 4;
const MIN_BATCHES_BETWEEN_EVALUATIONS: usize = 2;
const RECENT_ACTION_WINDOW: usize = 10;
const RECENT_ACTION_PREVIEW: usize = 4;
const DEFAULT_LLM_JUDGE_MODEL_SELECTOR: &str = "openai/gpt-5.4";
const DEFAULT_LLM_JUDGE_EFFORT_LEVEL: &str = "low";

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

/// Configures the optional LLM-based reflection judge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmJudgeConfig {
    pub mode: LlmJudgeMode,
    pub model_selector: Option<String>,
    pub effort_level: Option<String>,
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
}

#[derive(Debug, Clone)]
struct JudgeSignal {
    source: &'static str,
    summary: String,
    reason: String,
    next_action: Option<String>,
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

    pub(super) fn observe_batch(
        &mut self,
        invocations: &[ToolInvocation],
    ) -> Option<ReflectionCheckpoint> {
        self.observe_batch_at(invocations, unix_time_ms())
    }

    pub(super) fn observe_batch_at(
        &mut self,
        invocations: &[ToolInvocation],
        now_ms: u128,
    ) -> Option<ReflectionCheckpoint> {
        let assessment = self.observe_batch_internal(invocations, now_ms)?;
        let signal = self.code_judge_signal(&assessment)?;
        self.last_evaluation_batch = self.batch_count;
        Some(self.build_checkpoint(&assessment, &signal))
    }

    pub(super) fn observe_openai_batch(
        &mut self,
        invocations: &[ToolInvocation],
        items: &[ConversationItem],
        state: &AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        auth_store: &mut AuthStore,
    ) -> Option<ReflectionCheckpoint> {
        let assessment = self.observe_batch_internal(invocations, unix_time_ms())?;
        let code_signal = self.code_judge_signal(&assessment);
        let llm_signal = self.llm_judge_signal(
            &assessment,
            code_signal.as_ref(),
            items,
            state,
            resources,
            providers,
            auth_store,
        );
        let final_signal = select_final_signal(
            self.config.llm_judge.as_ref().map(|config| config.mode),
            code_signal,
            llm_signal,
        )?;
        self.last_evaluation_batch = self.batch_count;
        Some(self.build_checkpoint(&assessment, &final_signal))
    }

    fn observe_batch_internal(
        &mut self,
        invocations: &[ToolInvocation],
        now_ms: u128,
    ) -> Option<BatchAssessment> {
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

        if !self.should_evaluate(&assessment) {
            return None;
        }

        Some(assessment)
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

    fn should_evaluate(&self, assessment: &BatchAssessment) -> bool {
        if self.total_tool_calls < MIN_TOOL_CALLS_BEFORE_EVALUATION {
            return false;
        }
        if self.batch_count.saturating_sub(self.last_evaluation_batch)
            < MIN_BATCHES_BETWEEN_EVALUATIONS
        {
            return false;
        }
        if assessment.validation_progress
            || assessment.artifact_progress
            || assessment.edit_progress
        {
            return false;
        }

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
        score >= 3
    }

    fn code_judge_signal(&self, assessment: &BatchAssessment) -> Option<JudgeSignal> {
        let config = self.config.code_judge.as_ref()?;
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
    ) -> Option<Option<JudgeSignal>> {
        let config = self.config.llm_judge.as_ref()?;
        if matches!(config.mode, LlmJudgeMode::ConfirmCodeJudge) && code_signal.is_none() {
            return None;
        }

        let response = self.run_llm_judge(
            config,
            assessment,
            code_signal,
            items,
            state,
            resources,
            providers,
            auth_store,
        )?;
        let decision = parse_llm_judge_decision(&response.decision)?;
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

    fn run_llm_judge(
        &self,
        config: &LlmJudgeConfig,
        assessment: &BatchAssessment,
        code_signal: Option<&JudgeSignal>,
        items: &[ConversationItem],
        state: &AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        auth_store: &mut AuthStore,
    ) -> Option<LlmJudgeResponse> {
        let prompt = build_llm_judge_prompt(
            self.config.language,
            &self.goal,
            assessment,
            code_signal,
            &render_llm_judge_context(
                items,
                config.context_scope,
                config.recent_item_count,
                config.max_context_chars,
                config.max_tool_output_chars,
            ),
            &render_relevant_paths(&self.relevant_paths),
        );

        // The main agent keeps using its configured execution model. Reflection
        // judging is a side request that can be routed to a separate
        // provider/model pair so the decision policy stays configurable and
        // inexpensive by default.
        let mut side_state = state.clone();
        if let Some(selector) = &config.model_selector {
            side_state.current_model = Some(selector.clone());
            if let Some((provider, _)) = selector.split_once('/') {
                side_state.current_provider = Some(provider.to_string());
            }
        }
        if let Some(effort) = &config.effort_level {
            side_state.effort_level = effort.clone();
        }

        let mut side_resources = resources.clone();
        side_resources.tools.clear();
        let execution = execute_user_prompt_with_options(
            &mut side_state,
            &side_resources,
            providers,
            auth_store,
            &prompt,
            TurnRequestOptions {
                structured_output: None,
                tool_filter: None,
                reflection: None,
            },
        )
        .ok()?;
        parse_llm_judge_response(&execution.assistant_text)
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
        let prompt = build_prompt(
            self.config.language,
            &self.goal,
            &summary,
            &signal_lines,
            &recent_actions,
            &relevant_paths,
            &judge_lines,
        );
        ReflectionCheckpoint { prompt, summary }
    }
}

#[derive(Debug, Clone)]
struct WriteProgress {
    path: String,
    meaningful: bool,
    artifact: bool,
}

#[derive(Debug, Clone)]
struct EditProgress {
    path: String,
    meaningful: bool,
}

fn classify_write_progress(
    invocation: &ToolInvocation,
    artifact_paths: &BTreeSet<String>,
) -> Option<WriteProgress> {
    let input = serde_json::from_str::<Value>(&invocation.input).ok()?;
    let path = input.get("file_path")?.as_str()?.to_string();
    let content = input
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(WriteProgress {
        artifact: path_matches_targets(&path, artifact_paths),
        meaningful: content_is_meaningful(content),
        path,
    })
}

fn classify_edit_progress(
    invocation: &ToolInvocation,
    target_paths: &BTreeSet<String>,
) -> Option<EditProgress> {
    let input = serde_json::from_str::<Value>(&invocation.input).ok()?;
    let path = input.get("file_path")?.as_str()?.to_string();
    let old_string = input
        .get("old_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let new_string = input
        .get("new_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let meaningful = old_string.trim() != new_string.trim()
        && (!new_string.trim().is_empty() || path_matches_targets(&path, target_paths));
    Some(EditProgress { path, meaningful })
}

fn classify_validation(invocation: &ToolInvocation) -> Option<ValidationSnapshot> {
    let input = serde_json::from_str::<Value>(&invocation.input).ok()?;
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !looks_like_validation_command(command, description) {
        return None;
    }
    let failed = extract_count(invocation.output.as_str(), "failed");
    let passed = extract_count(invocation.output.as_str(), "passed");
    let error_count = Some(count_case_insensitive(invocation.output.as_str(), "error:") as u32);
    Some(ValidationSnapshot {
        success: invocation.success,
        failed,
        passed,
        error_count,
    })
}

fn validation_improved(previous: Option<ValidationSnapshot>, current: ValidationSnapshot) -> bool {
    let Some(previous) = previous else {
        return current.success;
    };
    if current.success && !previous.success {
        return true;
    }
    if let (Some(prev_failed), Some(curr_failed)) = (previous.failed, current.failed) {
        if curr_failed < prev_failed {
            return true;
        }
    }
    if let (Some(prev_passed), Some(curr_passed)) = (previous.passed, current.passed) {
        if curr_passed > prev_passed {
            return true;
        }
    }
    if let (Some(prev_errors), Some(curr_errors)) = (previous.error_count, current.error_count) {
        if curr_errors < prev_errors {
            return true;
        }
    }
    false
}

fn observe_invocation(invocation: &ToolInvocation) -> ActionObservation {
    let primary_path = primary_path(invocation);
    let fingerprint = normalized_fingerprint(invocation, primary_path.as_deref());
    let error_signature = if invocation.success {
        None
    } else {
        first_non_empty_line(&invocation.output).map(normalize_text)
    };
    ActionObservation {
        kind: action_kind(&invocation.tool_id),
        fingerprint,
        error_signature,
        primary_path,
    }
}

fn action_kind(tool_id: &str) -> ActionKind {
    match tool_id {
        "Read" => ActionKind::Read,
        "Write" => ActionKind::Write,
        "Edit" => ActionKind::Edit,
        "Bash" => ActionKind::Bash,
        _ => ActionKind::Other,
    }
}

fn render_action_preview(action: &ActionObservation) -> String {
    match &action.primary_path {
        Some(path) => format!("{:?} {}", action.kind, path),
        None => action.fingerprint.clone(),
    }
}

fn primary_path(invocation: &ToolInvocation) -> Option<String> {
    let input = serde_json::from_str::<Value>(&invocation.input).ok()?;
    if let Some(path) = input.get("file_path").and_then(Value::as_str) {
        return Some(path.to_string());
    }
    if invocation.tool_id == "Bash" {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return extract_path_candidates(command).into_iter().next();
    }
    None
}

fn normalized_fingerprint(invocation: &ToolInvocation, primary_path: Option<&str>) -> String {
    match invocation.tool_id.as_str() {
        "Read" | "Write" | "Edit" => format!(
            "{}:{}",
            invocation.tool_id.to_ascii_lowercase(),
            primary_path.unwrap_or("unknown")
        ),
        "Bash" => {
            let input = serde_json::from_str::<Value>(&invocation.input).ok();
            let command = input
                .as_ref()
                .and_then(|value| value.get("command"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let normalized = normalize_text(command);
            let head = normalized
                .split_whitespace()
                .take(4)
                .collect::<Vec<_>>()
                .join(" ");
            match primary_path {
                Some(path) => format!("bash:{head}:{path}"),
                None => format!("bash:{head}"),
            }
        }
        _ => format!(
            "{}:{}",
            invocation.tool_id.to_ascii_lowercase(),
            normalize_text(&invocation.input)
        ),
    }
}

fn build_prompt(
    language: ReflectionLanguage,
    goal: &str,
    summary: &str,
    signal_lines: &str,
    recent_actions: &str,
    relevant_paths: &str,
    judge_lines: &str,
) -> String {
    match language {
        ReflectionLanguage::Chinese => format!(
            "<system-reminder>\n反思检查点已触发。\n{summary}\n\n当前目标摘要：\n- {goal}\n\nJudge 结论：\n{judge_lines}\n\n最近信号：\n{signal_lines}\n\n最近动作：\n{recent_actions}\n\n相关文件：\n{relevant_paths}\n\n先在内部用中文回答下面 5 个问题，再继续执行任务。除非你决定升级处理，否则不要把这段反思原样告诉用户。\n1. 当前目标是什么？\n2. 有哪些证据说明当前方法有效或无效？\n3. 自上次 checkpoint 以来有什么变化？\n4. 现在最好的下一步动作是什么？\n5. 继续、重规划，还是升级处理？\n\n输出约束：\n- 先在内部得出一个决定：CONTINUE、REPLAN 或 ESCALATE。\n- 如果决定是 REPLAN，立刻换方法，不要重复刚才那条路径。\n- 如果决定是 ESCALATE，但当前没有用户可问，就简短说明阻塞点并采取成本最低的 fallback，而不是继续死循环。\n- 不要只停在反思；反思后要继续做事。\n</system-reminder>"
        ),
        ReflectionLanguage::English => format!(
            "<system-reminder>\nReflection checkpoint triggered.\n{summary}\n\nCurrent goal summary:\n- {goal}\n\nJudge verdict:\n{judge_lines}\n\nRecent signals:\n{signal_lines}\n\nRecent actions:\n{recent_actions}\n\nRelevant files:\n{relevant_paths}\n\nAnswer the following 5 questions internally in English before you continue. Do not echo the full reflection to the user unless you decide to escalate.\n1. What is the current goal?\n2. What evidence says the current approach is or is not working?\n3. What changed since the last checkpoint?\n4. What is the next best action?\n5. Continue, replan, or escalate?\n\nOutput constraints:\n- Decide internally: CONTINUE, REPLAN, or ESCALATE.\n- If the decision is REPLAN, switch methods immediately instead of repeating the current path.\n- If the decision is ESCALATE and no user interaction is available, state the blocker briefly and take the cheapest viable fallback instead of looping.\n- Do not stop at reflection; continue the task.\n</system-reminder>"
        ),
    }
}

fn language_label(language: ReflectionLanguage) -> &'static str {
    match language {
        ReflectionLanguage::English => "en",
        ReflectionLanguage::Chinese => "zh",
    }
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
