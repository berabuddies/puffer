use super::*;
use crate::contract::{
    ActionContract, ApprovalSpec, ArgumentPatternSpec, ArgumentSafetySpec, CapabilityContract,
    ContractStatus, Idempotency, RepairRuleSpec, Reversibility, RiskLevel, SemanticIntentSpec,
    SideEffectClass, StructuredArgumentSafetySpec, TrustLevel, VerificationSpec,
};
use crate::graph::scores_for_action;
use crate::llm::ModelCandidateProposal;
use crate::planner::{CandidateSource, CompletionRole};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use crate::rules::action_key;
use crate::trace::TraceEventType;

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
        repair_rules: repair_rules(name),
        cost_estimate: None,
        latency_estimate: None,
    }
}

fn repair_rules(name: &str) -> Vec<RepairRuleSpec> {
    if name != "Bash" {
        return Vec::new();
    }
    vec![RepairRuleSpec {
        failure_kinds: vec!["command_not_found".to_string()],
        contract_id: None,
        action_name: "Glob".to_string(),
        args: json!({"pattern": "{Cargo.toml,package.json,pyproject.toml,go.mod,uv.lock,poetry.lock}"}),
        arg_templates: Default::default(),
        bindings: Vec::new(),
        source: Some("repair".to_string()),
        completion_role: Some("repair".to_string()),
        rationale: Some("inspect metadata".to_string()),
        expected_progress: Some(0.9),
    }]
}

fn sleep_action() -> ActionContract {
    let mut sleep = action("Sleep", RiskLevel::Low);
    sleep.side_effect_class = SideEffectClass::PureObservation;
    sleep.input_schema = json!({
        "type": "object",
        "properties": {
            "duration_ms": {"type": "integer"},
            "reason": {"type": "string"}
        },
        "required": ["duration_ms"]
    });
    let mut defaults = std::collections::BTreeMap::new();
    defaults.insert("duration_ms".to_string(), json!(1000));
    sleep.semantic_intents = vec![SemanticIntentSpec {
        intent: "await_async_progress".to_string(),
        slots: Default::default(),
        optional_slots: Default::default(),
        defaults,
        side_effect_class: Some(SideEffectClass::PureObservation),
        slot_kinds: Default::default(),
    }];
    sleep
}

#[test]
fn scheduler_prefers_lower_risk_comparable_action() {
    let scheduler = Scheduler::default();
    let mut graph = PlanGraph::from_goal("goal".to_string());
    let mut low = scores_for_action(&action("low", RiskLevel::Low));
    let mut high = scores_for_action(&action("high", RiskLevel::High));
    low.expected_progress = 0.5;
    high.expected_progress = 0.5;
    let low_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "low".to_string(),
        payload: json!({}),
        action_ref: None,
        scores: low,
    });
    graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "high".to_string(),
        payload: json!({}),
        action_ref: None,
        scores: high,
    });
    assert_eq!(
        scheduler.choose_next_action(&graph).unwrap().node_id,
        low_id
    );
}

#[test]
fn low_scored_model_action_remains_schedulable_when_it_is_frontier() {
    let mut graph = PlanGraph::from_goal("solve".to_string());
    let mut scores = ActionScores {
        expected_progress: 0.1,
        information_gain: 0.0,
        verification_value: 0.0,
        reversibility_bonus: 0.0,
        cache_reuse_bonus: 0.0,
        risk_penalty: 4.0,
        cost_penalty: 1.0,
        latency_penalty: 1.0,
        uncertainty_penalty: 2.0,
        repeated_failure_penalty: 0.0,
    };
    scores.expected_progress = 0.1;
    let action_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "bash".to_string(),
        payload: json!({"command": "python solve.py"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Bash".to_string(),
        }),
        scores,
    });
    graph.add_edge(
        NodeId(0),
        action_id,
        PlanEdgeKind::Supports,
        json!({"source": "model_observation_proposal", "completion_role": "repair"}),
    );
    let selected = Scheduler::default().choose_next_action(&graph).unwrap();

    assert_eq!(selected.node_id, action_id);
    assert!(selected.breakdown.total < 0.0);
    assert_eq!(graph.node(action_id).unwrap().status, PlanStatus::Open);
}

#[test]
fn safety_gate_blocks_external_write_approval() {
    let mut registry = InMemoryContractRegistry::default();
    let mut write = action("Send", RiskLevel::Medium);
    write.side_effect_class = SideEffectClass::ExternalWrite;
    write.approval.required = true;
    registry
        .register(CapabilityContract {
            contract_id: "mail".to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::External,
            description: "mail".to_string(),
            actions: vec![write],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let node = PlanNode {
        id: NodeId(1),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "send".to_string(),
        payload: json!({}),
        action_ref: Some(ActionRef {
            contract_id: "mail".to_string(),
            action_name: "Send".to_string(),
        }),
        scores: ActionScores::default(),
    };
    let ctx = RuntimeContext {
        contracts: &registry,
        approvals: ApprovalStore::default(),
        beliefs: BeliefGraph::default(),
    };
    assert_eq!(
        SafetyGate.blocks(&node, &ctx).unwrap(),
        Some(BlockReason::ExternalWriteApprovalRequired)
    );
}

#[test]
fn safety_gate_blocks_explicit_contract_approval_even_with_safe_arguments() {
    let mut registry = InMemoryContractRegistry::default();
    let mut shell = action("Shell", RiskLevel::High);
    shell.side_effect_class = SideEffectClass::Unknown;
    shell.approval.required = true;
    shell.argument_safety = vec![ArgumentSafetySpec {
        source_arg: "command".to_string(),
        blocked_patterns: vec![ArgumentPatternSpec {
            name: "root_delete".to_string(),
            pattern: r"(^|[;&|]\s*)rm\s+-rf\s+(/|/\*)($|\s|[;&|])".to_string(),
            reason: None,
            confidence: 1.0,
        }],
        approval_patterns: vec![ArgumentPatternSpec {
            name: "recursive_delete".to_string(),
            pattern: r"(^|[;&|]\s*)rm\s+-r[f]?\s+".to_string(),
            reason: None,
            confidence: 0.9,
        }],
    }];
    registry
        .register(CapabilityContract {
            contract_id: "shell".to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "shell".to_string(),
            actions: vec![shell],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let node = PlanNode {
        id: NodeId(1),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "shell".to_string(),
        payload: json!({"command": "python -c 'print(1)'"}),
        action_ref: Some(ActionRef {
            contract_id: "shell".to_string(),
            action_name: "Shell".to_string(),
        }),
        scores: ActionScores::default(),
    };
    let ctx = RuntimeContext {
        contracts: &registry,
        approvals: ApprovalStore::default(),
        beliefs: BeliefGraph::default(),
    };
    assert_eq!(
        SafetyGate.blocks(&node, &ctx).unwrap(),
        Some(BlockReason::ApprovalRequired)
    );
}

#[test]
fn safety_gate_enforces_structured_argument_safety() {
    let mut registry = InMemoryContractRegistry::default();
    let mut shell = action("Shell", RiskLevel::High);
    shell.side_effect_class = SideEffectClass::Unknown;
    shell.structured_argument_safety = vec![
        StructuredArgumentSafetySpec::BlockPathPrefix {
            source_arg: "file_path".to_string(),
            prefixes: vec!["/etc".to_string()],
        },
        StructuredArgumentSafetySpec::RequireApprovalPathComponent {
            source_arg: "file_path".to_string(),
            component: ".git".to_string(),
        },
    ];
    registry
        .register(CapabilityContract {
            contract_id: "shell".to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "shell".to_string(),
            actions: vec![shell],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let mut node = PlanNode {
        id: NodeId(1),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "shell".to_string(),
        payload: json!({"file_path": "/etc/passwd"}),
        action_ref: Some(ActionRef {
            contract_id: "shell".to_string(),
            action_name: "Shell".to_string(),
        }),
        scores: ActionScores::default(),
    };
    let ctx = RuntimeContext {
        contracts: &registry,
        approvals: ApprovalStore::default(),
        beliefs: BeliefGraph::default(),
    };
    assert_eq!(
        SafetyGate.blocks(&node, &ctx).unwrap(),
        Some(BlockReason::ArgumentBlocked)
    );
    node.payload = json!({"file_path": "/workspace/.git/config"});
    assert_eq!(
        SafetyGate.blocks(&node, &ctx).unwrap(),
        Some(BlockReason::ArgumentApprovalRequired)
    );
}

#[test]
fn placeholder_verification_node_does_not_suppress_executable_verifier() {
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let runtime = BurbotRuntime::new(
        InMemoryContractRegistry::default(),
        workspace.path().into(),
        trace,
    )
    .unwrap();
    let mut graph = PlanGraph::from_goal("write".to_string());
    let target = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "write".to_string(),
        payload: json!({}),
        action_ref: None,
        scores: ActionScores::default(),
    });
    let placeholder = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Verification,
        status: PlanStatus::Open,
        label: "verify".to_string(),
        payload: json!({}),
        action_ref: None,
        scores: ActionScores::default(),
    });
    graph.add_edge(placeholder, target, PlanEdgeKind::Verifies, json!({}));

    assert!(runtime.should_schedule_verification(
        &graph,
        target,
        &VerificationOutcome {
            required: true,
            passed: false,
        },
    ));

    let verifier = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "read".to_string(),
        payload: json!({}),
        action_ref: None,
        scores: ActionScores::default(),
    });
    graph.add_edge(verifier, target, PlanEdgeKind::Verifies, json!({}));

    assert!(!runtime.should_schedule_verification(
        &graph,
        target,
        &VerificationOutcome {
            required: true,
            passed: false,
        },
    ));
}

#[test]
fn record_failure_adds_executable_repair_action() {
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
                action("Glob", RiskLevel::Low),
            ],
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
    let mut graph = PlanGraph::from_goal("repair".to_string());
    let action_ref = ActionRef {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        action_name: "Bash".to_string(),
    };
    let failed_action = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Failed,
        label: "bash".to_string(),
        payload: json!({"command": "missing-command"}),
        action_ref: Some(action_ref.clone()),
        scores: ActionScores::default(),
    });
    let mut beliefs = BeliefGraph::new();

    runtime
        .record_failure(
            run_id,
            &mut graph,
            &mut beliefs,
            failed_action,
            &action_ref,
            &json!({"command": "missing-command"}),
            &json!({"success": false, "stderr": "missing-command: command not found"}),
            FailureKind::CommandNotFound,
        )
        .unwrap();

    let repair_action = graph
        .nodes
        .values()
        .find(|node| {
            node.kind == PlanNodeKind::Action
                && node
                    .action_ref
                    .as_ref()
                    .is_some_and(|action_ref| action_ref.action_name == "Glob")
        })
        .expect("repair action should be executable");
    assert!(graph
        .edges
        .iter()
        .any(|edge| { edge.kind == PlanEdgeKind::Repairs && edge.target == repair_action.id }));
    let events = trace.events_for_run(run_id).unwrap();
    assert!(events
        .iter()
        .any(|event| event.event_type == TraceEventType::RepairAdded));
    assert!(events.iter().any(|event| {
        event.event_type == TraceEventType::FailureClassified
            && event.failure_mode.as_deref() == Some("command_not_found")
    }));
}

#[test]
fn expected_terminal_failure_can_complete_with_evidence() {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![action("Bash", RiskLevel::High)],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let trace_dir = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(trace_dir.path().join("traces"));
    let workspace_root = std::env::current_dir().unwrap();
    let mut runtime = BurbotRuntime::new(registry, workspace_root, trace.clone()).unwrap();

    let result = runtime
        .run_goal(
            "Run an intentionally failing command and surface the failure.".to_string(),
            RunOptions {
                puffer_tool: Some("Bash".to_string()),
                puffer_args: Some(json!({
                    "command": "bash -lc 'printf \"expected-burbot-failure\\n\" >&2; exit 7'",
                    "timeout": 60000,
                })),
                puffer_tool_source: CandidateSource::ExplicitUserTool,
                allow_failed_terminal_completion: true,
                enable_symbolic_workers: false,
                enable_parallel_read_only: false,
                enable_observe_act_llm: false,
                model: None,
                goal_verification_min_confidence: 0.75,
                yolo: false,
            },
        )
        .unwrap();

    let RunResult::Completed {
        run_id, artifact, ..
    } = result
    else {
        panic!("expected failed terminal action to complete as surfaced failure");
    };
    assert_eq!(artifact["expected_failure"], true);
    assert!(artifact["verification_evidence"]
        .to_string()
        .contains("expected-burbot-failure"));
    let events = trace.events_for_run(run_id).unwrap();
    assert!(events
        .iter()
        .any(|event| event.event_type == TraceEventType::FailureClassified));
    assert!(events
        .iter()
        .any(|event| event.event_type == TraceEventType::CompletionDeclared));
    assert!(trace.graph_snapshot_path_for_run(run_id).exists());
}

#[test]
fn seed_actions_groups_initial_candidates_under_plan_subgoals() {
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
                action("Glob", RiskLevel::Low),
            ],
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
    let mut graph = PlanGraph::from_goal("read cargo manifest".to_string());

    runtime
        .seed_actions(
            RunId::new(),
            &mut graph,
            "read cargo manifest",
            &RunOptions {
                puffer_tool: Some("Bash".to_string()),
                puffer_args: Some(json!({"command": "cat Cargo.toml"})),
                puffer_tool_source: CandidateSource::ExplicitUserTool,
                allow_failed_terminal_completion: false,
                enable_symbolic_workers: false,
                enable_parallel_read_only: false,
                enable_observe_act_llm: false,
                model: None,
                goal_verification_min_confidence: 0.75,
                yolo: false,
            },
        )
        .unwrap();

    let plan_nodes = graph
        .nodes
        .values()
        .filter(|node| node.kind == PlanNodeKind::Subgoal)
        .collect::<Vec<_>>();
    assert!(plan_nodes
        .iter()
        .any(|node| node.payload["plan_id"] == "explicit_user_tool"));
    assert!(graph
        .edges
        .iter()
        .any(|edge| { edge.source == NodeId(0) && edge.kind == PlanEdgeKind::DecomposesTo }));
}

#[test]
fn model_wait_candidate_is_not_inserted_when_terminal_action_is_executable() {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![action("Bash", RiskLevel::High), sleep_action()],
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
    let mut graph = PlanGraph::from_goal("solve".to_string());
    let bash_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Open,
        label: "bash".to_string(),
        payload: json!({"command": "python solve.py"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Bash".to_string(),
        }),
        scores: ActionScores::default(),
    });
    graph.add_edge(
        NodeId(0),
        bash_id,
        PlanEdgeKind::Supports,
        json!({"completion_role": "terminal"}),
    );

    let added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelObservationProposal,
            vec![ModelCandidateProposal {
                id: Some("wait".to_string()),
                tool_id: "Sleep".to_string(),
                args: json!({"duration_ms": 1000}),
                completion_role: CompletionRole::Support,
                depends_on: Vec::new(),
                rationale: "structured wait proposal".to_string(),
            }],
        )
        .unwrap();

    assert_eq!(added.added, 0);
    assert!(!graph.nodes.values().any(|node| {
        node.action_ref
            .as_ref()
            .is_some_and(|action_ref| action_ref.action_name == "Sleep")
    }));
}

#[test]
fn terminal_model_wait_candidate_is_allowed_when_it_is_the_only_path() {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![sleep_action()],
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
    let mut graph = PlanGraph::from_goal("wait".to_string());

    let added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelProposal,
            vec![ModelCandidateProposal {
                id: Some("wait".to_string()),
                tool_id: "Sleep".to_string(),
                args: json!({"duration_ms": 1000}),
                completion_role: CompletionRole::Terminal,
                depends_on: Vec::new(),
                rationale: "structured wait proposal".to_string(),
            }],
        )
        .unwrap();

    assert_eq!(added.added, 1);
    assert!(graph.nodes.values().any(|node| {
        node.action_ref
            .as_ref()
            .is_some_and(|action_ref| action_ref.action_name == "Sleep")
    }));
}

#[test]
fn model_candidate_dedupe_allows_rerun_after_state_change() {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![action("Bash", RiskLevel::High)],
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
    let mut graph = PlanGraph::from_goal("verify".to_string());
    let action_ref = ActionRef {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        action_name: "Bash".to_string(),
    };
    let args = json!({"command": "python test.py"});
    runtime
        .executed_action_epochs
        .insert(action_key(&action_ref, &args), runtime.state_epoch);

    let same_epoch_added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelGoalVerifier,
            vec![ModelCandidateProposal {
                id: Some("verify".to_string()),
                tool_id: "Bash".to_string(),
                args: args.clone(),
                completion_role: CompletionRole::Verification,
                depends_on: Vec::new(),
                rationale: "repeat verification".to_string(),
            }],
        )
        .unwrap();

    runtime.state_epoch = runtime.state_epoch.saturating_add(1);
    let next_epoch_added = runtime
        .add_model_candidate_proposals(
            RunId::new(),
            &mut graph,
            NodeId(0),
            None,
            CandidateSource::ModelGoalVerifier,
            vec![ModelCandidateProposal {
                id: Some("verify-after-repair".to_string()),
                tool_id: "Bash".to_string(),
                args,
                completion_role: CompletionRole::Verification,
                depends_on: Vec::new(),
                rationale: "repeat verification after repair".to_string(),
            }],
        )
        .unwrap();

    assert_eq!(same_epoch_added.added, 0);
    assert_eq!(next_epoch_added.added, 1);
}

#[test]
fn required_repair_action_schedules_verification() {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![action("Edit", RiskLevel::Medium)],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        })
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let trace = JsonlTraceStore::new(workspace.path().join("traces"));
    let runtime = BurbotRuntime::new(registry, workspace.path().into(), trace).unwrap();
    let mut graph = PlanGraph::from_goal("repair".to_string());
    let edit_id = graph.add_node(PlanNode {
        id: NodeId(0),
        kind: PlanNodeKind::Action,
        status: PlanStatus::Executed,
        label: "edit".to_string(),
        payload: json!({"file_path": "/tmp/example.txt"}),
        action_ref: Some(ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: "Edit".to_string(),
        }),
        scores: ActionScores::default(),
    });
    graph.add_edge(
        NodeId(0),
        edit_id,
        PlanEdgeKind::Supports,
        json!({"completion_role": "repair"}),
    );

    assert!(runtime.should_schedule_verification(
        &graph,
        edit_id,
        &VerificationOutcome {
            required: true,
            passed: false,
        },
    ));
}

#[test]
fn goal_unsatisfied_prunes_same_batch_model_siblings() {
    let mut registry = InMemoryContractRegistry::default();
    registry
        .register(CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![action("Bash", RiskLevel::High)],
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
    let mut graph = PlanGraph::from_goal("repair".to_string());
    let action_ref = ActionRef {
        contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
        action_name: "Bash".to_string(),
    };
    let first = runtime
        .add_candidate_node(
            RunId::new(),
            &mut graph,
            ActionCandidate {
                action_ref: action_ref.clone(),
                args: json!({"command": "first"}),
                source: CandidateSource::ModelObservationProposal,
                completion_role: CompletionRole::Terminal,
                rationale: "first".to_string(),
                scores: ActionScores::default(),
            },
            Some(NodeId(0)),
            None,
        )
        .unwrap();
    let sibling = runtime
        .add_candidate_node(
            RunId::new(),
            &mut graph,
            ActionCandidate {
                action_ref: action_ref.clone(),
                args: json!({"command": "stale sibling"}),
                source: CandidateSource::ModelObservationProposal,
                completion_role: CompletionRole::Terminal,
                rationale: "sibling".to_string(),
                scores: ActionScores::default(),
            },
            Some(NodeId(0)),
            None,
        )
        .unwrap();
    let explicit = runtime
        .add_candidate_node(
            RunId::new(),
            &mut graph,
            ActionCandidate {
                action_ref,
                args: json!({"command": "explicit"}),
                source: CandidateSource::ExplicitUserTool,
                completion_role: CompletionRole::Terminal,
                rationale: "explicit".to_string(),
                scores: ActionScores::default(),
            },
            Some(NodeId(0)),
            None,
        )
        .unwrap();

    let events =
        stale::prune_stale_model_siblings_after_goal_unsatisfied(RunId::new(), &mut graph, first);

    assert_eq!(events.len(), 1);
    assert_eq!(graph.node(sibling).unwrap().status, PlanStatus::Pruned);
    assert_eq!(graph.node(explicit).unwrap().status, PlanStatus::Open);
}
