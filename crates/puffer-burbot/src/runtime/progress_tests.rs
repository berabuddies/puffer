use super::*;
use crate::contract::{
    ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, Idempotency, Reversibility,
    RiskLevel, SideEffectClass, TrustLevel, VerificationSpec,
};
use crate::planner::{CandidateSource, CompletionRole};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use crate::trace::TraceEventType;
use serde_json::json;

fn action(name: &str, risk: RiskLevel) -> ActionContract {
    ActionContract {
        name: name.to_string(),
        description: "act".to_string(),
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        side_effect_class: SideEffectClass::LocalRead,
        reversibility: Reversibility::Reversible,
        idempotency: Idempotency::Idempotent,
        risk_level: risk,
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
        semantic_intents: Vec::new(),
        intent_extractors: Vec::new(),
        repair_rules: Vec::new(),
        cost_estimate: None,
        latency_estimate: None,
    }
}

#[test]
fn model_unknown_terminal_empty_output_records_no_progress() {
    let mut registry = InMemoryContractRegistry::default();
    let mut bash = action("Bash", RiskLevel::High);
    bash.side_effect_class = SideEffectClass::Unknown;
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![bash],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime = BurbotRuntime::new(registry, workspace.path().into(), trace.clone()).unwrap();
    let run_id = RunId::new();
    let mut graph = PlanGraph::from_goal("do work".to_string());
    let node_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "bash".to_string(),
        payload: json!({"command": "opaque"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Bash".to_string(),
        }),
        scores: ActionScores::default(),
    });
    graph.add_edge(
        NodeId(0),
        node_id,
        PlanEdgeKind::Supports,
        json!({"source": "model_proposal", "completion_role": "terminal"}),
    );
    let mut beliefs = BeliefGraph::new();

    runtime
        .process_observation(
            run_id,
            0,
            &mut graph,
            &mut beliefs,
            "do work",
            &RunOptions {
                puffer_tool: None,
                puffer_args: None,
                puffer_tool_source: CandidateSource::ModelProposal,
                allow_failed_terminal_completion: false,
                enable_symbolic_workers: false,
                enable_parallel_read_only: false,
                enable_observe_act_llm: false,
                model: None,
                goal_verification_min_confidence: 0.75,
                yolo: false,
            },
            Observation {
                invocation: ActionInvocation {
                    run_id,
                    plan_node_id: node_id,
                    contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                    action_name: "Bash".to_string(),
                    args: json!({"command": "opaque"}),
                },
                success: true,
                output: json!({
                    "success": true,
                    "tool_id": "Bash",
                    "stdout": "",
                    "stderr": "",
                    "metadata": {},
                    "structured_output": null
                }),
                error: None,
                raw: None,
                latency_ms: None,
                cost: None,
            },
        )
        .unwrap();

    assert_eq!(runtime.state_epoch, 0);
    assert_eq!(graph.node(node_id).unwrap().status, PlanStatus::Failed);
    let events = trace.events_for_run(run_id).unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == TraceEventType::FailureClassified
            && event.failure_mode.as_deref() == Some("no_progress")
    }));
}

#[test]
fn no_output_expected_flag_alone_is_not_progress_evidence() {
    let mut bash = action("Bash", RiskLevel::High);
    bash.side_effect_class = SideEffectClass::Unknown;

    let evidence = progress_evidence(
        Some(&bash),
        &json!({
            "success": true,
            "stdout": "",
            "stderr": "",
            "metadata": {},
            "structured_output": {
                "stdout": "",
                "stderr": "",
                "noOutputExpected": true,
                "interrupted": false,
                "dangerouslyDisableSandbox": false
            }
        }),
    );

    assert!(!evidence.changes_state);
    assert!(!evidence.has_structural_witness);
    assert!(!evidence.has_output_witness);
}

#[test]
fn serialized_empty_tool_output_is_not_progress_evidence() {
    let mut bash = action("Bash", RiskLevel::High);
    bash.side_effect_class = SideEffectClass::Unknown;

    let evidence = progress_evidence(
        Some(&bash),
        &json!({
            "success": true,
            "stdout": "{\n  \"stdout\": \"\",\n  \"stderr\": \"\",\n  \"interrupted\": false,\n  \"dangerouslyDisableSandbox\": false,\n  \"noOutputExpected\": true\n}",
            "stderr": "",
            "metadata": null,
            "structured_output": {
                "stdout": "",
                "stderr": "",
                "interrupted": false,
                "dangerouslyDisableSandbox": false,
                "noOutputExpected": true
            }
        }),
    );

    assert!(!evidence.changes_state);
    assert!(!evidence.has_structural_witness);
    assert!(!evidence.has_output_witness);
}

#[test]
fn arbitrary_structured_text_is_not_progress_evidence() {
    let mut bash = action("Bash", RiskLevel::High);
    bash.side_effect_class = SideEffectClass::Unknown;

    let evidence = progress_evidence(
        Some(&bash),
        &json!({
            "success": true,
            "stdout": "",
            "stderr": "",
            "metadata": null,
            "structured_output": {
                "message": "finished something",
                "summary": "looks useful",
                "stderr": "",
                "stdout": ""
            }
        }),
    );

    assert!(!evidence.changes_state);
    assert!(!evidence.has_structural_witness);
    assert!(!evidence.has_output_witness);
}

#[test]
fn declared_progress_fields_are_structural_progress_evidence() {
    let mut bash = action("Bash", RiskLevel::High);
    bash.side_effect_class = SideEffectClass::Unknown;

    let evidence = progress_evidence(
        Some(&bash),
        &json!({
            "success": true,
            "stdout": "",
            "stderr": "",
            "metadata": null,
            "structured_output": {
                "produced_output": true,
                "artifact_ids": ["artifact-1"]
            }
        }),
    );

    assert!(!evidence.changes_state);
    assert!(evidence.has_structural_witness);
    assert!(evidence.has_output_witness);
}

#[test]
fn explicit_progress_string_fields_are_not_progress_evidence() {
    let mut bash = action("Bash", RiskLevel::High);
    bash.side_effect_class = SideEffectClass::Unknown;

    let evidence = progress_evidence(
        Some(&bash),
        &json!({
            "success": true,
            "stdout": "",
            "stderr": "",
            "metadata": null,
            "structured_output": {
                "progress_kind": "artifact_verified",
                "verification_evidence": "looks good",
                "produced_output": false
            }
        }),
    );

    assert!(!evidence.changes_state);
    assert!(!evidence.has_structural_witness);
    assert!(!evidence.has_output_witness);
}

#[test]
fn model_unknown_repair_requires_structural_progress() {
    let mut bash = action("Bash", RiskLevel::High);
    bash.side_effect_class = SideEffectClass::Unknown;
    let evidence = progress_evidence(
        Some(&bash),
        &json!({
            "success": true,
            "stdout": "printed useful text",
            "stderr": "",
            "metadata": null,
            "structured_output": {
                "stdout": "printed useful text",
                "stderr": "",
                "interrupted": false
            }
        }),
    );

    assert!(!evidence.has_output_witness);
    assert!(!evidence.has_structural_witness);
    assert!(model_unknown_terminal_without_progress(
        Some(&bash),
        CompletionRole::Repair,
        true,
        &evidence
    ));
    assert!(!model_unknown_terminal_without_progress(
        Some(&bash),
        CompletionRole::Verification,
        true,
        &evidence
    ));
}

#[test]
fn filesystem_witness_counts_as_structural_progress() {
    let mut bash = action("Bash", RiskLevel::High);
    bash.side_effect_class = SideEffectClass::Unknown;

    let evidence = progress_evidence(
        Some(&bash),
        &json!({
            "success": true,
            "stdout": "",
            "stderr": "",
            "metadata": null,
            "structured_output": {
                "stdout": "",
                "stderr": "",
                "interrupted": false
            },
            "filesystem_witness": {
                "created": ["attack"],
                "modified": [],
                "removed": []
            }
        }),
    );

    assert!(evidence.changes_state);
    assert!(evidence.has_structural_witness);
}
