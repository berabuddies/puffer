use crate::contract::{ActionContract, ContractRegistry, SideEffectClass};
use crate::graph::{
    scores_for_action, ActionRef, PlanEdgeKind, PlanGraph, PlanNode, PlanNodeKind, PlanStatus,
};
use crate::ids::{NodeId, RunId};
use crate::semantics::semantic_symbol;
use crate::trace::{trace_event, TraceEvent, TraceEventType};
use crate::verification::executable_verification_templates_for;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) fn ensure_existing_file_read_precondition(
    run_id: RunId,
    graph: &mut PlanGraph,
    contracts: &dyn ContractRegistry,
    workspace_root: &Path,
    full_file_read_epochs: &HashMap<String, u64>,
    state_epoch: u64,
    target_id: NodeId,
) -> Result<Option<TraceEvent>> {
    let target = graph.node(target_id)?.clone();
    let Some(action_ref) = target.action_ref.as_ref() else {
        return Ok(None);
    };
    let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
    else {
        return Ok(None);
    };
    let Some(precondition) =
        read_precondition_for_action(contracts, &action, &target.payload, workspace_root)
    else {
        return Ok(None);
    };
    let Some(path_key) = existing_file_key(&precondition.path) else {
        return Ok(None);
    };
    if full_file_read_epochs
        .get(&path_key)
        .is_some_and(|epoch| *epoch == state_epoch)
        || graph_has_read_dependency(graph, target_id, &precondition)
    {
        return Ok(None);
    }

    let Some(read_action) = contracts.get_action(
        &precondition.action_ref.contract_id,
        &precondition.action_ref.action_name,
    ) else {
        return Ok(None);
    };
    let mut scores = scores_for_action(&read_action);
    scores.expected_progress = scores.expected_progress.max(0.95);
    scores.information_gain = scores.information_gain.max(1.0);
    scores.verification_value = scores.verification_value.max(0.35);
    let read_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: format!(
            "{}.{}",
            precondition.action_ref.contract_id, precondition.action_ref.action_name
        ),
        payload: precondition.args.clone(),
        action_ref: Some(precondition.action_ref.clone()),
        scores,
    });
    graph.add_edge(
        read_id,
        target_id,
        PlanEdgeKind::DependsOn,
        json!({"reason": "existing file full-read precondition"}),
    );
    graph.add_edge(
        read_id,
        target_id,
        PlanEdgeKind::PreconditionFor,
        json!({"reason": "existing file full-read precondition"}),
    );
    graph.add_edge(
        NodeId(0),
        read_id,
        PlanEdgeKind::Supports,
        json!({
            "source": "contract_precondition",
            "completion_role": "support",
            "rationale": "read existing target before a contract-declared file write",
            "target_node": target_id.0,
        }),
    );

    let mut event = trace_event(
        run_id,
        TraceEventType::NodeAdded,
        Some(read_id),
        json!({}),
        json!({
            "kind": "action",
            "source": "contract_precondition",
            "completion_role": "support",
            "rationale": "read existing target before a contract-declared file write",
            "target_node": target_id.0,
        }),
    );
    event.contract_id = Some(precondition.action_ref.contract_id);
    event.action_name = Some(precondition.action_ref.action_name);
    Ok(Some(event))
}

pub(super) fn add_failed_file_write_refresh(
    run_id: RunId,
    graph: &mut PlanGraph,
    contracts: &dyn ContractRegistry,
    workspace_root: &Path,
    failure_id: NodeId,
    failed_action_id: NodeId,
    action_ref: &ActionRef,
    args: &Value,
) -> Result<Option<TraceEvent>> {
    let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
    else {
        return Ok(None);
    };
    let Some(refresh) = read_precondition_for_action(contracts, &action, args, workspace_root)
    else {
        return Ok(None);
    };
    if existing_file_key(&refresh.path).is_none()
        || graph_has_failure_refresh(graph, failure_id, &refresh)
    {
        return Ok(None);
    }
    let Some(read_action) = contracts.get_action(
        &refresh.action_ref.contract_id,
        &refresh.action_ref.action_name,
    ) else {
        return Ok(None);
    };
    let mut scores = scores_for_action(&read_action);
    scores.information_gain = scores.information_gain.max(1.2);
    scores.expected_progress = scores.expected_progress.max(0.75);
    scores.verification_value = scores.verification_value.max(0.45);

    let read_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: format!(
            "{}.{}",
            refresh.action_ref.contract_id, refresh.action_ref.action_name
        ),
        payload: refresh.args.clone(),
        action_ref: Some(refresh.action_ref.clone()),
        scores,
    });
    graph.add_edge(
        failure_id,
        read_id,
        PlanEdgeKind::Supports,
        json!({
            "source": "failure_refresh",
            "completion_role": "support",
            "rationale": "read declared file target after a failed file write",
            "failed_action": failed_action_id.0,
        }),
    );

    let mut event = trace_event(
        run_id,
        TraceEventType::NodeAdded,
        Some(read_id),
        json!({}),
        json!({
            "kind": "action",
            "source": "failure_refresh",
            "completion_role": "support",
            "rationale": "read declared file target after a failed file write",
            "failed_action": failed_action_id.0,
            "failure_node": failure_id.0,
        }),
    );
    event.contract_id = Some(refresh.action_ref.contract_id);
    event.action_name = Some(refresh.action_ref.action_name);
    Ok(Some(event))
}

pub(super) fn record_full_file_read_epoch(
    contracts: &dyn ContractRegistry,
    action_ref: &ActionRef,
    args: &Value,
    success: bool,
    state_epoch: u64,
    workspace_root: &Path,
    full_file_read_epochs: &mut HashMap<String, u64>,
) {
    if !success {
        return;
    }
    let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
    else {
        return;
    };
    if reads_file(&action) {
        if let Some(path) = args.get("file_path").and_then(Value::as_str) {
            if full_read_request(args) {
                record_existing_path(path, workspace_root, state_epoch, full_file_read_epochs);
            } else {
                remove_existing_path(path, workspace_root, full_file_read_epochs);
            }
        }
        return;
    }
}

struct ReadPrecondition {
    action_ref: ActionRef,
    args: Value,
    path: PathBuf,
}

fn read_precondition_for_action(
    contracts: &dyn ContractRegistry,
    action: &ActionContract,
    args: &Value,
    workspace_root: &Path,
) -> Option<ReadPrecondition> {
    if !write_like_side_effect(&action.side_effect_class) {
        return None;
    }
    executable_verification_templates_for(action, args, &json!({}))
        .into_iter()
        .find_map(|template| {
            let read_action = contracts.get_action(&template.contract_id, &template.action_name)?;
            reads_file(&read_action).then_some(())?;
            let path = template.args.get("file_path")?.as_str()?.to_string();
            if path.contains('{') || path.trim().is_empty() {
                return None;
            }
            Some(ReadPrecondition {
                action_ref: ActionRef {
                    contract_id: template.contract_id,
                    action_name: template.action_name,
                },
                args: strip_partial_read_args(template.args),
                path: resolve_path(workspace_root, &path),
            })
        })
}

fn graph_has_read_dependency(
    graph: &PlanGraph,
    target_id: NodeId,
    precondition: &ReadPrecondition,
) -> bool {
    graph.edges.iter().any(|edge| {
        edge.target == target_id
            && edge.kind == PlanEdgeKind::DependsOn
            && graph.nodes.get(&edge.source).is_some_and(|node| {
                node.action_ref.as_ref() == Some(&precondition.action_ref)
                    && same_file_path(&node.payload, &precondition.args)
                    && matches!(node.status, PlanStatus::Open | PlanStatus::Executed)
            })
    })
}

fn graph_has_failure_refresh(
    graph: &PlanGraph,
    failure_id: NodeId,
    refresh: &ReadPrecondition,
) -> bool {
    graph.edges.iter().any(|edge| {
        edge.source == failure_id
            && edge.kind == PlanEdgeKind::Supports
            && edge.payload.get("source").and_then(Value::as_str) == Some("failure_refresh")
            && graph.nodes.get(&edge.target).is_some_and(|node| {
                node.action_ref.as_ref() == Some(&refresh.action_ref)
                    && same_file_path(&node.payload, &refresh.args)
                    && matches!(node.status, PlanStatus::Open | PlanStatus::Executed)
            })
    })
}

fn same_file_path(left: &Value, right: &Value) -> bool {
    left.get("file_path").and_then(Value::as_str) == right.get("file_path").and_then(Value::as_str)
}

fn record_existing_path(
    path: &str,
    workspace_root: &Path,
    state_epoch: u64,
    full_file_read_epochs: &mut HashMap<String, u64>,
) {
    let path = resolve_path(workspace_root, path);
    if let Some(path_key) = existing_file_key(&path) {
        full_file_read_epochs.insert(path_key, state_epoch);
    }
}

fn remove_existing_path(
    path: &str,
    workspace_root: &Path,
    full_file_read_epochs: &mut HashMap<String, u64>,
) {
    let path = resolve_path(workspace_root, path);
    if let Some(path_key) = existing_file_key(&path) {
        full_file_read_epochs.remove(&path_key);
    }
}

fn existing_file_key(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    Some(
        path.canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string(),
    )
}

fn resolve_path(workspace_root: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

fn strip_partial_read_args(mut args: Value) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.remove("offset");
        object.remove("limit");
        object.remove("pages");
    }
    args
}

fn full_read_request(args: &Value) -> bool {
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let limit_present = present(args, "limit");
    let pages_present = present(args, "pages");
    offset <= 1 && !limit_present && !pages_present
}

fn present(args: &Value, key: &str) -> bool {
    !matches!(args.get(key), None | Some(Value::Null))
}

fn reads_file(action: &ActionContract) -> bool {
    action
        .semantic_intents
        .iter()
        .any(|intent| semantic_symbol(&intent.intent) == "read_file")
}

fn write_like_side_effect(side_effect_class: &SideEffectClass) -> bool {
    matches!(
        side_effect_class,
        SideEffectClass::LocalWrite | SideEffectClass::DestructiveWrite
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ApprovalSpec, CapabilityContract, ContractStatus, Idempotency, InMemoryContractRegistry,
        Reversibility, RiskLevel, SemanticIntentSpec, TrustLevel, VerificationSpec,
        VerificationTemplateSpec,
    };
    use serde_json::json;
    use tempfile::tempdir;

    fn registry() -> InMemoryContractRegistry {
        let mut registry = InMemoryContractRegistry::default();
        registry
            .register(CapabilityContract {
                contract_id: "puffer.tools".to_string(),
                version: "0.1.0".to_string(),
                status: ContractStatus::Active,
                trust_level: TrustLevel::Sandboxed,
                description: "tools".to_string(),
                actions: vec![read_action(), write_action()],
                global_constraints: Vec::new(),
                forbidden_uses: Vec::new(),
                local_rules: Vec::new(),
                examples: Vec::new(),
                contract_hash: None,
            })
            .unwrap();
        registry
    }

    fn read_action() -> ActionContract {
        let mut slots = std::collections::BTreeMap::new();
        slots.insert("path".to_string(), "file_path".to_string());
        action(
            "Read",
            SideEffectClass::LocalRead,
            vec![SemanticIntentSpec {
                intent: "read_file".to_string(),
                slots,
                optional_slots: Default::default(),
                defaults: Default::default(),
                side_effect_class: None,
                slot_kinds: Default::default(),
            }],
            Vec::new(),
        )
    }

    fn write_action() -> ActionContract {
        action(
            "Write",
            SideEffectClass::LocalWrite,
            Vec::new(),
            vec![VerificationTemplateSpec {
                contract_id: "puffer.tools".to_string(),
                action_name: "Read".to_string(),
                args: json!({"file_path": "{arg.file_path}"}),
                reason: Some("read written file".to_string()),
            }],
        )
    }

    fn action(
        name: &str,
        side_effect_class: SideEffectClass,
        semantic_intents: Vec<SemanticIntentSpec>,
        templates: Vec<VerificationTemplateSpec>,
    ) -> ActionContract {
        ActionContract {
            name: name.to_string(),
            description: name.to_string(),
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
                observation_checks: Vec::new(),
                method_templates: Vec::new(),
                templates,
                required_before_completion: true,
                confidence: 0.5,
            },
            approval: ApprovalSpec {
                required: false,
                reason: None,
            },
            failure_modes: Vec::new(),
            forbidden_uses: Vec::new(),
            argument_safety: Vec::new(),
            structured_argument_safety: Vec::new(),
            semantic_intents,
            intent_extractors: Vec::new(),
            repair_rules: Vec::new(),
            cost_estimate: None,
            latency_estimate: None,
        }
    }

    fn target_node(file_path: &str) -> PlanNode {
        PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: "Write".to_string(),
            payload: json!({"file_path": file_path, "content": "new"}),
            action_ref: Some(ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: "Write".to_string(),
            }),
            scores: Default::default(),
        }
    }

    #[test]
    fn inserts_full_read_dependency_for_existing_file_write() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("target.txt");
        std::fs::write(&path, "old").unwrap();
        let mut graph = PlanGraph::from_goal("goal".to_string());
        let target = graph.add_node(target_node(path.to_str().unwrap()));
        let epochs = HashMap::new();

        let event = ensure_existing_file_read_precondition(
            RunId::new(),
            &mut graph,
            &registry(),
            temp.path(),
            &epochs,
            0,
            target,
        )
        .unwrap();

        assert!(event.is_some());
        let read = graph
            .edges
            .iter()
            .find(|edge| edge.target == target && edge.kind == PlanEdgeKind::DependsOn)
            .unwrap()
            .source;
        let support = graph
            .edges
            .iter()
            .find(|edge| edge.target == read && edge.kind == PlanEdgeKind::Supports)
            .unwrap();
        assert_eq!(
            graph.node(read).unwrap().payload,
            json!({"file_path": path})
        );
        assert_eq!(support.payload["completion_role"], json!("support"));
        assert_eq!(support.payload["source"], json!("contract_precondition"));
    }

    #[test]
    fn skips_precondition_for_new_file_write() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("new.txt");
        let mut graph = PlanGraph::from_goal("goal".to_string());
        let target = graph.add_node(target_node(path.to_str().unwrap()));

        let event = ensure_existing_file_read_precondition(
            RunId::new(),
            &mut graph,
            &registry(),
            temp.path(),
            &HashMap::new(),
            0,
            target,
        )
        .unwrap();

        assert!(event.is_none());
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn skips_precondition_when_current_epoch_has_full_read() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("target.txt");
        std::fs::write(&path, "old").unwrap();
        let mut graph = PlanGraph::from_goal("goal".to_string());
        let target = graph.add_node(target_node(path.to_str().unwrap()));
        let mut epochs = HashMap::new();
        epochs.insert(existing_file_key(&path).unwrap(), 7);

        let event = ensure_existing_file_read_precondition(
            RunId::new(),
            &mut graph,
            &registry(),
            temp.path(),
            &epochs,
            7,
            target,
        )
        .unwrap();

        assert!(event.is_none());
    }

    #[test]
    fn partial_read_invalidates_full_read_precondition_state() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("target.txt");
        std::fs::write(&path, "old").unwrap();
        let mut epochs = HashMap::new();
        let action_ref = ActionRef {
            contract_id: "puffer.tools".to_string(),
            action_name: "Read".to_string(),
        };

        record_full_file_read_epoch(
            &registry(),
            &action_ref,
            &json!({"file_path": path}),
            true,
            3,
            temp.path(),
            &mut epochs,
        );
        assert_eq!(epochs.get(&existing_file_key(&path).unwrap()), Some(&3));

        record_full_file_read_epoch(
            &registry(),
            &action_ref,
            &json!({"file_path": path, "offset": 10, "limit": 1}),
            true,
            4,
            temp.path(),
            &mut epochs,
        );
        assert!(epochs.is_empty());
    }

    #[test]
    fn write_action_does_not_satisfy_future_full_read_precondition() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("target.txt");
        std::fs::write(&path, "old").unwrap();
        let mut epochs = HashMap::new();
        let action_ref = ActionRef {
            contract_id: "puffer.tools".to_string(),
            action_name: "Write".to_string(),
        };

        record_full_file_read_epoch(
            &registry(),
            &action_ref,
            &json!({"file_path": path, "content": "new"}),
            true,
            5,
            temp.path(),
            &mut epochs,
        );

        assert!(epochs.is_empty());
    }

    #[test]
    fn failed_file_write_adds_fresh_target_read() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("target.txt");
        std::fs::write(&path, "old").unwrap();
        let mut graph = PlanGraph::from_goal("goal".to_string());
        let failed_action = graph.add_node(target_node(path.to_str().unwrap()));
        let failure = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Failure,
            status: PlanStatus::Open,
            label: "failure".to_string(),
            payload: json!({}),
            action_ref: None,
            scores: Default::default(),
        });
        let action_ref = ActionRef {
            contract_id: "puffer.tools".to_string(),
            action_name: "Write".to_string(),
        };

        let event = add_failed_file_write_refresh(
            RunId::new(),
            &mut graph,
            &registry(),
            temp.path(),
            failure,
            failed_action,
            &action_ref,
            &json!({"file_path": path, "content": "new"}),
        )
        .unwrap();

        assert!(event.is_some());
        let edge = graph
            .edges
            .iter()
            .find(|edge| edge.source == failure && edge.kind == PlanEdgeKind::Supports)
            .unwrap();
        let read = graph.node(edge.target).unwrap();
        assert_eq!(
            read.action_ref.as_ref().unwrap().action_name,
            "Read".to_string()
        );
        assert_eq!(read.payload, json!({"file_path": path}));
        assert_eq!(edge.payload["source"], json!("failure_refresh"));
        assert_eq!(edge.payload["completion_role"], json!("support"));
    }

    #[test]
    fn failed_file_write_refresh_is_not_duplicated_for_same_failure() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("target.txt");
        std::fs::write(&path, "old").unwrap();
        let mut graph = PlanGraph::from_goal("goal".to_string());
        let failed_action = graph.add_node(target_node(path.to_str().unwrap()));
        let failure = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Failure,
            status: PlanStatus::Open,
            label: "failure".to_string(),
            payload: json!({}),
            action_ref: None,
            scores: Default::default(),
        });
        let action_ref = ActionRef {
            contract_id: "puffer.tools".to_string(),
            action_name: "Write".to_string(),
        };
        let args = json!({"file_path": path, "content": "new"});

        let first = add_failed_file_write_refresh(
            RunId::new(),
            &mut graph,
            &registry(),
            temp.path(),
            failure,
            failed_action,
            &action_ref,
            &args,
        )
        .unwrap();
        let second = add_failed_file_write_refresh(
            RunId::new(),
            &mut graph,
            &registry(),
            temp.path(),
            failure,
            failed_action,
            &action_ref,
            &args,
        )
        .unwrap();

        assert!(first.is_some());
        assert!(second.is_none());
    }
}
