use super::BurbotRuntime;
use crate::belief::BeliefGraph;
use crate::contract::ContractRegistry;
use crate::failure::FailureKind;
use crate::graph::{
    scores_for_action, ActionRef, ActionScores, PlanEdgeKind, PlanGraph, PlanNode, PlanNodeKind,
    PlanStatus,
};
use crate::ids::{NodeId, RunId};
use crate::planner::{
    repair_candidates_for_failure, ActionCandidate, CandidateSource, CompletionRole,
};
use crate::rules::action_key;
use crate::trace::{trace_event, TraceEventType};
use anyhow::Result;
use serde_json::{json, Value};

impl BurbotRuntime {
    pub(super) fn record_failure(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        beliefs: &mut BeliefGraph,
        action_id: NodeId,
        action_ref: &ActionRef,
        args: &Value,
        output: &Value,
        failure_kind: FailureKind,
    ) -> Result<()> {
        self.repeated_failures
            .insert(action_key(action_ref, args), self.state_epoch);
        beliefs.record_action_failure(
            action_ref.contract_id.clone(),
            action_ref.action_name.clone(),
            args,
            "failure classified by runtime",
            output.clone(),
        );
        let failure_id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Failure,
            status: PlanStatus::Open,
            label: format!("Failure in {}", action_ref.action_name),
            payload: json!({
                "contract_id": action_ref.contract_id,
                "action_name": action_ref.action_name,
                "args": args,
                "failure_mode": failure_kind.as_str(),
            }),
            action_ref: None,
            scores: ActionScores::default(),
        });
        graph.add_edge(action_id, failure_id, PlanEdgeKind::FailedWith, json!({}));
        let mut classified = trace_event(
            run_id,
            TraceEventType::FailureClassified,
            Some(failure_id),
            args.clone(),
            json!({
                "retry_safety": "repair_observation_required",
            }),
        );
        classified.contract_id = Some(action_ref.contract_id.clone());
        classified.action_name = Some(action_ref.action_name.clone());
        classified.failure_mode = Some(failure_kind.as_str().to_string());
        let mut repair_candidates = repair_candidates_for_failure(
            self.contracts.as_ref(),
            action_ref,
            args,
            output,
            failure_kind.clone(),
        );
        if repair_candidates.is_empty() {
            if let Some(candidate) =
                self.symbolic_failure_diagnostic(action_ref, args, output, &failure_kind)
            {
                repair_candidates.push(candidate);
            }
        }
        classified.output["repairable"] = json!(!repair_candidates.is_empty());
        self.append(classified)?;
        for candidate in repair_candidates {
            self.add_repair_candidate_node(run_id, graph, failure_id, candidate)?;
        }
        Ok(())
    }

    fn symbolic_failure_diagnostic(
        &self,
        action_ref: &ActionRef,
        args: &Value,
        output: &Value,
        failure_kind: &FailureKind,
    ) -> Option<ActionCandidate> {
        let failed_action = action_ref.clone();
        let action_ref = crate::symbolic::symbolic_action_ref("DiagnoseFailure");
        let action = self
            .contracts
            .get_action(&action_ref.contract_id, &action_ref.action_name)?;
        let mut scores = scores_for_action(&action);
        scores.information_gain += 0.5;
        scores.expected_progress = 0.4;
        Some(ActionCandidate {
            action_ref,
            args: json!({
                "failed_action": {
                    "contract_id": failed_action.contract_id,
                    "action_name": failed_action.action_name,
                },
                "failed_args": args,
                "failure_output": output,
                "failure_kind": failure_kind.as_str(),
            }),
            source: CandidateSource::Repair,
            completion_role: CompletionRole::Repair,
            rationale:
                "generic symbolic failure diagnosis because the action has no declared repair rule"
                    .to_string(),
            scores,
        })
    }

    fn add_repair_candidate_node(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        failure_id: NodeId,
        candidate: ActionCandidate,
    ) -> Result<NodeId> {
        let rationale = candidate.rationale.clone();
        let id = self.add_candidate_node(run_id, graph, candidate, None, None)?;
        graph.add_edge(
            failure_id,
            id,
            PlanEdgeKind::Repairs,
            json!({"rationale": rationale}),
        );
        self.append(trace_event(
            run_id,
            TraceEventType::RepairAdded,
            Some(id),
            json!({"failure_node": failure_id.0}),
            json!({"rationale": rationale}),
        ))?;
        Ok(id)
    }
}
