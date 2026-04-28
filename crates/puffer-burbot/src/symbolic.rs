use crate::contract::{
    ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, Idempotency, Reversibility,
    RiskLevel, SideEffectClass, TrustLevel, VerificationSpec,
};
use crate::executor::{ActionInvocation, Executor, Observation};
use serde_json::{json, Value};

pub(crate) const BURBOT_SYMBOLIC_CONTRACT_ID: &str = "burbot.symbolic";

pub(crate) struct SymbolicExecutor;

impl Executor for SymbolicExecutor {
    fn contract_id(&self) -> &'static str {
        BURBOT_SYMBOLIC_CONTRACT_ID
    }

    fn execute(&self, invocation: ActionInvocation) -> anyhow::Result<Observation> {
        let output = match invocation.action_name.as_str() {
            "ProposePlan" => propose_plan_output(&invocation.args),
            "CritiquePlan" => critique_plan_output(&invocation.args),
            "VerifyClaim" => verify_claim_output(&invocation.args),
            "DiagnoseFailure" => diagnose_failure_output(&invocation.args),
            _ => json!({
                "success": false,
                "error": format!("unknown symbolic worker {}", invocation.action_name),
            }),
        };
        let success = output.get("success").and_then(Value::as_bool) == Some(true);
        Ok(Observation {
            invocation,
            success,
            output,
            error: None,
            raw: None,
            latency_ms: Some(0),
            cost: Some(0.0),
        })
    }
}

pub(crate) fn symbolic_contract() -> CapabilityContract {
    CapabilityContract {
        contract_id: BURBOT_SYMBOLIC_CONTRACT_ID.to_string(),
        version: "0.1.0".to_string(),
        status: ContractStatus::Active,
        trust_level: TrustLevel::Trusted,
        description: "Burbot internal symbolic planning workers".to_string(),
        actions: vec![
            symbolic_action(
                "ProposePlan",
                "propose guarded symbolic plan alternatives without executing tools",
            ),
            symbolic_action(
                "CritiquePlan",
                "critique symbolic plans for risk, missing observations, and verification gaps",
            ),
            symbolic_action(
                "VerifyClaim",
                "verify symbolic claims against declared contract and plan metadata",
            ),
            symbolic_action(
                "DiagnoseFailure",
                "diagnose a failed action without retrying or mutating external state",
            ),
        ],
        global_constraints: vec![
            "symbolic workers may propose, critique, or verify plan metadata only".to_string(),
            "symbolic workers must not execute external tools or mutate workspace state"
                .to_string(),
        ],
        forbidden_uses: vec![
            "executing shell commands".to_string(),
            "writing files or sending communications".to_string(),
            "overriding protected safety policy".to_string(),
        ],
        local_rules: Vec::new(),
        examples: Vec::new(),
        contract_hash: None,
    }
}

pub(crate) fn symbolic_action_ref(action_name: &str) -> crate::graph::ActionRef {
    crate::graph::ActionRef {
        contract_id: BURBOT_SYMBOLIC_CONTRACT_ID.to_string(),
        action_name: action_name.to_string(),
    }
}

fn symbolic_action(name: &str, description: &str) -> ActionContract {
    ActionContract {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        side_effect_class: SideEffectClass::PureObservation,
        reversibility: Reversibility::Reversible,
        idempotency: Idempotency::Idempotent,
        risk_level: RiskLevel::Low,
        preconditions: vec!["worker input is plan, claim, or contract metadata".to_string()],
        postconditions: vec!["symbolic observation is returned without tool execution".to_string()],
        verification: VerificationSpec {
            methods: vec!["inspect returned symbolic worker JSON".to_string()],
            templates: Vec::new(),
            required_before_completion: false,
            confidence: 0.8,
        },
        approval: ApprovalSpec {
            required: false,
            reason: None,
        },
        failure_modes: Vec::new(),
        forbidden_uses: vec![
            "performing physical tool execution".to_string(),
            "treating model-like text as safety policy".to_string(),
        ],
        argument_safety: Vec::new(),
        semantic_intents: Vec::new(),
        intent_extractors: Vec::new(),
        repair_rules: Vec::new(),
        cost_estimate: Some("0".to_string()),
        latency_estimate: Some("0".to_string()),
    }
}

fn propose_plan_output(args: &Value) -> Value {
    json!({
        "success": true,
        "worker": "proposal",
        "does_not_execute_tools": true,
        "goal": args.get("goal").cloned().unwrap_or(Value::Null),
        "proposal": {
            "strategy": "keep guarded alternatives alive and execute only the next safe frontier action",
            "requires_critique": true,
            "requires_contract_gate": true,
        },
    })
}

fn critique_plan_output(args: &Value) -> Value {
    json!({
        "success": true,
        "worker": "critic",
        "does_not_execute_tools": true,
        "goal": args.get("goal").cloned().unwrap_or(Value::Null),
        "findings": [
            "prefer low-risk observations before risky terminal actions",
            "block repeats unless repair evidence changes the state",
            "require declared verification before completion when contracts demand it"
        ],
    })
}

fn verify_claim_output(args: &Value) -> Value {
    json!({
        "success": true,
        "worker": "verifier",
        "does_not_execute_tools": true,
        "claim": args.get("claim").cloned().unwrap_or(Value::Null),
        "verified": true,
        "scope": "symbolic_plan_metadata",
    })
}

fn diagnose_failure_output(args: &Value) -> Value {
    json!({
        "success": true,
        "worker": "failure_diagnoser",
        "does_not_execute_tools": true,
        "failure": args,
        "diagnosis": {
            "retry_allowed": false,
            "requires_contract_repair": true,
            "requires_state_change_before_repeat": true,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ActionInvocation;
    use crate::ids::{NodeId, RunId};

    #[test]
    fn symbolic_executor_returns_pure_observation() {
        let executor = SymbolicExecutor;
        let observation = executor
            .execute(ActionInvocation {
                run_id: RunId::new(),
                plan_node_id: NodeId(1),
                contract_id: BURBOT_SYMBOLIC_CONTRACT_ID.to_string(),
                action_name: "ProposePlan".to_string(),
                args: json!({"goal": "inspect safely"}),
            })
            .unwrap();

        assert!(observation.success);
        assert_eq!(observation.output["does_not_execute_tools"], true);
        assert_eq!(observation.output["worker"], "proposal");
    }

    #[test]
    fn symbolic_contract_is_pure_and_low_risk() {
        let contract = symbolic_contract();

        assert_eq!(contract.contract_id, BURBOT_SYMBOLIC_CONTRACT_ID);
        assert!(contract.actions.iter().all(|action| {
            action.side_effect_class == SideEffectClass::PureObservation
                && action.risk_level == RiskLevel::Low
                && !action.approval.required
        }));
    }
}
