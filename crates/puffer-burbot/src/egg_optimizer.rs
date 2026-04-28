use crate::contract::{ActionContract, ContractRegistry};
use crate::graph::{ActionRef, PlanEdgeKind, PlanGraph, PlanNodeKind, PlanStatus};
use crate::ids::NodeId;
use crate::saturation::{audit_action, has_protected_scheduling_edges, read_only_proof_payload};
use crate::semantics::{
    intent_preference, safe_observation_intents, semantic_symbol, ActionIntentMatch,
    NormalizedIntent,
};
use anyhow::Result;
use egg::{rewrite, EGraph, Id, RecExpr, Rewrite, Runner, SymbolLang};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone)]
struct EligibleAction {
    node_id: NodeId,
    expr_id: Id,
    preference: (u8, u8, u64),
    normalized_intent: NormalizedIntent,
    proof: Value,
}

struct EligibleTerm {
    node_id: NodeId,
    expr: RecExpr<SymbolLang>,
    preference: (u8, u8, u64),
    normalized_intent: NormalizedIntent,
    proof: Value,
}

#[derive(Default)]
struct TermInterner {
    symbols: BTreeMap<String, String>,
}

/// Adds `CanReplace` edges discovered by bounded read-only egg normalization.
pub(crate) fn add_read_only_equivalence_edges(
    graph: &mut PlanGraph,
    contracts: &dyn ContractRegistry,
) -> Result<usize> {
    let mut interner = TermInterner::default();
    let mut egraph = EGraph::<SymbolLang, ()>::default();
    let mut entries = Vec::new();

    for node_id in eligible_open_action_ids(graph) {
        for term in expressions_for_node(graph, contracts, node_id, &mut interner) {
            let expr_id = egraph.add_expr(&term.expr);
            entries.push(EligibleAction {
                node_id: term.node_id,
                expr_id,
                preference: term.preference,
                normalized_intent: term.normalized_intent,
                proof: term.proof,
            });
        }
    }
    if entries.len() < 2 {
        return Ok(0);
    }

    let runner = Runner::default()
        .with_egraph(egraph)
        .with_iter_limit(6)
        .with_node_limit(10_000)
        .run(&rewrite_rules());
    let mut classes = BTreeMap::<usize, Vec<EligibleAction>>::new();
    for entry in entries {
        let class_id = runner.egraph.find(entry.expr_id);
        classes
            .entry(usize::from(class_id))
            .or_default()
            .push(entry);
    }

    let mut changes = 0;
    for (class_id, mut class_entries) in classes {
        class_entries.sort_by_key(|entry| (entry.node_id, entry.preference));
        class_entries.dedup_by_key(|entry| entry.node_id);
        if class_entries.len() < 2 {
            continue;
        }
        class_entries.sort_by_key(|entry| (entry.preference, entry.node_id));
        let replacement = class_entries[0].clone();
        let equivalence_class = egg_equivalence_class_payload(class_id, &class_entries);
        let proof_payloads = class_entries
            .iter()
            .map(|entry| entry.proof.clone())
            .collect::<Vec<_>>();
        for dominated in class_entries.iter().skip(1).cloned() {
            if graph.has_edge(
                replacement.node_id,
                dominated.node_id,
                PlanEdgeKind::CanReplace,
            ) {
                continue;
            }
            let payload = {
                let replacement_node = graph.node(replacement.node_id)?;
                let dominated_node = graph.node(dominated.node_id)?;
                json!({
                    "reason": "egg_read_only_equivalence",
                    "guard": "all actions are open read-only observations with equivalent normalized intent and no protected scheduling edges",
                    "optimizer": "egg",
                    "rules": rewrite_rule_names(),
                    "eclass": class_id,
                    "saturation_class": {
                        "class_id": format!("egg-saturation:{class_id}"),
                        "source": "egg_read_only_optimizer",
                        "dominance_strategy": "lowest_preference_tuple_wins",
                        "member_count": proof_payloads.len(),
                        "winner_node_id": replacement.node_id.0,
                        "dominated_node_id": dominated.node_id.0,
                    },
                    "equivalence_class": equivalence_class.clone(),
                    "dominance": egg_dominance_payload(&replacement, &dominated),
                    "proof": {
                        "read_only_safe_alternatives": proof_payloads.clone(),
                    },
                    "replacement": audit_action(replacement_node),
                    "dominated": audit_action(dominated_node),
                })
            };
            graph.add_edge(
                replacement.node_id,
                dominated.node_id,
                PlanEdgeKind::CanReplace,
                payload,
            );
            changes += 1;
        }
    }
    Ok(changes)
}

fn eligible_open_action_ids(graph: &PlanGraph) -> Vec<NodeId> {
    let mut ids = graph
        .nodes
        .iter()
        .filter_map(|(id, node)| {
            (node.kind == PlanNodeKind::Action
                && node.status == PlanStatus::Open
                && !has_protected_scheduling_edges(graph, *id))
            .then_some(*id)
        })
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

fn expressions_for_node(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
    node_id: NodeId,
    interner: &mut TermInterner,
) -> Vec<EligibleTerm> {
    let Some(node) = graph.nodes.get(&node_id) else {
        return Vec::new();
    };
    let Some(action_ref) = node.action_ref.as_ref() else {
        return Vec::new();
    };
    let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
    else {
        return Vec::new();
    };
    if !eligible_read_action(&action) {
        return Vec::new();
    }

    safe_observation_intents(&action, &node.payload)
        .into_iter()
        .filter_map(|intent| {
            let term = term_for_intent(&intent.normalized, interner);
            let expr = term.parse().ok()?;
            Some(EligibleTerm {
                node_id,
                expr,
                preference: preference(action_ref, &action, &intent, node_id),
                normalized_intent: intent.normalized.clone(),
                proof: read_only_proof_payload(node, &action, &intent),
            })
        })
        .collect()
}

fn eligible_read_action(action: &ActionContract) -> bool {
    !action.approval.required
}

fn term_for_intent(intent: &NormalizedIntent, interner: &mut TermInterner) -> String {
    let intent_symbol = semantic_symbol(&format!("intent_{}", intent.intent));
    let slots = slot_set_term(intent, interner);
    format!("(redundant_read (safe_read {intent_symbol} {slots}))")
}

fn slot_set_term(intent: &NormalizedIntent, interner: &mut TermInterner) -> String {
    let mut parts = Vec::new();
    for (slot, value) in &intent.slots {
        parts.push(format!(
            "(slot {} {})",
            semantic_symbol(slot),
            interner.symbol(slot, value)
        ));
    }
    match parts.len() {
        0 => "empty_slots".to_string(),
        1 => parts.remove(0),
        _ => format!("(slot_set {})", parts.join(" ")),
    }
}

fn preference(
    action_ref: &ActionRef,
    action: &ActionContract,
    intent: &ActionIntentMatch,
    node_id: NodeId,
) -> (u8, u8, u64) {
    intent_preference(action_ref, action, intent, node_id)
}

fn rewrite_rules() -> Vec<Rewrite<SymbolLang, ()>> {
    vec![
        rewrite!("safe-read-normal-form"; "(safe_read ?intent ?slots)" => "(read_observation ?intent ?slots)"),
        rewrite!("drop-redundant-normal-read"; "(redundant_read (read_observation ?intent ?slots))" => "(read_observation ?intent ?slots)"),
        rewrite!("drop-redundant-safe-read"; "(redundant_read (safe_read ?intent ?slots))" => "(safe_read ?intent ?slots)"),
    ]
}

fn rewrite_rule_names() -> Vec<&'static str> {
    vec![
        "safe-read-normal-form",
        "drop-redundant-normal-read",
        "drop-redundant-safe-read",
    ]
}

fn egg_equivalence_class_payload(class_id: usize, entries: &[EligibleAction]) -> Value {
    json!({
        "class_id": format!("egg-eclass:{class_id}"),
        "kind": "egg_normalized_read_only_intent",
        "members": entries.iter().map(egg_member_payload).collect::<Vec<_>>(),
    })
}

fn egg_member_payload(entry: &EligibleAction) -> Value {
    json!({
        "node_id": entry.node_id.0,
        "normalized_intent": entry.normalized_intent,
        "preference": preference_payload(entry.preference),
    })
}

fn egg_dominance_payload(replacement: &EligibleAction, dominated: &EligibleAction) -> Value {
    json!({
        "strategy": "lowest_preference_tuple_wins",
        "reasons": egg_dominance_reasons(replacement, dominated),
        "replacement": {
            "node_id": replacement.node_id.0,
            "preference": preference_payload(replacement.preference),
        },
        "dominated": {
            "node_id": dominated.node_id.0,
            "preference": preference_payload(dominated.preference),
        },
    })
}

fn egg_dominance_reasons(
    replacement: &EligibleAction,
    dominated: &EligibleAction,
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if replacement.preference.0 < dominated.preference.0 {
        reasons.push("direct_semantic_intent_preferred");
    }
    if replacement.preference.1 < dominated.preference.1 {
        reasons.push("lower_risk_contract_preferred");
    }
    if replacement.preference.0 == dominated.preference.0
        && replacement.preference.1 == dominated.preference.1
        && replacement.preference.2 < dominated.preference.2
    {
        reasons.push("stable_node_order_tiebreaker");
    }
    if reasons.is_empty() {
        reasons.push("equivalent_read_intent_preferred_by_total_order");
    }
    reasons
}

fn preference_payload(preference: (u8, u8, u64)) -> Value {
    json!({
        "intent_source_rank": preference.0,
        "risk_rank": preference.1,
        "node_id_tiebreaker": preference.2,
    })
}

impl TermInterner {
    fn symbol(&mut self, namespace: &str, value: &str) -> String {
        let key = format!("{namespace}:{value}");
        if let Some(symbol) = self.symbols.get(&key) {
            return symbol.clone();
        }
        let symbol = format!("{}_{}", semantic_symbol(namespace), self.symbols.len());
        self.symbols.insert(key, symbol.clone());
        symbol
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, ExtractorSlotSpec,
        Idempotency, InMemoryContractRegistry, IntentExtractorSpec, Reversibility, RiskLevel,
        SemanticIntentSpec, SideEffectClass, TrustLevel, VerificationSpec,
    };
    use crate::graph::{ActionScores, PlanNode};
    use std::collections::BTreeMap;

    fn registry_with(actions: Vec<ActionContract>) -> InMemoryContractRegistry {
        let mut registry = InMemoryContractRegistry::default();
        registry
            .register(CapabilityContract {
                contract_id: "puffer.tools".to_string(),
                version: "1.0.0".to_string(),
                status: ContractStatus::Active,
                trust_level: TrustLevel::Sandboxed,
                description: "test tools".to_string(),
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

    fn action(name: &str, side_effect_class: SideEffectClass) -> ActionContract {
        ActionContract {
            name: name.to_string(),
            description: format!("{name} action"),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            side_effect_class,
            reversibility: Reversibility::Reversible,
            idempotency: Idempotency::Idempotent,
            risk_level: if name == "Bash" {
                RiskLevel::High
            } else {
                RiskLevel::Low
            },
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
            failure_modes: Vec::new(),
            forbidden_uses: Vec::new(),
            argument_safety: Vec::new(),
            semantic_intents: semantic_intents(name),
            intent_extractors: intent_extractors(name),
            repair_rules: Vec::new(),
            cost_estimate: None,
            latency_estimate: None,
        }
    }

    fn read_capable_bash() -> ActionContract {
        let mut action = action("Bash", SideEffectClass::Unknown);
        action.semantic_intents = vec![SemanticIntentSpec {
            intent: "read_file".to_string(),
            slots: [("path".to_string(), "file_path".to_string())].into(),
            optional_slots: BTreeMap::new(),
            defaults: BTreeMap::new(),
            side_effect_class: Some(SideEffectClass::LocalRead),
        }];
        action
    }

    fn semantic_intents(name: &str) -> Vec<SemanticIntentSpec> {
        match name {
            "Read" => vec![SemanticIntentSpec {
                intent: "read_file".to_string(),
                slots: [("path".to_string(), "file_path".to_string())].into(),
                optional_slots: BTreeMap::new(),
                defaults: BTreeMap::new(),
                side_effect_class: None,
            }],
            _ => Vec::new(),
        }
    }

    fn intent_extractors(name: &str) -> Vec<IntentExtractorSpec> {
        match name {
            "Bash" => vec![IntentExtractorSpec {
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
            }],
            _ => Vec::new(),
        }
    }

    fn action_node(name: &str, payload: serde_json::Value) -> PlanNode {
        PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: name.to_string(),
            payload,
            action_ref: Some(ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: name.to_string(),
            }),
            scores: ActionScores::default(),
        }
    }

    #[test]
    fn egg_does_not_parse_bash_command_text_for_equivalence() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::PureObservation),
            action("Bash", SideEffectClass::Unknown),
        ]);
        let mut graph = PlanGraph::new();
        let bash_id = graph.add_node(action_node("Bash", json!({"command": "cat src/lib.rs"})));
        let read_id = graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));

        let changes = add_read_only_equivalence_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
        assert!(!graph.has_edge(read_id, bash_id, PlanEdgeKind::CanReplace));
    }

    #[test]
    fn egg_payload_records_class_proof_rules_and_dominance() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::PureObservation),
            read_capable_bash(),
        ]);
        let mut graph = PlanGraph::new();
        let bash_id = graph.add_node(action_node("Bash", json!({"file_path": "src/lib.rs"})));
        let read_id = graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));

        let changes = add_read_only_equivalence_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 1);
        let edge = graph
            .edges
            .iter()
            .find(|edge| {
                edge.source == read_id
                    && edge.target == bash_id
                    && edge.kind == PlanEdgeKind::CanReplace
            })
            .unwrap();
        assert_eq!(edge.payload["optimizer"], json!("egg"));
        assert_eq!(edge.payload["rules"][0], json!("safe-read-normal-form"));
        assert_eq!(
            edge.payload["equivalence_class"]["kind"],
            json!("egg_normalized_read_only_intent")
        );
        assert_eq!(
            edge.payload["proof"]["read_only_safe_alternatives"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            edge.payload["dominance"]["reasons"][0],
            json!("direct_semantic_intent_preferred")
        );
    }

    #[test]
    fn egg_normalizes_redundant_read_paths() {
        let registry = registry_with(vec![action("Read", SideEffectClass::PureObservation)]);
        let mut graph = PlanGraph::new();
        let canonical_id = graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        let redundant_id =
            graph.add_node(action_node("Read", json!({"file_path": "./src//lib.rs/"})));

        let changes = add_read_only_equivalence_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 1);
        assert!(graph.has_edge(canonical_id, redundant_id, PlanEdgeKind::CanReplace));
    }

    #[test]
    fn egg_does_not_optimize_write_contract() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::LocalWrite),
            action("Bash", SideEffectClass::Unknown),
        ]);
        let mut graph = PlanGraph::new();
        graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        graph.add_node(action_node("Bash", json!({"command": "cat src/lib.rs"})));

        let changes = add_read_only_equivalence_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
    }

    #[test]
    fn egg_does_not_optimize_unproven_unknown_side_effects() {
        let registry = registry_with(vec![action("Read", SideEffectClass::Unknown)]);
        let mut graph = PlanGraph::new();
        graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));

        let changes = add_read_only_equivalence_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
    }

    #[test]
    fn egg_does_not_optimize_redirecting_bash() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::PureObservation),
            action("Bash", SideEffectClass::Unknown),
        ]);
        let mut graph = PlanGraph::new();
        graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        graph.add_node(action_node(
            "Bash",
            json!({"command": "cat src/lib.rs > /tmp/out"}),
        ));

        let changes = add_read_only_equivalence_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
    }

    #[test]
    fn egg_preserves_dependency_edges_by_skipping_node() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::PureObservation),
            read_capable_bash(),
        ]);
        let mut graph = PlanGraph::new();
        let read_id = graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        let bash_id = graph.add_node(action_node("Bash", json!({"file_path": "src/lib.rs"})));
        graph.add_edge(read_id, bash_id, PlanEdgeKind::DependsOn, json!({}));

        let changes = add_read_only_equivalence_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
    }
}
