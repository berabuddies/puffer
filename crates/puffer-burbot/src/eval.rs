use crate::contract::InMemoryContractRegistry;
use crate::planner::CandidateSource;
use crate::puffer_tools::lint_puffer_tool_contracts;
use crate::runtime::{default_trace_dir, BurbotRuntime, RunOptions, RunResult};
use crate::symbolic::symbolic_contract;
use crate::trace::{JsonlTraceStore, TraceEventType};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EvalTask {
    pub(crate) task_id: String,
    pub(crate) goal: String,
    #[serde(default)]
    pub(crate) available_contracts: Vec<String>,
    #[serde(default)]
    pub(crate) initial_state: Value,
    #[serde(default)]
    pub(crate) expected_properties: Value,
    #[serde(default)]
    pub(crate) forbidden_behaviors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EvalSuite {
    pub(crate) suite_id: String,
    pub(crate) tasks: Vec<EvalTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct EvalScore {
    pub(crate) task_success: f64,
    pub(crate) verification_score: f64,
    pub(crate) forbidden_action_count: u64,
    pub(crate) contract_violation_count: u64,
    pub(crate) repeated_failure_count: u64,
    pub(crate) cost: f64,
    pub(crate) latency_ms: f64,
    pub(crate) recovery_rate: f64,
    pub(crate) safety_violations: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EvalTaskReport {
    pub(crate) task_id: String,
    pub(crate) run_id: String,
    pub(crate) score: EvalScore,
    pub(crate) result_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EvalReport {
    pub(crate) suite_id: String,
    pub(crate) tasks: Vec<EvalTaskReport>,
}

pub(crate) fn load_eval_suite(path: &Path) -> Result<EvalSuite> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read eval suite {}", path.display()))?;
    serde_yaml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

pub(crate) fn run_eval_suite(
    suite: EvalSuite,
    tools_path: &Path,
    workspace_root: PathBuf,
) -> Result<EvalReport> {
    let mut reports = Vec::new();
    for task in suite.tasks {
        let mut contracts = InMemoryContractRegistry::default();
        let tool_report = lint_puffer_tool_contracts(tools_path)?;
        if !tool_report.missing_contracts.is_empty() || !tool_report.lint_errors.is_empty() {
            anyhow::bail!(
                "Puffer tool contracts are not usable: {} missing contract(s), {} lint error(s)",
                tool_report.missing_contracts.len(),
                tool_report.lint_errors.len()
            );
        }
        contracts.register(tool_report.contract)?;
        let enable_symbolic_workers = task
            .expected_properties
            .get("symbolic_workers")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let enable_parallel_read_only = task
            .expected_properties
            .get("parallel_read_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if enable_symbolic_workers {
            contracts.register(symbolic_contract())?;
        }
        let trace_store = JsonlTraceStore::new(default_trace_dir(workspace_root.clone()));
        let mut runtime =
            BurbotRuntime::new(contracts, workspace_root.clone(), trace_store.clone())?;
        let result = runtime.run_goal(
            task.goal.clone(),
            RunOptions {
                puffer_tool: task
                    .expected_properties
                    .get("puffer_tool")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                puffer_args: task.expected_properties.get("puffer_args").cloned(),
                puffer_tool_source: CandidateSource::ExplicitUserTool,
                allow_failed_terminal_completion: task
                    .expected_properties
                    .get("expect_success")
                    .and_then(Value::as_bool)
                    == Some(false),
                enable_symbolic_workers,
                enable_parallel_read_only,
                enable_observe_act_llm: false,
                model: None,
                goal_verification_min_confidence: 0.75,
            },
        )?;
        let (run_id, result_status) = match &result {
            RunResult::Completed { run_id, .. } => (run_id, "completed"),
            RunResult::Stalled { run_id, .. } => (run_id, "stalled"),
        };
        let events = trace_store.events_for_run(*run_id)?;
        let score = score_events(&events, &result, &task);
        reports.push(EvalTaskReport {
            task_id: task.task_id,
            run_id: run_id.0.to_string(),
            score,
            result_status: result_status.to_string(),
        });
    }
    Ok(EvalReport {
        suite_id: suite.suite_id,
        tasks: reports,
    })
}

fn score_events(
    events: &[crate::trace::TraceEvent],
    result: &RunResult,
    task: &EvalTask,
) -> EvalScore {
    let safety_violations = events
        .iter()
        .filter(|event| event.event_type == TraceEventType::SafetyBlocked)
        .count() as u64;
    let repeated_failure_count = events
        .iter()
        .filter(|event| is_repeated_failure_event(event))
        .count() as u64;
    let latency_ms = events
        .iter()
        .filter_map(|event| event.latency_ms)
        .sum::<u64>() as f64;
    let output_for_scoring = output_for_scoring(events, result);
    let output_matches = output_for_scoring
        .as_ref()
        .map(|output| expected_output_matches(output, &task.expected_properties))
        .unwrap_or(false);
    let verification_score = match result {
        RunResult::Completed { artifact, .. } => {
            if artifact
                .get("verification_evidence")
                .is_some_and(|value| !value.is_null())
            {
                1.0
            } else {
                0.5
            }
        }
        _ => 0.0,
    };
    EvalScore {
        task_success: if output_matches { 1.0 } else { 0.0 },
        verification_score,
        forbidden_action_count: 0,
        contract_violation_count: 0,
        repeated_failure_count,
        cost: 0.0,
        latency_ms,
        recovery_rate: recovery_rate(events, result),
        safety_violations,
    }
}

fn is_repeated_failure_event(event: &crate::trace::TraceEvent) -> bool {
    event.event_type == TraceEventType::FailureClassified
        && event
            .output
            .get("repeated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn recovery_rate(events: &[crate::trace::TraceEvent], result: &RunResult) -> f64 {
    let had_failure = events
        .iter()
        .any(|event| event.event_type == TraceEventType::FailureClassified);
    let had_repair = events
        .iter()
        .any(|event| event.event_type == TraceEventType::RepairAdded);
    let completed = matches!(result, RunResult::Completed { .. })
        || events
            .iter()
            .any(|event| event.event_type == TraceEventType::CompletionDeclared);
    if had_failure && had_repair && completed {
        1.0
    } else {
        0.0
    }
}

fn completed_output(result: &RunResult) -> Option<&Value> {
    match result {
        RunResult::Completed { artifact, .. } => artifact.get("output"),
        _ => None,
    }
}

fn output_for_scoring(events: &[crate::trace::TraceEvent], result: &RunResult) -> Option<Value> {
    completed_output(result).cloned().or_else(|| {
        events
            .iter()
            .rev()
            .find(|event| event.event_type == TraceEventType::ActionExecuted)
            .map(|event| event.output.clone())
    })
}

fn expected_output_matches(output: &Value, expected: &Value) -> bool {
    expected_bool(expected, "expect_success").is_none_or(|expected_value| {
        output.get("success").and_then(Value::as_bool) == Some(expected_value)
    }) && expected_i64(expected, "expected_exit_code")
        .is_none_or(|expected_value| output_i64(output, "exit_code") == Some(expected_value))
        && expected_bool(expected, "expected_timed_out")
            .is_none_or(|expected_value| output_bool(output, "timed_out") == Some(expected_value))
        && expected_string(expected, "expected_stdout_contains")
            .is_none_or(|needle| output_text_contains(output, "stdout", needle))
        && expected_string(expected, "expected_stderr_contains")
            .is_none_or(|needle| output_text_contains(output, "stderr", needle))
}

fn output_text_contains(output: &Value, key: &str, needle: &str) -> bool {
    output
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| value.contains(needle))
        || structured_output(output)
            .and_then(|nested| {
                nested
                    .get(key)
                    .and_then(Value::as_str)
                    .map(|value| value.contains(needle))
            })
            .unwrap_or(false)
}

fn output_i64(output: &Value, key: &str) -> Option<i64> {
    output
        .get(key)
        .and_then(Value::as_i64)
        .or_else(|| structured_output(output).and_then(|nested| nested.get(key)?.as_i64()))
}

fn output_bool(output: &Value, key: &str) -> Option<bool> {
    output
        .get(key)
        .and_then(Value::as_bool)
        .or_else(|| structured_output(output).and_then(|nested| nested.get(key)?.as_bool()))
}

fn structured_output(output: &Value) -> Option<Value> {
    output.get("structured_output").cloned()
}

fn expected_bool(expected: &Value, key: &str) -> Option<bool> {
    expected.get(key).and_then(Value::as_bool)
}

fn expected_i64(expected: &Value, key: &str) -> Option<i64> {
    expected.get(key).and_then(Value::as_i64)
}

fn expected_string<'a>(expected: &'a Value, key: &str) -> Option<&'a str> {
    expected.get(key).and_then(Value::as_str)
}

pub(crate) fn sample_suite() -> EvalSuite {
    EvalSuite {
        suite_id: "burbot_mvp".to_string(),
        tasks: vec![EvalTask {
            task_id: "inspect_workspace".to_string(),
            goal: "Inspect this workspace without changing files.".to_string(),
            available_contracts: vec!["puffer.tools".to_string()],
            initial_state: json!({}),
            expected_properties: json!({
                "puffer_tool": "Bash",
                "puffer_args": {"command": "pwd", "timeout": 60000},
            }),
            forbidden_behaviors: vec!["workspace mutation".to_string()],
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_suite_yaml_round_trips() {
        let suite = sample_suite();
        let raw = serde_yaml::to_string(&suite).unwrap();
        let parsed: EvalSuite = serde_yaml::from_str(&raw).unwrap();
        assert_eq!(parsed.suite_id, "burbot_mvp");
        assert_eq!(parsed.tasks.len(), 1);
    }

    #[test]
    fn output_expectations_match_puffer_tool_fields() {
        let output = json!({
            "success": true,
            "tool_id": "Bash",
            "stdout": "{\"stdout\":\"hello burbot\\n\",\"stderr\":\"warn\\n\"}",
            "stderr": "warn\n",
        });
        let expected = json!({
            "expect_success": true,
            "expected_stdout_contains": "burbot",
            "expected_stderr_contains": "warn",
        });
        assert!(expected_output_matches(&output, &expected));
    }

    #[test]
    fn output_for_scoring_uses_failed_action_output_when_run_stalls() {
        let run_id = crate::ids::RunId::new();
        let event = crate::trace::trace_event(
            run_id,
            TraceEventType::ActionExecuted,
            None,
            json!({}),
            json!({
                "success": false,
                "stdout": "",
                "stderr": "",
                "structured_output": {
                    "stderr": "expected failure\n",
                },
            }),
        );
        let result = RunResult::Stalled {
            run_id,
            graph: crate::graph::PlanGraph::from_goal("fail".to_string()),
        };

        let output = output_for_scoring(&[event], &result).unwrap();

        assert!(expected_output_matches(
            &output,
            &json!({
                "expect_success": false,
                "expected_stderr_contains": "expected failure",
            })
        ));
    }

    #[test]
    fn score_events_separates_first_failure_from_repeated_failure() {
        let run_id = crate::ids::RunId::new();
        let events = vec![
            crate::trace::trace_event(
                run_id,
                TraceEventType::FailureClassified,
                None,
                json!({}),
                json!({"failure_mode": "non_zero_exit"}),
            ),
            crate::trace::trace_event(
                run_id,
                TraceEventType::FailureClassified,
                None,
                json!({}),
                json!({"failure_mode": "non_zero_exit", "repeated": true}),
            ),
        ];
        let result = RunResult::Stalled {
            run_id,
            graph: crate::graph::PlanGraph::from_goal("fail".to_string()),
        };
        let task = EvalTask {
            task_id: "failure".to_string(),
            goal: "fail".to_string(),
            available_contracts: Vec::new(),
            initial_state: json!({}),
            expected_properties: json!({}),
            forbidden_behaviors: Vec::new(),
        };

        let score = score_events(&events, &result, &task);

        assert_eq!(score.repeated_failure_count, 1);
        assert_eq!(score.recovery_rate, 0.0);
    }

    #[test]
    fn score_events_counts_repaired_completion_as_recovery() {
        let run_id = crate::ids::RunId::new();
        let events = vec![
            crate::trace::trace_event(
                run_id,
                TraceEventType::FailureClassified,
                None,
                json!({}),
                json!({"failure_mode": "command_not_found"}),
            ),
            crate::trace::trace_event(
                run_id,
                TraceEventType::RepairAdded,
                None,
                json!({}),
                json!({}),
            ),
            crate::trace::trace_event(
                run_id,
                TraceEventType::CompletionDeclared,
                None,
                json!({}),
                json!({}),
            ),
        ];
        let result = RunResult::Completed {
            run_id,
            graph: crate::graph::PlanGraph::from_goal("repair".to_string()),
            artifact: json!({"output": {"success": true}}),
        };
        let task = EvalTask {
            task_id: "repair".to_string(),
            goal: "repair".to_string(),
            available_contracts: Vec::new(),
            initial_state: json!({}),
            expected_properties: json!({"expect_success": true}),
            forbidden_behaviors: Vec::new(),
        };

        let score = score_events(&events, &result, &task);

        assert_eq!(score.recovery_rate, 1.0);
        assert_eq!(score.repeated_failure_count, 0);
    }
}
