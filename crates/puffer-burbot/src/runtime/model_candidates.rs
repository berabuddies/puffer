use super::model_loop_support::{
    action_has_intent, compact_value, graph_has_async_progress_source,
    ModelCandidateProposalOutcome, AWAIT_ASYNC_PROGRESS_INTENT,
};
use super::BurbotRuntime;
use crate::contract::{ContractRegistry, SideEffectClass};
use crate::graph::{
    scores_for_action, ActionRef, PlanEdgeKind, PlanGraph, PlanNodeKind, PlanStatus,
};
use crate::ids::{NodeId, RunId};
use crate::llm::ModelCandidateProposal;
use crate::model_policy::adjust_model_action_scores;
use crate::planner::{ActionCandidate, CandidateSource, CompletionRole};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use crate::rules::action_key_for_contracts;
use crate::semantics::{action_intents, read_only_side_effect};
use crate::trace::{trace_event, TraceEventType};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

impl BurbotRuntime {
    /// Adds model proposals to the plan graph and reports accepted/skipped work.
    pub(super) fn add_model_candidate_proposals(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        support_source: NodeId,
        verified_target: Option<NodeId>,
        source: CandidateSource,
        proposals: Vec<ModelCandidateProposal>,
    ) -> Result<ModelCandidateProposalOutcome> {
        let mut added: usize = 0;
        let proposed = proposals.len();
        let mut skipped_wait = 0;
        let mut skipped_duplicate = 0;
        let mut skipped_dependency = 0;
        let mut skipped_policy = 0;
        let mut skipped_duplicate_candidates = Vec::new();
        let mut skipped_policy_candidates = Vec::new();
        let mut candidate_nodes_by_id = BTreeMap::new();
        let mut pending_dependencies = Vec::new();
        for proposal in proposals {
            let proposal_id = proposal.id.clone();
            let depends_on = proposal.depends_on.clone();
            let candidate = self.model_candidate_to_action_candidate(proposal, source.clone())?;
            if self.should_skip_model_wait_candidate(graph, &candidate) {
                skipped_wait += 1;
                continue;
            }
            if let Some(reason) = self.model_candidate_policy_skip_reason(graph, &candidate) {
                skipped_policy += 1;
                push_skipped_candidate(&mut skipped_policy_candidates, &candidate, reason);
                continue;
            }
            if let Some(existing) =
                self.current_action_candidate_node(graph, &candidate.action_ref, &candidate.args)
            {
                if let Some(id) = proposal_id {
                    candidate_nodes_by_id.insert(id, existing);
                }
                skipped_duplicate += 1;
                push_duplicate_candidate(&mut skipped_duplicate_candidates, &candidate);
                continue;
            }
            if self.has_current_action_candidate(graph, &candidate.action_ref, &candidate.args) {
                skipped_duplicate += 1;
                push_duplicate_candidate(&mut skipped_duplicate_candidates, &candidate);
                continue;
            }
            let verifies = (source == CandidateSource::ModelGoalVerifier
                && candidate.completion_role == CompletionRole::Verification)
                .then_some(verified_target)
                .flatten();
            let node_id =
                self.add_candidate_node(run_id, graph, candidate, Some(support_source), verifies)?;
            if let Some(id) = proposal_id {
                candidate_nodes_by_id.insert(id, node_id);
            }
            if !depends_on.is_empty() {
                pending_dependencies.push((node_id, depends_on));
            }
            added += 1;
        }
        for (node_id, dependencies) in pending_dependencies {
            let mut unresolved = Vec::new();
            for dependency in dependencies {
                if let Some(previous) = candidate_nodes_by_id.get(&dependency) {
                    graph.add_edge(
                        *previous,
                        node_id,
                        PlanEdgeKind::DependsOn,
                        json!({
                            "source": "model_candidate_dependency",
                            "candidate_id": dependency,
                        }),
                    );
                } else {
                    unresolved.push(dependency);
                }
            }
            if !unresolved.is_empty() {
                skipped_dependency += 1;
                if let Some(node) = graph.nodes.get_mut(&node_id) {
                    if node.status == PlanStatus::Open {
                        node.status = PlanStatus::Blocked;
                        added = added.saturating_sub(1);
                    }
                }
                self.append(trace_event(
                    run_id,
                    TraceEventType::RewriteApplied,
                    Some(node_id),
                    json!({"source": source, "unresolved_depends_on": unresolved}),
                    json!({"reason": "ignored_unresolved_model_candidate_dependency"}),
                ))?;
            }
        }
        self.append(trace_event(
            run_id,
            TraceEventType::ModelCandidatesProposed,
            Some(support_source),
            json!({"source": source}),
            json!({
                "added": added,
                "proposed": proposed,
                "skipped_wait": skipped_wait,
                "skipped_duplicate": skipped_duplicate,
                "skipped_dependency": skipped_dependency,
                "skipped_policy": skipped_policy,
                "no_new_candidates": added == 0 && proposed > 0,
                "duplicate_candidates": skipped_duplicate_candidates.clone(),
                "policy_skipped_candidates": skipped_policy_candidates.clone(),
            }),
        ))?;
        Ok(ModelCandidateProposalOutcome {
            added,
            proposed,
            skipped_wait,
            skipped_duplicate,
            skipped_dependency,
            skipped_policy,
            skipped_duplicate_candidates,
            skipped_policy_candidates,
        })
    }

    fn model_candidate_to_action_candidate(
        &self,
        proposal: ModelCandidateProposal,
        source: CandidateSource,
    ) -> Result<ActionCandidate> {
        let action_ref = ActionRef {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            action_name: proposal.tool_id,
        };
        let action = self
            .contracts
            .get_action(&action_ref.contract_id, &action_ref.action_name)
            .ok_or_else(|| anyhow!("model proposed missing action {}", action_ref.action_name))?;
        let completion_role =
            normalized_model_completion_role(&action, &source, proposal.completion_role);
        let mut scores = scores_for_action(&action);
        match completion_role {
            CompletionRole::Terminal => {
                scores.expected_progress += 0.6;
                scores.verification_value += 0.2;
            }
            CompletionRole::Verification => {
                scores.expected_progress += 0.25;
                scores.verification_value += 1.0;
            }
            CompletionRole::Repair => {
                scores.expected_progress += 0.25;
                scores.information_gain += 0.3;
            }
            CompletionRole::Support => {
                scores.information_gain += 0.35;
            }
        }
        scores.uncertainty_penalty += 0.15;
        adjust_model_action_scores(&action, &proposal.args, &mut scores);
        Ok(ActionCandidate {
            action_ref,
            args: proposal.args,
            source,
            completion_role,
            rationale: proposal.rationale,
            scores,
        })
    }

    fn should_skip_model_wait_candidate(
        &self,
        graph: &PlanGraph,
        candidate: &ActionCandidate,
    ) -> bool {
        let Some(candidate_action) = self.contracts.get_action(
            &candidate.action_ref.contract_id,
            &candidate.action_ref.action_name,
        ) else {
            return false;
        };
        if !action_has_intent(&candidate_action, AWAIT_ASYNC_PROGRESS_INTENT)
            || graph_has_async_progress_source(graph, self.contracts.as_ref())
        {
            return false;
        }
        candidate.completion_role != CompletionRole::Terminal
            || graph
                .executable_frontier_actions()
                .into_iter()
                .any(|node_id| {
                    self.completion_role_for_node(graph, node_id) == Some(CompletionRole::Terminal)
                        && graph
                            .node(node_id)
                            .ok()
                            .and_then(|node| node.action_ref.as_ref())
                            .and_then(|action_ref| {
                                self.contracts
                                    .get_action(&action_ref.contract_id, &action_ref.action_name)
                            })
                            .is_some_and(|action| {
                                !action_has_intent(&action, AWAIT_ASYNC_PROGRESS_INTENT)
                            })
                })
    }

    fn model_candidate_policy_skip_reason(
        &self,
        graph: &PlanGraph,
        candidate: &ActionCandidate,
    ) -> Option<&'static str> {
        let action = self.contracts.get_action(
            &candidate.action_ref.contract_id,
            &candidate.action_ref.action_name,
        )?;
        let candidate_key = action_key_for_contracts(
            self.contracts.as_ref(),
            &candidate.action_ref,
            &candidate.args,
        );
        if action.side_effect_class == SideEffectClass::Unknown
            && !self.quarantined_artifact_paths.is_empty()
        {
            return Some("unknown_side_effect_blocked_by_quarantined_artifact");
        }
        if self
            .repeated_failures
            .get(&candidate_key)
            .is_some_and(|epoch| *epoch == self.state_epoch)
        {
            return Some("repeated_failure_until_state_changes");
        }
        if candidate.completion_role != CompletionRole::Repair {
            return None;
        }
        if action.side_effect_class != SideEffectClass::Unknown {
            return None;
        }
        graph
            .edges
            .iter()
            .filter(|edge| edge.kind == PlanEdgeKind::FailedWith)
            .filter_map(|edge| graph.nodes.get(&edge.target))
            .any(|failure| {
                failure.payload.get("contract_id").and_then(Value::as_str)
                    == Some(candidate.action_ref.contract_id.as_str())
                    && failure.payload.get("action_name").and_then(Value::as_str)
                        == Some(candidate.action_ref.action_name.as_str())
                    && failure.payload.get("failure_mode").and_then(Value::as_str)
                        == Some("no_progress")
                    && failure
                        .payload
                        .get("completion_role")
                        .and_then(Value::as_str)
                        == Some("repair")
                    && failure.payload.get("state_epoch").and_then(Value::as_u64)
                        == Some(self.state_epoch)
                    && failure.payload.get("args").is_some_and(|args| {
                        let failed_ref = ActionRef {
                            contract_id: candidate.action_ref.contract_id.clone(),
                            action_name: candidate.action_ref.action_name.clone(),
                        };
                        action_key_for_contracts(self.contracts.as_ref(), &failed_ref, args)
                            == candidate_key
                    })
            })
            .then_some("unknown_side_effect_repair_no_progress_until_state_changes")
    }

    pub(super) fn has_current_action_candidate(
        &self,
        graph: &PlanGraph,
        action_ref: &ActionRef,
        args: &Value,
    ) -> bool {
        if self
            .current_action_candidate_node(graph, action_ref, args)
            .is_some()
        {
            return true;
        }
        if !self.action_execution_fresh(action_ref, args) {
            return false;
        }
        !self.read_refresh_allowed_after_newer_failure(graph, NodeId(0), action_ref, args)
    }

    fn current_action_candidate_node(
        &self,
        graph: &PlanGraph,
        action_ref: &ActionRef,
        args: &Value,
    ) -> Option<NodeId> {
        let target_key = action_key_for_contracts(self.contracts.as_ref(), action_ref, args);
        if let Some(id) = graph.nodes.iter().find_map(|(id, node)| {
            (node.kind == PlanNodeKind::Action
                && node.status == PlanStatus::Open
                && node.action_ref.as_ref().is_some_and(|node_action_ref| {
                    action_key_for_contracts(
                        self.contracts.as_ref(),
                        node_action_ref,
                        &node.payload,
                    ) == target_key
                }))
            .then_some(*id)
        }) {
            return Some(id);
        }
        if let Some(id) = graph.nodes.iter().find_map(|(id, node)| {
            (node.kind == PlanNodeKind::Action
                && node.status == PlanStatus::Open
                && node.action_ref.as_ref() == Some(action_ref)
                && action_args_semantically_dominate(
                    self.contracts.as_ref(),
                    action_ref,
                    &node.payload,
                    args,
                ))
            .then_some(*id)
        }) {
            return Some(id);
        }
        if let Some(id) = graph.nodes.iter().find_map(|(id, node)| {
            (node.kind == PlanNodeKind::Action
                && matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied)
                && node.action_ref.as_ref().is_some_and(|node_action_ref| {
                    action_key_for_contracts(
                        self.contracts.as_ref(),
                        node_action_ref,
                        &node.payload,
                    ) == target_key
                        && self.executed_action_fresh_for_candidate(
                            graph,
                            *id,
                            node_action_ref,
                            &node.payload,
                            action_ref,
                            args,
                        )
                }))
            .then_some(*id)
        }) {
            return Some(id);
        }
        graph.nodes.iter().find_map(|(id, node)| {
            (node.kind == PlanNodeKind::Action
                && matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied)
                && node.action_ref.as_ref() == Some(action_ref)
                && self.executed_action_fresh_for_candidate(
                    graph,
                    *id,
                    action_ref,
                    &node.payload,
                    action_ref,
                    args,
                )
                && action_args_semantically_dominate(
                    self.contracts.as_ref(),
                    action_ref,
                    &node.payload,
                    args,
                ))
            .then_some(*id)
        })
    }

    fn executed_action_fresh_for_candidate(
        &self,
        graph: &PlanGraph,
        existing_id: NodeId,
        existing_action_ref: &ActionRef,
        existing_args: &Value,
        candidate_action_ref: &ActionRef,
        candidate_args: &Value,
    ) -> bool {
        if !self.action_execution_fresh(existing_action_ref, existing_args) {
            return false;
        }
        !self.read_refresh_allowed_after_newer_failure(
            graph,
            existing_id,
            candidate_action_ref,
            candidate_args,
        )
    }

    fn read_refresh_allowed_after_newer_failure(
        &self,
        graph: &PlanGraph,
        existing_read_id: NodeId,
        candidate_action_ref: &ActionRef,
        candidate_args: &Value,
    ) -> bool {
        let Some(candidate_path_key) =
            self.read_file_path_key(candidate_action_ref, candidate_args)
        else {
            return false;
        };
        let Some(failure_id) = self.newest_current_epoch_failure_after(graph, existing_read_id)
        else {
            return false;
        };
        !self.has_read_file_observation_after(graph, &candidate_path_key, failure_id)
    }

    fn newest_current_epoch_failure_after(
        &self,
        graph: &PlanGraph,
        node_id: NodeId,
    ) -> Option<NodeId> {
        graph
            .nodes
            .iter()
            .filter_map(|(id, node)| {
                (id.0 > node_id.0
                    && node.kind == PlanNodeKind::Failure
                    && node.payload.get("state_epoch").and_then(Value::as_u64)
                        == Some(self.state_epoch))
                .then_some(*id)
            })
            .max()
    }

    fn has_read_file_observation_after(
        &self,
        graph: &PlanGraph,
        candidate_path_key: &str,
        failure_id: NodeId,
    ) -> bool {
        graph.nodes.iter().any(|(id, node)| {
            id.0 > failure_id.0
                && node.kind == PlanNodeKind::Action
                && matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied)
                && node
                    .action_ref
                    .as_ref()
                    .and_then(|action_ref| self.read_file_path_key(action_ref, &node.payload))
                    .as_deref()
                    == Some(candidate_path_key)
        })
    }
}

fn push_duplicate_candidate(duplicates: &mut Vec<Value>, candidate: &ActionCandidate) {
    push_skipped_candidate(
        duplicates,
        candidate,
        "already_open_or_executed_in_current_state_epoch",
    );
}

fn push_skipped_candidate(
    skipped_candidates: &mut Vec<Value>,
    candidate: &ActionCandidate,
    reason: &'static str,
) {
    skipped_candidates.push(json!({
        "tool_id": candidate.action_ref.action_name,
        "args": compact_value(&candidate.args),
        "reason": reason,
    }));
}

fn action_args_semantically_dominate(
    contracts: &dyn ContractRegistry,
    action_ref: &ActionRef,
    existing_args: &Value,
    candidate_args: &Value,
) -> bool {
    let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
    else {
        return false;
    };
    if !read_only_side_effect(&action.side_effect_class)
        || !object_subset(existing_args, candidate_args)
    {
        return false;
    }
    let existing_intents = action_intents(&action, existing_args);
    let candidate_intents = action_intents(&action, candidate_args);
    existing_intents.iter().any(|existing| {
        existing.normalized.intent == "read_file"
            && candidate_intents.iter().any(|candidate| {
                candidate.normalized.intent == existing.normalized.intent
                    && candidate.normalized.slots == existing.normalized.slots
            })
    })
}

fn object_subset(existing: &Value, candidate: &Value) -> bool {
    let (Some(existing), Some(candidate)) = (existing.as_object(), candidate.as_object()) else {
        return false;
    };
    existing
        .iter()
        .all(|(key, value)| candidate.get(key) == Some(value))
}

fn normalized_model_completion_role(
    action: &crate::contract::ActionContract,
    source: &CandidateSource,
    requested: CompletionRole,
) -> CompletionRole {
    if !matches!(
        requested,
        CompletionRole::Terminal | CompletionRole::Verification
    ) || !read_only_side_effect(&action.side_effect_class)
        || action_has_intent(action, AWAIT_ASYNC_PROGRESS_INTENT)
    {
        return requested;
    }
    if *source == CandidateSource::ModelGoalVerifier {
        CompletionRole::Verification
    } else {
        CompletionRole::Support
    }
}
