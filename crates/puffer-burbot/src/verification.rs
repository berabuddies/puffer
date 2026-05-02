use crate::contract::{
    ActionContract, ObservationCheckSpec, SideEffectClass, VerificationTemplateSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Describes the amount of verification Burbot should plan after an action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct VerificationRequirement {
    pub(crate) required_before_completion: bool,
    pub(crate) confidence: f64,
    pub(crate) methods: Vec<String>,
    pub(crate) reasons: Vec<String>,
}

/// Describes a verification action Burbot may schedule later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct VerificationActionTemplate {
    pub(crate) contract_id: String,
    pub(crate) action_name: String,
    pub(crate) args: Value,
    pub(crate) reason: String,
}

/// Builds a verification requirement from an action contract and execution output.
pub(crate) fn verification_requirement_for(
    action: &ActionContract,
    executed_output: &Value,
) -> VerificationRequirement {
    let mut reasons = Vec::new();
    if action.verification.required_before_completion {
        reasons.push("contract requires verification before completion".to_string());
    }
    if !action.postconditions.is_empty() {
        reasons.push("action declares postconditions".to_string());
    }
    if output_indicates_failure(executed_output) {
        reasons.push("execution output indicates failure".to_string());
    }
    if state_changes_need_verification(action) {
        reasons.push("action can change state".to_string());
    }

    VerificationRequirement {
        required_before_completion: action.verification.required_before_completion
            || output_indicates_failure(executed_output),
        confidence: action.verification.confidence,
        methods: action.verification.methods.clone(),
        reasons,
    }
}

/// Builds verification action templates from declared verification methods.
pub(crate) fn verification_templates_for(
    action: &ActionContract,
    executed_output: &Value,
) -> Vec<VerificationActionTemplate> {
    action
        .verification
        .method_templates
        .iter()
        .map(|template| {
            executable_template_from_structured(template, action, &json!({}), executed_output)
        })
        .collect()
}

/// Builds executable verification action templates from declared structured templates.
pub(crate) fn executable_verification_templates_for(
    action: &ActionContract,
    action_args: &Value,
    executed_output: &Value,
) -> Vec<VerificationActionTemplate> {
    let templates = action
        .verification
        .templates
        .iter()
        .map(|template| {
            executable_template_from_structured(template, action, action_args, executed_output)
        })
        .collect();
    dedupe_templates(templates)
}

/// Evaluates machine-readable observation checks declared in verification methods.
pub(crate) fn observation_checks_pass(action: &ActionContract, output: &Value) -> Option<bool> {
    let checks = &action.verification.observation_checks;
    if checks.is_empty() {
        return None;
    }
    Some(
        checks
            .iter()
            .all(|check| observation_check_matches(check, output)),
    )
}

/// Builds both simple requirements and action templates for a completed action.
pub(crate) fn plan_verification_for(
    action: &ActionContract,
    executed_output: &Value,
) -> PlannedVerification {
    PlannedVerification {
        requirement: verification_requirement_for(action, executed_output),
        templates: verification_templates_for(action, executed_output),
    }
}

/// Bundles verification requirements with optional future action templates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlannedVerification {
    pub(crate) requirement: VerificationRequirement,
    pub(crate) templates: Vec<VerificationActionTemplate>,
}

fn executable_template_from_structured(
    template: &VerificationTemplateSpec,
    action: &ActionContract,
    action_args: &Value,
    executed_output: &Value,
) -> VerificationActionTemplate {
    VerificationActionTemplate {
        contract_id: template.contract_id.clone(),
        action_name: template.action_name.clone(),
        args: render_structured_args(&template.args, action, action_args, executed_output),
        reason: template
            .reason
            .clone()
            .clone()
            .unwrap_or_else(|| format!("verify {}", action.name)),
    }
}

fn render_structured_args(
    value: &Value,
    action: &ActionContract,
    action_args: &Value,
    executed_output: &Value,
) -> Value {
    match value {
        Value::String(template) => Value::String(render_template_string(
            template,
            action,
            action_args,
            executed_output,
        )),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| render_structured_args(value, action, action_args, executed_output))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        render_structured_args(value, action, action_args, executed_output),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn render_template_string(
    template: &str,
    action: &ActionContract,
    action_args: &Value,
    executed_output: &Value,
) -> String {
    let mut rendered = template.replace("{source_action}", &action.name);
    if let Some(args) = action_args.as_object() {
        for (key, value) in args {
            let Some(value) = scalar_template_value(value) else {
                continue;
            };
            rendered = rendered.replace(
                &format!("{{shell_quote.arg.{key}}}"),
                &shell_single_quote(value),
            );
            rendered = rendered.replace(&format!("{{arg.{key}}}"), value);
        }
    }
    rendered = rendered.replace("{output}", &executed_output.to_string());
    rendered
}

fn scalar_template_value(value: &Value) -> Option<&str> {
    value.as_str()
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn dedupe_templates(templates: Vec<VerificationActionTemplate>) -> Vec<VerificationActionTemplate> {
    let mut seen = std::collections::HashSet::new();
    templates
        .into_iter()
        .filter(|template| {
            seen.insert(format!(
                "{}.{}:{}",
                template.contract_id,
                template.action_name,
                serde_json::to_string(&template.args).unwrap_or_default()
            ))
        })
        .collect()
}

fn output_indicates_failure(output: &Value) -> bool {
    output
        .get("success")
        .and_then(Value::as_bool)
        .is_some_and(|success| !success)
        || output.get("error").is_some_and(|error| !error.is_null())
}

fn observation_check_matches(check: &ObservationCheckSpec, output: &Value) -> bool {
    match check {
        ObservationCheckSpec::JsonBool { path, expected } => {
            value_at_path(output, path).and_then(Value::as_bool) == Some(*expected)
        }
        ObservationCheckSpec::Present { path } => {
            value_at_path(output, path).is_some_and(|value| !value.is_null())
        }
        ObservationCheckSpec::NonEmpty { path } => {
            value_at_path(output, path).is_some_and(non_empty_structural_value)
        }
    }
}

fn non_empty_structural_value(value: &Value) -> bool {
    match value {
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn value_at_path<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(segment.as_str())?;
    }
    Some(current)
}

fn state_changes_need_verification(action: &ActionContract) -> bool {
    !matches!(
        action.side_effect_class,
        SideEffectClass::PureObservation
            | SideEffectClass::LocalRead
            | SideEffectClass::ExternalRead
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        legacy_observation_check_specs, legacy_verification_method_templates, ApprovalSpec,
        Idempotency, ObservationCheckSpec, Reversibility, RiskLevel, VerificationSpec,
        VerificationTemplateSpec,
    };
    use serde_json::json;

    fn action(methods: Vec<String>, required_before_completion: bool) -> ActionContract {
        action_with_templates(methods, Vec::new(), required_before_completion)
    }

    fn action_with_templates(
        methods: Vec<String>,
        templates: Vec<VerificationTemplateSpec>,
        required_before_completion: bool,
    ) -> ActionContract {
        ActionContract {
            name: "WriteFile".to_string(),
            description: "write a file".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            side_effect_class: SideEffectClass::LocalWrite,
            reversibility: Reversibility::Reversible,
            idempotency: Idempotency::Idempotent,
            risk_level: RiskLevel::Medium,
            preconditions: Vec::new(),
            postconditions: vec!["file exists".to_string()],
            verification: VerificationSpec {
                observation_checks: legacy_observation_check_specs(&methods),
                method_templates: legacy_verification_method_templates(&methods),
                methods,
                templates,
                required_before_completion,
                confidence: 0.8,
            },
            approval: ApprovalSpec {
                required: false,
                reason: None,
            },
            failure_modes: Vec::new(),
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

    #[test]
    fn requirement_reflects_contract_output_and_state_change() {
        let action = action(Vec::new(), true);

        let requirement = verification_requirement_for(&action, &json!({"success": false}));

        assert!(requirement.required_before_completion);
        assert_eq!(requirement.confidence, 0.8);
        assert!(requirement
            .reasons
            .contains(&"contract requires verification before completion".to_string()));
        assert!(requirement
            .reasons
            .contains(&"execution output indicates failure".to_string()));
        assert!(requirement
            .reasons
            .contains(&"action can change state".to_string()));
    }

    #[test]
    fn templates_parse_action_methods_without_executing_tools() {
        let action = action(
            vec![
                "action:puffer.tools.Read".to_string(),
                "manual inspection".to_string(),
                "puffer.tools.Stat".to_string(),
            ],
            false,
        );

        let templates = verification_templates_for(&action, &json!({"path": "src/lib.rs"}));

        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0].contract_id, "puffer.tools");
        assert_eq!(templates[0].action_name, "Read");
        assert_eq!(templates[0].args["source_action"], "WriteFile");
        assert_eq!(templates[1].action_name, "Stat");
    }

    #[test]
    fn executable_templates_resolve_file_content_methods_to_read() {
        let action = action_with_templates(
            Vec::new(),
            vec![VerificationTemplateSpec {
                contract_id: "puffer.tools".to_string(),
                action_name: "Read".to_string(),
                args: json!({"file_path": "{arg.file_path}"}),
                reason: Some("read changed file".to_string()),
            }],
            true,
        );

        let templates = executable_verification_templates_for(
            &action,
            &json!({"file_path": "/tmp/example.txt"}),
            &json!({"success": true}),
        );

        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].contract_id, "puffer.tools");
        assert_eq!(templates[0].action_name, "Read");
        assert_eq!(templates[0].args, json!({"file_path": "/tmp/example.txt"}));
    }

    #[test]
    fn executable_templates_resolve_notebook_path_to_read() {
        let action = action_with_templates(
            Vec::new(),
            vec![VerificationTemplateSpec {
                contract_id: "puffer.tools".to_string(),
                action_name: "Read".to_string(),
                args: json!({"file_path": "{arg.notebook_path}"}),
                reason: Some("read changed notebook".to_string()),
            }],
            true,
        );

        let templates = executable_verification_templates_for(
            &action,
            &json!({"notebook_path": "/tmp/notebook.ipynb"}),
            &json!({"success": true}),
        );

        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].action_name, "Read");
        assert_eq!(
            templates[0].args,
            json!({"file_path": "/tmp/notebook.ipynb"})
        );
    }

    #[test]
    fn executable_templates_resolve_version_control_methods_to_bash() {
        let action = action_with_templates(
            Vec::new(),
            vec![VerificationTemplateSpec {
                contract_id: "puffer.tools".to_string(),
                action_name: "Bash".to_string(),
                args: json!({
                    "command": "git diff --stat -- {shell_quote.arg.file_path} && git status --short -- {shell_quote.arg.file_path}",
                    "timeout": 60000
                }),
                reason: Some("inspect version control state".to_string()),
            }],
            true,
        );

        let templates = executable_verification_templates_for(
            &action,
            &json!({"file_path": "/tmp/it's.txt"}),
            &json!({"success": true}),
        );

        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].action_name, "Bash");
        assert!(templates[0].args["command"]
            .as_str()
            .unwrap()
            .contains("git diff --stat"));
        assert!(templates[0].args["command"]
            .as_str()
            .unwrap()
            .contains("'\"'\"'"));
    }

    #[test]
    fn command_output_methods_do_not_schedule_extra_tools() {
        let action = action(
            vec!["inspect command output and exit status".to_string()],
            true,
        );

        let templates =
            executable_verification_templates_for(&action, &json!({}), &json!({"success": true}));

        assert!(templates.is_empty());
    }

    #[test]
    fn legacy_non_empty_observation_method_is_ignored() {
        let action = action(vec!["observation:non_empty:stdout".to_string()], true);

        assert!(action.verification.observation_checks.is_empty());
        assert_eq!(
            observation_checks_pass(&action, &json!({"stdout": "free text"})),
            None
        );
    }

    #[test]
    fn explicit_non_empty_observation_check_does_not_match_strings() {
        let mut action = action(Vec::new(), true);
        action.verification.observation_checks = vec![ObservationCheckSpec::NonEmpty {
            path: vec!["stdout".to_string()],
        }];

        assert_eq!(
            observation_checks_pass(&action, &json!({"stdout": "free text"})),
            Some(false)
        );
        assert_eq!(
            observation_checks_pass(&action, &json!({"items": ["structural"]})),
            Some(false)
        );
    }

    #[test]
    fn explicit_non_empty_observation_check_matches_structural_containers() {
        let mut action = action(Vec::new(), true);
        action.verification.observation_checks = vec![ObservationCheckSpec::NonEmpty {
            path: vec!["items".to_string()],
        }];

        assert_eq!(
            observation_checks_pass(&action, &json!({"items": ["structural"]})),
            Some(true)
        );
        assert_eq!(
            observation_checks_pass(&action, &json!({"items": []})),
            Some(false)
        );
    }
}
