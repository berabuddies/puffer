use super::BurbotRuntime;
use crate::contract::{ActionContract, CapabilityContract, ContractRegistry};
use crate::graph::{
    scores_for_action, ActionRef, PlanEdgeKind, PlanGraph, PlanNodeKind, PlanStatus,
};
use crate::ids::{NodeId, RunId};
use crate::llm::{
    propose_observe_act_candidates, propose_puffer_tool_call, verify_goal_satisfied,
    ModelCandidateProposal,
};
use crate::planner::{ActionCandidate, CandidateSource, CompletionRole};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use crate::runtime::RunOptions;
use crate::semantics::{
    payload_for_intent, read_only_side_effect, semantic_symbol, NormalizedIntent,
};
use crate::trace::{trace_event, TraceEventType};
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const MAX_MODEL_STRING_CHARS: usize = 6_000;
const MAX_MODEL_ARRAY_ITEMS: usize = 20;
const AWAIT_ASYNC_PROGRESS_INTENT: &str = "await_async_progress";
const CREATES_ASYNC_PROGRESS_INTENT: &str = "creates_async_progress";

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
            "history": action_history_summary(graph),
        });
        let mut added = 0;
        if let Some(candidate) = self.workspace_survey_candidate() {
            if !graph_has_action_candidate(graph, &candidate.action_ref, &candidate.args) {
                self.add_candidate_node(run_id, graph, candidate, Some(NodeId(0)), None)?;
                added += 1;
            }
        }
        let contract = self.puffer_tools_contract()?;
        let image_context = accumulated_image_context(graph, None);
        let mut proposals = propose_observe_act_candidates(
            &self.workspace_root,
            model,
            goal,
            &contract,
            &context,
            Some(&image_context),
        )?;
        if proposals.is_empty() {
            if graph_has_executable_frontier(graph) {
                return Ok(0);
            }
            proposals.push(self.single_tool_proposal(model, goal, &contract, Some(&context))?);
        }
        added += self.add_model_candidate_proposals(
            run_id,
            graph,
            NodeId(0),
            None,
            CandidateSource::ModelProposal,
            proposals,
        )?;
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
            "history": action_history_summary(graph),
        });
        let contract = self.puffer_tools_contract()?;
        let image_context = accumulated_image_context(graph, Some(output));
        let mut proposals = propose_observe_act_candidates(
            &self.workspace_root,
            model,
            goal,
            &contract,
            &context,
            Some(&image_context),
        )?;
        if proposals.is_empty() {
            proposals.push(self.single_tool_proposal(model, goal, &contract, Some(&context))?);
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
        self.add_recovery_model_candidates(
            run_id,
            graph,
            goal,
            observation_id,
            "empty_observation_candidates",
            context,
            Some(output),
            options,
        )
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
            "history": action_history_summary(graph),
        });
        let contract = self.puffer_tools_contract()?;
        let image_context = accumulated_image_context(graph, image_context);
        let proposals = propose_observe_act_candidates(
            &self.workspace_root,
            model,
            goal,
            &contract,
            &context,
            Some(&image_context),
        )?;
        let added = self.add_model_candidate_proposals(
            run_id,
            graph,
            support_source,
            None,
            CandidateSource::ModelObservationProposal,
            proposals,
        )?;
        if added > 0 {
            return Ok(added);
        }
        if graph_has_executable_frontier(graph) {
            return Ok(0);
        }
        let fallback = self.single_tool_proposal(model, goal, &contract, Some(&context))?;
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
            "history": action_history_summary(graph),
        });
        let contract = self.puffer_tools_contract()?;
        let image_context = accumulated_image_context(graph, Some(output));
        let decision = verify_goal_satisfied(
            &self.workspace_root,
            model,
            goal,
            &contract,
            &context,
            Some(&image_context),
        )?;
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
        let added = self.add_model_candidate_proposals(
            run_id,
            graph,
            action_id,
            Some(action_id),
            CandidateSource::ModelGoalVerifier,
            decision.suggested_candidates,
        )?;
        if added == 0 && !graph_has_executable_frontier(graph) {
            self.add_recovery_model_candidates(
                run_id,
                graph,
                goal,
                action_id,
                "goal_verifier_no_candidates",
                json!({
                    "satisfied": satisfied,
                    "confidence": confidence,
                    "missing_evidence": missing_evidence,
                    "verification_context": context,
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
        for proposal in proposals {
            let candidate = self.model_candidate_to_action_candidate(proposal, source.clone())?;
            if self.should_skip_model_wait_candidate(graph, &candidate) {
                skipped_wait += 1;
                continue;
            }
            if graph_has_action_candidate(graph, &candidate.action_ref, &candidate.args) {
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
            json!({"added": added, "skipped_wait": skipped_wait}),
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
            context,
        )?;
        Ok(ModelCandidateProposal {
            tool_id: proposal.tool_id,
            args: proposal.args,
            completion_role: CompletionRole::Terminal,
            rationale: "single structural tool-call fallback after empty candidate set".to_string(),
        })
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
}

fn graph_has_action_candidate(graph: &PlanGraph, action_ref: &ActionRef, args: &Value) -> bool {
    graph.nodes.values().any(|node| {
        node.kind == PlanNodeKind::Action
            && matches!(
                node.status,
                PlanStatus::Open | PlanStatus::Executed | PlanStatus::Satisfied
            )
            && node.action_ref.as_ref() == Some(action_ref)
            && node.payload == *args
    })
}

fn graph_has_executable_frontier(graph: &PlanGraph) -> bool {
    !graph.executable_frontier_actions().is_empty()
}

fn graph_has_async_progress_source(graph: &PlanGraph, contracts: &dyn ContractRegistry) -> bool {
    graph.nodes.values().any(|node| {
        matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied)
            && node.kind == PlanNodeKind::Action
            && node
                .action_ref
                .as_ref()
                .and_then(|action_ref| {
                    contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
                })
                .is_some_and(|action| action_has_intent(&action, CREATES_ASYNC_PROGRESS_INTENT))
    })
}

fn action_has_intent(action: &ActionContract, intent: &str) -> bool {
    action
        .semantic_intents
        .iter()
        .any(|declared| semantic_symbol(&declared.intent) == intent)
}

fn frontier_summary(graph: &PlanGraph) -> Vec<Value> {
    graph
        .frontier_actions()
        .into_iter()
        .filter_map(|id| graph.node(id).ok())
        .take(20)
        .map(|node| {
            json!({
                "id": node.id.0,
                "label": node.label,
                "action_ref": node.action_ref,
                "args": compact_value(&node.payload),
            })
        })
        .collect()
}

fn action_history_summary(graph: &PlanGraph) -> Vec<Value> {
    let mut actions = graph
        .nodes
        .iter()
        .filter_map(|(id, node)| {
            (node.kind == PlanNodeKind::Action).then(|| {
                json!({
                    "id": id.0,
                    "status": node.status,
                    "action_ref": node.action_ref,
                    "args": compact_value(&node.payload),
                    "output": observation_output_for_action(graph, *id).map(|value| compact_value(&value)),
                })
            })
        })
        .collect::<Vec<_>>();
    if actions.len() > 40 {
        actions = actions.split_off(actions.len() - 40);
    }
    actions
}

fn observation_output_for_action(graph: &PlanGraph, action_id: NodeId) -> Option<Value> {
    graph
        .edges
        .iter()
        .find(|edge| edge.source == action_id && edge.kind == PlanEdgeKind::Produces)
        .and_then(|edge| graph.nodes.get(&edge.target))
        .map(|node| node.payload.clone())
}

fn accumulated_image_context(graph: &PlanGraph, current: Option<&Value>) -> Value {
    let mut values = Vec::new();
    if let Some(current) = current {
        values.push(current.clone());
    }
    for node in graph.nodes.values() {
        if node.kind == PlanNodeKind::Observation {
            values.push(node.payload.clone());
        }
    }
    Value::Array(values)
}

fn compact_value(value: &Value) -> Value {
    match value {
        Value::String(text) if text.chars().count() > MAX_MODEL_STRING_CHARS => {
            let prefix = text
                .chars()
                .take(MAX_MODEL_STRING_CHARS)
                .collect::<String>();
            json!({
                "truncated_string_prefix": prefix,
                "original_chars": text.chars().count(),
            })
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(MAX_MODEL_ARRAY_ITEMS)
                .map(compact_value)
                .collect(),
        ),
        Value::Object(object) => {
            if let Some(image) = compact_image_metadata(object) {
                return image;
            }
            let mut compact = Map::new();
            for (key, value) in object.iter().take(MAX_MODEL_ARRAY_ITEMS) {
                compact.insert(key.clone(), compact_value(value));
            }
            Value::Object(compact)
        }
        other => other.clone(),
    }
}

fn compact_image_metadata(object: &Map<String, Value>) -> Option<Value> {
    let mime = string_field(object, &["type", "mime_type", "media_type"])?;
    if !mime.starts_with("image/") {
        return None;
    }
    let _ = string_field(object, &["base64", "data"])?;
    Some(json!({
        "type": "image_observation",
        "mime_type": mime,
        "base64_attached_to_model": true,
        "original_size": object.get("originalSize")
            .or_else(|| object.get("original_size"))
            .cloned()
            .unwrap_or(Value::Null),
    }))
}

fn string_field<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
}
