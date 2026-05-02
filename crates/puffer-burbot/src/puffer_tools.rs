use crate::contract::{
    legacy_argument_safety_specs, legacy_failure_detection_specs, legacy_observation_check_specs,
    legacy_verification_method_templates, lint_contract, normalize_legacy_semantic_slot_kinds,
    ActionContract, ApprovalSpec, ArgumentSafetySpec, CapabilityContract, ContractLintError,
    ContractStatus, FailureDetectionSpec, FailureModeSpec, Idempotency, IntentExtractorSpec,
    ObservationCheckSpec, RepairRuleSpec, Reversibility, RiskLevel, SemanticIntentSpec,
    SideEffectClass, TrustLevel, VerificationSpec, VerificationTemplateSpec,
};
use anyhow::{anyhow, Context, Result};
use puffer_resources::ToolSpec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const PUFFER_TOOLS_CONTRACT_ID: &str = "puffer.tools";

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PufferToolSummary {
    pub(crate) id: String,
    pub(crate) handler: String,
    pub(crate) path: String,
    pub(crate) has_contract: bool,
    pub(crate) exclusion_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PufferToolContractReport {
    pub(crate) contract: CapabilityContract,
    pub(crate) lint_errors: Vec<ContractLintError>,
    pub(crate) missing_contracts: Vec<PufferToolSummary>,
    pub(crate) excluded_tools: Vec<PufferToolSummary>,
    pub(crate) contracted_tools: Vec<PufferToolSummary>,
    pub(crate) loaded_tools: usize,
}

#[derive(Debug, Deserialize)]
struct RawToolContract {
    side_effect_class: String,
    reversibility: String,
    idempotency: String,
    risk_level: String,
    #[serde(default)]
    preconditions: Vec<String>,
    #[serde(default)]
    postconditions: Vec<String>,
    #[serde(default)]
    verification: RawVerificationSpec,
    #[serde(default)]
    semantic_intents: Vec<SemanticIntentSpec>,
    #[serde(default)]
    intent_extractors: Vec<IntentExtractorSpec>,
    #[serde(default)]
    repair_rules: Vec<RepairRuleSpec>,
    #[serde(default)]
    approval: Option<ApprovalSpec>,
    #[serde(default)]
    failure_modes: Vec<RawFailureMode>,
    #[serde(default)]
    forbidden_uses: Vec<String>,
    #[serde(default)]
    argument_safety: Vec<ArgumentSafetySpec>,
    #[serde(default)]
    output_schema: Option<Value>,
    #[serde(default)]
    cost_estimate: Option<String>,
    #[serde(default)]
    latency_estimate: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawVerificationSpec {
    #[serde(default)]
    methods: Vec<String>,
    #[serde(default)]
    observation_checks: Vec<ObservationCheckSpec>,
    #[serde(default)]
    method_templates: Vec<VerificationTemplateSpec>,
    #[serde(default)]
    templates: Vec<VerificationTemplateSpec>,
    #[serde(default)]
    required_before_completion: bool,
    #[serde(default)]
    confidence: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RawFailureMode {
    name: String,
    detection: String,
    #[serde(default)]
    detection_rules: Vec<FailureDetectionSpec>,
    repair_strategy: String,
    #[serde(default)]
    confidence: Option<f64>,
}

pub(crate) fn lint_puffer_tool_contracts(path: &Path) -> Result<PufferToolContractReport> {
    let mut actions = Vec::new();
    let mut missing_contracts = Vec::new();
    let mut excluded_tools = Vec::new();
    let mut contracted_tools = Vec::new();
    let mut lint_errors = Vec::new();
    let mut loaded_tools = 0;

    for tool_path in tool_files(path)? {
        let raw = fs::read_to_string(&tool_path)
            .with_context(|| format!("failed to read {}", tool_path.display()))?;
        let spec: ToolSpec = serde_yaml::from_str(&raw)
            .with_context(|| format!("failed to parse {}", tool_path.display()))?;
        loaded_tools += 1;

        let exclusion_reason = exclusion_reason(&spec).map(str::to_string);
        let summary = PufferToolSummary {
            id: spec.id.clone(),
            handler: spec.handler.clone(),
            path: tool_path.display().to_string(),
            has_contract: spec.contract.is_some(),
            exclusion_reason: exclusion_reason.clone(),
        };

        if exclusion_reason.is_some() {
            excluded_tools.push(summary);
            continue;
        }

        let Some(contract_value) = &spec.contract else {
            missing_contracts.push(summary);
            continue;
        };

        match action_from_tool_contract(&spec, contract_value.clone()) {
            Ok(action) => {
                actions.push(action);
                contracted_tools.push(summary);
            }
            Err(error) => lint_errors.push(ContractLintError {
                path: format!("{}.{}", PUFFER_TOOLS_CONTRACT_ID, spec.id),
                message: error.to_string(),
            }),
        }
    }

    actions.sort_by(|left, right| left.name.cmp(&right.name));
    contracted_tools.sort_by(|left, right| left.id.cmp(&right.id));
    missing_contracts.sort_by(|left, right| left.id.cmp(&right.id));
    excluded_tools.sort_by(|left, right| left.id.cmp(&right.id));

    let contract = CapabilityContract {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        version: "0.1.0".to_string(),
        status: ContractStatus::Active,
        trust_level: TrustLevel::Sandboxed,
        description: "Puffer runtime tool contracts imported from resource metadata.".to_string(),
        actions,
        global_constraints: vec![
            "Puffer workflow, AppState-heavy, browser-session, and subagent tools are excluded"
                .to_string(),
            "Imported contracts describe execution semantics; Puffer tool implementations remain the authority for actual behavior"
                .to_string(),
        ],
        forbidden_uses: vec![
            "spawning subagents through Burbot".to_string(),
            "calling workflow/team/task/session tools through Burbot".to_string(),
        ],
        local_rules: Vec::new(),
        examples: Vec::new(),
        contract_hash: None,
    };
    if !contract.actions.is_empty() {
        lint_errors.extend(lint_contract(&contract));
    }

    Ok(PufferToolContractReport {
        contract,
        lint_errors,
        missing_contracts,
        excluded_tools,
        contracted_tools,
        loaded_tools,
    })
}

pub(crate) fn report_as_json(report: &PufferToolContractReport) -> Value {
    json!({
        "contract_id": report.contract.contract_id,
        "loaded_tools": report.loaded_tools,
        "eligible_with_contract": report.contracted_tools,
        "eligible_missing_contract": report.missing_contracts,
        "excluded_tools": report.excluded_tools,
        "lint_errors": report.lint_errors.iter().map(|error| {
            json!({
                "path": error.path,
                "message": error.message,
            })
        }).collect::<Vec<_>>(),
    })
}

fn action_from_tool_contract(spec: &ToolSpec, value: Value) -> Result<ActionContract> {
    let raw: RawToolContract = serde_json::from_value(value)
        .with_context(|| format!("invalid contract metadata for tool {}", spec.id))?;
    let input_schema = spec
        .input_schema
        .clone()
        .unwrap_or_else(default_open_object_schema);
    let output_schema = raw.output_schema.unwrap_or_else(default_open_object_schema);
    let argument_safety = raw.argument_safety;
    let structured_argument_safety = legacy_argument_safety_specs(&argument_safety);
    let mut semantic_intents = raw.semantic_intents;
    for intent in &mut semantic_intents {
        normalize_legacy_semantic_slot_kinds(intent);
    }

    jsonschema::validator_for(&input_schema)
        .with_context(|| format!("invalid input_schema for tool {}", spec.id))?;
    jsonschema::validator_for(&output_schema)
        .with_context(|| format!("invalid contract output_schema for tool {}", spec.id))?;

    Ok(ActionContract {
        name: spec.id.clone(),
        description: spec.description.clone(),
        input_schema,
        output_schema,
        side_effect_class: parse_side_effect_class(&raw.side_effect_class)?,
        reversibility: parse_reversibility(&raw.reversibility)?,
        idempotency: parse_idempotency(&raw.idempotency)?,
        risk_level: parse_risk_level(&raw.risk_level)?,
        preconditions: raw.preconditions,
        postconditions: raw.postconditions,
        verification: VerificationSpec {
            observation_checks: if raw.verification.observation_checks.is_empty() {
                legacy_observation_check_specs(&raw.verification.methods)
            } else {
                raw.verification.observation_checks
            },
            method_templates: if raw.verification.method_templates.is_empty() {
                legacy_verification_method_templates(&raw.verification.methods)
            } else {
                raw.verification.method_templates
            },
            methods: raw.verification.methods,
            templates: raw.verification.templates,
            required_before_completion: raw.verification.required_before_completion,
            confidence: raw.verification.confidence.unwrap_or(0.5),
        },
        approval: raw
            .approval
            .unwrap_or_else(|| approval_from_tool_policy(spec)),
        failure_modes: raw
            .failure_modes
            .into_iter()
            .map(raw_failure_mode)
            .collect(),
        forbidden_uses: raw.forbidden_uses,
        argument_safety,
        structured_argument_safety,
        semantic_intents,
        intent_extractors: Vec::new(),
        repair_rules: raw
            .repair_rules
            .into_iter()
            .filter(|rule| rule.bindings.is_empty())
            .collect(),
        cost_estimate: raw.cost_estimate,
        latency_estimate: raw.latency_estimate,
    })
}

fn raw_failure_mode(raw: RawFailureMode) -> FailureModeSpec {
    FailureModeSpec {
        name: raw.name,
        detection_rules: if raw.detection_rules.is_empty() {
            legacy_failure_detection_specs(&raw.detection)
        } else {
            raw.detection_rules
        },
        detection: raw.detection,
        repair_strategy: raw.repair_strategy,
        confidence: raw.confidence.unwrap_or(0.5),
    }
}

fn parse_side_effect_class(value: &str) -> Result<SideEffectClass> {
    match normalize_token(value).as_str() {
        "none" | "pure_observation" | "pure_observe" | "observation" => {
            Ok(SideEffectClass::PureObservation)
        }
        "local_read" | "filesystem_read" | "file_read" => Ok(SideEffectClass::LocalRead),
        "external_read" | "network_read" | "web_read" => Ok(SideEffectClass::ExternalRead),
        "local_write" | "filesystem_write" | "file_write" => Ok(SideEffectClass::LocalWrite),
        "external_write" | "network_write" | "web_write" => Ok(SideEffectClass::ExternalWrite),
        "communication" => Ok(SideEffectClass::Communication),
        "destructive_write" | "destructive" => Ok(SideEffectClass::DestructiveWrite),
        "financial_or_legal_effect" | "financial_legal_effect" => {
            Ok(SideEffectClass::FinancialOrLegalEffect)
        }
        "unknown" | "command_execution" | "shell" | "process" => Ok(SideEffectClass::Unknown),
        _ => Err(anyhow!("unknown side_effect_class: {value}")),
    }
}

fn parse_reversibility(value: &str) -> Result<Reversibility> {
    match normalize_token(value).as_str() {
        "reversible" | "not_applicable" | "none" => Ok(Reversibility::Reversible),
        "conditionally_reversible" | "conditional" => Ok(Reversibility::ConditionallyReversible),
        "compensatable" => Ok(Reversibility::Compensatable),
        "irreversible" => Ok(Reversibility::Irreversible),
        "unknown" | "command_dependent" => Ok(Reversibility::Unknown),
        _ => Err(anyhow!("unknown reversibility: {value}")),
    }
}

fn parse_idempotency(value: &str) -> Result<Idempotency> {
    match normalize_token(value).as_str() {
        "idempotent" | "idempotent_with_same_content" => Ok(Idempotency::Idempotent),
        "non_idempotent" => Ok(Idempotency::NonIdempotent),
        "unknown" | "conditionally_idempotent" | "command_dependent" => Ok(Idempotency::Unknown),
        _ => Err(anyhow!("unknown idempotency: {value}")),
    }
}

fn parse_risk_level(value: &str) -> Result<RiskLevel> {
    match normalize_token(value).as_str() {
        "low" => Ok(RiskLevel::Low),
        "medium" => Ok(RiskLevel::Medium),
        "high" => Ok(RiskLevel::High),
        "critical" => Ok(RiskLevel::Critical),
        _ => Err(anyhow!("unknown risk_level: {value}")),
    }
}

fn approval_from_tool_policy(spec: &ToolSpec) -> ApprovalSpec {
    let policy = spec.approval_policy.as_deref().map(normalize_token);
    ApprovalSpec {
        required: false,
        reason: policy.map(|policy| format!("Puffer tool approval policy is {policy}")),
    }
}

fn exclusion_reason(spec: &ToolSpec) -> Option<&'static str> {
    if spec.enabled_if.as_deref() == Some("never") {
        return Some("disabled");
    }
    if spec.id == "Agent" || spec.handler == "runtime:agent" {
        return Some("subagent");
    }
    if spec.handler.starts_with("runtime:workflow:") {
        return Some("workflow_or_appstate");
    }
    if spec.handler == "runtime:browser" || spec.id == "Browser" {
        return Some("browser_session");
    }
    None
}

fn tool_files(root: &Path) -> Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    if !root.exists() {
        return Err(anyhow!("tool path does not exist: {}", root.display()));
    }
    let mut files = Vec::new();
    collect_tool_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_tool_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_tool_files(&path, files)?;
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("yaml") {
            files.push(path);
        }
    }
    Ok(())
}

fn normalize_token(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('-', "_")
}

fn default_open_object_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn imports_contract_with_aliases_and_structured_failure_modes() {
        let spec: ToolSpec = serde_yaml::from_str(
            r#"
id: Read
name: Read
description: Read a file.
handler: runtime:claude_read
input_schema:
  type: object
contract:
  side_effect_class: none
  reversibility: not_applicable
  idempotency: idempotent
  risk_level: low
  verification:
    methods: []
    required_before_completion: false
  failure_modes:
    - name: missing_path
      detection: json_bool:success=false
      repair_strategy: inspect available paths before retrying with a corrected file_path.
"#,
        )
        .unwrap();
        let action = action_from_tool_contract(&spec, spec.contract.clone().unwrap()).unwrap();
        assert_eq!(action.side_effect_class, SideEffectClass::PureObservation);
        assert_eq!(action.reversibility, Reversibility::Reversible);
        assert_eq!(action.failure_modes[0].name, "missing_path");
    }

    #[test]
    fn import_drops_command_text_extractors_and_regex_bound_repairs() {
        let spec: ToolSpec = serde_yaml::from_str(
            r#"
id: Bash
name: Bash
description: Run shell.
handler: runtime:claude_bash
input_schema:
  type: object
contract:
  side_effect_class: command-execution
  reversibility: command_dependent
  idempotency: command_dependent
  risk_level: high
  verification:
    methods: []
    required_before_completion: false
  intent_extractors:
    - intent: read_file
      parser: regex
      source_arg: command
      pattern: '^cat\s+(?P<path>.+)$'
      slot_groups:
        path: path
  repair_rules:
    - failure_kinds: [command_not_found]
      action_name: Read
      args: {}
      bindings:
        - source_arg: command
          pattern: 'cat (?P<path>.+)'
"#,
        )
        .unwrap();

        let action = action_from_tool_contract(&spec, spec.contract.clone().unwrap()).unwrap();

        assert!(action.intent_extractors.is_empty());
        assert!(action.repair_rules.is_empty());
    }

    #[test]
    fn excludes_subagent_workflow_browser_and_disabled_tools() {
        let agent: ToolSpec = serde_yaml::from_str(
            r#"
id: Agent
name: Agent
description: Spawn an agent.
handler: runtime:agent
"#,
        )
        .unwrap();
        let workflow: ToolSpec = serde_yaml::from_str(
            r#"
id: TeamCreate
name: TeamCreate
description: Create a team.
handler: runtime:workflow:team_create
"#,
        )
        .unwrap();
        let browser: ToolSpec = serde_yaml::from_str(
            r#"
id: Browser
name: Browser
description: Browser session.
handler: runtime:browser
"#,
        )
        .unwrap();
        let disabled: ToolSpec = serde_yaml::from_str(
            r#"
id: read_file
name: read_file
description: Legacy read.
handler: read_file
enabled_if: never
"#,
        )
        .unwrap();

        assert_eq!(exclusion_reason(&agent), Some("subagent"));
        assert_eq!(exclusion_reason(&workflow), Some("workflow_or_appstate"));
        assert_eq!(exclusion_reason(&browser), Some("browser_session"));
        assert_eq!(exclusion_reason(&disabled), Some("disabled"));
    }

    #[test]
    fn reports_missing_contract_for_active_non_excluded_tool() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("tool.yaml"),
            r#"
id: ToolSearch
name: ToolSearch
description: Find tools.
handler: runtime:tool_search
"#,
        )
        .unwrap();

        let report = lint_puffer_tool_contracts(dir.path()).unwrap();
        assert_eq!(report.loaded_tools, 1);
        assert_eq!(report.missing_contracts[0].id, "ToolSearch");
        assert!(report.lint_errors.is_empty());
    }
}
