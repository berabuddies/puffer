use crate::contract::{ActionContract, FailureDetectionSpec, FailureModeSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureKind {
    CommandNotFound,
    MissingPath,
    PermissionDenied,
    TimedOut,
    VerificationFailed,
    GoalUnsatisfied,
    NoProgress,
    NonZeroExit,
    ToolExecutionError,
    Contract(String),
    Unknown,
}

impl FailureKind {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::CommandNotFound => "command_not_found",
            Self::MissingPath => "missing_path",
            Self::PermissionDenied => "permission_denied",
            Self::TimedOut => "timed_out",
            Self::VerificationFailed => "verification_failed",
            Self::GoalUnsatisfied => "goal_unsatisfied",
            Self::NoProgress => "no_progress",
            Self::NonZeroExit => "non_zero_exit",
            Self::ToolExecutionError => "tool_execution_error",
            Self::Contract(name) => name,
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn from_str(value: &str) -> Self {
        match value {
            "command_not_found" => Self::CommandNotFound,
            "missing_path" => Self::MissingPath,
            "permission_denied" => Self::PermissionDenied,
            "timed_out" => Self::TimedOut,
            "verification_failed" => Self::VerificationFailed,
            "goal_unsatisfied" => Self::GoalUnsatisfied,
            "no_progress" => Self::NoProgress,
            "non_zero_exit" => Self::NonZeroExit,
            "tool_execution_error" => Self::ToolExecutionError,
            "unknown" => Self::Unknown,
            other => Self::Contract(other.to_string()),
        }
    }
}

pub(crate) fn classify_failure(action: Option<&ActionContract>, output: &Value) -> FailureKind {
    if structurally_timed_out(output) {
        return FailureKind::TimedOut;
    }
    if let Some(kind) = action.and_then(|action| classify_contract_failure(action, output)) {
        return kind;
    }
    FailureKind::Unknown
}

fn structurally_timed_out(output: &Value) -> bool {
    value_deep(output, &[String::from("timed_out")], true).and_then(|value| value.as_bool())
        == Some(true)
        || value_deep(output, &[String::from("interrupted")], true)
            .and_then(|value| value.as_bool())
            == Some(true)
}

fn classify_contract_failure(action: &ActionContract, output: &Value) -> Option<FailureKind> {
    action
        .failure_modes
        .iter()
        .find(|mode| failure_mode_matches(mode, output))
        .map(|mode| FailureKind::from_str(&mode.name))
}

fn failure_mode_matches(mode: &FailureModeSpec, output: &Value) -> bool {
    mode.detection_rules
        .iter()
        .any(|rule| detection_rule_matches(rule, output))
}

fn detection_rule_matches(rule: &FailureDetectionSpec, output: &Value) -> bool {
    match rule {
        FailureDetectionSpec::JsonBool {
            path,
            expected,
            structured_output_fallback,
        } => {
            value_deep(output, path, *structured_output_fallback).and_then(|value| value.as_bool())
                == Some(*expected)
        }
        FailureDetectionSpec::JsonI64NonZero {
            path,
            structured_output_fallback,
        } => value_deep(output, path, *structured_output_fallback)
            .and_then(|value| value.as_i64())
            .is_some_and(|value| value != 0),
        FailureDetectionSpec::Present {
            path,
            structured_output_fallback,
        } => value_deep(output, path, *structured_output_fallback)
            .is_some_and(|value| !value.is_null()),
    }
}

fn value_deep(output: &Value, path: &[String], structured_output_fallback: bool) -> Option<Value> {
    value_at_path(output, path).cloned().or_else(|| {
        structured_output_fallback
            .then(|| output.get("structured_output"))
            .flatten()
            .and_then(|nested| value_at_path(nested, path).cloned())
    })
}

fn value_at_path<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(segment.as_str())?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        legacy_failure_detection_specs, ApprovalSpec, Idempotency, Reversibility, RiskLevel,
        SideEffectClass, VerificationSpec,
    };
    use serde_json::json;

    fn action(failure_modes: Vec<FailureModeSpec>) -> ActionContract {
        ActionContract {
            name: "Tool".to_string(),
            description: "test tool".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            side_effect_class: SideEffectClass::Unknown,
            reversibility: Reversibility::Unknown,
            idempotency: Idempotency::Unknown,
            risk_level: RiskLevel::Medium,
            preconditions: Vec::new(),
            postconditions: Vec::new(),
            verification: VerificationSpec {
                methods: Vec::new(),
                observation_checks: Vec::new(),
                method_templates: Vec::new(),
                templates: Vec::new(),
                required_before_completion: false,
                confidence: 0.5,
            },
            approval: ApprovalSpec {
                required: false,
                reason: None,
            },
            failure_modes,
            forbidden_uses: Vec::new(),
            argument_safety: Vec::new(),
            structured_argument_safety: Vec::new(),
            semantic_intents: Vec::new(),
            intent_extractors: Vec::new(),
            repair_rules: Vec::new(),
            cost_estimate: None,
            latency_estimate: None,
        }
    }

    fn mode(name: &str, detection: &str) -> FailureModeSpec {
        FailureModeSpec {
            name: name.to_string(),
            detection: detection.to_string(),
            detection_rules: legacy_failure_detection_specs(detection),
            repair_strategy: "repair".to_string(),
            confidence: 0.8,
        }
    }

    #[test]
    fn ignores_text_failure_detection_for_command_not_found() {
        let output = json!({
            "success": false,
            "stdout": "{\"stderr\":\"sh: nope: command not found\\n\",\"exit_code\":127}",
        });
        let action = action(vec![mode(
            "command_not_found",
            "text_contains:command not found",
        )]);

        let kind = classify_failure(Some(&action), &output);

        assert_eq!(kind, FailureKind::Unknown);
    }

    #[test]
    fn ignores_text_failure_detection_for_missing_path() {
        let output = json!({
            "success": false,
            "stdout": "{\"stderr\":\"cat: crates/nope.rs: No such file or directory\\n\",\"exit_code\":1}",
        });
        let action = action(vec![mode(
            "missing_path",
            "text_contains:no such file or directory",
        )]);

        let kind = classify_failure(Some(&action), &output);

        assert_eq!(kind, FailureKind::Unknown);
    }

    #[test]
    fn classifies_nonzero_exit() {
        let output = json!({
            "success": false,
            "structured_output": {
                "stderr": "ordinary failure\n",
                "exit_code": 7,
            },
        });
        let action = action(vec![mode("non_zero_exit", "json_i64_nonzero:exit_code")]);

        let kind = classify_failure(Some(&action), &output);

        assert_eq!(kind, FailureKind::NonZeroExit);
    }

    #[test]
    fn structural_timeout_takes_priority_over_generic_nonzero_exit() {
        let output = json!({
            "success": false,
            "structured_output": {
                "interrupted": true,
            },
        });
        let action = action(vec![mode("non_zero_exit", "json_bool:success=false")]);

        let kind = classify_failure(Some(&action), &output);

        assert_eq!(kind, FailureKind::TimedOut);
    }

    #[test]
    fn supports_contract_specific_failure_names_from_structured_detection() {
        let output = json!({
            "success": false,
            "structured_output": {
                "quota_exhausted": true,
            },
        });
        let action = action(vec![mode(
            "provider_quota_exhausted",
            "json_bool:quota_exhausted=true",
        )]);

        let kind = classify_failure(Some(&action), &output);

        assert_eq!(
            kind,
            FailureKind::Contract("provider_quota_exhausted".to_string())
        );
    }
}
