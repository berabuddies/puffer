use super::model_feedback::{invalid_candidate_context, non_advancing_candidate_context};
use super::model_loop_support::ModelCandidateProposalOutcome;
use super::*;
use crate::contract::{
    ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, Idempotency, Reversibility,
    RiskLevel, SemanticIntentSpec, SideEffectClass, TrustLevel, VerificationSpec,
};
use crate::graph::{ActionRef, ActionScores, PlanEdgeKind, PlanNode, PlanNodeKind, PlanStatus};
use crate::llm::ModelCandidateProposal;
use crate::model_policy::{ModelProposalViolation, MAX_MODEL_EXECUTABLE_ARG_CHARS};
use crate::planner::{CandidateSource, CompletionRole};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use serde_json::json;
use std::collections::BTreeMap;

fn read_action() -> ActionContract {
    let mut slots = BTreeMap::new();
    slots.insert("path".to_string(), "file_path".to_string());
    ActionContract {
        name: "Read".to_string(),
        description: "read file".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "note": {"type": "string"}
            },
            "required": ["file_path"]
        }),
        output_schema: json!({"type": "object"}),
        side_effect_class: SideEffectClass::LocalRead,
        reversibility: Reversibility::Reversible,
        idempotency: Idempotency::Idempotent,
        risk_level: RiskLevel::Low,
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
        failure_modes: Vec::new(),
        forbidden_uses: Vec::new(),
        argument_safety: Vec::new(),
        structured_argument_safety: Vec::new(),
        semantic_intents: vec![SemanticIntentSpec {
            intent: "read_file".to_string(),
            slots,
            optional_slots: BTreeMap::new(),
            defaults: BTreeMap::new(),
            side_effect_class: Some(SideEffectClass::LocalRead),
            slot_kinds: Default::default(),
        }],
        intent_extractors: Vec::new(),
        repair_rules: Vec::new(),
        cost_estimate: None,
        latency_estimate: None,
    }
}

fn shell_action() -> ActionContract {
    let mut slots = BTreeMap::new();
    slots.insert("command".to_string(), "command".to_string());
    ActionContract {
        name: "Bash".to_string(),
        description: "run shell".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "description": {"type": "string"}
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        output_schema: json!({"type": "object"}),
        side_effect_class: SideEffectClass::Unknown,
        reversibility: Reversibility::Unknown,
        idempotency: Idempotency::Unknown,
        risk_level: RiskLevel::High,
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
        failure_modes: Vec::new(),
        forbidden_uses: Vec::new(),
        argument_safety: Vec::new(),
        structured_argument_safety: Vec::new(),
        semantic_intents: vec![SemanticIntentSpec {
            intent: "shell_command".to_string(),
            slots,
            optional_slots: BTreeMap::new(),
            defaults: BTreeMap::new(),
            side_effect_class: Some(SideEffectClass::Unknown),
            slot_kinds: Default::default(),
        }],
        intent_extractors: Vec::new(),
        repair_rules: Vec::new(),
        cost_estimate: None,
        latency_estimate: None,
    }
}

fn registry() -> InMemoryContractRegistry {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![read_action()],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    registry
}

fn shell_registry() -> InMemoryContractRegistry {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![shell_action()],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    registry
}

fn read_shell_registry() -> InMemoryContractRegistry {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![read_action(), shell_action()],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    registry
}

#[test]
fn invalid_candidate_context_uses_structured_feedback_without_payload() {
    let context = json!({"phase": "after_observation"});
    let retry_context = invalid_candidate_context(
        &context,
        "observe_act",
        &[ModelProposalViolation {
            code: "executable_arg_char_limit_exceeded",
            path: "candidates[0].args.command".to_string(),
            tool_id: Some("Bash".to_string()),
            actual: Some((MAX_MODEL_EXECUTABLE_ARG_CHARS + 1) as u64),
            limit: Some(MAX_MODEL_EXECUTABLE_ARG_CHARS as u64),
            repair_hint: "decompose generated programs or long command payloads into file/artifact actions and bounded execution",
        }],
    );

    assert_eq!(
        retry_context["phase"],
        json!("after_invalid_model_candidates")
    );
    assert_eq!(
        retry_context["invalid_prior_candidates"][0]["code"],
        json!("executable_arg_char_limit_exceeded")
    );
    assert!(retry_context.to_string().contains("repair_hint"));
    assert!(retry_context.to_string().contains("raw JSON object only"));
    assert!(retry_context
        .to_string()
        .contains("no markdown fences or prose"));
    assert!(!retry_context.to_string().contains("printf x"));
}

#[test]
fn non_advancing_candidate_context_reports_duplicate_candidates_structurally() {
    let context = json!({"phase": "after_observation"});
    let outcome = ModelCandidateProposalOutcome {
        added: 0,
        proposed: 1,
        skipped_wait: 0,
        skipped_duplicate: 1,
        skipped_dependency: 0,
        skipped_policy: 0,
        skipped_duplicate_candidates: vec![json!({
            "tool_id": "Read",
            "args": {"file_path": "/tmp/input.txt"},
            "reason": "already_open_or_executed_in_current_state_epoch",
        })],
        skipped_policy_candidates: Vec::new(),
    };

    let retry_context = non_advancing_candidate_context(&context, "observe_act", &outcome);

    assert_eq!(
        retry_context["phase"],
        json!("after_non_advancing_model_candidates")
    );
    assert_eq!(
        retry_context["non_advancing_prior_candidates"][0]["code"],
        json!("no_new_model_candidates")
    );
    assert_eq!(
        retry_context["non_advancing_prior_candidates"][0]["skipped_duplicate"],
        json!(1)
    );
    assert_eq!(
        retry_context["non_advancing_prior_candidates"][0]["duplicate_candidates"][0]["tool_id"],
        json!("Read")
    );
}

#[test]
fn model_candidates_dedupe_open_actions_by_canonical_contract_key() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime = BurbotRuntime::new(registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("read once".to_string());
    let first = ModelCandidateProposal {
        id: Some("first".to_string()),
        tool_id: "Read".to_string(),
        args: json!({"file_path": "/tmp/input.txt", "note": "first"}),
        completion_role: CompletionRole::Support,
        depends_on: Vec::new(),
        rationale: "read input".to_string(),
    };
    let second = ModelCandidateProposal {
        id: Some("second".to_string()),
        tool_id: "Read".to_string(),
        args: json!({"file_path": "/tmp/input.txt", "note": "second"}),
        completion_role: CompletionRole::Support,
        depends_on: Vec::new(),
        rationale: "read same input".to_string(),
    };

    let added_first = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelObservationProposal,
            vec![first],
        )
        .unwrap();
    let added_second = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelObservationProposal,
            vec![second],
        )
        .unwrap();

    assert_eq!(added_first.added, 1);
    assert_eq!(added_second.added, 0);
    assert_eq!(added_second.proposed, 1);
    assert_eq!(added_second.skipped_duplicate, 1);
    assert_eq!(
        added_second.skipped_duplicate_candidates[0]["tool_id"],
        json!("Read")
    );
}

#[test]
fn model_candidates_dedupe_partial_read_after_full_semantic_read() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime = BurbotRuntime::new(registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("read once".to_string());
    let full = ModelCandidateProposal {
        id: Some("full".to_string()),
        tool_id: "Read".to_string(),
        args: json!({"file_path": "/tmp/input.txt"}),
        completion_role: CompletionRole::Support,
        depends_on: Vec::new(),
        rationale: "read whole input".to_string(),
    };
    let partial = ModelCandidateProposal {
        id: Some("partial".to_string()),
        tool_id: "Read".to_string(),
        args: json!({"file_path": "/tmp/input.txt", "offset": 10, "limit": 5}),
        completion_role: CompletionRole::Support,
        depends_on: Vec::new(),
        rationale: "read slice already covered by full read".to_string(),
    };

    let added_full = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelObservationProposal,
            vec![full],
        )
        .unwrap();
    let added_partial = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelObservationProposal,
            vec![partial],
        )
        .unwrap();

    assert_eq!(added_full.added, 1);
    assert_eq!(added_partial.added, 0);
    assert_eq!(added_partial.skipped_duplicate, 1);
}

#[test]
fn no_progress_unknown_repair_gate_blocks_same_args_not_different_args() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime = BurbotRuntime::new(shell_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("recover".to_string());
    let action_ref = ActionRef {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        action_name: "Bash".to_string(),
    };
    let failed_args = json!({"command": "cd /app && cat pairs.txt | wc -l"});
    let failed_action = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Failed,
        label: "failed bash".to_string(),
        payload: failed_args.clone(),
        action_ref: Some(action_ref.clone()),
        scores: ActionScores::default(),
    });
    let failure = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Failure,
        status: PlanStatus::Open,
        label: "no progress".to_string(),
        payload: json!({
            "contract_id": PUFFER_TOOLS_CONTRACT_ID,
            "action_name": "Bash",
            "args": failed_args,
            "failure_mode": "no_progress",
            "completion_role": "repair",
            "state_epoch": runtime.state_epoch,
        }),
        action_ref: None,
        scores: ActionScores::default(),
    });
    graph.add_edge(failed_action, failure, PlanEdgeKind::FailedWith, json!({}));

    let same = ModelCandidateProposal {
        id: Some("same".to_string()),
        tool_id: "Bash".to_string(),
        args: json!({"command": "cd /app && cat pairs.txt | wc -l"}),
        completion_role: CompletionRole::Repair,
        depends_on: Vec::new(),
        rationale: "repeat probe".to_string(),
    };
    let different = ModelCandidateProposal {
        id: Some("different".to_string()),
        tool_id: "Bash".to_string(),
        args: json!({"command": "cd /app && ls -la"}),
        completion_role: CompletionRole::Repair,
        depends_on: Vec::new(),
        rationale: "different bounded probe".to_string(),
    };

    let outcome = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelObservationProposal,
            vec![same, different],
        )
        .unwrap();

    assert_eq!(outcome.added, 1);
    assert_eq!(outcome.skipped_policy, 1);
    assert_eq!(
        outcome.skipped_policy_candidates[0]["reason"],
        json!("unknown_side_effect_repair_no_progress_until_state_changes")
    );
}

#[test]
fn quarantined_artifact_blocks_unknown_actions_but_allows_structured_reads() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(read_shell_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("repair rejected artifact".to_string());
    let artifact = workspace.path().join("linear_attack.py");
    let action_ref = ActionRef {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        action_name: "Bash".to_string(),
    };
    runtime.quarantine_artifact_paths(
        &action_ref,
        &json!({}),
        &json!({"filesystem_witness": {"created": [artifact.display().to_string()]}}),
    );

    let unknown = ModelCandidateProposal {
        id: Some("run".to_string()),
        tool_id: "Bash".to_string(),
        args: json!({"command": "python3 /app/linear_attack.py"}),
        completion_role: CompletionRole::Repair,
        depends_on: Vec::new(),
        rationale: "execute current artifact".to_string(),
    };
    let read = ModelCandidateProposal {
        id: Some("read".to_string()),
        tool_id: "Read".to_string(),
        args: json!({"file_path": artifact.display().to_string()}),
        completion_role: CompletionRole::Support,
        depends_on: Vec::new(),
        rationale: "inspect current artifact structurally".to_string(),
    };

    let outcome = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelObservationProposal,
            vec![unknown, read],
        )
        .unwrap();

    assert_eq!(outcome.added, 1);
    assert_eq!(outcome.skipped_policy, 1);
    assert_eq!(
        outcome.skipped_policy_candidates[0]["reason"],
        json!("unknown_side_effect_blocked_by_quarantined_artifact")
    );
}

#[test]
fn fresh_read_survives_unrelated_state_change_until_source_path_changes() {
    let workspace = tempfile::tempdir().unwrap();
    let input_path = workspace.path().join("input.txt");
    let output_path = workspace.path().join("output.txt");
    std::fs::write(&input_path, "input").unwrap();
    std::fs::write(&output_path, "output").unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime = BurbotRuntime::new(registry(), workspace.path().into(), trace).unwrap();
    let graph = PlanGraph::from_goal("read once".to_string());
    let action_ref = ActionRef {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        action_name: "Read".to_string(),
    };
    let args = json!({"file_path": input_path.display().to_string()});

    runtime.record_fresh_read_action(&action_ref, &args);
    runtime.state_epoch += 1;
    runtime.invalidate_fresh_reads_for_observation(&json!({
        "structured_output": {"filePath": output_path.display().to_string()}
    }));

    assert!(runtime.has_current_action_candidate(&graph, &action_ref, &args));

    runtime.invalidate_fresh_reads_for_observation(&json!({
        "filesystem_witness": {"modified": [input_path.display().to_string()]}
    }));

    assert!(!runtime.has_current_action_candidate(&graph, &action_ref, &args));
}

#[test]
fn fresh_read_can_refresh_once_after_newer_failure_context() {
    let workspace = tempfile::tempdir().unwrap();
    let input_path = workspace.path().join("input.txt");
    std::fs::write(&input_path, "input").unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime = BurbotRuntime::new(registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("repair after failure".to_string());
    let action_ref = ActionRef {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        action_name: "Read".to_string(),
    };
    let args = json!({"file_path": input_path.display().to_string()});
    let first_read = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "Read".to_string(),
        payload: args.clone(),
        action_ref: Some(action_ref.clone()),
        scores: ActionScores::default(),
    });
    runtime.record_fresh_read_action(&action_ref, &args);
    let failed_action = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Failed,
        label: "Failed check".to_string(),
        payload: json!({"checked": true}),
        action_ref: Some(action_ref.clone()),
        scores: ActionScores::default(),
    });
    let failure = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Failure,
        status: PlanStatus::Open,
        label: "Failure".to_string(),
        payload: json!({"state_epoch": runtime.state_epoch}),
        action_ref: None,
        scores: ActionScores::default(),
    });
    graph.add_edge(failed_action, failure, PlanEdgeKind::FailedWith, json!({}));
    assert!(first_read.0 < failure.0);

    let refresh = ModelCandidateProposal {
        id: Some("refresh".to_string()),
        tool_id: "Read".to_string(),
        args: args.clone(),
        completion_role: CompletionRole::Support,
        depends_on: Vec::new(),
        rationale: "refresh source evidence after failure".to_string(),
    };
    let refreshed = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            failure,
            None,
            CandidateSource::ModelObservationProposal,
            vec![refresh],
        )
        .unwrap();

    assert_eq!(refreshed.added, 1);
    assert_eq!(refreshed.skipped_duplicate, 0);

    let refreshed_read = graph
        .nodes
        .iter()
        .find_map(|(id, node)| {
            (id.0 > failure.0
                && node.kind == PlanNodeKind::Action
                && node.action_ref.as_ref() == Some(&action_ref))
            .then_some(*id)
        })
        .unwrap();
    graph.node_mut(refreshed_read).unwrap().status = PlanStatus::Executed;
    runtime.record_fresh_read_action(&action_ref, &args);

    let duplicate = ModelCandidateProposal {
        id: Some("duplicate".to_string()),
        tool_id: "Read".to_string(),
        args,
        completion_role: CompletionRole::Support,
        depends_on: Vec::new(),
        rationale: "repeat source refresh without a new failure".to_string(),
    };
    let skipped = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            failure,
            None,
            CandidateSource::ModelObservationProposal,
            vec![duplicate],
        )
        .unwrap();

    assert_eq!(skipped.added, 0);
    assert_eq!(skipped.skipped_duplicate, 1);
}
