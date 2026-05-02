use super::{BurbotRuntime, VerificationOutcome};
use crate::contract::ContractRegistry;
use crate::graph::ActionRef;
use crate::ids::{NodeId, RunId};
use crate::trace::{trace_event, TraceEventType};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

impl BurbotRuntime {
    pub(super) fn verify_observation(
        &self,
        run_id: RunId,
        node_id: NodeId,
        action_ref: &ActionRef,
        observation_success: bool,
        output: &Value,
    ) -> Result<VerificationOutcome> {
        let action = self
            .contracts
            .get_action(&action_ref.contract_id, &action_ref.action_name)
            .ok_or_else(|| anyhow!("missing action contract"))?;
        let required = action.verification.required_before_completion;
        let passed = if required {
            observation_success
                && crate::verification::observation_checks_pass(&action, output).unwrap_or(false)
        } else {
            observation_success
        };
        if required {
            let mut event = trace_event(
                run_id,
                TraceEventType::VerificationPerformed,
                Some(node_id),
                output.clone(),
                json!({
                    "passed": passed,
                    "methods": action.verification.methods,
                    "source": if passed { "observation_check" } else { "pending_or_failed" },
                }),
            );
            event.contract_id = Some(action_ref.contract_id.clone());
            event.action_name = Some(action_ref.action_name.clone());
            event.success = Some(passed);
            self.append(event)?;
        }
        Ok(VerificationOutcome { required, passed })
    }
}
