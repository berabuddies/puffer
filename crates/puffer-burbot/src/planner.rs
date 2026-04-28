use crate::contract::{ActionContract, ContractRegistry, RepairRuleSpec};
use crate::failure::FailureKind;
use crate::graph::{scores_for_action, ActionRef, ActionScores};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use crate::runtime::RunOptions;
use crate::semantics::{action_intents, payload_for_intent, read_only_side_effect};
use crate::verification::executable_verification_templates_for;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CandidateSource {
    ExplicitUserTool,
    ModelProposal,
    ModelObservationProposal,
    ModelGoalVerifier,
    ContractGoal,
    CheapObservation,
    GuardedSubstitution,
    Repair,
    Verification,
    SymbolicProposal,
    SymbolicCritic,
    SymbolicVerifier,
    SynthesizedPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletionRole {
    Terminal,
    Support,
    Verification,
    Repair,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ActionCandidate {
    pub(crate) action_ref: ActionRef,
    pub(crate) args: Value,
    pub(crate) source: CandidateSource,
    pub(crate) completion_role: CompletionRole,
    pub(crate) rationale: String,
    pub(crate) scores: ActionScores,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanDependencyKind {
    DependsOn,
    PreconditionFor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PlanDependency {
    pub(crate) prerequisite_index: usize,
    pub(crate) dependent_index: usize,
    pub(crate) kind: PlanDependencyKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ActionPlan {
    pub(crate) source: CandidateSource,
    pub(crate) rationale: String,
    pub(crate) steps: Vec<ActionCandidate>,
    pub(crate) dependencies: Vec<PlanDependency>,
}

pub(crate) fn initial_candidates(
    contracts: &dyn ContractRegistry,
    _goal: &str,
    options: &RunOptions,
) -> Vec<ActionCandidate> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    if let Some(tool_id) = &options.puffer_tool {
        let action_ref = puffer_action(tool_id);
        let args = options.puffer_args.clone().unwrap_or_else(|| json!({}));
        push_candidate(
            contracts,
            &mut candidates,
            &mut seen,
            CandidateSeed {
                action_ref,
                args,
                source: options.puffer_tool_source.clone(),
                completion_role: CompletionRole::Terminal,
                rationale: "explicit Puffer tool requested by caller".to_string(),
                expected_progress_override: Some(2.5),
            },
        );
    }

    for candidate in guarded_substitutions_for_explicit_tool(contracts, _goal, options) {
        push_candidate(contracts, &mut candidates, &mut seen, candidate);
    }

    candidates
}

pub(crate) fn initial_candidate_plans(
    contracts: &dyn ContractRegistry,
    goal: &str,
    options: &RunOptions,
) -> Vec<ActionPlan> {
    let candidates = initial_candidates(contracts, goal, options);
    crate::plan_synthesis::synthesize_candidate_plans(contracts, goal, options, candidates)
}

pub(crate) fn repair_candidates_for_failure(
    contracts: &dyn ContractRegistry,
    failed_action: &ActionRef,
    failed_args: &Value,
    failure_output: &Value,
    failure_kind: FailureKind,
) -> Vec<ActionCandidate> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    let Some(action) = contracts.get_action(&failed_action.contract_id, &failed_action.action_name)
    else {
        return candidates;
    };
    for rule in &action.repair_rules {
        if let Some(seed) = repair_seed_from_rule(
            rule,
            failed_action,
            failed_args,
            failure_output,
            failure_kind.clone(),
        ) {
            push_candidate(contracts, &mut candidates, &mut seen, seed);
        }
    }

    candidates
}

pub(crate) fn verification_candidates_for_action(
    contracts: &dyn ContractRegistry,
    action_ref: &ActionRef,
    args: &Value,
    executed_output: &Value,
) -> Vec<ActionCandidate> {
    let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
    else {
        return Vec::new();
    };
    if !action.verification.required_before_completion {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for template in executable_verification_templates_for(&action, args, executed_output) {
        let expected_progress = contracts
            .get_action(&template.contract_id, &template.action_name)
            .map(|action| {
                if read_only_side_effect(&action.side_effect_class) {
                    1.0
                } else {
                    0.8
                }
            })
            .unwrap_or(0.8);
        push_candidate(
            contracts,
            &mut candidates,
            &mut seen,
            CandidateSeed {
                action_ref: ActionRef {
                    contract_id: template.contract_id,
                    action_name: template.action_name,
                },
                args: template.args,
                source: CandidateSource::Verification,
                completion_role: CompletionRole::Verification,
                rationale: template.reason,
                expected_progress_override: Some(expected_progress),
            },
        );
    }
    candidates
}

pub(crate) fn puffer_action(action_name: &str) -> ActionRef {
    ActionRef {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        action_name: action_name.to_string(),
    }
}

struct CandidateSeed {
    action_ref: ActionRef,
    args: Value,
    source: CandidateSource,
    completion_role: CompletionRole,
    rationale: String,
    expected_progress_override: Option<f64>,
}

fn push_candidate(
    contracts: &dyn ContractRegistry,
    candidates: &mut Vec<ActionCandidate>,
    seen: &mut HashSet<String>,
    seed: CandidateSeed,
) {
    let Some(action) =
        contracts.get_action(&seed.action_ref.contract_id, &seed.action_ref.action_name)
    else {
        return;
    };
    if !seen.insert(candidate_key(&seed.action_ref, &seed.args)) {
        return;
    }
    let mut scores = scores_for_candidate(&action, &seed);
    if matches!(seed.source, CandidateSource::ExplicitUserTool) {
        scores.information_gain = scores.information_gain.max(1.0);
        scores.verification_value = scores.verification_value.max(1.0);
    }
    candidates.push(ActionCandidate {
        action_ref: seed.action_ref,
        args: seed.args,
        source: seed.source,
        completion_role: seed.completion_role,
        rationale: seed.rationale,
        scores,
    });
}

fn guarded_substitutions_for_explicit_tool(
    contracts: &dyn ContractRegistry,
    _goal: &str,
    options: &RunOptions,
) -> Vec<CandidateSeed> {
    let Some(tool_id) = options.puffer_tool.as_deref() else {
        return Vec::new();
    };
    let Some(args) = &options.puffer_args else {
        return Vec::new();
    };
    let explicit_ref = puffer_action(tool_id);
    let Some(explicit_action) =
        contracts.get_action(&explicit_ref.contract_id, &explicit_ref.action_name)
    else {
        return Vec::new();
    };
    let explicit_intents = action_intents(&explicit_action, args);

    let mut candidates = Vec::new();
    for contract in contracts.active_contracts() {
        for action in contract.actions {
            if !read_only_side_effect(&action.side_effect_class) {
                continue;
            }
            let action_ref = ActionRef {
                contract_id: contract.contract_id.clone(),
                action_name: action.name.clone(),
            };
            if action_ref == explicit_ref {
                continue;
            }
            for intent in &explicit_intents {
                if !read_only_side_effect(&intent.side_effect_class) {
                    continue;
                }
                let Some(args) = payload_for_intent(&action, &intent.normalized) else {
                    continue;
                };
                candidates.push(CandidateSeed {
                    action_ref: action_ref.clone(),
                    args,
                    source: CandidateSource::GuardedSubstitution,
                    completion_role: CompletionRole::Terminal,
                    rationale: format!(
                        "contract-declared {} intent can replace the explicit tool action",
                        intent.normalized.intent
                    ),
                    expected_progress_override: Some(1.7),
                });
            }
        }
    }
    candidates
}

fn repair_seed_from_rule(
    rule: &RepairRuleSpec,
    failed_action: &ActionRef,
    failed_args: &Value,
    _observation: &Value,
    failure_kind: FailureKind,
) -> Option<CandidateSeed> {
    if !rule.failure_kinds.is_empty()
        && !rule
            .failure_kinds
            .iter()
            .any(|kind| FailureKind::from_str(kind) == failure_kind)
    {
        return None;
    }
    let bindings = capture_repair_bindings(rule, failed_args)?;
    let mut args = rule.args.clone();
    apply_arg_templates(&mut args, rule, failed_args, &bindings)?;
    Some(CandidateSeed {
        action_ref: ActionRef {
            contract_id: rule
                .contract_id
                .clone()
                .unwrap_or_else(|| failed_action.contract_id.clone()),
            action_name: rule.action_name.clone(),
        },
        args,
        source: parse_candidate_source(rule.source.as_deref(), CandidateSource::Repair),
        completion_role: parse_seed_completion_role(rule.completion_role.as_deref()),
        rationale: rule
            .rationale
            .clone()
            .unwrap_or_else(|| "contract-declared repair rule matched the failure".to_string()),
        expected_progress_override: rule.expected_progress,
    })
}

fn apply_arg_templates(
    args: &mut Value,
    rule: &RepairRuleSpec,
    failed_args: &Value,
    bindings: &BTreeMap<String, String>,
) -> Option<()> {
    if rule.arg_templates.is_empty() {
        return Some(());
    }
    let object = args.as_object_mut()?;
    for (key, template) in &rule.arg_templates {
        object.insert(
            key.clone(),
            json!(render_template(template, failed_args, bindings)?),
        );
    }
    Some(())
}

fn capture_repair_bindings(
    rule: &RepairRuleSpec,
    _failed_args: &Value,
) -> Option<BTreeMap<String, String>> {
    if !rule.bindings.is_empty() {
        return None;
    }
    Some(BTreeMap::new())
}

fn render_template(
    template: &str,
    failed_args: &Value,
    bindings: &BTreeMap<String, String>,
) -> Option<String> {
    let mut rendered = template.to_string();
    for (key, value) in failed_args.as_object()? {
        if let Some(text) = value.as_str() {
            rendered = rendered.replace(&format!("{{input.{key}}}"), text);
        }
    }
    for (key, value) in bindings {
        rendered = rendered.replace(&format!("{{binding.{key}}}"), value);
    }
    Some(rendered.trim().to_string())
}

fn parse_candidate_source(value: Option<&str>, default: CandidateSource) -> CandidateSource {
    match value {
        Some("cheap_observation") => CandidateSource::CheapObservation,
        Some("guarded_substitution") => CandidateSource::GuardedSubstitution,
        Some("repair") => CandidateSource::Repair,
        Some("verification") => CandidateSource::Verification,
        Some("contract_goal") => CandidateSource::ContractGoal,
        Some("model_proposal") => CandidateSource::ModelProposal,
        Some("model_observation_proposal") => CandidateSource::ModelObservationProposal,
        Some("model_goal_verifier") => CandidateSource::ModelGoalVerifier,
        _ => default,
    }
}

fn parse_seed_completion_role(value: Option<&str>) -> CompletionRole {
    match value {
        Some("support") => CompletionRole::Support,
        Some("verification") => CompletionRole::Verification,
        Some("repair") => CompletionRole::Repair,
        _ => CompletionRole::Terminal,
    }
}

fn scores_for_candidate(action: &ActionContract, seed: &CandidateSeed) -> ActionScores {
    let mut scores = scores_for_action(action);
    if let Some(expected_progress) = seed.expected_progress_override {
        scores.expected_progress = expected_progress;
    }
    match seed.completion_role {
        CompletionRole::Terminal => scores.verification_value += 0.15,
        CompletionRole::Verification => {
            scores.verification_value += 1.0;
            scores.expected_progress += 0.25;
        }
        CompletionRole::Repair => {
            scores.information_gain += 0.3;
            scores.expected_progress += 0.2;
        }
        CompletionRole::Support => {
            scores.information_gain += 0.2;
        }
    }
    scores
}

fn candidate_key(action_ref: &ActionRef, args: &Value) -> String {
    format!(
        "{}.{}:{}",
        action_ref.contract_id,
        action_ref.action_name,
        serde_json::to_string(args).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ApprovalSpec, CapabilityContract, ContractStatus, ExtractorSlotSpec, Idempotency,
        InMemoryContractRegistry, IntentExtractorSpec, RepairBindingSpec, RepairRuleSpec,
        Reversibility, RiskLevel, SemanticIntentSpec, SideEffectClass, TrustLevel,
        VerificationSpec, VerificationTemplateSpec,
    };
    use std::collections::BTreeMap;

    fn registry() -> InMemoryContractRegistry {
        let mut registry = InMemoryContractRegistry::default();
        let mut actions = Vec::new();
        for (name, side_effect_class, risk_level) in [
            ("Bash", SideEffectClass::Unknown, RiskLevel::High),
            ("Read", SideEffectClass::LocalRead, RiskLevel::Low),
            ("Grep", SideEffectClass::LocalRead, RiskLevel::Low),
            ("Glob", SideEffectClass::LocalRead, RiskLevel::Low),
            ("Write", SideEffectClass::LocalWrite, RiskLevel::Medium),
        ] {
            actions.push(ActionContract {
                name: name.to_string(),
                description: name.to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: json!({"type": "object"}),
                side_effect_class,
                reversibility: Reversibility::Reversible,
                idempotency: Idempotency::Idempotent,
                risk_level,
                preconditions: Vec::new(),
                postconditions: Vec::new(),
                verification: VerificationSpec {
                    methods: Vec::new(),
                    templates: verification_templates(name),
                    required_before_completion: matches!(name, "Bash" | "Write"),
                    confidence: 0.5,
                },
                approval: ApprovalSpec {
                    required: false,
                    reason: None,
                },
                failure_modes: Vec::new(),
                forbidden_uses: Vec::new(),
                argument_safety: Vec::new(),
                semantic_intents: semantic_intents(name),
                intent_extractors: intent_extractors(name),
                repair_rules: repair_rules(name),
                cost_estimate: None,
                latency_estimate: None,
            });
        }
        registry
            .register(CapabilityContract {
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                version: "0.1.0".to_string(),
                status: ContractStatus::Active,
                trust_level: TrustLevel::Sandboxed,
                description: "tools".to_string(),
                actions,
                global_constraints: Vec::new(),
                forbidden_uses: Vec::new(),
                local_rules: Vec::new(),
                examples: Vec::new(),
                contract_hash: None,
            })
            .unwrap();
        registry
    }

    fn verification_templates(name: &str) -> Vec<VerificationTemplateSpec> {
        if name != "Write" {
            return Vec::new();
        }
        vec![VerificationTemplateSpec {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Read".to_string(),
            args: json!({"file_path": "{arg.file_path}"}),
            reason: Some("read changed file".to_string()),
        }]
    }

    fn semantic_intents(name: &str) -> Vec<SemanticIntentSpec> {
        match name {
            "Read" => vec![intent("read_file", [("path", "file_path")])],
            "Grep" => vec![intent("search_text", [("pattern", "pattern")])],
            "Glob" => vec![intent("glob_paths", [("pattern", "pattern")])],
            _ => Vec::new(),
        }
    }

    fn intent<const N: usize>(
        name: &str,
        slots: [(&'static str, &'static str); N],
    ) -> SemanticIntentSpec {
        SemanticIntentSpec {
            intent: name.to_string(),
            slots: slots
                .into_iter()
                .map(|(slot, arg)| (slot.to_string(), arg.to_string()))
                .collect(),
            optional_slots: BTreeMap::new(),
            defaults: BTreeMap::new(),
            side_effect_class: None,
        }
    }

    fn intent_extractors(name: &str) -> Vec<IntentExtractorSpec> {
        if name != "Bash" {
            return Vec::new();
        }
        vec![IntentExtractorSpec {
            intent: "read_file".to_string(),
            parser: "simple_shell".to_string(),
            source_arg: "command".to_string(),
            pattern: None,
            command: "cat".to_string(),
            slots: vec![ExtractorSlotSpec {
                name: "path".to_string(),
                position: 0,
            }],
            optional_slots: Vec::new(),
            slot_groups: BTreeMap::new(),
            optional_slot_groups: BTreeMap::new(),
            literals: Vec::new(),
            side_effect_class: Some(SideEffectClass::LocalRead),
        }]
    }

    fn repair_rules(name: &str) -> Vec<RepairRuleSpec> {
        if name != "Bash" {
            return Vec::new();
        }
        vec![RepairRuleSpec {
            failure_kinds: vec!["command_not_found".to_string()],
            contract_id: None,
            action_name: "Glob".to_string(),
            args: json!({"pattern": "{Cargo.toml,package.json,pyproject.toml,go.mod}"}),
            arg_templates: BTreeMap::new(),
            bindings: Vec::new(),
            source: Some("repair".to_string()),
            completion_role: Some("repair".to_string()),
            rationale: Some("inspect project metadata".to_string()),
            expected_progress: Some(0.9),
        }]
    }

    #[test]
    fn explicit_bash_cat_does_not_get_read_substitution_without_structural_intent() {
        let options = RunOptions {
            puffer_tool: Some("Bash".to_string()),
            puffer_args: Some(json!({"command": "cat /tmp/example.txt"})),
            puffer_tool_source: CandidateSource::ExplicitUserTool,
            allow_failed_terminal_completion: false,
            enable_symbolic_workers: false,
            enable_parallel_read_only: false,
            enable_observe_act_llm: false,
            model: None,
            goal_verification_min_confidence: 0.75,
        };
        let candidates = initial_candidates(&registry(), "read file", &options);
        assert!(!candidates
            .iter()
            .any(|candidate| candidate.action_ref.action_name == "Read"));
    }

    #[test]
    fn piped_find_does_not_get_glob_substitution() {
        let options = RunOptions {
            puffer_tool: Some("Bash".to_string()),
            puffer_args: Some(json!({
                "command": "find resources/tools -name '*.yaml' | sort",
                "timeout": 60000,
            })),
            puffer_tool_source: CandidateSource::ExplicitUserTool,
            allow_failed_terminal_completion: false,
            enable_symbolic_workers: false,
            enable_parallel_read_only: false,
            enable_observe_act_llm: false,
            model: None,
            goal_verification_min_confidence: 0.75,
        };
        let candidates = initial_candidates(&registry(), "find tool contracts", &options);
        assert!(!candidates
            .iter()
            .any(|candidate| candidate.action_ref.action_name == "Glob"));
    }

    #[test]
    fn verification_candidates_use_declared_method_resolution() {
        let candidates = verification_candidates_for_action(
            &registry(),
            &puffer_action("Write"),
            &json!({"file_path": "/tmp/example.txt", "content": "hello"}),
            &json!({"success": true}),
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].action_ref, puffer_action("Read"));
        assert_eq!(candidates[0].args, json!({"file_path": "/tmp/example.txt"}));
        assert_eq!(candidates[0].completion_role, CompletionRole::Verification);
    }

    #[test]
    fn repair_rules_with_regex_bindings_are_ignored() {
        let rule = RepairRuleSpec {
            failure_kinds: Vec::new(),
            contract_id: None,
            action_name: "Read".to_string(),
            args: json!({"file_path": "{binding.path}"}),
            arg_templates: BTreeMap::new(),
            bindings: vec![RepairBindingSpec {
                source_arg: "command".to_string(),
                pattern: "cat (?P<path>.+)".to_string(),
            }],
            source: Some("repair".to_string()),
            completion_role: Some("repair".to_string()),
            rationale: None,
            expected_progress: None,
        };

        let seed = repair_seed_from_rule(
            &rule,
            &puffer_action("Bash"),
            &json!({"command": "cat src/lib.rs"}),
            &json!({"success": false}),
            FailureKind::Unknown,
        );

        assert!(seed.is_none());
    }
}
