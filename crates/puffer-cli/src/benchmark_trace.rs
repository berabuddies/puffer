use anyhow::{Context, Result};
use puffer_core::{ToolInvocation, TurnStreamEvent};
use puffer_session_store::{SessionMetadata, TranscriptEvent};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APP_NAME: &str = "puffer-code";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Stores one executed tool call in benchmark artifacts.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct BenchmarkToolInvocation {
    pub(crate) call_id: String,
    pub(crate) tool_id: String,
    pub(crate) input: String,
    pub(crate) output: String,
    pub(crate) success: bool,
}

/// Stores the final visible outcome of one benchmark turn.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct BenchmarkResult {
    pub(crate) success: bool,
    pub(crate) session_id: String,
    pub(crate) prompt_cache_key: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) effort: String,
    pub(crate) fast_mode: bool,
    pub(crate) prompt: String,
    pub(crate) assistant_text: String,
    pub(crate) tool_invocations: Vec<BenchmarkToolInvocation>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct BenchmarkToolCallRequest {
    tool_id: String,
    input: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BenchmarkTraceEvent {
    UserPrompt {
        message: String,
        timestamp_ms: u64,
    },
    ThinkingDelta {
        delta: String,
        timestamp_ms: u64,
    },
    TextDelta {
        delta: String,
        timestamp_ms: u64,
    },
    ToolCallsRequested {
        tool_calls: Vec<BenchmarkToolCallRequest>,
        timestamp_ms: u64,
    },
    ToolInvocations {
        invocations: Vec<BenchmarkToolInvocation>,
        timestamp_ms: u64,
    },
    RetryAttempt {
        attempt: usize,
        max_attempts: usize,
        error: String,
        timestamp_ms: u64,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
        timestamp_ms: u64,
    },
    AssistantMessage {
        message: String,
        timestamp_ms: u64,
    },
    Failure {
        error: String,
        timestamp_ms: u64,
    },
}

#[derive(Debug, Default)]
struct PendingTraceStep {
    message: String,
    thinking: String,
    requested_tool_calls: Vec<Value>,
    tool_calls: Vec<Value>,
    observations: Vec<Value>,
    retry_attempts: Vec<Value>,
    usage: Vec<Value>,
}

impl PendingTraceStep {
    fn has_content(&self) -> bool {
        !self.message.is_empty()
            || !self.thinking.is_empty()
            || !self.requested_tool_calls.is_empty()
            || !self.tool_calls.is_empty()
            || !self.observations.is_empty()
            || !self.retry_attempts.is_empty()
            || !self.usage.is_empty()
    }

    fn step_message(&self) -> String {
        if !self.message.trim().is_empty() {
            return self.message.clone();
        }
        if !self.tool_calls.is_empty() || !self.requested_tool_calls.is_empty() {
            return "Executed tool calls".to_string();
        }
        if !self.thinking.trim().is_empty() {
            return "Reasoned about the task".to_string();
        }
        "Processed benchmark turn".to_string()
    }
}

/// Captures benchmark streaming events and writes trace artifacts.
#[derive(Debug)]
pub(crate) struct BenchmarkTraceRecorder {
    events: Vec<BenchmarkTraceEvent>,
    incremental_file: Option<File>,
}

impl BenchmarkTraceRecorder {
    /// Creates a recorder for one benchmark run and primes the incremental trace file.
    pub(crate) fn new(trajectory_path: Option<&Path>) -> Self {
        let incremental_file = trajectory_path
            .map(|path| path.with_extension("incremental.jsonl"))
            .and_then(|path| open_incremental_file(&path).ok());
        Self {
            events: Vec::new(),
            incremental_file,
        }
    }

    /// Records the benchmark prompt as the first event in the trace.
    pub(crate) fn record_prompt(&mut self, prompt: &str) {
        self.push_event(BenchmarkTraceEvent::UserPrompt {
            message: prompt.to_string(),
            timestamp_ms: unix_time_ms(),
        });
    }

    /// Records one runtime streaming event in the trace.
    pub(crate) fn record_stream_event(&mut self, event: &TurnStreamEvent) {
        match event {
            TurnStreamEvent::ThinkingDelta(delta) => {
                self.push_event(BenchmarkTraceEvent::ThinkingDelta {
                    delta: delta.clone(),
                    timestamp_ms: unix_time_ms(),
                });
            }
            TurnStreamEvent::TextDelta(delta) => {
                self.push_event(BenchmarkTraceEvent::TextDelta {
                    delta: delta.clone(),
                    timestamp_ms: unix_time_ms(),
                });
            }
            TurnStreamEvent::ToolCallsRequested(tool_calls) => {
                self.push_event(BenchmarkTraceEvent::ToolCallsRequested {
                    tool_calls: tool_calls
                        .iter()
                        .map(|tool_call| BenchmarkToolCallRequest {
                            tool_id: tool_call.tool_id.clone(),
                            input: tool_call.input.clone(),
                        })
                        .collect(),
                    timestamp_ms: unix_time_ms(),
                });
            }
            TurnStreamEvent::ToolInvocations(invocations) => {
                self.push_event(BenchmarkTraceEvent::ToolInvocations {
                    invocations: invocations.iter().map(map_tool_invocation).collect(),
                    timestamp_ms: unix_time_ms(),
                });
            }
            TurnStreamEvent::RetryAttempt {
                attempt,
                max_attempts,
                error,
            } => {
                self.push_event(BenchmarkTraceEvent::RetryAttempt {
                    attempt: *attempt,
                    max_attempts: *max_attempts,
                    error: error.clone(),
                    timestamp_ms: unix_time_ms(),
                });
            }
            TurnStreamEvent::Usage(report) => {
                self.push_event(BenchmarkTraceEvent::Usage {
                    input_tokens: report.input_tokens,
                    output_tokens: report.output_tokens,
                    cache_read_tokens: report.cache_read_tokens,
                    cache_creation_tokens: report.cache_creation_tokens,
                    timestamp_ms: unix_time_ms(),
                });
            }
        }
    }

    /// Records the final assistant message for the benchmark run.
    pub(crate) fn record_final_assistant(&mut self, message: &str) {
        self.push_event(BenchmarkTraceEvent::AssistantMessage {
            message: message.to_string(),
            timestamp_ms: unix_time_ms(),
        });
    }

    /// Records a terminal benchmark failure in the trace.
    pub(crate) fn record_failure(&mut self, error: &str) {
        self.push_event(BenchmarkTraceEvent::Failure {
            error: error.to_string(),
            timestamp_ms: unix_time_ms(),
        });
    }

    /// Writes the ATIF trajectory artifact for the recorded benchmark trace.
    pub(crate) fn write_trajectory_json(
        &self,
        path: &str,
        result: &BenchmarkResult,
        metadata: &SessionMetadata,
        events: &[TranscriptEvent],
    ) -> Result<()> {
        write_json_file(
            path,
            &build_trajectory_json(result, &self.events, metadata, events),
        )
    }

    /// Writes a session transcript artifact beside the trajectory file.
    pub(crate) fn write_session_json(
        &self,
        trajectory_path: &str,
        metadata: &SessionMetadata,
        events: &[TranscriptEvent],
    ) -> Result<()> {
        let session_path = PathBuf::from(trajectory_path).with_extension("session.json");
        let value = json!({
            "metadata": metadata,
            "events": events,
        });
        write_json_path(&session_path, &value)
    }

    fn push_event(&mut self, event: BenchmarkTraceEvent) {
        if let Some(file) = &mut self.incremental_file {
            let _ = writeln!(
                file,
                "{}",
                serde_json::to_string(&event).unwrap_or_default()
            );
            let _ = file.flush();
        }
        self.events.push(event);
    }
}

/// Writes one benchmark JSON artifact to disk.
pub(crate) fn write_json_file(path: &str, value: &Value) -> Result<()> {
    write_json_path(&PathBuf::from(path), value)
}

fn write_json_path(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn open_incremental_file(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    File::create(path).with_context(|| format!("failed to create {}", path.display()))
}

fn map_tool_invocation(invocation: &ToolInvocation) -> BenchmarkToolInvocation {
    BenchmarkToolInvocation {
        call_id: invocation.call_id.clone(),
        tool_id: invocation.tool_id.clone(),
        input: invocation.input.clone(),
        output: invocation.output.clone(),
        success: invocation.success,
    }
}

fn build_trajectory_json(
    result: &BenchmarkResult,
    events: &[BenchmarkTraceEvent],
    metadata: &SessionMetadata,
    session_events: &[TranscriptEvent],
) -> Value {
    let mut steps = vec![json!({
        "step_id": 1,
        "source": "user",
        "message": result.prompt,
    })];
    let mut pending = PendingTraceStep::default();

    for event in events {
        match event {
            BenchmarkTraceEvent::UserPrompt { .. } => {}
            BenchmarkTraceEvent::ThinkingDelta { delta, .. } => pending.thinking.push_str(delta),
            BenchmarkTraceEvent::TextDelta { delta, .. } => pending.message.push_str(delta),
            BenchmarkTraceEvent::ToolCallsRequested { tool_calls, .. } => {
                pending.requested_tool_calls.extend(
                    tool_calls
                        .iter()
                        .map(|tool_call| build_requested_tool_call(tool_call)),
                );
            }
            BenchmarkTraceEvent::ToolInvocations { invocations, .. } => {
                if pending.tool_calls.is_empty() {
                    pending
                        .tool_calls
                        .extend(invocations.iter().map(build_tool_call_from_invocation));
                }
                pending
                    .observations
                    .extend(invocations.iter().map(build_observation_result));
                finalize_pending_step(&mut steps, &mut pending, &result.model);
            }
            BenchmarkTraceEvent::RetryAttempt {
                attempt,
                max_attempts,
                error,
                ..
            } => {
                pending.retry_attempts.push(json!({
                    "attempt": attempt,
                    "max_attempts": max_attempts,
                    "error": error,
                }));
            }
            BenchmarkTraceEvent::Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                ..
            } => {
                pending.usage.push(json!({
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "cache_read_tokens": cache_read_tokens,
                    "cache_creation_tokens": cache_creation_tokens,
                }));
            }
            BenchmarkTraceEvent::AssistantMessage { message, .. } => {
                if pending.message.trim().is_empty() || pending.message.trim() != message.trim() {
                    pending.message = message.clone();
                }
                finalize_pending_step(&mut steps, &mut pending, &result.model);
            }
            BenchmarkTraceEvent::Failure { error, .. } => {
                finalize_pending_step(&mut steps, &mut pending, &result.model);
                steps.push(json!({
                    "step_id": steps.len() + 1,
                    "source": "agent",
                    "model_name": result.model,
                    "message": format!("Benchmark run failed: {error}"),
                }));
            }
        }
    }

    finalize_pending_step(&mut steps, &mut pending, &result.model);
    if steps.len() == 1 {
        let final_message = result
            .error
            .as_deref()
            .map(|error| format!("Benchmark run failed: {error}"))
            .unwrap_or_else(|| result.assistant_text.clone());
        steps.push(json!({
            "step_id": 2,
            "source": "agent",
            "model_name": result.model,
            "message": final_message,
        }));
    }

    json!({
        "schema_version": "ATIF-v1.6",
        "session_id": result.session_id,
        "agent": {
            "name": APP_NAME,
            "version": APP_VERSION,
            "model_name": result.model,
            "extra": {
                "provider": result.provider,
                "effort": result.effort,
                "fast_mode": result.fast_mode,
                "prompt_cache_key": result.prompt_cache_key,
                "stream_event_count": events.len(),
            }
        },
        "steps": steps,
        "stream_events": events,
        "session": {
            "metadata": metadata,
            "events": session_events,
        },
        "extra": {
            "success": result.success,
            "full_incremental_trace": "trajectory.incremental.jsonl",
            "full_session_trace": "trajectory.session.json",
        }
    })
}

fn finalize_pending_step(steps: &mut Vec<Value>, pending: &mut PendingTraceStep, model: &str) {
    if !pending.has_content() {
        return;
    }
    let mut step = serde_json::Map::new();
    step.insert("step_id".to_string(), json!(steps.len() + 1));
    step.insert("source".to_string(), json!("agent"));
    step.insert("model_name".to_string(), json!(model));
    step.insert("message".to_string(), json!(pending.step_message()));
    if !pending.tool_calls.is_empty() {
        step.insert(
            "tool_calls".to_string(),
            Value::Array(std::mem::take(&mut pending.tool_calls)),
        );
    }
    if !pending.observations.is_empty() {
        step.insert(
            "observation".to_string(),
            json!({
                "results": std::mem::take(&mut pending.observations),
            }),
        );
    }
    let mut extra = serde_json::Map::new();
    if !pending.thinking.is_empty() {
        extra.insert(
            "thinking".to_string(),
            Value::String(std::mem::take(&mut pending.thinking)),
        );
    }
    if !pending.requested_tool_calls.is_empty() {
        extra.insert(
            "requested_tool_calls".to_string(),
            Value::Array(std::mem::take(&mut pending.requested_tool_calls)),
        );
    }
    if !pending.retry_attempts.is_empty() {
        extra.insert(
            "retry_attempts".to_string(),
            Value::Array(std::mem::take(&mut pending.retry_attempts)),
        );
    }
    if !pending.usage.is_empty() {
        extra.insert(
            "usage".to_string(),
            Value::Array(std::mem::take(&mut pending.usage)),
        );
    }
    if !extra.is_empty() {
        step.insert("extra".to_string(), Value::Object(extra));
    }
    steps.push(Value::Object(step));
    *pending = PendingTraceStep::default();
}

fn build_requested_tool_call(tool_call: &BenchmarkToolCallRequest) -> Value {
    json!({
        "function_name": tool_call.tool_id,
        "arguments": parse_tool_arguments(&tool_call.input),
    })
}

fn build_tool_call_from_invocation(invocation: &BenchmarkToolInvocation) -> Value {
    json!({
        "tool_call_id": invocation.call_id,
        "function_name": invocation.tool_id,
        "arguments": parse_tool_arguments(&invocation.input),
    })
}

fn build_observation_result(invocation: &BenchmarkToolInvocation) -> Value {
    json!({
        "source_call_id": invocation.call_id,
        "content": invocation.output,
        "success": invocation.success,
    })
}

fn parse_tool_arguments(raw: &str) -> Value {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        if value.is_object() {
            return value;
        }
        return json!({ "value": value });
    }
    json!({ "value": raw })
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trajectory_includes_streamed_tool_rounds() {
        let mut recorder = BenchmarkTraceRecorder::new(None);
        recorder.record_prompt("solve task");
        recorder.record_stream_event(&TurnStreamEvent::ThinkingDelta("plan".to_string()));
        recorder.record_stream_event(&TurnStreamEvent::TextDelta("Inspecting files.".to_string()));
        recorder.record_stream_event(&TurnStreamEvent::ToolCallsRequested(vec![
            puffer_core::ToolCallRequest {
                tool_id: "Read".to_string(),
                input: r#"{"path":"src/main.rs"}"#.to_string(),
            },
        ]));
        recorder.record_stream_event(&TurnStreamEvent::ToolInvocations(vec![ToolInvocation {
            call_id: "call-1".to_string(),
            tool_id: "Read".to_string(),
            input: r#"{"path":"src/main.rs"}"#.to_string(),
            output: "fn main() {}".to_string(),
            success: true,
        }]));
        recorder.record_stream_event(&TurnStreamEvent::TextDelta("Done.".to_string()));
        recorder.record_final_assistant("Done.");

        let value = build_trajectory_json(
            &BenchmarkResult {
                success: true,
                session_id: "session-123".to_string(),
                prompt_cache_key: "cache-123".to_string(),
                provider: "openai".to_string(),
                model: "openai/gpt-5.4".to_string(),
                effort: "high".to_string(),
                fast_mode: true,
                prompt: "solve task".to_string(),
                assistant_text: "Done.".to_string(),
                tool_invocations: vec![],
                error: None,
            },
            &recorder.events,
            &SessionMetadata {
                id: uuid::Uuid::nil(),
                display_name: Some("benchmark-run".to_string()),
                cwd: std::path::PathBuf::from("/tmp/bench"),
                created_at_ms: 1,
                updated_at_ms: 1,
                parent_session_id: None,
                slug: None,
                tags: vec!["benchmark".to_string()],
                note: None,
            },
            &[
                TranscriptEvent::UserMessage {
                    text: "solve task".to_string(),
                },
                TranscriptEvent::ToolInvocation {
                    call_id: "call-1".to_string(),
                    tool_id: "Read".to_string(),
                    input: r#"{"path":"src/main.rs"}"#.to_string(),
                    output: "fn main() {}".to_string(),
                    success: true,
                },
                TranscriptEvent::AssistantMessage {
                    text: "Done.".to_string(),
                },
            ],
        );

        assert_eq!(value["steps"].as_array().unwrap().len(), 3);
        assert_eq!(value["steps"][1]["message"], "Inspecting files.");
        assert_eq!(value["steps"][1]["tool_calls"][0]["tool_call_id"], "call-1");
        assert_eq!(
            value["steps"][1]["observation"]["results"][0]["content"],
            "fn main() {}"
        );
        assert_eq!(value["steps"][1]["extra"]["thinking"], "plan");
        assert_eq!(value["steps"][2]["message"], "Done.");
        assert_eq!(value["stream_events"].as_array().unwrap().len(), 7);
        assert_eq!(value["session"]["events"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn trajectory_records_failures_after_partial_progress() {
        let events = vec![
            BenchmarkTraceEvent::UserPrompt {
                message: "prompt".to_string(),
                timestamp_ms: 1,
            },
            BenchmarkTraceEvent::TextDelta {
                delta: "partial".to_string(),
                timestamp_ms: 2,
            },
            BenchmarkTraceEvent::Failure {
                error: "boom".to_string(),
                timestamp_ms: 3,
            },
        ];
        let value = build_trajectory_json(
            &BenchmarkResult {
                success: false,
                session_id: "session-123".to_string(),
                prompt_cache_key: "cache-123".to_string(),
                provider: "openai".to_string(),
                model: "openai/gpt-5.4".to_string(),
                effort: "high".to_string(),
                fast_mode: true,
                prompt: "prompt".to_string(),
                assistant_text: String::new(),
                tool_invocations: vec![],
                error: Some("boom".to_string()),
            },
            &events,
            &SessionMetadata {
                id: uuid::Uuid::nil(),
                display_name: Some("benchmark-run".to_string()),
                cwd: std::path::PathBuf::from("/tmp/bench"),
                created_at_ms: 1,
                updated_at_ms: 1,
                parent_session_id: None,
                slug: None,
                tags: vec!["benchmark".to_string()],
                note: None,
            },
            &[
                TranscriptEvent::UserMessage {
                    text: "prompt".to_string(),
                },
                TranscriptEvent::SystemMessage {
                    text: "Benchmark run failed: boom".to_string(),
                },
            ],
        );

        assert_eq!(value["steps"][1]["message"], "partial");
        assert_eq!(value["steps"][2]["message"], "Benchmark run failed: boom");
        assert_eq!(value["session"]["events"][1]["type"], "system_message");
    }
}
