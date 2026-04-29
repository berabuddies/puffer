use super::BurbotRuntime;
use crate::belief::BeliefGraph;
use crate::contract::{CapabilityContract, ContractRegistry};
use crate::failure::FailureKind;
use crate::graph::{
    scores_for_action, ActionRef, PlanEdgeKind, PlanGraph, PlanNodeKind, PlanStatus,
};
use crate::ids::{NodeId, RunId};
use crate::llm::{
    propose_observe_act_candidates, propose_puffer_tool_call, retryable_openai_error_message,
    verify_goal_satisfied, ModelCandidateProposal,
};
use crate::planner::{ActionCandidate, CandidateSource, CompletionRole};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use crate::rules::action_key;
use crate::runtime::model_loop_support::{
    accumulated_image_context, action_has_intent, action_history_summary, compact_value,
    frontier_summary, graph_has_async_progress_source, graph_has_executable_frontier,
    model_support_edge_for, ModelProposalAttempt, SingleModelProposalAttempt,
    AWAIT_ASYNC_PROGRESS_INTENT,
};
use crate::runtime::RunOptions;
use crate::semantics::{payload_for_intent, read_only_side_effect, NormalizedIntent};
use crate::trace::{trace_event, TraceEventType};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

impl BurbotRuntime {
    pub(super) fn add_initial_model_candidates(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        goal: &str,
        options: &RunOptions,
    ) -> Result<usize> {
        let Some(model) = options.model.as_deref() else {
            return Ok(0);
        };
        let context = json!({
            "phase": "initial",
            "frontier": frontier_summary(graph),
            "history": action_history_summary(graph, self.contracts.as_ref()),
        });
        let mut added = 0;
        if let Some(candidate) = self.workspace_survey_candidate() {
            if !self.has_current_action_candidate(graph, &candidate.action_ref, &candidate.args) {
                self.add_candidate_node(run_id, graph, candidate, Some(NodeId(0)), None)?;
                added += 1;
            }
        }
        let contract = self.puffer_tools_contract()?;
        let image_context = accumulated_image_context(graph, None);
        let attempt = self.propose_observe_act_candidates_attempt(
            run_id,
            NodeId(0),
            CandidateSource::ModelProposal,
            &self.workspace_root,
            model,
            goal,
            &contract,
            &context,
            Some(&image_context),
        )?;
        let mut proposals = attempt.proposals;
        let mut retryable_error = attempt.retryable_error;
        if proposals.is_empty() {
            if graph_has_executable_frontier(graph) {
                return Ok(0);
            }
            let fallback = self.single_tool_proposal_attempt(
                run_id,
                NodeId(0),
                CandidateSource::ModelProposal,
                model,
                goal,
                &contract,
                Some(&context),
                Some(&image_context),
                CompletionRole::Support,
                "single structural tool-call fallback for initial evidence gathering",
            )?;
            retryable_error = retryable_error.or(fallback.retryable_error);
            if let Some(proposal) = fallback.proposal {
                proposals.push(proposal);
            }
        }
        added += self.add_model_candidate_proposals(
            run_id,
            graph,
            NodeId(0),
            None,
            CandidateSource::ModelProposal,
            proposals,
        )?;
        if added == 0 {
            if let Some(error) = retryable_error {
                added += self.add_model_retry_candidate(
                    run_id,
                    graph,
                    NodeId(0),
                    "initial_model_proposal_unavailable",
                    &error,
                )?;
            }
        }
        Ok(added)
    }

    pub(super) fn add_model_observe_act_candidates(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        goal: &str,
        observation_id: NodeId,
        action_id: NodeId,
        action_ref: &ActionRef,
        args: &Value,
        success: bool,
        output: &Value,
        options: &RunOptions,
    ) -> Result<usize> {
        let Some(model) = options.model.as_deref() else {
            return Ok(0);
        };
        let context = json!({
            "phase": "after_observation",
            "action_id": action_id.0,
            "action_ref": action_ref,
            "args": compact_value(args),
            "success": success,
            "output": compact_value(output),
            "frontier": frontier_summary(graph),
            "history": action_history_summary(graph, self.contracts.as_ref()),
        });
        let contract = self.puffer_tools_contract()?;
        let image_context = accumulated_image_context(graph, Some(output));
        let attempt = self.propose_observe_act_candidates_attempt(
            run_id,
            observation_id,
            CandidateSource::ModelObservationProposal,
            &self.workspace_root,
            model,
            goal,
            &contract,
            &context,
            Some(&image_context),
        )?;
        let mut proposals = attempt.proposals;
        let mut retryable_error = attempt.retryable_error;
        if proposals.is_empty() {
            let fallback = self.single_tool_proposal_attempt(
                run_id,
                observation_id,
                CandidateSource::ModelObservationProposal,
                model,
                goal,
                &contract,
                Some(&context),
                Some(&image_context),
                CompletionRole::Support,
                "single structural tool-call fallback for post-observation evidence gathering",
            )?;
            retryable_error = retryable_error.or(fallback.retryable_error);
            if let Some(proposal) = fallback.proposal {
                proposals.push(proposal);
            }
        }
        let added = self.add_model_candidate_proposals(
            run_id,
            graph,
            observation_id,
            Some(action_id),
            CandidateSource::ModelObservationProposal,
            proposals,
        )?;
        if added > 0 {
            return Ok(added);
        }
        if graph_has_executable_frontier(graph) {
            return Ok(0);
        }
        let mut recovered = self.add_recovery_model_candidates(
            run_id,
            graph,
            goal,
            observation_id,
            "empty_observation_candidates",
            context,
            Some(output),
            options,
        )?;
        if recovered == 0 {
            if let Some(error) = retryable_error {
                recovered += self.add_model_retry_candidate(
                    run_id,
                    graph,
                    observation_id,
                    "post_observation_model_proposal_unavailable",
                    &error,
                )?;
            }
        }
        Ok(recovered)
    }

    /// Adds structural recovery candidates when the graph has exhausted runnable actions.
    pub(super) fn add_recovery_model_candidates(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        goal: &str,
        support_source: NodeId,
        reason: &str,
        detail: Value,
        image_context: Option<&Value>,
        options: &RunOptions,
    ) -> Result<usize> {
        let Some(model) = options.model.as_deref() else {
            return Ok(0);
        };
        let context = json!({
            "phase": "recovery",
            "reason": reason,
            "detail": compact_value(&detail),
            "frontier": frontier_summary(graph),
            "history": action_history_summary(graph, self.contracts.as_ref()),
        });
        let contract = self.puffer_tools_contract()?;
        let image_context = accumulated_image_context(graph, image_context);
        let attempt = self.propose_observe_act_candidates_attempt(
            run_id,
            support_source,
            CandidateSource::ModelObservationProposal,
            &self.workspace_root,
            model,
            goal,
            &contract,
            &context,
            Some(&image_context),
        )?;
        let mut retryable_error = attempt.retryable_error;
        let added = self.add_model_candidate_proposals(
            run_id,
            graph,
            support_source,
            None,
            CandidateSource::ModelObservationProposal,
            attempt.proposals,
        )?;
        if added > 0 {
            return Ok(added);
        }
        let fallback = self.single_tool_proposal_attempt(
            run_id,
            support_source,
            CandidateSource::ModelObservationProposal,
            model,
            goal,
            &contract,
            Some(&context),
            Some(&image_context),
            CompletionRole::Support,
            "single structural tool-call fallback for recovery evidence gathering",
        )?;
        retryable_error = retryable_error.or(fallback.retryable_error);
        let Some(fallback) = fallback.proposal else {
            if let Some(error) = retryable_error {
                return self.add_model_retry_candidate(
                    run_id,
                    graph,
                    support_source,
                    "recovery_model_proposal_unavailable",
                    &error,
                );
            }
            return Ok(0);
        };
        self.add_model_candidate_proposals(
            run_id,
            graph,
            support_source,
            None,
            CandidateSource::ModelObservationProposal,
            vec![fallback],
        )
    }

    pub(super) fn goal_verified_or_expand(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        beliefs: &mut BeliefGraph,
        goal: &str,
        action_id: NodeId,
        action_ref: &ActionRef,
        args: &Value,
        output: &Value,
        options: &RunOptions,
    ) -> Result<bool> {
        let Some(model) = options.model.as_deref() else {
            return Ok(true);
        };
        let context = json!({
            "action_id": action_id.0,
            "action_ref": action_ref,
            "args": compact_value(args),
            "output": compact_value(output),
            "frontier": frontier_summary(graph),
            "history": action_history_summary(graph, self.contracts.as_ref()),
        });
        let contract = self.puffer_tools_contract()?;
        let image_context = accumulated_image_context(graph, Some(output));
        let decision = match verify_goal_satisfied(
            &self.workspace_root,
            model,
            goal,
            &contract,
            &context,
            Some(&image_context),
        ) {
            Ok(decision) => decision,
            Err(error) => {
                let error = error.to_string();
                let retryable = retryable_openai_error_message(&error);
                let failure_output = json!({
                    "satisfied": false,
                    "confidence": 0.0,
                    "missing_evidence": [],
                    "error": error,
                    "retryable": retryable,
                    "verification_context": context.clone(),
                });
                self.append(trace_event(
                    run_id,
                    TraceEventType::GoalVerificationPerformed,
                    Some(action_id),
                    context.clone(),
                    {
                        let mut output = failure_output.clone();
                        output["suggested_candidates"] = Value::Null;
                        output
                    },
                ))?;
                if retryable {
                    self.add_model_retry_candidate(
                        run_id,
                        graph,
                        action_id,
                        "goal_verifier_unavailable",
                        failure_output["error"].as_str().unwrap_or_default(),
                    )?;
                    return Ok(false);
                }
                self.record_failure(
                    run_id,
                    graph,
                    beliefs,
                    action_id,
                    action_ref,
                    args,
                    &failure_output,
                    FailureKind::GoalUnsatisfied,
                )?;
                self.add_recovery_model_candidates(
                    run_id,
                    graph,
                    goal,
                    action_id,
                    "goal_verifier_failed",
                    failure_output,
                    Some(output),
                    options,
                )?;
                return Ok(false);
            }
        };
        let satisfied = decision.satisfied;
        let confidence = decision.confidence;
        let missing_evidence = decision.missing_evidence.clone();
        self.append(trace_event(
            run_id,
            TraceEventType::GoalVerificationPerformed,
            Some(action_id),
            context.clone(),
            json!({
                "satisfied": satisfied,
                "confidence": confidence,
                "missing_evidence": missing_evidence,
                "suggested_candidates": decision.suggested_candidates.len(),
            }),
        ))?;
        if satisfied
            && confidence >= options.goal_verification_min_confidence
            && missing_evidence.is_empty()
        {
            return Ok(true);
        }
        let insufficiency_output = json!({
            "satisfied": satisfied,
            "confidence": confidence,
            "missing_evidence": missing_evidence.clone(),
            "verification_context": context,
        });
        self.record_failure(
            run_id,
            graph,
            beliefs,
            action_id,
            action_ref,
            args,
            &insufficiency_output,
            FailureKind::GoalUnsatisfied,
        )?;
        self.prune_stale_model_siblings_after_goal_unsatisfied(run_id, graph, action_id)?;
        let added = self.add_model_candidate_proposals(
            run_id,
            graph,
            action_id,
            Some(action_id),
            CandidateSource::ModelGoalVerifier,
            decision.suggested_candidates,
        )?;
        if added == 0 {
            self.add_recovery_model_candidates(
                run_id,
                graph,
                goal,
                action_id,
                "goal_verifier_no_candidates",
                json!({
                    "satisfied": satisfied,
                    "confidence": confidence,
                    "missing_evidence": missing_evidence.clone(),
                    "verification_context": insufficiency_output["verification_context"].clone(),
                }),
                Some(output),
                options,
            )?;
        }
        Ok(false)
    }

    pub(super) fn add_model_candidate_proposals(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        support_source: NodeId,
        verified_target: Option<NodeId>,
        source: CandidateSource,
        proposals: Vec<ModelCandidateProposal>,
    ) -> Result<usize> {
        let mut added = 0;
        let mut skipped_wait = 0;
        let mut skipped_duplicate = 0;
        for proposal in proposals {
            let candidate = self.model_candidate_to_action_candidate(proposal, source.clone())?;
            if self.should_skip_model_wait_candidate(graph, &candidate) {
                skipped_wait += 1;
                continue;
            }
            if self.has_current_action_candidate(graph, &candidate.action_ref, &candidate.args) {
                skipped_duplicate += 1;
                continue;
            }
            let verifies = (candidate.completion_role == CompletionRole::Verification)
                .then_some(verified_target)
                .flatten();
            self.add_candidate_node(run_id, graph, candidate, Some(support_source), verifies)?;
            added += 1;
        }
        self.append(trace_event(
            run_id,
            TraceEventType::ModelCandidatesProposed,
            Some(support_source),
            json!({"source": source}),
            json!({
                "added": added,
                "skipped_wait": skipped_wait,
                "skipped_duplicate": skipped_duplicate,
            }),
        ))?;
        Ok(added)
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
        let mut scores = scores_for_action(&action);
        match proposal.completion_role {
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
        Ok(ActionCandidate {
            action_ref,
            args: proposal.args,
            source,
            completion_role: proposal.completion_role,
            rationale: proposal.rationale,
            scores,
        })
    }

    fn puffer_tools_contract(&self) -> Result<CapabilityContract> {
        self.contracts
            .active_contracts()
            .into_iter()
            .find(|contract| contract.contract_id == PUFFER_TOOLS_CONTRACT_ID)
            .ok_or_else(|| anyhow!("missing puffer.tools contract for model proposals"))
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
                    self.completion_role_for_node(graph, node_id) == CompletionRole::Terminal
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

    fn single_tool_proposal(
        &self,
        model: &str,
        goal: &str,
        contract: &CapabilityContract,
        context: Option<&Value>,
        image_context: Option<&Value>,
        completion_role: CompletionRole,
        rationale: &str,
    ) -> Result<ModelCandidateProposal> {
        let expanded_goal;
        let proposal_goal = if let Some(context) = context {
            expanded_goal = format!(
                "{goal}\n\nStructural context JSON:\n{}",
                serde_json::to_string_pretty(context)?
            );
            expanded_goal.as_str()
        } else {
            goal
        };
        let proposal = propose_puffer_tool_call(
            &self.workspace_root,
            model,
            proposal_goal,
            contract,
            image_context,
        )?;
        Ok(ModelCandidateProposal {
            tool_id: proposal.tool_id,
            args: proposal.args,
            completion_role,
            rationale: rationale.to_string(),
        })
    }

    fn propose_observe_act_candidates_attempt(
        &self,
        run_id: RunId,
        support_source: NodeId,
        source: CandidateSource,
        workspace_root: &std::path::Path,
        model: &str,
        goal: &str,
        contract: &CapabilityContract,
        context: &Value,
        image_context: Option<&Value>,
    ) -> Result<ModelProposalAttempt> {
        match propose_observe_act_candidates(
            workspace_root,
            model,
            goal,
            contract,
            context,
            image_context,
        ) {
            Ok(proposals) => Ok(ModelProposalAttempt {
                proposals,
                retryable_error: None,
            }),
            Err(error) => {
                let error = error.to_string();
                self.append(trace_event(
                    run_id,
                    TraceEventType::ModelCandidatesProposed,
                    Some(support_source),
                    json!({
                        "source": source,
                        "proposal_failure": "observe_act",
                    }),
                    json!({
                        "added": 0,
                        "skipped_wait": 0,
                        "retryable": retryable_openai_error_message(&error),
                        "error": error,
                    }),
                ))?;
                let retryable_error = retryable_openai_error_message(&error).then_some(error);
                Ok(ModelProposalAttempt {
                    proposals: Vec::new(),
                    retryable_error,
                })
            }
        }
    }

    fn single_tool_proposal_attempt(
        &self,
        run_id: RunId,
        support_source: NodeId,
        source: CandidateSource,
        model: &str,
        goal: &str,
        contract: &CapabilityContract,
        context: Option<&Value>,
        image_context: Option<&Value>,
        completion_role: CompletionRole,
        rationale: &str,
    ) -> Result<SingleModelProposalAttempt> {
        match self.single_tool_proposal(
            model,
            goal,
            contract,
            context,
            image_context,
            completion_role,
            rationale,
        ) {
            Ok(proposal) => Ok(SingleModelProposalAttempt {
                proposal: Some(proposal),
                retryable_error: None,
            }),
            Err(error) => {
                let error = error.to_string();
                self.append(trace_event(
                    run_id,
                    TraceEventType::ModelCandidatesProposed,
                    Some(support_source),
                    json!({
                        "source": source,
                        "proposal_failure": "single_tool",
                    }),
                    json!({
                        "added": 0,
                        "skipped_wait": 0,
                        "retryable": retryable_openai_error_message(&error),
                        "error": error,
                    }),
                ))?;
                let retryable_error = retryable_openai_error_message(&error).then_some(error);
                Ok(SingleModelProposalAttempt {
                    proposal: None,
                    retryable_error,
                })
            }
        }
    }

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
        if self.model_retry_epoch == Some(self.state_epoch) {
            self.append(trace_event(
                run_id,
                TraceEventType::ModelCandidatesProposed,
                Some(support_source),
                json!({
                    "source": "model_retry_wait",
                    "reason": reason,
                    "blocked": "repeated_model_provider_error",
                }),
                json!({
                    "added": 0,
                    "state_epoch": self.state_epoch,
                    "error": error,
                }),
            ))?;
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
            completion_role: CompletionRole::Terminal,
            rationale: format!(
                "retry structural model proposal after retryable provider error: {error}"
            ),
            scores,
        };
        self.add_candidate_node(run_id, graph, candidate, Some(support_source), None)?;
        self.model_retry_epoch = Some(self.state_epoch);
        Ok(1)
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

    /// Prunes stale same-batch model actions after a concrete unsatisfied verifier result.
    pub(super) fn prune_stale_model_siblings_after_goal_unsatisfied(
        &self,
        run_id: RunId,
        graph: &mut PlanGraph,
        action_id: NodeId,
    ) -> Result<usize> {
        let Some((support_source, source)) = model_support_edge_for(graph, action_id) else {
            return Ok(0);
        };
        if !matches!(
            source.as_str(),
            "model_proposal" | "model_observation_proposal" | "model_goal_verifier"
        ) {
            return Ok(0);
        }
        let mut pruned = 0;
        let sibling_ids = graph
            .edges
            .iter()
            .filter(|edge| {
                edge.kind == PlanEdgeKind::Supports
                    && edge.source == support_source
                    && edge.target != action_id
                    && edge.payload.get("source").and_then(Value::as_str) == Some(source.as_str())
                    && edge
                        .payload
                        .get("completion_role")
                        .and_then(Value::as_str)
                        .map(|role| matches!(role, "terminal" | "support"))
                        .unwrap_or(true)
            })
            .map(|edge| edge.target)
            .collect::<Vec<_>>();
        for sibling_id in sibling_ids {
            let node = graph.node_mut(sibling_id)?;
            if node.kind == PlanNodeKind::Action && node.status == PlanStatus::Open {
                node.status = PlanStatus::Pruned;
                pruned += 1;
            }
        }
        if pruned > 0 {
            self.append(trace_event(
                run_id,
                TraceEventType::RewriteApplied,
                Some(action_id),
                json!({"rule": "PruneStaleModelSiblingsAfterGoalUnsatisfied"}),
                json!({
                    "pruned": pruned,
                    "support_source": support_source.0,
                    "source": source,
                }),
            ))?;
        }
        Ok(pruned)
    }

    fn workspace_survey_candidate(&self) -> Option<ActionCandidate> {
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

    fn has_current_action_candidate(
        &self,
        graph: &PlanGraph,
        action_ref: &ActionRef,
        args: &Value,
    ) -> bool {
        if graph.nodes.values().any(|node| {
            node.kind == PlanNodeKind::Action
                && node.status == PlanStatus::Open
                && node.action_ref.as_ref() == Some(action_ref)
                && node.payload == *args
        }) {
            return true;
        }
        self.executed_action_epochs
            .get(&action_key(action_ref, args))
            .is_some_and(|epoch| *epoch == self.state_epoch)
    }
}
