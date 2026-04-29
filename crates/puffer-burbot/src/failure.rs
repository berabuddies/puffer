use crate::contract::{ActionContract, FailureModeSpec};
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
    if let Some(kind) = action.and_then(|action| classify_contract_failure(action, output)) {
        return kind;
    }
    FailureKind::Unknown
}

fn classify_contract_failure(action: &ActionContract, output: &Value) -> Option<FailureKind> {
    action
        .failure_modes
        .iter()
        .find(|mode| failure_mode_matches(mode, output))
        .map(|mode| FailureKind::from_str(&mode.name))
}

fn failure_mode_matches(mode: &FailureModeSpec, output: &Value) -> bool {
    mode.detection
        .split("||")
        .any(|clause| detection_clause_matches(clause.trim(), output))
}

fn detection_clause_matches(clause: &str, output: &Value) -> bool {
    if clause.is_empty() {
        return false;
    }
    if let Some(expression) = clause.strip_prefix("json_bool:") {
        let Some((key, expected)) = expression.split_once('=') else {
            return false;
        };
        let expected = match expected.trim() {
            "true" => true,
            "false" => false,
            _ => return false,
        };
        return output_bool_deep(output, key.trim()) == Some(expected);
    }
    if let Some(key) = clause.strip_prefix("json_i64_nonzero:") {
        return output_i64_deep(output, key.trim()).is_some_and(|value| value != 0);
    }
    if let Some(key) = clause.strip_prefix("json_present:") {
        return value_deep(output, key.trim()).is_some_and(|value| !value.is_null());
    }
    false
}

fn output_i64_deep(output: &Value, key: &str) -> Option<i64> {
    value_deep(output, key).and_then(|value| value.as_i64())
}

fn output_bool_deep(output: &Value, key: &str) -> Option<bool> {
    value_deep(output, key).and_then(|value| value.as_bool())
}

fn value_deep(output: &Value, key: &str) -> Option<Value> {
    value_at_path(output, key).cloned().or_else(|| {
        output
            .get("structured_output")
            .and_then(|nested| value_at_path(nested, key).cloned())
    })
}

fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = current.get(segment)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ApprovalSpec, Idempotency, Reversibility, RiskLevel, SideEffectClass, VerificationSpec,
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
