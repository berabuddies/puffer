use super::*;
use crate::contract::{
    ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, Idempotency, Reversibility,
    RiskLevel, SemanticIntentSpec, SideEffectClass, TrustLevel, VerificationSpec,
};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
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
fn retryable_model_error_adds_contract_declared_wait_candidate() {
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
    let mut graph = PlanGraph::from_goal("retry".to_string());

    let added = runtime
        .add_model_retry_candidate(
            RunId::new(),
            &mut graph,
            NodeId(0),
            "provider_unavailable",
            "OpenAI request failed with status 503",
        )
        .unwrap();

    assert_eq!(added, 1);
    let wait = graph
        .nodes
        .values()
        .find(|node| {
            node.action_ref
                .as_ref()
                .is_some_and(|action_ref| action_ref.action_name == "Sleep")
        })
        .expect("sleep retry candidate should be present");
    assert_eq!(wait.payload["duration_ms"], 1000);
    assert!(wait.payload["reason"]
        .as_str()
        .is_some_and(|reason| reason.contains("provider_unavailable")));
    assert!(graph.edges.iter().any(|edge| {
        edge.target == wait.id && edge.payload["completion_role"] == json!("support")
    }));
    graph.node_mut(wait.id).unwrap().status = PlanStatus::Executed;
    let allowed_second_provider_retry = runtime
        .add_model_retry_candidate(
            RunId::new(),
            &mut graph,
            NodeId(0),
            "provider_unavailable",
            "OpenAI request failed with status 503",
        )
        .unwrap();
    assert_eq!(allowed_second_provider_retry, 1);
}
