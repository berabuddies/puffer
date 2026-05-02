use super::artifact_context::generated_artifact_context;
use super::*;
use crate::contract::{
    ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, Idempotency, Reversibility,
    RiskLevel, SideEffectClass, TrustLevel, VerificationSpec,
};
use crate::graph::scores_for_action;
use crate::llm::ModelCandidateProposal;
use crate::planner::{CandidateSource, CompletionRole};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use crate::runtime::model_loop_support::{
    action_history_summary_excluding, artifact_review_history_summary_excluding,
    compact_history_value, compact_value, verified_target_summary,
};
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

fn puffer_tools_registry() -> InMemoryContractRegistry {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![
                action("Bash", RiskLevel::High),
                action("Read", RiskLevel::Low),
                action("Write", RiskLevel::Medium),
            ],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    registry
}

fn puffer_tools_registry_with_unknown_bash() -> InMemoryContractRegistry {
    let mut bash = action("Bash", RiskLevel::High);
    bash.side_effect_class = SideEffectClass::Unknown;
    let mut registry = InMemoryContractRegistry::default();
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
    registry
}

fn run_options_with_model() -> RunOptions {
    RunOptions {
        puffer_tool: None,
        puffer_args: None,
        puffer_tool_source: CandidateSource::ModelProposal,
        allow_failed_terminal_completion: false,
        enable_symbolic_workers: false,
        enable_parallel_read_only: false,
        enable_observe_act_llm: true,
        model: Some("unreachable-model".to_string()),
        goal_verification_min_confidence: 0.75,
        yolo: false,
    }
}

fn pending_bash_node(graph: &mut PlanGraph) -> NodeId {
    graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "pending bash".to_string(),
        payload: json!({"command": "printf ok"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Bash".to_string(),
        }),
        scores: scores_for_action(&action("Bash", RiskLevel::High)),
    })
}

#[test]
fn repeated_model_unknown_verification_probes_saturate_observations() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let runtime = BurbotRuntime::new(
        puffer_tools_registry_with_unknown_bash(),
        workspace.path().into(),
        trace,
    )
    .unwrap();
    let mut graph = PlanGraph::from_goal("produce artifact".to_string());
    for index in 0..3 {
        let action_id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Executed,
            label: "Bash".to_string(),
            payload: json!({"command": format!("printf probe-{index}")}),
            action_ref: Some(ActionRef {
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                action_name: "Bash".to_string(),
            }),
            scores: scores_for_action(&action("Bash", RiskLevel::High)),
        });
        graph.add_edge(
            NodeId(0),
            action_id,
            PlanEdgeKind::Supports,
            json!({
                "source": "model_observation_proposal",
                "completion_role": "verification",
            }),
        );
        let observation_id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Observation,
            status: PlanStatus::Satisfied,
            label: "Observation".to_string(),
            payload: json!({
                "success": true,
                "structured_output": {
                    "stdout": format!("probe-{index}\n"),
                    "stderr": "",
                    "interrupted": false,
                }
            }),
            action_ref: None,
            scores: Default::default(),
        });
        graph.add_edge(action_id, observation_id, PlanEdgeKind::Produces, json!({}));
    }

    let saturation = runtime.observation_saturation_context(&graph).unwrap();

    assert_eq!(
        saturation["code"],
        json!("model_observation_probe_saturated")
    );
    assert_eq!(saturation["streak"], json!(3));
}

#[test]
fn no_executable_recovery_anchors_latest_open_failure() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("recover".to_string());
    let failure = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Failure,
        status: PlanStatus::Open,
        label: "Failure in Bash".to_string(),
        payload: json!({
            "contract_id": PUFFER_TOOLS_CONTRACT_ID,
            "action_name": "Bash",
            "failure_mode": "timed_out",
        }),
        action_ref: None,
        scores: Default::default(),
    });

    let (source, reason, detail) = runtime.no_executable_recovery_context(&graph, 12);

    assert_eq!(source, failure);
    assert_eq!(reason, "no_executable_actions_after_failure");
    assert_eq!(detail["failure"]["failure_mode"], json!("timed_out"));
}

#[test]
fn no_executable_recovery_after_support_requires_advancing_work() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("recover".to_string());
    let read = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "Read".to_string(),
        payload: json!({"file_path": "/tmp/input.txt"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Read".to_string(),
        }),
        scores: Default::default(),
    });
    graph.add_edge(
        NodeId(0),
        read,
        PlanEdgeKind::Supports,
        json!({"completion_role": "support"}),
    );

    let (source, reason, detail) = runtime.no_executable_recovery_context(&graph, 7);

    assert_eq!(source, NodeId(0));
    assert_eq!(reason, "no_executable_actions_after_support");
    assert_eq!(detail["step"], json!(7));
}

#[test]
fn verification_read_yields_to_existing_frontier_before_goal_llm() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let run_id = RunId::new();
    let mut graph = PlanGraph::from_goal("compile written artifact".to_string());
    let mut beliefs = BeliefGraph::default();
    let write_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "Write".to_string(),
        payload: json!({"file_path": "/tmp/linear_attack.c", "content": "int main(){return 0;}"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Write".to_string(),
        }),
        scores: scores_for_action(&action("Write", RiskLevel::Medium)),
    });
    let read_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "Read".to_string(),
        payload: json!({"file_path": "/tmp/linear_attack.c"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Read".to_string(),
        }),
        scores: scores_for_action(&action("Read", RiskLevel::Low)),
    });
    graph.add_edge(read_id, write_id, PlanEdgeKind::Verifies, json!({}));
    let compile_id = pending_bash_node(&mut graph);

    let result = runtime
        .process_observation(
            run_id,
            0,
            &mut graph,
            &mut beliefs,
            "compile written artifact",
            &run_options_with_model(),
            Observation {
                invocation: ActionInvocation {
                    run_id,
                    plan_node_id: read_id,
                    contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                    action_name: "Read".to_string(),
                    args: json!({"file_path": "/tmp/linear_attack.c"}),
                },
                success: true,
                output: json!({
                    "success": true,
                    "tool_id": "Read",
                    "structured_output": {
                        "type": "file_unchanged",
                        "file": {"filePath": "/tmp/linear_attack.c"}
                    }
                }),
                error: None,
                raw: None,
                latency_ms: Some(1),
                cost: None,
            },
        )
        .unwrap();

    assert!(result.is_none());
    assert_eq!(graph.node(read_id).unwrap().status, PlanStatus::Executed);
    assert_eq!(graph.node(compile_id).unwrap().status, PlanStatus::Open);
}

#[test]
fn terminal_verification_yields_to_existing_frontier_before_goal_llm() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let run_id = RunId::new();
    let mut graph = PlanGraph::from_goal("repair generated artifact".to_string());
    let mut beliefs = BeliefGraph::default();
    let terminal_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Failed,
        label: "terminal".to_string(),
        payload: json!({"command": "cd /tmp && ./attack"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Bash".to_string(),
        }),
        scores: scores_for_action(&action("Bash", RiskLevel::High)),
    });
    graph.add_edge(
        NodeId(0),
        terminal_id,
        PlanEdgeKind::Supports,
        json!({"completion_role": "terminal"}),
    );
    let verifier_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "Read generated output".to_string(),
        payload: json!({"file_path": "/tmp/plaintexts.txt"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Read".to_string(),
        }),
        scores: scores_for_action(&action("Read", RiskLevel::Low)),
    });
    graph.add_edge(verifier_id, terminal_id, PlanEdgeKind::Verifies, json!({}));
    let repair_id = pending_bash_node(&mut graph);
    graph.add_edge(
        NodeId(0),
        repair_id,
        PlanEdgeKind::Supports,
        json!({"completion_role": "repair"}),
    );

    let result = runtime
        .process_observation(
            run_id,
            0,
            &mut graph,
            &mut beliefs,
            "repair generated artifact",
            &run_options_with_model(),
            Observation {
                invocation: ActionInvocation {
                    run_id,
                    plan_node_id: verifier_id,
                    contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                    action_name: "Read".to_string(),
                    args: json!({"file_path": "/tmp/plaintexts.txt"}),
                },
                success: true,
                output: json!({
                    "success": true,
                    "tool_id": "Read",
                    "structured_output": {
                        "type": "text",
                        "file": {
                            "filePath": "/tmp/plaintexts.txt",
                            "content": "not yet repaired"
                        }
                    }
                }),
                error: None,
                raw: None,
                latency_ms: Some(1),
                cost: None,
            },
        )
        .unwrap();

    assert!(result.is_none());
    assert_eq!(
        graph.node(verifier_id).unwrap().status,
        PlanStatus::Executed
    );
    assert_eq!(graph.node(repair_id).unwrap().status, PlanStatus::Open);
}

#[test]
fn non_terminal_verification_read_does_not_complete_goal() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let run_id = RunId::new();
    let mut graph = PlanGraph::from_goal("produce final artifact after repair".to_string());
    let mut beliefs = BeliefGraph::default();
    let write_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "Write".to_string(),
        payload: json!({"file_path": "/tmp/attack.py", "content": "print('incomplete')"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Write".to_string(),
        }),
        scores: scores_for_action(&action("Write", RiskLevel::Medium)),
    });
    graph.add_edge(
        NodeId(0),
        write_id,
        PlanEdgeKind::Supports,
        json!({"completion_role": "repair"}),
    );
    let read_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "Read".to_string(),
        payload: json!({"file_path": "/tmp/attack.py"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Read".to_string(),
        }),
        scores: scores_for_action(&action("Read", RiskLevel::Low)),
    });
    graph.add_edge(read_id, write_id, PlanEdgeKind::Verifies, json!({}));
    graph.add_edge(
        NodeId(0),
        read_id,
        PlanEdgeKind::Supports,
        json!({"completion_role": "verification"}),
    );
    let options = RunOptions {
        model: None,
        enable_observe_act_llm: false,
        ..run_options_with_model()
    };

    let result = runtime
        .process_observation(
            run_id,
            0,
            &mut graph,
            &mut beliefs,
            "produce final artifact after repair",
            &options,
            Observation {
                invocation: ActionInvocation {
                    run_id,
                    plan_node_id: read_id,
                    contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                    action_name: "Read".to_string(),
                    args: json!({"file_path": "/tmp/attack.py"}),
                },
                success: true,
                output: json!({
                    "success": true,
                    "tool_id": "Read",
                    "structured_output": {
                        "type": "file_unchanged",
                        "file": {"filePath": "/tmp/attack.py"}
                    }
                }),
                error: None,
                raw: None,
                latency_ms: Some(1),
                cost: None,
            },
        )
        .unwrap();

    assert!(result.is_none());
    assert_eq!(graph.node(read_id).unwrap().status, PlanStatus::Executed);
    assert_eq!(graph.node(write_id).unwrap().status, PlanStatus::Executed);
}

#[test]
fn post_observation_model_proposal_yields_to_existing_frontier() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("continue pending implementation".to_string());
    let pending = pending_bash_node(&mut graph);
    let action_ref = ActionRef {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        action_name: "Bash".to_string(),
    };

    let added = runtime
        .add_model_observe_act_candidates(
            RunId::new(),
            &mut graph,
            "goal",
            NodeId(0),
            pending,
            &action_ref,
            &json!({"command": "cat missing"}),
            false,
            &json!({"success": false}),
            &run_options_with_model(),
        )
        .unwrap();

    assert_eq!(added, 0);
    assert_eq!(graph.node(pending).unwrap().status, PlanStatus::Open);
}

#[test]
fn compact_model_context_prefers_structured_tool_output() {
    let compact = compact_value(&json!({
        "success": true,
        "tool_id": "Read",
        "stdout": "{\"file\":{\"content\":\"authoritative\",\"filePath\":\"/tmp/example.txt\"}}",
        "stderr": "",
        "structured_output_source": "puffer_runtime_handler",
        "structured_output": {
            "file": {
                "filePath": "/tmp/example.txt",
                "content": "authoritative"
            }
        }
    }));

    assert!(compact.get("stdout").is_none());
    assert_eq!(
        compact["structured_output"]["file"]["content"],
        json!("authoritative")
    );
}

#[test]
fn compact_model_context_preserves_non_duplicate_stdout() {
    let compact = compact_value(&json!({
        "success": true,
        "tool_id": "Bash",
        "stdout": "compiled attack.c",
        "stderr": "",
        "exit_code": 0,
        "structured_output": {
            "exit_code": 0,
            "stdout": "compiled attack.c",
            "stderr": ""
        }
    }));

    assert_eq!(compact["stdout"], json!("compiled attack.c"));
    assert_eq!(compact["exit_code"], json!(0));
    assert_eq!(compact["structured_output"]["exit_code"], json!(0));
}

#[test]
fn compact_model_context_does_not_parse_stdout_json_for_duplicates() {
    let compact = compact_value(&json!({
        "success": true,
        "tool_id": "External",
        "stdout": "{\"filePath\":\"/tmp/fake\"}",
        "stderr": "",
        "structured_output": {
            "filePath": "/tmp/fake"
        }
    }));

    assert_eq!(compact["stdout"], json!("{\"filePath\":\"/tmp/fake\"}"));
    assert_eq!(compact["structured_output"]["filePath"], json!("/tmp/fake"));
}

#[test]
fn compact_model_context_omits_trusted_structured_stdout_echo() {
    let compact = compact_value(&json!({
        "success": true,
        "tool_id": "Bash",
        "stdout": "{\"stdout\":\"ok\",\"stderr\":\"\"}",
        "stderr": "",
        "structured_output_source": "puffer_runtime_handler",
        "structured_output": {
            "stdout": "ok",
            "stderr": ""
        }
    }));

    assert!(compact.get("stdout").is_none());
    assert_eq!(compact["structured_output"]["stdout"], json!("ok"));
}

#[test]
fn compact_history_context_uses_smaller_string_budget() {
    let text = "x".repeat(12_000);
    let compact = compact_history_value(&json!({"content": text}));

    assert_eq!(compact["content"]["original_chars"], json!(12000));
    assert!(compact["content"]["truncated_string_prefix"]
        .as_str()
        .is_some_and(|prefix| prefix.len() == 4000));
}

#[test]
fn artifact_review_history_elides_large_prior_text_content() {
    let registry = puffer_tools_registry();
    let mut graph = PlanGraph::from_goal("review generated artifact".to_string());
    let read_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "Read".to_string(),
        payload: json!({"file_path": "/tmp/source.c"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Read".to_string(),
        }),
        scores: scores_for_action(&action("Read", RiskLevel::Low)),
    });
    let observation_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Observation,
        status: PlanStatus::Satisfied,
        label: "Observation".to_string(),
        payload: json!({
            "success": true,
            "tool_id": "Read",
            "structured_output": {
                "type": "text",
                "file": {
                    "filePath": "/tmp/source.c",
                    "content": "x".repeat(2_000),
                    "numLines": 50,
                }
            }
        }),
        action_ref: None,
        scores: ActionScores::default(),
    });
    graph.add_edge(read_id, observation_id, PlanEdgeKind::Produces, json!({}));
    let write_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "Write".to_string(),
        payload: json!({"file_path": "/tmp/generated.c", "content": "int main(){return 0;}"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Write".to_string(),
        }),
        scores: scores_for_action(&action("Write", RiskLevel::Medium)),
    });

    let history = artifact_review_history_summary_excluding(&graph, &registry, Some(write_id));

    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0]["output"]["structured_output"]["file"]["content"]["omitted_from_context"],
        json!(true)
    );
    assert_eq!(
        history[0]["output"]["structured_output"]["file"]["content"]["original_chars"],
        json!(2000)
    );
}

#[test]
fn compact_model_context_elides_file_write_echo_output() {
    let compact = compact_value(&json!({
        "success": true,
        "tool_id": "Write",
        "stdout": "",
        "stderr": "",
        "structured_output": {
            "type": "create",
            "filePath": "/tmp/generated.py",
            "content": "x".repeat(10_000),
            "gitDiff": "diff".repeat(1000),
            "structuredPatch": [{"line": 1, "text": "x".repeat(1000)}]
        }
    }));

    assert_eq!(
        compact["structured_output"]["content"]["omitted_from_context"],
        json!(true)
    );
    assert_eq!(
        compact["structured_output"]["content"]["original_chars"],
        json!(10000)
    );
    assert_eq!(
        compact["structured_output"]["filePath"],
        json!("/tmp/generated.py")
    );
}

#[test]
fn generated_artifact_context_preserves_recent_model_write_payload() {
    let mut registry = InMemoryContractRegistry::default();
    let mut write = action("Write", RiskLevel::Medium);
    write.side_effect_class = SideEffectClass::LocalWrite;
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![write],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let mut graph = PlanGraph::from_goal("repair generated artifact".to_string());
    let content = "x".repeat(9_500);
    let write_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "Write".to_string(),
        payload: json!({"file_path": "/tmp/generated.c", "content": content}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Write".to_string(),
        }),
        scores: scores_for_action(&action("Write", RiskLevel::Medium)),
    });
    graph.add_edge(
        NodeId(0),
        write_id,
        PlanEdgeKind::Supports,
        json!({
            "source": "model_observation_proposal",
            "completion_role": "repair",
        }),
    );
    let observation = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Observation,
        status: PlanStatus::Satisfied,
        label: "Observation".to_string(),
        payload: json!({
            "success": true,
            "structured_output": {
                "file": {"filePath": "/tmp/generated.c"}
            }
        }),
        action_ref: None,
        scores: ActionScores::default(),
    });
    graph.add_edge(write_id, observation, PlanEdgeKind::Produces, json!({}));

    let context = generated_artifact_context(&graph, &registry, None);

    assert_eq!(context["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        context["items"][0]["args"]["content"]
            .as_str()
            .unwrap()
            .len(),
        9500
    );
}

#[test]
fn action_history_can_exclude_current_observation_action() {
    let registry = puffer_tools_registry();
    let mut graph = PlanGraph::from_goal("avoid duplicate current output".to_string());
    let action_id = pending_bash_node(&mut graph);
    let observation_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Observation,
        status: PlanStatus::Satisfied,
        label: "Observation".to_string(),
        payload: json!({"success": true, "stdout": "current"}),
        action_ref: None,
        scores: ActionScores::default(),
    });
    graph.add_edge(action_id, observation_id, PlanEdgeKind::Produces, json!({}));

    let history = action_history_summary_excluding(&graph, &registry, Some(action_id));

    assert!(history.is_empty());
}

#[test]
fn verified_target_summary_recovers_content_for_file_unchanged_verifier() {
    let mut graph = PlanGraph::from_goal("verify written artifact".to_string());
    let write_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "Write".to_string(),
        payload: json!({"file_path": "/tmp/attack.c", "content": "int main(){return 0;}"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Write".to_string(),
        }),
        scores: scores_for_action(&action("Write", RiskLevel::Medium)),
    });
    let observation_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Observation,
        status: PlanStatus::Satisfied,
        label: "Observation".to_string(),
        payload: json!({"success": true, "structured_output": {"type": "create"}}),
        action_ref: None,
        scores: ActionScores::default(),
    });
    graph.add_edge(write_id, observation_id, PlanEdgeKind::Produces, json!({}));
    let verifier_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "Read".to_string(),
        payload: json!({"file_path": "/tmp/attack.c"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Read".to_string(),
        }),
        scores: scores_for_action(&action("Write", RiskLevel::Medium)),
    });
    graph.add_edge(verifier_id, write_id, PlanEdgeKind::Verifies, json!({}));

    let summary = verified_target_summary(&graph, verifier_id).unwrap();

    assert_eq!(summary["args"]["content"], json!("int main(){return 0;}"));
    assert_eq!(
        summary["output"]["structured_output"]["type"],
        json!("create")
    );
}

#[test]
fn observe_act_verification_role_does_not_attach_final_verifier_edge() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("check environment before implementation".to_string());
    let prior = pending_bash_node(&mut graph);

    let added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            Some(prior),
            CandidateSource::ModelObservationProposal,
            vec![ModelCandidateProposal {
                id: Some("check-python".to_string()),
                tool_id: "Bash".to_string(),
                args: json!({"command": "python3 --version"}),
                completion_role: CompletionRole::Verification,
                depends_on: Vec::new(),
                rationale: "verify local interpreter availability".to_string(),
            }],
        )
        .unwrap();

    assert_eq!(added.added, 1);
    assert!(!graph
        .edges
        .iter()
        .any(|edge| edge.kind == PlanEdgeKind::Verifies));
}

#[test]
fn goal_verifier_followups_keep_structural_order_dependencies() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("repair then verify".to_string());

    let added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            Some(NodeId(0)),
            CandidateSource::ModelGoalVerifier,
            vec![
                ModelCandidateProposal {
                    id: Some("write-artifact".to_string()),
                    tool_id: "Write".to_string(),
                    args: json!({"file_path": "/tmp/out", "content": "data"}),
                    completion_role: CompletionRole::Repair,
                    depends_on: Vec::new(),
                    rationale: "write artifact".to_string(),
                },
                ModelCandidateProposal {
                    id: Some("verify-artifact".to_string()),
                    tool_id: "Bash".to_string(),
                    args: json!({"command": "test -f /tmp/out"}),
                    completion_role: CompletionRole::Verification,
                    depends_on: vec!["write-artifact".to_string()],
                    rationale: "verify artifact".to_string(),
                },
            ],
        )
        .unwrap();

    assert_eq!(added.added, 2);
    let write_id = graph
        .nodes
        .iter()
        .find_map(|(id, node)| {
            node.action_ref
                .as_ref()
                .is_some_and(|action_ref| action_ref.action_name == "Write")
                .then_some(*id)
        })
        .unwrap();
    let bash_id = graph
        .nodes
        .iter()
        .find_map(|(id, node)| {
            node.action_ref
                .as_ref()
                .is_some_and(|action_ref| action_ref.action_name == "Bash")
                .then_some(*id)
        })
        .unwrap();

    assert!(graph.has_edge(write_id, bash_id, PlanEdgeKind::DependsOn));
}

#[test]
fn unresolved_model_candidate_dependencies_are_blocked_before_execution() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("write after already-satisfied read".to_string());

    let added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelObservationProposal,
            vec![ModelCandidateProposal {
                id: Some("write-header".to_string()),
                tool_id: "Write".to_string(),
                args: json!({"file_path": "/tmp/generated.h", "content": "ok"}),
                completion_role: CompletionRole::Repair,
                depends_on: vec!["read-full-feal".to_string()],
                rationale: "write derived artifact after read evidence".to_string(),
            }],
        )
        .unwrap();

    assert_eq!(added.added, 0);
    assert_eq!(added.skipped_dependency, 1);
    let write = graph
        .nodes
        .values()
        .find(|node| {
            node.action_ref
                .as_ref()
                .is_some_and(|action_ref| action_ref.action_name == "Write")
        })
        .unwrap();
    assert_eq!(write.status, PlanStatus::Blocked);
}

#[test]
fn recovery_model_proposal_yields_to_existing_frontier() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("continue pending implementation".to_string());
    let pending = pending_bash_node(&mut graph);

    let added = runtime
        .add_recovery_model_candidates(
            RunId::new(),
            &mut graph,
            "goal",
            NodeId(0),
            "test_recovery",
            json!({}),
            None,
            &run_options_with_model(),
        )
        .unwrap();

    assert_eq!(added, 0);
    assert_eq!(graph.node(pending).unwrap().status, PlanStatus::Open);
}

#[test]
fn terminal_output_probe_before_state_change_does_not_trigger_goal_verification() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime =
        BurbotRuntime::new(puffer_tools_registry(), workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("create required artifact".to_string());
    let terminal = pending_bash_node(&mut graph);
    graph.add_edge(
        NodeId(0),
        terminal,
        PlanEdgeKind::Supports,
        json!({"completion_role": "terminal"}),
    );
    let output_only = ProgressEvidence {
        changes_state: false,
        has_state_witness: false,
        has_structural_witness: false,
        has_output_witness: true,
    };

    assert!(!runtime.should_verify_terminal_completion(&graph, terminal, &output_only));

    runtime.state_epoch = 1;
    assert!(!runtime.should_verify_terminal_completion(&graph, terminal, &output_only));
}

#[test]
fn model_terminal_read_only_candidate_is_downgraded_to_support() {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![action("Read", RiskLevel::Low)],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime = BurbotRuntime::new(registry, workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("inspect state".to_string());

    let added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelProposal,
            vec![ModelCandidateProposal {
                id: Some("read".to_string()),
                tool_id: "Read".to_string(),
                args: json!({}),
                completion_role: CompletionRole::Terminal,
                depends_on: Vec::new(),
                rationale: "inspect final state".to_string(),
            }],
        )
        .unwrap();

    assert_eq!(added.added, 1);
    let edge = graph
        .edges
        .iter()
        .find(|edge| edge.kind == PlanEdgeKind::Supports && edge.target != NodeId(0))
        .unwrap();
    assert_eq!(edge.payload["completion_role"], json!("support"));
    let node = graph.node(edge.target).unwrap();
    assert_eq!(node.scores.expected_progress, 0.5);
}

#[test]
fn model_verification_read_only_candidate_without_target_is_downgraded_to_support() {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![action("Read", RiskLevel::Low)],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime = BurbotRuntime::new(registry, workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("inspect state".to_string());

    let added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelProposal,
            vec![ModelCandidateProposal {
                id: Some("read".to_string()),
                tool_id: "Read".to_string(),
                args: json!({}),
                completion_role: CompletionRole::Verification,
                depends_on: Vec::new(),
                rationale: "inspect final state".to_string(),
            }],
        )
        .unwrap();

    assert_eq!(added.added, 1);
    let edge = graph
        .edges
        .iter()
        .find(|edge| edge.kind == PlanEdgeKind::Supports && edge.target != NodeId(0))
        .unwrap();
    assert_eq!(edge.payload["completion_role"], json!("support"));
    let node = graph.node(edge.target).unwrap();
    assert!(node.scores.verification_value < 1.0);
}

#[test]
fn goal_verifier_terminal_read_only_candidate_becomes_verification() {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![action("Read", RiskLevel::Low)],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let mut runtime = BurbotRuntime::new(registry, workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("verify artifact".to_string());
    let target = pending_bash_node(&mut graph);

    let added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            Some(target),
            CandidateSource::ModelGoalVerifier,
            vec![ModelCandidateProposal {
                id: Some("read".to_string()),
                tool_id: "Read".to_string(),
                args: json!({}),
                completion_role: CompletionRole::Terminal,
                depends_on: Vec::new(),
                rationale: "verify final state".to_string(),
            }],
        )
        .unwrap();

    assert_eq!(added.added, 1);
    let verifier = graph
        .edges
        .iter()
        .find(|edge| edge.kind == PlanEdgeKind::Verifies && edge.target == target)
        .map(|edge| edge.source)
        .unwrap();
    let support = graph
        .edges
        .iter()
        .find(|edge| edge.kind == PlanEdgeKind::Supports && edge.target == verifier)
        .unwrap();
    assert_eq!(support.payload["completion_role"], json!("verification"));
}
