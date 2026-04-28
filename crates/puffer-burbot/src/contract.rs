use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContractStatus {
    Draft,
    Candidate,
    Validated,
    Shadow,
    Canary,
    Active,
    Deprecated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TrustLevel {
    Trusted,
    Sandboxed,
    External,
    Dangerous,
    Unverified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SideEffectClass {
    PureObservation,
    LocalRead,
    ExternalRead,
    LocalWrite,
    ExternalWrite,
    Communication,
    DestructiveWrite,
    FinancialOrLegalEffect,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reversibility {
    Reversible,
    ConditionallyReversible,
    Compensatable,
    Irreversible,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Idempotency {
    Idempotent,
    NonIdempotent,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct VerificationSpec {
    pub(crate) methods: Vec<String>,
    #[serde(default)]
    pub(crate) templates: Vec<VerificationTemplateSpec>,
    pub(crate) required_before_completion: bool,
    #[serde(default = "default_half")]
    pub(crate) confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct VerificationTemplateSpec {
    pub(crate) contract_id: String,
    pub(crate) action_name: String,
    #[serde(default)]
    pub(crate) args: Value,
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ArgumentSafetySpec {
    pub(crate) source_arg: String,
    #[serde(default)]
    pub(crate) blocked_patterns: Vec<ArgumentPatternSpec>,
    #[serde(default)]
    pub(crate) approval_patterns: Vec<ArgumentPatternSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ArgumentPatternSpec {
    pub(crate) name: String,
    pub(crate) pattern: String,
    pub(crate) reason: Option<String>,
    #[serde(default = "default_half")]
    pub(crate) confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ApprovalSpec {
    pub(crate) required: bool,
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct FailureModeSpec {
    pub(crate) name: String,
    pub(crate) detection: String,
    pub(crate) repair_strategy: String,
    #[serde(default = "default_half")]
    pub(crate) confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct SemanticIntentSpec {
    pub(crate) intent: String,
    #[serde(default)]
    pub(crate) slots: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) optional_slots: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) defaults: BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) side_effect_class: Option<SideEffectClass>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct IntentExtractorSpec {
    pub(crate) intent: String,
    pub(crate) parser: String,
    pub(crate) source_arg: String,
    #[serde(default)]
    pub(crate) pattern: Option<String>,
    #[serde(default)]
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) slots: Vec<ExtractorSlotSpec>,
    #[serde(default)]
    pub(crate) optional_slots: Vec<ExtractorSlotSpec>,
    #[serde(default)]
    pub(crate) slot_groups: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) optional_slot_groups: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) literals: Vec<ExtractorLiteralSpec>,
    #[serde(default)]
    pub(crate) side_effect_class: Option<SideEffectClass>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ExtractorSlotSpec {
    pub(crate) name: String,
    pub(crate) position: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ExtractorLiteralSpec {
    pub(crate) position: usize,
    pub(crate) value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RepairRuleSpec {
    #[serde(default)]
    pub(crate) failure_kinds: Vec<String>,
    pub(crate) contract_id: Option<String>,
    pub(crate) action_name: String,
    #[serde(default)]
    pub(crate) args: Value,
    #[serde(default)]
    pub(crate) arg_templates: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) bindings: Vec<RepairBindingSpec>,
    pub(crate) source: Option<String>,
    pub(crate) completion_role: Option<String>,
    pub(crate) rationale: Option<String>,
    pub(crate) expected_progress: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RepairBindingSpec {
    pub(crate) source_arg: String,
    pub(crate) pattern: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ActionContract {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_schema: Value,
    pub(crate) output_schema: Value,
    pub(crate) side_effect_class: SideEffectClass,
    pub(crate) reversibility: Reversibility,
    pub(crate) idempotency: Idempotency,
    pub(crate) risk_level: RiskLevel,
    #[serde(default)]
    pub(crate) preconditions: Vec<String>,
    #[serde(default)]
    pub(crate) postconditions: Vec<String>,
    pub(crate) verification: VerificationSpec,
    pub(crate) approval: ApprovalSpec,
    #[serde(default)]
    pub(crate) failure_modes: Vec<FailureModeSpec>,
    #[serde(default)]
    pub(crate) forbidden_uses: Vec<String>,
    #[serde(default)]
    pub(crate) argument_safety: Vec<ArgumentSafetySpec>,
    #[serde(default)]
    pub(crate) semantic_intents: Vec<SemanticIntentSpec>,
    #[serde(default)]
    pub(crate) intent_extractors: Vec<IntentExtractorSpec>,
    #[serde(default)]
    pub(crate) repair_rules: Vec<RepairRuleSpec>,
    pub(crate) cost_estimate: Option<String>,
    pub(crate) latency_estimate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct CapabilityContract {
    pub(crate) contract_id: String,
    pub(crate) version: String,
    pub(crate) status: ContractStatus,
    pub(crate) trust_level: TrustLevel,
    pub(crate) description: String,
    pub(crate) actions: Vec<ActionContract>,
    #[serde(default)]
    pub(crate) global_constraints: Vec<String>,
    #[serde(default)]
    pub(crate) forbidden_uses: Vec<String>,
    #[serde(default)]
    pub(crate) local_rules: Vec<Value>,
    #[serde(default)]
    pub(crate) examples: Vec<Value>,
    pub(crate) contract_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContractLintError {
    pub(crate) path: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct InMemoryContractRegistry {
    contracts: HashMap<String, CapabilityContract>,
}

pub(crate) trait ContractRegistry {
    fn active_contracts(&self) -> Vec<CapabilityContract>;
    fn get_action(&self, contract_id: &str, action_name: &str) -> Option<ActionContract>;
}

impl InMemoryContractRegistry {
    pub(crate) fn register(&mut self, contract: CapabilityContract) -> Result<()> {
        if self.contracts.contains_key(&contract.contract_id) {
            return Err(anyhow!("duplicate contract id: {}", contract.contract_id));
        }
        self.contracts
            .insert(contract.contract_id.clone(), contract);
        Ok(())
    }

    pub(crate) fn load_dir(path: &Path) -> Result<Self> {
        let mut registry = Self::default();
        for contract_path in contract_files(path)? {
            let raw = fs::read_to_string(&contract_path)
                .with_context(|| format!("failed to read {}", contract_path.display()))?;
            let contract = parse_contract_md(&raw)
                .with_context(|| format!("failed to parse {}", contract_path.display()))?;
            registry.register(contract)?;
        }
        Ok(registry)
    }

    pub(crate) fn lint_dir(path: &Path) -> Result<Vec<ContractLintError>> {
        let mut errors = Vec::new();
        for contract_path in contract_files(path)? {
            let raw = fs::read_to_string(&contract_path)
                .with_context(|| format!("failed to read {}", contract_path.display()))?;
            match parse_contract_md(&raw) {
                Ok(contract) => {
                    errors.extend(lint_contract(&contract));
                }
                Err(error) => errors.push(ContractLintError {
                    path: contract_path.display().to_string(),
                    message: error.to_string(),
                }),
            }
        }
        Ok(errors)
    }
}

impl ContractRegistry for InMemoryContractRegistry {
    fn active_contracts(&self) -> Vec<CapabilityContract> {
        self.contracts.values().cloned().collect()
    }

    fn get_action(&self, contract_id: &str, action_name: &str) -> Option<ActionContract> {
        self.contracts
            .get(contract_id)?
            .actions
            .iter()
            .find(|action| action.name == action_name)
            .cloned()
    }
}

pub(crate) fn parse_contract_md(input: &str) -> Result<CapabilityContract> {
    let yaml = extract_machine_contract_block(input)?;
    let mut contract: CapabilityContract =
        serde_yaml::from_str(&yaml).context("failed to deserialize machine contract YAML")?;
    contract.contract_hash = Some(contract_hash(&yaml));
    validate_action_schemas(&contract)?;
    Ok(contract)
}

pub(crate) fn lint_contract(contract: &CapabilityContract) -> Vec<ContractLintError> {
    let mut errors = Vec::new();
    let mut names = std::collections::HashSet::new();

    if contract.actions.is_empty() {
        errors.push(ContractLintError {
            path: contract.contract_id.clone(),
            message: "contract must declare at least one action".to_string(),
        });
    }

    for action in &contract.actions {
        let path = format!("{}.{}", contract.contract_id, action.name);
        if !names.insert(action.name.clone()) {
            errors.push(ContractLintError {
                path: path.clone(),
                message: "duplicate action name".to_string(),
            });
        }
        if action.side_effect_class == SideEffectClass::Unknown
            && action.risk_level == RiskLevel::Low
        {
            errors.push(ContractLintError {
                path: path.clone(),
                message: "unknown side effects cannot be low risk".to_string(),
            });
        }
        if matches!(
            action.side_effect_class,
            SideEffectClass::ExternalWrite
                | SideEffectClass::Communication
                | SideEffectClass::FinancialOrLegalEffect
        ) && !action.approval.required
        {
            errors.push(ContractLintError {
                path: path.clone(),
                message:
                    "external write, communication, and financial/legal actions require approval"
                        .to_string(),
            });
        }
        if action.side_effect_class == SideEffectClass::DestructiveWrite
            && !matches!(action.risk_level, RiskLevel::High | RiskLevel::Critical)
        {
            errors.push(ContractLintError {
                path: path.clone(),
                message: "destructive writes must be high or critical risk".to_string(),
            });
        }
        if action.verification.required_before_completion
            && action.verification.methods.is_empty()
            && action.verification.templates.is_empty()
        {
            errors.push(ContractLintError {
                path,
                message: "verification required but no verification methods or templates declared"
                    .to_string(),
            });
        }
    }
    errors
}

fn default_half() -> f64 {
    0.5
}

fn extract_machine_contract_block(input: &str) -> Result<String> {
    let search_area = input
        .split_once("## Machine Contract")
        .map(|(_, rest)| rest)
        .unwrap_or(input);
    let marker = "```yaml";
    let start = search_area
        .find(marker)
        .ok_or_else(|| anyhow!("contract.md missing ```yaml machine block"))?;
    let rest = &search_area[start + marker.len()..];
    let end = rest
        .find("```")
        .ok_or_else(|| anyhow!("contract.md YAML block is not closed"))?;
    Ok(rest[..end].trim().to_string())
}

fn validate_action_schemas(contract: &CapabilityContract) -> Result<()> {
    for action in &contract.actions {
        jsonschema::validator_for(&action.input_schema).with_context(|| {
            format!(
                "invalid input_schema for {}.{}",
                contract.contract_id, action.name
            )
        })?;
        jsonschema::validator_for(&action.output_schema).with_context(|| {
            format!(
                "invalid output_schema for {}.{}",
                contract.contract_id, action.name
            )
        })?;
    }
    Ok(())
}

fn contract_hash(yaml: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(yaml.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn contract_files(root: &Path) -> Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    if !root.exists() {
        return Err(anyhow!("contract path does not exist: {}", root.display()));
    }
    let mut files = Vec::new();
    collect_contract_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_contract_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_contract_files(&path, files)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some("contract.md") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_contract() -> &'static str {
        r#"# Contract: Test

## Machine Contract

```yaml
contract_id: test
version: 0.1.0
status: active
trust_level: sandboxed
description: Test capability.
actions:
  - name: Observe
    description: Observe state.
    input_schema:
      type: object
    output_schema:
      type: object
    side_effect_class: pure_observation
    reversibility: reversible
    idempotency: idempotent
    risk_level: low
    verification:
      methods: []
      required_before_completion: false
    approval:
      required: false
```
"#
    }

    #[test]
    fn parser_reads_machine_contract_block() {
        let contract = parse_contract_md(valid_contract()).unwrap();
        assert_eq!(contract.contract_id, "test");
        assert_eq!(contract.actions[0].name, "Observe");
        assert!(contract.contract_hash.is_some());
    }

    #[test]
    fn linter_rejects_unknown_low_risk() {
        let mut contract = parse_contract_md(valid_contract()).unwrap();
        contract.actions[0].side_effect_class = SideEffectClass::Unknown;
        let errors = lint_contract(&contract);
        assert!(errors
            .iter()
            .any(|error| error.message.contains("unknown side effects")));
    }

    #[test]
    fn registry_rejects_duplicate_contract_ids() {
        let contract = parse_contract_md(valid_contract()).unwrap();
        let mut registry = InMemoryContractRegistry::default();
        registry.register(contract.clone()).unwrap();
        assert!(registry.register(contract).is_err());
    }
}
