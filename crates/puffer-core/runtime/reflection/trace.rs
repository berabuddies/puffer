use super::{
    judge::LlmJudgeAttempt, BatchAssessment, JudgeSignal, LlmJudgeConfig, LlmJudgeContextScope,
    LlmJudgeMode, LlmJudgePromptCacheMode, ReflectionCheckpoint,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MAX_RELEVANT_PATHS_IN_TRACE: usize = 12;

/// Records one structured reflection-stage event for trajectory and debug traces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReflectionTraceEvent {
    /// Records the heuristic assessment collected after one batch of tool calls.
    BatchObserved {
        batch_count: usize,
        total_tool_calls: usize,
        should_evaluate: bool,
        skip_reason: Option<String>,
        evaluation_score: u8,
        evaluation_threshold: u8,
        validation_progress: bool,
        artifact_progress: bool,
        edit_progress: bool,
        loopiness_score: u8,
        focus_bad: bool,
        time_since_progress_ms: u128,
        signal_notes: Vec<String>,
        recent_actions: Vec<String>,
        relevant_paths: Vec<String>,
    },
    /// Records the code-judge score and whether it crossed its trigger threshold.
    CodeJudgeDecision {
        triggered: bool,
        score: u8,
        threshold: u8,
        summary: Option<String>,
        reason: Option<String>,
    },
    /// Records why the LLM judge did not run for this evaluation.
    LlmJudgeSkipped { mode: String, reason: String },
    /// Records the side request that was dispatched to the LLM judge.
    /// Emitted after `run_llm_judge` returns so the event carries the live
    /// URL, cache key, and serialized body that actually went on the wire.
    LlmJudgeRequest {
        mode: String,
        provider: Option<String>,
        model: Option<String>,
        effort_level: Option<String>,
        context_scope: String,
        prompt_cache_mode: String,
        recent_item_count: usize,
        max_context_chars: usize,
        max_tool_output_chars: usize,
        prompt_chars: usize,
        prompt: String,
        request_url: Option<String>,
        prompt_cache_key: Option<String>,
        request_body: Option<String>,
    },
    /// Records the parsed LLM judge response.
    LlmJudgeResponse {
        provider: Option<String>,
        model: Option<String>,
        decision: String,
        confidence: Option<String>,
        reason: String,
        next_action: String,
        response_id: Option<String>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        cache_hit_ratio: Option<String>,
        raw_response_text: String,
        raw_response_body: Option<String>,
    },
    /// Records why the LLM judge failed or could not be interpreted.
    LlmJudgeError {
        provider: Option<String>,
        model: Option<String>,
        stage: String,
        error: String,
        request_url: Option<String>,
        prompt_cache_key: Option<String>,
        request_body: Option<String>,
        raw_response_text: Option<String>,
        raw_response_body: Option<String>,
        fallback_to_code_judge: bool,
    },
    /// Records the final selection made between available judge signals.
    FinalDecision {
        selected_source: Option<String>,
        triggered_checkpoint: bool,
        summary: Option<String>,
        reason: Option<String>,
        next_action: Option<String>,
        checkpoint_prompt: Option<String>,
    },
}

pub(super) fn batch_observed_event(
    assessment: &BatchAssessment,
    batch_count: usize,
    total_tool_calls: usize,
    should_evaluate: bool,
    skip_reason: Option<String>,
    evaluation_score: u8,
    evaluation_threshold: u8,
    relevant_paths: &BTreeSet<String>,
) -> ReflectionTraceEvent {
    ReflectionTraceEvent::BatchObserved {
        batch_count,
        total_tool_calls,
        should_evaluate,
        skip_reason,
        evaluation_score,
        evaluation_threshold,
        validation_progress: assessment.validation_progress,
        artifact_progress: assessment.artifact_progress,
        edit_progress: assessment.edit_progress,
        loopiness_score: assessment.loopiness_score,
        focus_bad: assessment.focus_bad,
        time_since_progress_ms: assessment.time_since_progress_ms,
        signal_notes: assessment.signal_notes.clone(),
        recent_actions: assessment.recent_actions.clone(),
        relevant_paths: relevant_paths_snapshot(relevant_paths),
    }
}

pub(super) fn code_judge_decision_event(
    score: u8,
    threshold: u8,
    signal: Option<&JudgeSignal>,
) -> ReflectionTraceEvent {
    ReflectionTraceEvent::CodeJudgeDecision {
        triggered: signal.is_some(),
        score,
        threshold,
        summary: signal.map(|value| value.summary.clone()),
        reason: signal.map(|value| value.reason.clone()),
    }
}

pub(super) fn llm_judge_skipped_event(
    mode: LlmJudgeMode,
    reason: impl Into<String>,
) -> ReflectionTraceEvent {
    ReflectionTraceEvent::LlmJudgeSkipped {
        mode: llm_mode_label(mode).to_string(),
        reason: reason.into(),
    }
}

/// Records the special skip case where the LLM judge was not configured at
/// all (i.e. `ReflectionConfig::llm_judge` is `None`). Kept separate from
/// `llm_judge_skipped_event` so the `"disabled"` label lives in the same
/// dictionary as the mode labels below.
pub(super) fn llm_judge_disabled_event(reason: impl Into<String>) -> ReflectionTraceEvent {
    ReflectionTraceEvent::LlmJudgeSkipped {
        mode: LLM_MODE_DISABLED_LABEL.to_string(),
        reason: reason.into(),
    }
}

const LLM_MODE_DISABLED_LABEL: &str = "disabled";

pub(super) fn llm_judge_request_event(
    config: &LlmJudgeConfig,
    attempt: &LlmJudgeAttempt,
) -> ReflectionTraceEvent {
    ReflectionTraceEvent::LlmJudgeRequest {
        mode: llm_mode_label(config.mode).to_string(),
        provider: attempt.provider.clone(),
        model: attempt.model.clone(),
        effort_level: config.effort_level.clone(),
        context_scope: context_scope_label(config.context_scope).to_string(),
        prompt_cache_mode: prompt_cache_mode_label(config.prompt_cache_mode).to_string(),
        recent_item_count: config.recent_item_count,
        max_context_chars: config.max_context_chars,
        max_tool_output_chars: config.max_tool_output_chars,
        prompt_chars: attempt.prompt.chars().count(),
        prompt: attempt.prompt.clone(),
        request_url: attempt.request_url.clone(),
        prompt_cache_key: attempt.prompt_cache_key.clone(),
        request_body: attempt.request_body.clone(),
    }
}

pub(super) fn llm_judge_response_event(
    attempt: &LlmJudgeAttempt,
    decision: &str,
    confidence: Option<String>,
    reason: &str,
    next_action: &str,
) -> ReflectionTraceEvent {
    ReflectionTraceEvent::LlmJudgeResponse {
        provider: attempt.provider.clone(),
        model: attempt.model.clone(),
        decision: decision.to_string(),
        confidence,
        reason: reason.to_string(),
        next_action: next_action.to_string(),
        response_id: attempt.response_id.clone(),
        input_tokens: attempt.input_tokens,
        output_tokens: attempt.output_tokens,
        cached_input_tokens: attempt.cached_input_tokens,
        cache_hit_ratio: attempt.cache_hit_ratio.clone(),
        raw_response_text: attempt.raw_response_text.clone().unwrap_or_default(),
        raw_response_body: attempt.raw_response_body.clone(),
    }
}

pub(super) fn llm_judge_error_event(
    stage: impl Into<String>,
    error: impl Into<String>,
    attempt: &LlmJudgeAttempt,
    fallback_to_code_judge: bool,
) -> ReflectionTraceEvent {
    ReflectionTraceEvent::LlmJudgeError {
        provider: attempt.provider.clone(),
        model: attempt.model.clone(),
        stage: stage.into(),
        error: error.into(),
        request_url: attempt.request_url.clone(),
        prompt_cache_key: attempt.prompt_cache_key.clone(),
        request_body: attempt.request_body.clone(),
        raw_response_text: attempt.raw_response_text.clone(),
        raw_response_body: attempt.raw_response_body.clone(),
        fallback_to_code_judge,
    }
}

pub(super) fn final_decision_event(
    signal: Option<&JudgeSignal>,
    checkpoint: Option<&ReflectionCheckpoint>,
) -> ReflectionTraceEvent {
    ReflectionTraceEvent::FinalDecision {
        selected_source: signal.map(|value| value.source.to_string()),
        triggered_checkpoint: checkpoint.is_some(),
        summary: signal.map(|value| value.summary.clone()),
        reason: signal.map(|value| value.reason.clone()),
        next_action: signal.and_then(|value| value.next_action.clone()),
        checkpoint_prompt: checkpoint.map(|value| value.prompt.clone()),
    }
}

fn relevant_paths_snapshot(paths: &BTreeSet<String>) -> Vec<String> {
    paths
        .iter()
        .take(MAX_RELEVANT_PATHS_IN_TRACE)
        .cloned()
        .collect()
}

fn llm_mode_label(mode: LlmJudgeMode) -> &'static str {
    match mode {
        LlmJudgeMode::Independent => "independent",
        LlmJudgeMode::ConfirmCodeJudge => "confirm_code_judge",
    }
}

fn context_scope_label(scope: LlmJudgeContextScope) -> &'static str {
    match scope {
        LlmJudgeContextScope::CurrentWindow => "current_window",
        LlmJudgeContextScope::RecentWindow => "recent_window",
        LlmJudgeContextScope::SummaryAndRecent => "summary_and_recent",
    }
}

fn prompt_cache_mode_label(mode: LlmJudgePromptCacheMode) -> &'static str {
    match mode {
        LlmJudgePromptCacheMode::InheritMainAgent => "inherit_main_agent",
        LlmJudgePromptCacheMode::Dedicated => "dedicated",
    }
}
