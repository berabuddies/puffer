use crate::contract::{ActionContract, ContractRegistry, SideEffectClass};
use crate::graph::{ActionRef, PlanEdgeKind, PlanGraph, PlanNode, PlanNodeKind, PlanStatus};
use crate::ids::NodeId;
use crate::semantics::{
    intent_preference, read_only_side_effect, safe_observation_intents, ActionIntentMatch,
    IntentSource, NormalizedIntent,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SaturationReport {
    pub(crate) can_replace_edges: usize,
    pub(crate) pruned_actions: usize,
}

#[derive(Debug, Clone)]
struct ClassMember {
    node_id: NodeId,
    label: String,
    payload: Value,
    action_ref: ActionRef,
    action: ActionContract,
    intent: ActionIntentMatch,
    preference: (u8, u8, u64),
}

#[derive(Debug, Clone)]
struct EquivalenceClass {
    class_id: String,
    normalized_intent: NormalizedIntent,
    members: Vec<ClassMember>,
}

#[derive(Debug, Clone)]
struct Substitution {
    replacement: ClassMember,
    dominated: ClassMember,
    class: EquivalenceClass,
    dominance_reasons: Vec<&'static str>,
}

/// Adds deterministic guarded substitution edges and prunes dominated open actions.
pub(crate) fn saturate_guarded_substitutions(
    graph: &mut PlanGraph,
    contracts: &dyn ContractRegistry,
) -> Result<SaturationReport> {
    let can_replace_edges = add_guarded_substitution_edges(graph, contracts)?;
    #[cfg(feature = "egg-optimizer")]
    let can_replace_edges = can_replace_edges
        + crate::egg_optimizer::add_read_only_equivalence_edges(graph, contracts)?;
    let pruned_actions = prune_dominated_open_actions(graph)?;
    Ok(SaturationReport {
        can_replace_edges,
        pruned_actions,
    })
}

/// Adds `CanReplace` edges for safe read-only substitutions.
pub(crate) fn add_guarded_substitution_edges(
    graph: &mut PlanGraph,
    contracts: &dyn ContractRegistry,
) -> Result<usize> {
    let classes = guarded_equivalence_classes(graph, contracts)?;
    let mut substitutions = substitutions_from_classes(&classes);

    substitutions.sort_by_key(|substitution| {
        (
            substitution.replacement.node_id,
            substitution.dominated.node_id,
            substitution.class.class_id.clone(),
        )
    });
    let mut changes = 0;
    for substitution in substitutions {
        if graph.has_edge(
            substitution.replacement.node_id,
            substitution.dominated.node_id,
            PlanEdgeKind::CanReplace,
        ) {
            continue;
        }
        graph.add_edge(
            substitution.replacement.node_id,
            substitution.dominated.node_id,
            PlanEdgeKind::CanReplace,
            substitution_payload(&substitution),
        );
        changes += 1;
    }
    Ok(changes)
}

/// Marks open actions dominated by `CanReplace` edges as pruned with audit payload.
pub(crate) fn prune_dominated_open_actions(graph: &mut PlanGraph) -> Result<usize> {
    let mut dominated = graph
        .edges
        .iter()
        .filter(|edge| edge.kind == PlanEdgeKind::CanReplace)
        .filter_map(|edge| {
            let replacement = graph.nodes.get(&edge.source)?;
            let dominated = graph.nodes.get(&edge.target)?;
            (replacement.kind == PlanNodeKind::Action
                && replacement.status == PlanStatus::Open
                && dominated.kind == PlanNodeKind::Action
                && dominated.status == PlanStatus::Open
                && !has_protected_scheduling_edges(graph, edge.source)
                && !has_protected_scheduling_edges(graph, edge.target))
            .then_some((edge.source, edge.target, edge.payload.clone()))
        })
        .collect::<Vec<_>>();
    dominated.sort_by_key(|(_, target, _)| *target);
    dominated.dedup_by_key(|(_, target, _)| *target);

    let mut changes = 0;
    for (replacement_id, dominated_id, edge_payload) in dominated {
        let replacement = graph.node(replacement_id)?.clone();
        let dominated = graph.node_mut(dominated_id)?;
        let original_payload = dominated.payload.clone();
        dominated.status = PlanStatus::Pruned;
        dominated.payload = json!({
            "pruned": true,
            "original_payload": original_payload,
            "dominated_by": {
                "node_id": replacement_id.0,
                "label": replacement.label,
                "action": replacement.action_ref.map(|action_ref| {
                    json!({
                        "contract_id": action_ref.contract_id,
                        "action_name": action_ref.action_name,
                    })
                }),
            },
            "can_replace": edge_payload,
        });
        changes += 1;
    }
    Ok(changes)
}

fn guarded_equivalence_classes(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
) -> Result<Vec<EquivalenceClass>> {
    let mut classes = BTreeMap::<NormalizedIntent, Vec<ClassMember>>::new();
    for node_id in eligible_open_action_ids(graph) {
        for member in class_members_for_node(graph, contracts, node_id)? {
            classes
                .entry(member.intent.normalized.clone())
                .or_default()
                .push(member);
        }
    }

    let mut result = Vec::new();
    for (normalized_intent, mut members) in classes {
        members.sort_by_key(|member| (member.node_id, member.preference));
        members.dedup_by_key(|member| member.node_id);
        if members.len() < 2 {
            continue;
        }
        members.sort_by_key(|member| (member.preference, member.node_id));
        result.push(EquivalenceClass {
            class_id: class_id_for_intent(&normalized_intent),
            normalized_intent,
            members,
        });
    }
    result.sort_by(|left, right| left.class_id.cmp(&right.class_id));
    Ok(result)
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

/// Returns true when replacing a node could change dependency or write ordering.
pub(crate) fn has_protected_scheduling_edges(graph: &PlanGraph, node_id: NodeId) -> bool {
    graph.edges.iter().any(|edge| {
        (edge.source == node_id || edge.target == node_id)
            && matches!(
                edge.kind,
                PlanEdgeKind::Requires
                    | PlanEdgeKind::DependsOn
                    | PlanEdgeKind::PreconditionFor
                    | PlanEdgeKind::Repairs
                    | PlanEdgeKind::FailedWith
                    | PlanEdgeKind::Verifies
                    | PlanEdgeKind::Produces
                    | PlanEdgeKind::ApprovalRequiredFor
            )
    })
}

fn class_members_for_node(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
    node_id: NodeId,
) -> Result<Vec<ClassMember>> {
    let node = graph.node(node_id)?;
    let Some(action_ref) = node.action_ref.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
    else {
        return Ok(Vec::new());
    };
    if action.approval.required {
        return Ok(Vec::new());
    }

    Ok(safe_observation_intents(&action, &node.payload)
        .into_iter()
        .map(|intent| ClassMember {
            node_id,
            label: node.label.clone(),
            payload: node.payload.clone(),
            action_ref: action_ref.clone(),
            preference: intent_preference(action_ref, &action, &intent, node_id),
            action: action.clone(),
            intent,
        })
        .collect())
}

fn substitutions_from_classes(classes: &[EquivalenceClass]) -> Vec<Substitution> {
    let mut substitutions = Vec::new();
    for class in classes {
        let Some(replacement) = class.members.first().cloned() else {
            continue;
        };
        for dominated in class.members.iter().skip(1).cloned() {
            substitutions.push(Substitution {
                dominance_reasons: dominance_reasons(&replacement, &dominated),
                replacement: replacement.clone(),
                dominated,
                class: class.clone(),
            });
        }
    }
    substitutions
}

fn substitution_payload(substitution: &Substitution) -> Value {
    json!({
        "reason": "guarded_read_only_equivalence_class",
        "guard": "all class members are open read-only observations with contract or payload intent proof",
        "saturation_class": {
            "class_id": format!("saturation:{}", substitution.class.class_id),
            "source": "guarded_substitution",
            "dominance_strategy": "lowest_preference_tuple_wins",
            "member_count": substitution.class.members.len(),
            "winner_node_id": substitution.replacement.node_id.0,
            "dominated_node_id": substitution.dominated.node_id.0,
        },
        "equivalence_class": equivalence_class_payload(&substitution.class),
        "dominance": dominance_payload(
            &substitution.replacement,
            &substitution.dominated,
            &substitution.dominance_reasons,
        ),
        "proof": {
            "read_only_safe_alternatives": substitution
                .class
                .members
                .iter()
                .map(member_read_only_proof_payload)
                .collect::<Vec<_>>(),
        },
        "replacement": audit_member(&substitution.replacement),
        "dominated": audit_member(&substitution.dominated),
    })
}

fn equivalence_class_payload(class: &EquivalenceClass) -> Value {
    json!({
        "class_id": class.class_id,
        "kind": "normalized_read_only_intent",
        "normalized_intent": class.normalized_intent,
        "members": class.members.iter().map(equivalence_member_payload).collect::<Vec<_>>(),
    })
}

fn equivalence_member_payload(member: &ClassMember) -> Value {
    json!({
        "node_id": member.node_id.0,
        "label": member.label,
        "action": {
            "contract_id": member.action_ref.contract_id,
            "action_name": member.action_ref.action_name,
        },
        "intent_source": intent_source_label(&member.intent.source),
        "preference": preference_payload(member.preference),
    })
}

fn member_read_only_proof_payload(member: &ClassMember) -> Value {
    proof_payload(
        member.node_id,
        &member.label,
        &member.action_ref,
        &member.payload,
        &member.action,
        &member.intent,
    )
}

/// Builds the proof payload used when an optimizer treats a node as read-only.
pub(crate) fn read_only_proof_payload(
    node: &PlanNode,
    action: &ActionContract,
    intent: &ActionIntentMatch,
) -> Value {
    let action_ref = node.action_ref.as_ref();
    json!({
        "node_id": node.id.0,
        "label": node.label,
        "action": action_ref.map(|action_ref| {
            json!({
                "contract_id": action_ref.contract_id,
                "action_name": action_ref.action_name,
            })
        }),
        "payload": node.payload,
        "approval_required": action.approval.required,
        "contract_side_effect_class": side_effect_payload(&action.side_effect_class),
        "intent_side_effect_class": side_effect_payload(&intent.side_effect_class),
        "intent_source": intent_source_label(&intent.source),
        "normalized_intent": intent.normalized,
        "safe_by": read_only_safe_by(action),
    })
}

fn proof_payload(
    node_id: NodeId,
    label: &str,
    action_ref: &ActionRef,
    payload: &Value,
    action: &ActionContract,
    intent: &ActionIntentMatch,
) -> Value {
    json!({
        "node_id": node_id.0,
        "label": label,
        "action": {
            "contract_id": action_ref.contract_id,
            "action_name": action_ref.action_name,
        },
        "payload": payload,
        "approval_required": action.approval.required,
        "contract_side_effect_class": side_effect_payload(&action.side_effect_class),
        "intent_side_effect_class": side_effect_payload(&intent.side_effect_class),
        "intent_source": intent_source_label(&intent.source),
        "normalized_intent": intent.normalized,
        "safe_by": read_only_safe_by(action),
    })
}

fn dominance_payload(
    replacement: &ClassMember,
    dominated: &ClassMember,
    reasons: &[&'static str],
) -> Value {
    json!({
        "strategy": "lowest_preference_tuple_wins",
        "reasons": reasons,
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

fn dominance_reasons(replacement: &ClassMember, dominated: &ClassMember) -> Vec<&'static str> {
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

fn audit_member(member: &ClassMember) -> Value {
    json!({
        "node_id": member.node_id.0,
        "label": member.label,
        "action": {
            "contract_id": member.action_ref.contract_id,
            "action_name": member.action_ref.action_name,
        },
        "payload": member.payload,
    })
}

fn class_id_for_intent(intent: &NormalizedIntent) -> String {
    let slots = intent
        .slots
        .iter()
        .map(|(slot, value)| format!("{slot}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("read-intent:{}:{slots}", intent.intent)
}

fn read_only_safe_by(action: &ActionContract) -> &'static str {
    if read_only_side_effect(&action.side_effect_class) {
        "contract_side_effect_class"
    } else {
        "semantic_intent_side_effect_class"
    }
}

fn side_effect_payload(side_effect_class: &SideEffectClass) -> Value {
    serde_json::to_value(side_effect_class)
        .unwrap_or_else(|_| json!(format!("{side_effect_class:?}")))
}

fn intent_source_label(source: &IntentSource) -> &'static str {
    match source {
        IntentSource::Direct => "direct",
    }
}

/// Builds a compact audit snapshot for a plan node.
pub(crate) fn audit_action(node: &crate::graph::PlanNode) -> Value {
    json!({
        "node_id": node.id.0,
        "label": node.label.clone(),
        "action": node.action_ref.as_ref().map(|action_ref| {
            json!({
                "contract_id": action_ref.contract_id.clone(),
                "action_name": action_ref.action_name.clone(),
            })
        }),
        "payload": node.payload.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, ExtractorLiteralSpec,
        ExtractorSlotSpec, Idempotency, InMemoryContractRegistry, IntentExtractorSpec,
        Reversibility, RiskLevel, SemanticIntentSpec, SideEffectClass, TrustLevel,
        VerificationSpec,
    };
    use crate::graph::{ActionRef, ActionScores, PlanNode};
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
            risk_level: RiskLevel::Low,
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
            "Read" => vec![intent("read_file", [("path", "file_path")])],
            "Grep" => vec![intent(
                "search_text",
                [("pattern", "pattern"), ("path", "path")],
            )],
            "Glob" => vec![intent(
                "glob_paths",
                [("pattern", "pattern"), ("path", "path")],
            )],
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
        match name {
            "Bash" => vec![
                extractor("read_file", "cat", [("path", 0)], []),
                extractor("search_text", "grep", [("pattern", 0), ("path", 1)], []),
                extractor(
                    "glob_paths",
                    "find",
                    [("path", 0), ("pattern", 2)],
                    [(1, "-name")],
                ),
            ],
            _ => Vec::new(),
        }
    }

    fn extractor<const S: usize, const L: usize>(
        intent: &str,
        command: &str,
        slots: [(&'static str, usize); S],
        literals: [(usize, &'static str); L],
    ) -> IntentExtractorSpec {
        IntentExtractorSpec {
            intent: intent.to_string(),
            parser: "simple_shell".to_string(),
            source_arg: "command".to_string(),
            pattern: None,
            command: command.to_string(),
            slots: slots
                .into_iter()
                .map(|(name, position)| ExtractorSlotSpec {
                    name: name.to_string(),
                    position,
                })
                .collect(),
            optional_slots: Vec::new(),
            slot_groups: std::collections::BTreeMap::new(),
            optional_slot_groups: std::collections::BTreeMap::new(),
            literals: literals
                .into_iter()
                .map(|(position, value)| ExtractorLiteralSpec {
                    position,
                    value: value.to_string(),
                })
                .collect(),
            side_effect_class: Some(SideEffectClass::LocalRead),
        }
    }

    fn action_node(name: &str, payload: Value) -> PlanNode {
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
    fn read_does_not_replace_bash_command_without_structural_intent() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::PureObservation),
            action("Bash", SideEffectClass::Unknown),
        ]);
        let mut graph = PlanGraph::new();
        let read_id = graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        let bash_id = graph.add_node(action_node("Bash", json!({"command": "cat src/lib.rs"})));

        let report = saturate_guarded_substitutions(&mut graph, &registry).unwrap();

        assert_eq!(report.can_replace_edges, 0);
        assert_eq!(report.pruned_actions, 0);
        assert!(!graph.has_edge(read_id, bash_id, PlanEdgeKind::CanReplace));
        assert_eq!(graph.node(bash_id).unwrap().status, PlanStatus::Open);
    }

    #[test]
    fn guarded_edge_payload_records_class_proof_and_dominance() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::PureObservation),
            read_capable_bash(),
        ]);
        let mut graph = PlanGraph::new();
        let read_id = graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        let bash_id = graph.add_node(action_node("Bash", json!({"file_path": "src/lib.rs"})));

        let changes = add_guarded_substitution_edges(&mut graph, &registry).unwrap();

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
        assert_eq!(
            edge.payload["equivalence_class"]["kind"],
            json!("normalized_read_only_intent")
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
            json!("stable_node_order_tiebreaker")
        );
        assert_eq!(
            edge.payload["proof"]["read_only_safe_alternatives"][1]["safe_by"],
            json!("semantic_intent_side_effect_class")
        );
    }

    #[test]
    fn grep_does_not_replace_bash_command_without_structural_intent() {
        let registry = registry_with(vec![
            action("Grep", SideEffectClass::PureObservation),
            action("Bash", SideEffectClass::Unknown),
        ]);
        let mut graph = PlanGraph::new();
        let grep_id = graph.add_node(action_node(
            "Grep",
            json!({"pattern": "PlanGraph", "path": "crates/puffer-burbot/src"}),
        ));
        let bash_id = graph.add_node(action_node(
            "Bash",
            json!({"command": "grep PlanGraph crates/puffer-burbot/src"}),
        ));

        let changes = add_guarded_substitution_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
        assert!(!graph.has_edge(grep_id, bash_id, PlanEdgeKind::CanReplace));
    }

    #[test]
    fn glob_does_not_replace_bash_command_without_structural_intent() {
        let registry = registry_with(vec![
            action("Glob", SideEffectClass::PureObservation),
            action("Bash", SideEffectClass::Unknown),
        ]);
        let mut graph = PlanGraph::new();
        let glob_id = graph.add_node(action_node(
            "Glob",
            json!({"pattern": "*.rs", "path": "crates"}),
        ));
        let bash_id = graph.add_node(action_node(
            "Bash",
            json!({"command": "find crates -name '*.rs'"}),
        ));

        let changes = add_guarded_substitution_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
        assert!(!graph.has_edge(glob_id, bash_id, PlanEdgeKind::CanReplace));
    }

    #[test]
    fn blocks_unknown_non_read_bash_command() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::PureObservation),
            action("Bash", SideEffectClass::Unknown),
        ]);
        let mut graph = PlanGraph::new();
        graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        graph.add_node(action_node(
            "Bash",
            json!({"command": "cat src/lib.rs > /tmp/copy"}),
        ));

        let changes = add_guarded_substitution_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
    }

    #[test]
    fn preserves_protected_scheduling_edges() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::PureObservation),
            read_capable_bash(),
        ]);
        let mut graph = PlanGraph::new();
        let read_id = graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        let bash_id = graph.add_node(action_node("Bash", json!({"file_path": "src/lib.rs"})));
        graph.add_edge(read_id, bash_id, PlanEdgeKind::DependsOn, json!({}));

        let changes = add_guarded_substitution_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
    }

    #[test]
    fn ignored_extractors_do_not_create_self_replacement() {
        let mut read = action("Read", SideEffectClass::PureObservation);
        read.intent_extractors = vec![extractor("read_file", "cat", [("path", 0)], [])];
        let registry = registry_with(vec![read]);
        let mut graph = PlanGraph::new();
        let read_id = graph.add_node(action_node(
            "Read",
            json!({"file_path": "src/lib.rs", "command": "cat src/lib.rs"}),
        ));

        let changes = add_guarded_substitution_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
        assert!(!graph.has_edge(read_id, read_id, PlanEdgeKind::CanReplace));
    }

    #[test]
    fn blocks_write_side_effects_even_with_matching_payload_shape() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::LocalWrite),
            action("Bash", SideEffectClass::Unknown),
        ]);
        let mut graph = PlanGraph::new();
        graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        graph.add_node(action_node("Bash", json!({"command": "cat src/lib.rs"})));

        let changes = add_guarded_substitution_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
    }

    #[test]
    fn does_not_duplicate_existing_can_replace_edge() {
        let registry = registry_with(vec![
            action("Read", SideEffectClass::PureObservation),
            read_capable_bash(),
        ]);
        let mut graph = PlanGraph::new();
        let read_id = graph.add_node(action_node("Read", json!({"file_path": "src/lib.rs"})));
        let bash_id = graph.add_node(action_node("Bash", json!({"file_path": "src/lib.rs"})));
        graph.add_edge(read_id, bash_id, PlanEdgeKind::CanReplace, json!({}));

        let changes = add_guarded_substitution_edges(&mut graph, &registry).unwrap();

        assert_eq!(changes, 0);
    }
}
