use super::artifact_context::generated_artifact_context;
use super::progress::{model_proposed_node, progress_evidence};
use super::stale::prune_stale_model_siblings_after_goal_unsatisfied;
use super::BurbotRuntime;
use crate::belief::BeliefGraph;
use crate::contract::{CapabilityContract, ContractRegistry, SideEffectClass};
use crate::failure::FailureKind;
use crate::graph::{ActionRef, PlanGraph, PlanNodeKind, PlanStatus};
use crate::ids::{NodeId, RunId};
use crate::llm::{
    model_proposal_violations, propose_observe_act_candidates, propose_puffer_tool_call,
    retryable_openai_error, verify_goal_satisfied, ModelCandidateProposal,
};
use crate::model_policy::{validate_model_tool_args, ModelProposalViolation};
use crate::planner::{CandidateSource, CompletionRole};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use crate::runtime::model_feedback::{
    advancing_candidate_role_context, advancing_state_candidate_role_context,
    invalid_candidate_context, invalid_candidate_detail, non_advancing_candidate_context,
    non_advancing_candidate_detail,
};
use crate::runtime::model_loop_support::{
    accumulated_image_context, action_has_intent, action_history_summary,
    action_history_summary_excluding, compact_history_value, compact_value,
    failure_summary_for_action, frontier_summary, graph_has_executable_frontier,
    verified_target_summary, ModelProposalAttempt, SingleModelProposalAttempt,
    AWAIT_ASYNC_PROGRESS_INTENT,
};
use crate::runtime::RunOptions;
use crate::semantics::read_only_side_effect;
use crate::trace::{trace_event, TraceEventType};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

const OBSERVATION_SATURATION_STREAK: usize = 3;

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
        let mut context = json!({
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
        let mut structural_error = attempt.structural_error;
        let mut structural_violations = attempt.structural_violations;
        if proposals.is_empty() && structural_error.is_some() {
            context =
                invalid_candidate_context(&context, "initial_observe_act", &structural_violations);
            let retry = self.propose_observe_act_candidates_attempt(
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
            proposals = retry.proposals;
            retryable_error = retryable_error.or(retry.retryable_error);
            structural_error = retry.structural_error.or(structural_error);
            if !retry.structural_violations.is_empty() {
                structural_violations = retry.structural_violations;
            }
        }
        if proposals.is_empty() && single_tool_fallback_allowed(false, retryable_error.is_some()) {
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
            structural_error = fallback.structural_error.or(structural_error);
            if !fallback.structural_violations.is_empty() {
                structural_violations = fallback.structural_violations;
            }
            if let Some(proposal) = fallback.proposal {
                proposals.push(proposal);
            }
        }
        added += self
            .add_model_candidate_proposals(
                run_id,
                graph,
                NodeId(0),
                None,
                CandidateSource::ModelProposal,
                proposals,
            )?
            .added;
        if added == 0 {
            if let Some(error) = retryable_error {
                added += self.add_model_retry_candidate(
                    run_id,
                    graph,
                    NodeId(0),
                    "initial_model_proposal_unavailable",
                    &error,
                )?;
            } else if structural_error.is_some() && !graph_has_executable_frontier(graph) {
                added += self.add_recovery_model_candidates(
                    run_id,
                    graph,
                    goal,
                    NodeId(0),
                    "invalid_initial_model_candidates",
                    invalid_candidate_detail(
                        context,
                        "initial_observe_act",
                        &structural_violations,
                    ),
                    None,
                    options,
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
        if graph_has_executable_frontier(graph) {
            return Ok(0);
        }
        let failure_summary = failure_summary_for_action(graph, action_id);
        let saturation = self.observation_saturation_context(graph);
        let mut requires_advancing = failure_requires_advancing_candidates(&failure_summary);
        let mut requires_state_advancing =
            failure_requires_state_advancing_candidates(&failure_summary);
        if saturation.is_some() && !requires_advancing {
            requires_advancing = true;
            requires_state_advancing = true;
        }
        let compact_output = if requires_advancing {
            compact_history_value(output)
        } else {
            compact_value(output)
        };
        let mut context = json!({
            "phase": "after_observation",
            "action_id": action_id.0,
            "action_ref": action_ref,
            "args": compact_value(args),
            "success": success,
            "output": compact_output,
            "failure": failure_summary,
            "verified_target": verified_target_summary(graph, action_id),
            "generated_artifacts": generated_artifact_context(graph, self.contracts.as_ref(), Some(action_id)),
            "frontier": frontier_summary(graph),
            "history": action_history_summary_excluding(graph, self.contracts.as_ref(), Some(action_id)),
        });
        if let Some(saturation) = saturation {
            context["observation_saturation"] = saturation;
        }
        if requires_state_advancing {
            context = advancing_state_candidate_role_context(
                &context,
                "recent model-authored observation/probe actions produced output but no state or artifact witness",
            );
        } else if requires_advancing {
            context = advancing_candidate_role_context(
                &context,
                "the previous action failed with no_progress; support observations are exhausted until state changes",
            );
        }
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
        let mut structural_error = attempt.structural_error;
        let mut structural_violations = attempt.structural_violations;
        if proposals.is_empty() && structural_error.is_some() {
            context = invalid_candidate_context(&context, "observe_act", &structural_violations);
            let retry = self.propose_observe_act_candidates_attempt(
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
            proposals = retry.proposals;
            retryable_error = retryable_error.or(retry.retryable_error);
            structural_error = retry.structural_error.or(structural_error);
            if !retry.structural_violations.is_empty() {
                structural_violations = retry.structural_violations;
            }
        }
        if requires_advancing {
            let violations =
                reject_recovery_candidates(&contract, &mut proposals, requires_state_advancing);
            if !violations.is_empty() {
                context = invalid_candidate_context(&context, "observe_act", &violations);
                let retry = self.propose_observe_act_candidates_attempt(
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
                retryable_error = retryable_error.or(retry.retryable_error);
                structural_error = retry.structural_error.or(structural_error);
                if !retry.structural_violations.is_empty() {
                    structural_violations = retry.structural_violations;
                }
                proposals = retry.proposals;
                let retry_violations =
                    reject_recovery_candidates(&contract, &mut proposals, requires_state_advancing);
                if !retry_violations.is_empty() {
                    context = invalid_candidate_context(&context, "observe_act", &retry_violations);
                }
            }
        }
        if proposals.is_empty()
            && single_tool_fallback_allowed(requires_advancing, retryable_error.is_some())
        {
            let (fallback_role, fallback_rationale) = (
                CompletionRole::Support,
                "single structural tool-call fallback for post-observation evidence gathering",
            );
            let fallback = self.single_tool_proposal_attempt(
                run_id,
                observation_id,
                CandidateSource::ModelObservationProposal,
                model,
                goal,
                &contract,
                Some(&context),
                Some(&image_context),
                fallback_role,
                fallback_rationale,
            )?;
            retryable_error = retryable_error.or(fallback.retryable_error);
            structural_error = fallback.structural_error.or(structural_error);
            if !fallback.structural_violations.is_empty() {
                structural_violations = fallback.structural_violations;
            }
            if let Some(proposal) = fallback.proposal {
                proposals.push(proposal);
            }
        }
        let mut outcome = self.add_model_candidate_proposals(
            run_id,
            graph,
            observation_id,
            Some(action_id),
            CandidateSource::ModelObservationProposal,
            proposals,
        )?;
        if outcome.added > 0 {
            return Ok(outcome.added);
        }
        if outcome.is_non_advancing() && !graph_has_executable_frontier(graph) {
            context = non_advancing_candidate_context(&context, "observe_act", &outcome);
            requires_advancing = true;
            if requires_state_advancing {
                context = advancing_state_candidate_role_context(
                    &context,
                    "the previous observe-act proposal only repeated blocked, duplicate, or dependency-blocked work after no-progress model work",
                );
            } else {
                context = advancing_candidate_role_context(
                    &context,
                    "the previous observe-act proposal only repeated blocked, duplicate, or dependency-blocked work",
                );
            }
            let retry = self.propose_observe_act_candidates_attempt(
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
            retryable_error = retryable_error.or(retry.retryable_error);
            structural_error = retry.structural_error.or(structural_error);
            if !retry.structural_violations.is_empty() {
                structural_violations = retry.structural_violations;
            }
            let mut retry_proposals = retry.proposals;
            if requires_advancing {
                let violations = reject_recovery_candidates(
                    &contract,
                    &mut retry_proposals,
                    requires_state_advancing,
                );
                if !violations.is_empty() {
                    context = invalid_candidate_context(&context, "observe_act", &violations);
                }
            }
            outcome = self.add_model_candidate_proposals(
                run_id,
                graph,
                observation_id,
                Some(action_id),
                CandidateSource::ModelObservationProposal,
                retry_proposals,
            )?;
            if outcome.added > 0 {
                return Ok(outcome.added);
            }
        }
        if graph_has_executable_frontier(graph) {
            return Ok(0);
        }
        if let Some(error) = retryable_error {
            return self.add_model_retry_candidate(
                run_id,
                graph,
                observation_id,
                "post_observation_model_proposal_unavailable",
                &error,
            );
        }
        let recovery_reason = if structural_error.is_some() {
            if requires_advancing {
                "invalid_model_candidates_after_no_progress"
            } else {
                "invalid_model_candidates"
            }
        } else if outcome.is_non_advancing() {
            "non_advancing_model_candidates"
        } else {
            "empty_observation_candidates"
        };
        let recovery_detail = if structural_error.is_some() {
            invalid_candidate_detail(context, "observe_act", &structural_violations)
        } else if outcome.is_non_advancing() {
            non_advancing_candidate_detail(context, "observe_act", &outcome)
        } else {
            context
        };
        let recovered = self.add_recovery_model_candidates(
            run_id,
            graph,
            goal,
            observation_id,
            recovery_reason,
            recovery_detail,
            Some(output),
            options,
        )?;
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
        if graph_has_executable_frontier(graph) {
            return Ok(0);
        }
        let requirements = recovery_candidate_requirements(reason, &detail);
        let mut requires_advancing = requirements.requires_advancing;
        let requires_state_advancing = requirements.requires_state_advancing;
        let mut context = json!({
            "phase": "recovery",
            "reason": reason,
            "detail": compact_value(&detail),
            "generated_artifacts": generated_artifact_context(graph, self.contracts.as_ref(), None),
            "frontier": frontier_summary(graph),
            "history": action_history_summary(graph, self.contracts.as_ref()),
        });
        if requires_state_advancing {
            context = advancing_state_candidate_role_context(
                &context,
                "recovery followed model-authored work that produced no state or artifact witness",
            );
        } else if requires_advancing {
            context = advancing_candidate_role_context(
                &context,
                "recovery was entered because prior model candidates added no new executable work",
            );
        }
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
        let mut structural_error = attempt.structural_error;
        let mut structural_violations = attempt.structural_violations;
        let mut proposals = attempt.proposals;
        if proposals.is_empty() && structural_error.is_some() {
            context =
                invalid_candidate_context(&context, "recovery_observe_act", &structural_violations);
            let retry = self.propose_observe_act_candidates_attempt(
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
            retryable_error = retryable_error.or(retry.retryable_error);
            if !retry.structural_violations.is_empty() {
                structural_violations = retry.structural_violations;
            }
            structural_error = retry.structural_error.or(structural_error);
            proposals = retry.proposals;
        }
        if requires_advancing {
            let violations =
                reject_recovery_candidates(&contract, &mut proposals, requires_state_advancing);
            if !violations.is_empty() {
                context = invalid_candidate_context(&context, "recovery_observe_act", &violations);
                structural_violations = violations;
                structural_error.get_or_insert_with(|| {
                    "support candidates are not valid after support is exhausted".to_string()
                });
                let retry = self.propose_observe_act_candidates_attempt(
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
                retryable_error = retryable_error.or(retry.retryable_error);
                if !retry.structural_violations.is_empty() {
                    structural_violations = retry.structural_violations;
                }
                structural_error = retry.structural_error.or(structural_error);
                proposals = retry.proposals;
                let retry_violations =
                    reject_recovery_candidates(&contract, &mut proposals, requires_state_advancing);
                if !retry_violations.is_empty() {
                    context = invalid_candidate_context(
                        &context,
                        "recovery_observe_act",
                        &retry_violations,
                    );
                    structural_violations = retry_violations;
                    structural_error.get_or_insert_with(|| {
                        "support candidates are not valid after support is exhausted".to_string()
                    });
                }
            }
        }
        let mut outcome = self.add_model_candidate_proposals(
            run_id,
            graph,
            support_source,
            None,
            CandidateSource::ModelObservationProposal,
            proposals,
        )?;
        if outcome.added > 0 {
            return Ok(outcome.added);
        }
        if outcome.is_non_advancing() && !graph_has_executable_frontier(graph) {
            context = non_advancing_candidate_context(&context, "recovery_observe_act", &outcome);
            requires_advancing = true;
            if requires_state_advancing {
                context = advancing_state_candidate_role_context(
                    &context,
                    "the previous recovery proposal only repeated blocked, duplicate, or dependency-blocked work after no-progress model work",
                );
            } else {
                context = advancing_candidate_role_context(
                    &context,
                    "the previous recovery proposal only repeated blocked, duplicate, or dependency-blocked work",
                );
            }
            let retry = self.propose_observe_act_candidates_attempt(
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
            retryable_error = retryable_error.or(retry.retryable_error);
            if !retry.structural_violations.is_empty() {
                structural_violations = retry.structural_violations;
            }
            structural_error = retry.structural_error.or(structural_error);
            let mut retry_proposals = retry.proposals;
            if requires_advancing {
                let violations = reject_recovery_candidates(
                    &contract,
                    &mut retry_proposals,
                    requires_state_advancing,
                );
                if !violations.is_empty() {
                    context =
                        invalid_candidate_context(&context, "recovery_observe_act", &violations);
                    structural_violations = violations;
                    structural_error.get_or_insert_with(|| {
                        "support candidates are not valid after support is exhausted".to_string()
                    });
                }
            }
            outcome = self.add_model_candidate_proposals(
                run_id,
                graph,
                support_source,
                None,
                CandidateSource::ModelObservationProposal,
                retry_proposals,
            )?;
            if outcome.added > 0 {
                return Ok(outcome.added);
            }
            if outcome.is_non_advancing() {
                context =
                    non_advancing_candidate_context(&context, "recovery_observe_act", &outcome);
            }
        }
        if !single_tool_fallback_allowed(requires_advancing, retryable_error.is_some()) {
            if let Some(error) = retryable_error {
                return self.add_model_retry_candidate(
                    run_id,
                    graph,
                    support_source,
                    "recovery_model_proposal_unavailable",
                    &error,
                );
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
                CompletionRole::Repair,
                "single structural tool-call fallback for advancing recovery",
            )?;
            if let Some(error) = fallback.retryable_error {
                return self.add_model_retry_candidate(
                    run_id,
                    graph,
                    support_source,
                    "recovery_model_proposal_unavailable",
                    &error,
                );
            }
            if let Some(proposal) = fallback.proposal {
                let mut proposals = vec![proposal];
                let violations =
                    reject_recovery_candidates(&contract, &mut proposals, requires_state_advancing);
                if !violations.is_empty() {
                    context =
                        invalid_candidate_context(&context, "recovery_single_tool", &violations);
                    structural_violations = violations;
                    structural_error.get_or_insert_with(|| {
                        "state-advancing recovery requires a local write/edit repair or terminal action".to_string()
                    });
                }
                let outcome = self.add_model_candidate_proposals(
                    run_id,
                    graph,
                    support_source,
                    None,
                    CandidateSource::ModelObservationProposal,
                    proposals,
                )?;
                if outcome.added > 0 {
                    return Ok(outcome.added);
                }
                context =
                    non_advancing_candidate_context(&context, "recovery_single_tool", &outcome);
            } else if !fallback.structural_violations.is_empty() {
                structural_violations = fallback.structural_violations;
                structural_error = fallback.structural_error.or(structural_error);
            }
            if structural_error.is_some() {
                context = invalid_candidate_context(
                    &context,
                    "recovery_observe_act",
                    &structural_violations,
                );
                let retry = self.propose_observe_act_candidates_attempt(
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
                if let Some(error) = retry.retryable_error {
                    return self.add_model_retry_candidate(
                        run_id,
                        graph,
                        support_source,
                        "recovery_model_proposal_unavailable",
                        &error,
                    );
                }
                let mut retry_proposals = retry.proposals;
                let violations = reject_recovery_candidates(
                    &contract,
                    &mut retry_proposals,
                    requires_state_advancing,
                );
                if !violations.is_empty() {
                    self.append(trace_event(
                        run_id,
                        TraceEventType::ModelCandidatesProposed,
                        Some(support_source),
                        json!({
                            "source": CandidateSource::ModelObservationProposal,
                            "proposal_failure": "recovery_advancing_role",
                        }),
                        json!({
                            "added": 0,
                            "skipped_wait": 0,
                            "retryable": false,
                            "structural_violations": violations,
                            "error": "support candidates are not valid after support is exhausted",
                        }),
                    ))?;
                }
                let outcome = self.add_model_candidate_proposals(
                    run_id,
                    graph,
                    support_source,
                    None,
                    CandidateSource::ModelObservationProposal,
                    retry_proposals,
                )?;
                return Ok(outcome.added);
            }
            return Ok(0);
        }
        let (fallback_role, fallback_rationale) = (
            CompletionRole::Support,
            "single structural tool-call fallback for recovery evidence gathering",
        );
        if let Some(error) = retryable_error {
            return self.add_model_retry_candidate(
                run_id,
                graph,
                support_source,
                "recovery_model_proposal_unavailable",
                &error,
            );
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
            fallback_role,
            fallback_rationale,
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
            if structural_error.is_some() {
                return Ok(0);
            }
            return Ok(0);
        };
        Ok(self
            .add_model_candidate_proposals(
                run_id,
                graph,
                support_source,
                None,
                CandidateSource::ModelObservationProposal,
                vec![fallback],
            )?
            .added)
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
            "verified_target": verified_target_summary(graph, action_id),
            "generated_artifacts": generated_artifact_context(graph, self.contracts.as_ref(), Some(action_id)),
            "history": action_history_summary_excluding(graph, self.contracts.as_ref(), Some(action_id)),
        });
        let contract = self.puffer_tools_contract()?;
        let image_context = accumulated_image_context(graph, Some(output));
        self.append(trace_event(
            run_id,
            TraceEventType::GoalVerificationPerformed,
            Some(action_id),
            context.clone(),
            json!({"started": true}),
        ))?;
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
                let retryable = retryable_openai_error(&error);
                let error = error.to_string();
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
                "rejected_suggested_candidates": decision.rejected_suggested_candidates,
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
        for event in prune_stale_model_siblings_after_goal_unsatisfied(run_id, graph, action_id) {
            self.append(event)?;
        }
        let outcome = self.add_model_candidate_proposals(
            run_id,
            graph,
            action_id,
            Some(action_id),
            CandidateSource::ModelGoalVerifier,
            decision.suggested_candidates,
        )?;
        if outcome.added == 0 {
            let base_detail = json!({
                "satisfied": satisfied,
                "confidence": confidence,
                "missing_evidence": missing_evidence.clone(),
                "verification_context": insufficiency_output["verification_context"].clone(),
            });
            let (reason, detail) = if outcome.is_non_advancing() {
                (
                    "goal_verifier_non_advancing_candidates",
                    non_advancing_candidate_detail(base_detail, "goal_verifier", &outcome),
                )
            } else {
                ("goal_verifier_no_candidates", base_detail)
            };
            self.add_recovery_model_candidates(
                run_id,
                graph,
                goal,
                action_id,
                reason,
                detail,
                Some(output),
                options,
            )?;
        }
        Ok(false)
    }

    fn puffer_tools_contract(&self) -> Result<CapabilityContract> {
        self.contracts
            .active_contracts()
            .into_iter()
            .find(|contract| contract.contract_id == PUFFER_TOOLS_CONTRACT_ID)
            .ok_or_else(|| anyhow!("missing puffer.tools contract for model proposals"))
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
        let action = contract
            .actions
            .iter()
            .find(|action| action.name == proposal.tool_id)
            .ok_or_else(|| anyhow!("model proposed missing action {}", proposal.tool_id))?;
        let completion_role = single_tool_fallback_role(action, completion_role);
        validate_model_tool_args(action, &proposal.args, Some(completion_role.clone()))?;
        Ok(ModelCandidateProposal {
            id: None,
            tool_id: proposal.tool_id,
            args: proposal.args,
            completion_role,
            depends_on: Vec::new(),
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
        self.append(trace_event(
            run_id,
            TraceEventType::ModelCandidatesProposed,
            Some(support_source),
            json!({
                "source": source.clone(),
                "proposal_started": "observe_act",
            }),
            json!({
                "started": true,
                "context_phase": context.get("phase").cloned().unwrap_or(Value::Null),
            }),
        ))?;
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
                structural_error: None,
                structural_violations: Vec::new(),
            }),
            Err(error) => {
                let retryable = retryable_openai_error(&error);
                let structural_violations =
                    structural_violations_or_default(retryable, model_proposal_violations(&error));
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
                        "retryable": retryable,
                        "structural_violations": structural_violations.clone(),
                        "error": error,
                    }),
                ))?;
                let retryable_error = retryable.then_some(error.clone());
                let structural_error = (!retryable).then_some(error);
                Ok(ModelProposalAttempt {
                    proposals: Vec::new(),
                    retryable_error,
                    structural_error,
                    structural_violations,
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
        self.append(trace_event(
            run_id,
            TraceEventType::ModelCandidatesProposed,
            Some(support_source),
            json!({
                "source": source.clone(),
                "proposal_started": "single_tool",
            }),
            json!({
                "started": true,
                "completion_role": completion_role.clone(),
            }),
        ))?;
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
                structural_error: None,
                structural_violations: Vec::new(),
            }),
            Err(error) => {
                let retryable = retryable_openai_error(&error);
                let structural_violations =
                    structural_violations_or_default(retryable, model_proposal_violations(&error));
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
                        "retryable": retryable,
                        "structural_violations": structural_violations.clone(),
                        "error": error,
                    }),
                ))?;
                let retryable_error = retryable.then_some(error.clone());
                let structural_error = (!retryable).then_some(error);
                Ok(SingleModelProposalAttempt {
                    proposal: None,
                    retryable_error,
                    structural_error,
                    structural_violations,
                })
            }
        }
    }

    pub(super) fn observation_saturation_context(&self, graph: &PlanGraph) -> Option<Value> {
        let mut streak = 0usize;
        let mut recent_actions = Vec::new();
        let mut action_ids = graph
            .nodes
            .iter()
            .filter_map(|(id, node)| {
                (node.kind == PlanNodeKind::Action
                    && matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied))
                .then_some(*id)
            })
            .collect::<Vec<_>>();
        action_ids.sort_by_key(|id| id.0);
        for action_id in action_ids.into_iter().rev() {
            let Ok(node) = graph.node(action_id) else {
                continue;
            };
            let Some(action_ref) = node.action_ref.as_ref() else {
                continue;
            };
            let Some(action) = self
                .contracts
                .get_action(&action_ref.contract_id, &action_ref.action_name)
            else {
                continue;
            };
            let Some(role) = self.completion_role_for_node(graph, action_id) else {
                continue;
            };
            if matches!(role, CompletionRole::Repair | CompletionRole::Terminal) {
                break;
            }
            let output = self.observation_output_for_action(graph, action_id);
            if output
                .as_ref()
                .is_some_and(|output| progress_evidence(Some(&action), output).changes_state)
            {
                break;
            }
            if action.side_effect_class != SideEffectClass::Unknown
                || !matches!(role, CompletionRole::Support | CompletionRole::Verification)
                || !model_proposed_node(graph, action_id)
            {
                continue;
            }
            streak += 1;
            recent_actions.push(json!({
                "action_id": action_id.0,
                "action_ref": action_ref,
                "completion_role": role,
                "args": compact_value(&node.payload),
            }));
            if streak >= OBSERVATION_SATURATION_STREAK {
                recent_actions.reverse();
                return Some(json!({
                    "code": "model_observation_probe_saturated",
                    "streak": streak,
                    "threshold": OBSERVATION_SATURATION_STREAK,
                    "recent_actions": recent_actions,
                    "repair_hint": "use existing observations to propose repair or terminal work; do not add another non-state-changing probe unless state changed",
                }));
            }
        }
        None
    }
}

fn single_tool_fallback_allowed(requires_advancing: bool, retryable_provider_error: bool) -> bool {
    !requires_advancing && !retryable_provider_error
}

fn single_tool_fallback_role(
    action: &crate::contract::ActionContract,
    requested: CompletionRole,
) -> CompletionRole {
    if requested == CompletionRole::Support
        && action.side_effect_class == crate::contract::SideEffectClass::Unknown
    {
        return CompletionRole::Verification;
    }
    if requested == CompletionRole::Support && !read_only_side_effect(&action.side_effect_class) {
        return CompletionRole::Repair;
    }
    requested
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecoveryCandidateRequirements {
    requires_advancing: bool,
    requires_state_advancing: bool,
}

fn recovery_candidate_requirements(reason: &str, detail: &Value) -> RecoveryCandidateRequirements {
    let requires_state_advancing = recovery_requires_state_advancing_candidates(reason, detail);
    RecoveryCandidateRequirements {
        requires_advancing: recovery_requires_advancing_candidates(reason)
            || requires_state_advancing,
        requires_state_advancing,
    }
}

fn recovery_requires_advancing_candidates(reason: &str) -> bool {
    matches!(
        reason,
        "non_advancing_model_candidates"
            | "goal_verifier_non_advancing_candidates"
            | "artifact_review_non_advancing_candidates"
            | "artifact_review_no_candidates"
            | "no_executable_actions_after_failure"
            | "no_executable_actions_after_model_work"
            | "no_executable_actions_after_support"
            | "invalid_model_candidates_after_no_progress"
    )
}

fn failure_requires_advancing_candidates(failure: &Option<Value>) -> bool {
    failure
        .as_ref()
        .is_some_and(|failure| failure_mode_at_structured_context_paths(failure, "no_progress"))
}

fn failure_requires_state_advancing_candidates(failure: &Option<Value>) -> bool {
    failure_requires_advancing_candidates(failure)
}

fn recovery_requires_state_advancing_candidates(reason: &str, detail: &Value) -> bool {
    reason == "invalid_model_candidates_after_no_progress"
        || failure_mode_at_structured_context_paths(detail, "no_progress")
}

fn failure_mode_at_structured_context_paths(value: &Value, failure_mode: &str) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    if map.get("failure_mode").and_then(Value::as_str) == Some(failure_mode) {
        return true;
    }
    if map
        .get("failure")
        .is_some_and(|value| failure_mode_at_structured_context_paths(value, failure_mode))
    {
        return true;
    }
    if map
        .get("detail")
        .is_some_and(|value| failure_mode_at_structured_context_paths(value, failure_mode))
    {
        return true;
    }
    if let Some(previous) = map.get("previous_context") {
        return failure_mode_at_structured_context_paths(previous, failure_mode);
    }
    false
}

fn reject_support_recovery_candidates(
    contract: &CapabilityContract,
    proposals: &mut Vec<ModelCandidateProposal>,
) -> Vec<ModelProposalViolation> {
    let mut violations = Vec::new();
    let mut index = 0usize;
    proposals.retain(|proposal| {
        let reject = proposal.completion_role == CompletionRole::Support
            || contract
                .actions
                .iter()
                .find(|action| action.name == proposal.tool_id)
                .is_some_and(|action| {
                    matches!(
                        proposal.completion_role,
                        CompletionRole::Terminal | CompletionRole::Verification
                    ) && read_only_side_effect(&action.side_effect_class)
                        && !action_has_intent(action, AWAIT_ASYNC_PROGRESS_INTENT)
                });
        if reject {
            violations.push(ModelProposalViolation {
                code: "support_candidate_after_support_exhausted",
                path: format!("candidates[{index}].completion_role"),
                tool_id: Some(proposal.tool_id.clone()),
                actual: None,
                limit: None,
                repair_hint: "support observations are non-advancing in this recovery context; propose repair, verification, or terminal candidates with explicit roles",
            });
        }
        index += 1;
        !reject
    });
    violations
}

fn reject_state_advancing_candidates(
    contract: &CapabilityContract,
    proposals: &mut Vec<ModelCandidateProposal>,
) -> Vec<ModelProposalViolation> {
    let mut violations = Vec::new();
    let mut index = 0usize;
    proposals.retain(|proposal| {
        let action = contract
            .actions
            .iter()
            .find(|action| action.name == proposal.tool_id);
        let reject = matches!(
            proposal.completion_role,
            CompletionRole::Support | CompletionRole::Verification
        ) || action.is_some_and(|action| {
            read_only_side_effect(&action.side_effect_class)
                || (action.side_effect_class == SideEffectClass::Unknown
                    && proposal.completion_role != CompletionRole::Terminal)
        });
        if reject {
            violations.push(ModelProposalViolation {
                code: "non_state_advancing_candidate_after_probe_saturated",
                path: format!("candidates[{index}].completion_role"),
                tool_id: Some(proposal.tool_id.clone()),
                actual: None,
                limit: None,
                repair_hint: "recent model-authored work did not change state; propose a local write/edit repair that creates or updates an artifact, or a terminal action if existing evidence is enough",
            });
        }
        index += 1;
        !reject
    });
    violations
}

fn reject_recovery_candidates(
    contract: &CapabilityContract,
    proposals: &mut Vec<ModelCandidateProposal>,
    requires_state_advancing: bool,
) -> Vec<ModelProposalViolation> {
    if requires_state_advancing {
        return reject_state_advancing_candidates(contract, proposals);
    }
    reject_support_recovery_candidates(contract, proposals)
}

fn structural_violations_or_default(
    retryable: bool,
    violations: Vec<ModelProposalViolation>,
) -> Vec<ModelProposalViolation> {
    if retryable || !violations.is_empty() {
        return violations;
    }
    vec![ModelProposalViolation {
        code: "invalid_structural_output",
        path: "$".to_string(),
        tool_id: None,
        actual: None,
        limit: None,
        repair_hint:
            "return valid contract-gated JSON or tool-call arguments; escape string newlines and quotes as JSON string content",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ActionContract, ApprovalSpec, Idempotency, Reversibility, RiskLevel, SideEffectClass,
        VerificationSpec,
    };
    use serde_json::json;

    fn action(side_effect_class: SideEffectClass) -> ActionContract {
        ActionContract {
            name: "Tool".to_string(),
            description: "tool".to_string(),
            input_schema: json!({"type": "object", "additionalProperties": false}),
            output_schema: json!({"type": "object"}),
            side_effect_class,
            reversibility: Reversibility::Unknown,
            idempotency: Idempotency::Unknown,
            risk_level: RiskLevel::Medium,
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
    fn single_tool_support_fallback_wraps_unknown_actions_as_verification() {
        assert_eq!(
            single_tool_fallback_role(&action(SideEffectClass::Unknown), CompletionRole::Support),
            CompletionRole::Verification
        );
        assert_eq!(
            single_tool_fallback_role(&action(SideEffectClass::LocalRead), CompletionRole::Support),
            CompletionRole::Support
        );
    }

    #[test]
    fn single_tool_fallback_is_disabled_when_advancing_work_is_required() {
        assert!(!single_tool_fallback_allowed(true, false));
        assert!(!single_tool_fallback_allowed(false, true));
        assert!(single_tool_fallback_allowed(false, false));
    }

    #[test]
    fn invalid_candidates_after_no_progress_require_advancing_recovery() {
        assert!(recovery_requires_advancing_candidates(
            "invalid_model_candidates_after_no_progress"
        ));
        assert!(recovery_requires_advancing_candidates(
            "no_executable_actions_after_failure"
        ));
        assert!(recovery_requires_advancing_candidates(
            "no_executable_actions_after_model_work"
        ));
        assert!(recovery_requires_advancing_candidates(
            "no_executable_actions_after_support"
        ));
        assert!(recovery_requires_advancing_candidates(
            "artifact_review_no_candidates"
        ));
        assert!(!recovery_requires_advancing_candidates(
            "invalid_model_candidates"
        ));
        assert!(recovery_requires_state_advancing_candidates(
            "invalid_model_candidates_after_no_progress",
            &json!({})
        ));
        assert!(recovery_requires_state_advancing_candidates(
            "no_executable_actions_after_failure",
            &json!({"failure": {"failure_mode": "no_progress"}})
        ));
        assert!(recovery_requires_state_advancing_candidates(
            "invalid_model_candidates",
            &json!({
                "previous_context": {
                    "phase": "after_invalid_model_candidates",
                    "previous_context": {
                        "failure": {"failure_mode": "no_progress"}
                    }
                }
            })
        ));
        assert!(!recovery_requires_state_advancing_candidates(
            "no_executable_actions_after_failure",
            &json!({"failure": {"failure_mode": "non_zero_exit"}})
        ));
        assert!(!recovery_requires_state_advancing_candidates(
            "invalid_model_candidates",
            &json!({
                "previous_context": {
                    "output": {
                        "structured_output": {
                            "failure_mode": "no_progress"
                        }
                    }
                }
            })
        ));
        assert_eq!(
            recovery_candidate_requirements(
                "invalid_model_candidates",
                &json!({
                    "previous_context": {
                        "detail": {
                            "failure": {"failure_mode": "no_progress"}
                        }
                    }
                })
            ),
            RecoveryCandidateRequirements {
                requires_advancing: true,
                requires_state_advancing: true,
            }
        );
    }

    #[test]
    fn structural_parse_errors_get_machine_readable_feedback() {
        let violations = structural_violations_or_default(false, Vec::new());

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].code, "invalid_structural_output");
        assert_eq!(violations[0].path, "$");
    }

    #[test]
    fn state_advancing_recovery_rejects_probes_but_keeps_write_and_terminal() {
        let mut write_action = action(SideEffectClass::LocalWrite);
        write_action.name = "Write".to_string();
        let contract = CapabilityContract {
            contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
            version: "0.1.0".to_string(),
            status: crate::contract::ContractStatus::Active,
            trust_level: crate::contract::TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![action(SideEffectClass::Unknown), write_action],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        };
        let mut proposals = vec![
            ModelCandidateProposal {
                id: Some("probe".to_string()),
                tool_id: "Tool".to_string(),
                args: json!({}),
                completion_role: CompletionRole::Verification,
                depends_on: Vec::new(),
                rationale: "probe".to_string(),
            },
            ModelCandidateProposal {
                id: Some("repair".to_string()),
                tool_id: "Tool".to_string(),
                args: json!({}),
                completion_role: CompletionRole::Repair,
                depends_on: Vec::new(),
                rationale: "repair".to_string(),
            },
            ModelCandidateProposal {
                id: Some("terminal".to_string()),
                tool_id: "Tool".to_string(),
                args: json!({}),
                completion_role: CompletionRole::Terminal,
                depends_on: Vec::new(),
                rationale: "terminal".to_string(),
            },
            ModelCandidateProposal {
                id: Some("write".to_string()),
                tool_id: "Write".to_string(),
                args: json!({}),
                completion_role: CompletionRole::Repair,
                depends_on: Vec::new(),
                rationale: "write".to_string(),
            },
        ];

        let violations = reject_state_advancing_candidates(&contract, &mut proposals);

        assert_eq!(violations.len(), 2);
        assert_eq!(
            violations[0].code,
            "non_state_advancing_candidate_after_probe_saturated"
        );
        assert_eq!(
            proposals
                .iter()
                .map(|proposal| proposal.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["terminal", "write"]
        );
    }
}
