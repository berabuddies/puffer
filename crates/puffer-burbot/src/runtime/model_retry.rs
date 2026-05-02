use super::model_loop_support::{
    action_has_intent, graph_has_executable_frontier, AWAIT_ASYNC_PROGRESS_INTENT,
};
use super::BurbotRuntime;
use crate::contract::ContractRegistry;
use crate::graph::{scores_for_action, ActionRef, PlanGraph};
use crate::ids::{NodeId, RunId};
use crate::planner::{ActionCandidate, CandidateSource, CompletionRole};
use crate::semantics::{payload_for_intent, read_only_side_effect, NormalizedIntent};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

impl BurbotRuntime {
    /// Adds a contract-declared wait action after a retryable model provider failure.
    pub(super) fn add_model_retry_candidate(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        support_source: NodeId,
        reason: &str,
        error: &str,
    ) -> Result<usize> {
        if graph_has_executable_frontier(graph) {
            return Ok(0);
        }
        let Some(action_ref) = self.await_progress_action_ref() else {
            return Ok(0);
        };
        let action = self
            .contracts
            .get_action(&action_ref.contract_id, &action_ref.action_name)
            .ok_or_else(|| anyhow!("missing await-progress action {}", action_ref.action_name))?;
        self.model_retry_sequence = self.model_retry_sequence.saturating_add(1);
        let mut scores = scores_for_action(&action);
        scores.information_gain += 0.1;
        scores.uncertainty_penalty += 0.05;
        let intent = NormalizedIntent {
            intent: AWAIT_ASYNC_PROGRESS_INTENT.to_string(),
            slots: BTreeMap::new(),
        };
        let mut args = payload_for_intent(&action, &intent).unwrap_or_else(|| json!({}));
        if let Value::Object(object) = &mut args {
            if action.input_schema.pointer("/properties/reason").is_some() {
                object.insert(
                    "reason".to_string(),
                    json!(format!(
                        "{reason}: retryable model provider error #{}",
                        self.model_retry_sequence
                    )),
                );
            }
        }
        let candidate = ActionCandidate {
            action_ref,
            args,
            source: CandidateSource::ModelObservationProposal,
            completion_role: CompletionRole::Support,
            rationale: format!(
                "retry structural model proposal after retryable provider error: {error}"
            ),
            scores,
        };
        self.add_candidate_node(run_id, graph, candidate, Some(support_source), None)?;
        Ok(1)
    }

    /// Builds a cheap read-only workspace survey candidate from contract semantics.
    pub(super) fn workspace_survey_candidate(&self) -> Option<ActionCandidate> {
        let mut slots = BTreeMap::new();
        slots.insert("pattern".to_string(), "*".to_string());
        let intent = NormalizedIntent {
            intent: "glob_paths".to_string(),
            slots,
        };
        for contract in self.contracts.active_contracts() {
            let contract_id = contract.contract_id.clone();
            for action in contract.actions {
                if !read_only_side_effect(&action.side_effect_class) {
                    continue;
                }
                let Some(args) = payload_for_intent(&action, &intent) else {
                    continue;
                };
                let mut scores = scores_for_action(&action);
                scores.information_gain += 1.0;
                scores.expected_progress += 0.2;
                return Some(ActionCandidate {
                    action_ref: ActionRef {
                        contract_id,
                        action_name: action.name,
                    },
                    args,
                    source: CandidateSource::CheapObservation,
                    completion_role: CompletionRole::Support,
                    rationale: "contract-declared read-only workspace survey before acting"
                        .to_string(),
                    scores,
                });
            }
        }
        None
    }

    fn await_progress_action_ref(&self) -> Option<ActionRef> {
        for contract in self.contracts.active_contracts() {
            let contract_id = contract.contract_id;
            for action in contract.actions {
                if action_has_intent(&action, AWAIT_ASYNC_PROGRESS_INTENT) {
                    return Some(ActionRef {
                        contract_id,
                        action_name: action.name,
                    });
                }
            }
        }
        None
    }
}
