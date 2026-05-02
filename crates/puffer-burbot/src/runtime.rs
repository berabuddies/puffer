use crate::belief::BeliefGraph;
use crate::contract::{
    ActionContract, ContractRegistry, InMemoryContractRegistry, SemanticSlotKind,
    StructuredArgumentSafetySpec,
};
use crate::executor::{ActionInvocation, ExecutorDispatcher, Observation, PufferToolExecutor};
use crate::failure::{classify_failure, FailureKind};
use crate::graph::{
    ActionRef, ActionScores, PlanEdgeKind, PlanGraph, PlanNode, PlanNodeKind, PlanStatus,
};
use crate::ids::{NodeId, RunId};
use crate::planner::{
    initial_candidate_plans, verification_candidates_for_action, ActionCandidate, ActionPlan,
    CompletionRole, PlanDependencyKind,
};
use crate::rules::{action_key_for_contracts, RewriteEngine, RuleContext};
use crate::saturation::saturate_guarded_substitutions;
use crate::scheduler::Scheduler;
use crate::semantics::action_intents;
use crate::stats::ActionTraceStats;
use crate::trace::{trace_event, JsonlTraceStore, TraceEvent, TraceEventType};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

mod support;
pub(crate) use support::default_trace_dir;
mod artifact_context;
mod artifact_review;
mod dependencies;
mod filesystem_witness;
mod model;
mod model_candidates;
mod model_feedback;
mod model_loop;
mod model_loop_support;
mod model_retry;
mod observation;
mod parallel;
mod progress;
mod repair;
mod safety;
mod snapshot;
mod stale;
mod write_preconditions;
use dependencies::block_actions_with_dead_dependencies;
use filesystem_witness::{snapshot_workspace, witness_has_changes, workspace_change_witness};
use model::VerificationOutcome;
pub(crate) use model::{RunOptions, RunResult};
use parallel::{parallel_batch_payload, select_parallel_read_batch};
use progress::ProgressEvidence;
use progress::{model_proposed_node, model_unknown_terminal_without_progress, progress_evidence};
#[cfg(test)]
pub(crate) use safety::BlockReason;
pub(crate) use safety::{ApprovalStore, RuntimeContext, SafetyGate};
use snapshot::persist_run_snapshot;
use support::{
    explicit_puffer_action, failed_terminal_completion_allowed, failure_completion_artifact,
    parse_completion_role, plan_key, plan_label, satisfy_dependent_preconditions,
    satisfy_selected_preconditions_from_graph, success_artifact,
};

pub(crate) struct BurbotRuntime {
    pub(crate) contracts: Arc<InMemoryContractRegistry>,
    pub(crate) rewrite_engine: RewriteEngine,
    pub(crate) scheduler: Scheduler,
    pub(crate) safety_gate: SafetyGate,
    pub(crate) executors: ExecutorDispatcher,
    pub(crate) trace_store: JsonlTraceStore,
    workspace_root: PathBuf,
    trace_stats: ActionTraceStats,
    repeated_failures: HashMap<String, u64>,
    executed_action_epochs: HashMap<String, u64>,
    fresh_read_action_keys: HashSet<String>,
    fresh_read_paths: HashMap<String, HashSet<String>>,
    full_file_read_epochs: HashMap<String, u64>,
    quarantined_artifact_paths: HashSet<String>,
    model_retry_sequence: u64,
    state_epoch: u64,
    state_advancing_since_goal_check: u64,
}

const FORCED_GOAL_CHECK_INTERVAL: u64 = 4;

impl BurbotRuntime {
    pub(crate) fn new(
        contracts: InMemoryContractRegistry,
        workspace_root: PathBuf,
        trace_store: JsonlTraceStore,
    ) -> Result<Self> {
        let mut executors = ExecutorDispatcher::new();
        executors.register(Arc::new(PufferToolExecutor::new(workspace_root.clone())?));
        executors.register(Arc::new(crate::symbolic::SymbolicExecutor));
        Ok(Self {
            contracts: Arc::new(contracts),
            rewrite_engine: RewriteEngine::universal(),
            scheduler: Scheduler::default(),
            safety_gate: SafetyGate,
            executors,
            trace_store,
            workspace_root,
            trace_stats: ActionTraceStats::default(),
            repeated_failures: HashMap::new(),
            executed_action_epochs: HashMap::new(),
            fresh_read_action_keys: HashSet::new(),
            fresh_read_paths: HashMap::new(),
            full_file_read_epochs: HashMap::new(),
            quarantined_artifact_paths: HashSet::new(),
            model_retry_sequence: 0,
            state_epoch: 0,
            state_advancing_since_goal_check: 0,
        })
    }

    pub(crate) fn run_goal(&mut self, goal: String, options: RunOptions) -> Result<RunResult> {
        self.trace_stats = ActionTraceStats::from_trace_store(&self.trace_store);
        self.full_file_read_epochs.clear();
        self.fresh_read_action_keys.clear();
        self.fresh_read_paths.clear();
        self.quarantined_artifact_paths.clear();
        self.state_advancing_since_goal_check = 0;
        let run_id = RunId::new();
        let mut graph = PlanGraph::from_goal(goal.clone());
        let mut beliefs = BeliefGraph::new();
        self.append(trace_event(
            run_id,
            TraceEventType::RunStarted,
            None,
            json!({"goal": goal}),
            json!({}),
        ))?;
        self.append(trace_event(
            run_id,
            TraceEventType::GoalParsed,
            Some(NodeId(0)),
            json!({}),
            json!({"goal_node": 0}),
        ))?;
        self.seed_actions(run_id, &mut graph, &goal, &options)?;
        if options.enable_observe_act_llm {
            self.add_initial_model_candidates(run_id, &mut graph, &goal, &options)?;
        }

        let mut step = 0usize;
        loop {
            let current_step = step;
            step = step.saturating_add(1);
            self.apply_rewrites(run_id, &mut graph, &options)?;
            self.apply_saturation(run_id, &mut graph)?;
            let Some(selected_action) = self.scheduler.choose_next_action(&graph) else {
                let events = block_actions_with_dead_dependencies(run_id, &mut graph);
                for event in events {
                    self.append(event)?;
                }
                if self.scheduler.choose_next_action(&graph).is_some() {
                    continue;
                }
                if options.enable_observe_act_llm {
                    let (support_source, reason, detail) =
                        self.no_executable_recovery_context(&graph, current_step);
                    let added = self.add_recovery_model_candidates(
                        run_id,
                        &mut graph,
                        &goal,
                        support_source,
                        reason,
                        detail,
                        None,
                        &options,
                    )?;
                    if added > 0 {
                        continue;
                    }
                }
                self.append(trace_event(
                    run_id,
                    TraceEventType::RunFinished,
                    None,
                    json!({"reason": "stalled"}),
                    json!({}),
                ))?;
                persist_run_snapshot(&self.trace_store, run_id, &graph, &beliefs)?;
                return Ok(RunResult::Stalled { run_id, graph });
            };
            let node_id = selected_action.node_id;
            let selected = graph.node(node_id)?.clone();
            if let Some(event) = write_preconditions::ensure_existing_file_read_precondition(
                run_id,
                &mut graph,
                self.contracts.as_ref(),
                &self.workspace_root,
                &self.full_file_read_epochs,
                self.state_epoch,
                node_id,
            )? {
                self.append(event)?;
                continue;
            }
            let mut selected_event = trace_event(
                run_id,
                TraceEventType::ActionSelected,
                Some(node_id),
                selected.payload.clone(),
                json!({"score": selected_action.breakdown.total, "score_breakdown": selected_action.breakdown}),
            );
            if let Some(action_ref) = &selected.action_ref {
                selected_event.contract_id = Some(action_ref.contract_id.clone());
                selected_event.action_name = Some(action_ref.action_name.clone());
            }
            self.append(selected_event)?;

            let ctx = self.context_for_selected(&graph, &selected, &options, &beliefs);
            if let Some(reason) = self.safety_gate.blocks(&selected, &ctx)? {
                if options.yolo {
                    self.append(trace_event(
                        run_id,
                        TraceEventType::SafetyBlocked,
                        Some(node_id),
                        selected.payload.clone(),
                        json!({"reason": reason, "bypassed": true}),
                    ))?;
                } else {
                    graph.node_mut(node_id)?.status = PlanStatus::Blocked;
                    self.append(trace_event(
                        run_id,
                        TraceEventType::SafetyBlocked,
                        Some(node_id),
                        selected.payload.clone(),
                        json!({"reason": reason.clone()}),
                    ))?;
                    if options.enable_observe_act_llm {
                        self.add_recovery_model_candidates(
                            run_id,
                            &mut graph,
                            &goal,
                            node_id,
                            "safety_blocked",
                            json!({
                                "blocked_action": selected.action_ref,
                                "args": selected.payload,
                                "reason": reason,
                            }),
                            None,
                            &options,
                        )?;
                    }
                    continue;
                }
            }

            if options.enable_parallel_read_only {
                if let Some(batch) = select_parallel_read_batch(
                    &graph,
                    self.contracts.as_ref(),
                    &self.safety_gate,
                    &ctx,
                    node_id,
                    8,
                )? {
                    self.append(trace_event(
                        run_id,
                        TraceEventType::ParallelBatchSelected,
                        Some(node_id),
                        json!({"selected": node_id.0}),
                        parallel_batch_payload(&batch),
                    ))?;
                    let invocations = self.invocations_for_nodes(run_id, &graph, &batch.nodes)?;
                    let observations = self.executors.execute_many_parallel(invocations);
                    for observation in observations {
                        let observation = observation?;
                        if let Some(artifact) = self.process_observation(
                            run_id,
                            current_step,
                            &mut graph,
                            &mut beliefs,
                            &goal,
                            &options,
                            observation,
                        )? {
                            self.append(trace_event(
                                run_id,
                                TraceEventType::RunFinished,
                                None,
                                json!({"reason": "completed"}),
                                json!({}),
                            ))?;
                            persist_run_snapshot(&self.trace_store, run_id, &graph, &beliefs)?;
                            return Ok(RunResult::Completed {
                                run_id,
                                graph,
                                artifact,
                            });
                        }
                    }
                    continue;
                }
            }

            let invocation = self.invocation_for_node(run_id, &graph, node_id)?;
            let before_snapshot = self.workspace_snapshot_for_action(&graph, node_id)?;
            let mut observation = self.executors.execute(invocation)?;
            self.attach_workspace_change_witness(before_snapshot, &mut observation)?;
            if let Some(artifact) = self.process_observation(
                run_id,
                current_step,
                &mut graph,
                &mut beliefs,
                &goal,
                &options,
                observation,
            )? {
                self.append(trace_event(
                    run_id,
                    TraceEventType::RunFinished,
                    None,
                    json!({"reason": "completed"}),
                    json!({}),
                ))?;
                persist_run_snapshot(&self.trace_store, run_id, &graph, &beliefs)?;
                return Ok(RunResult::Completed {
                    run_id,
                    graph,
                    artifact,
                });
            }
        }
    }

    fn seed_actions(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        goal: &str,
        options: &RunOptions,
    ) -> Result<()> {
        let contracts = self.contracts.active_contracts();
        for contract in &contracts {
            self.append(trace_event(
                run_id,
                TraceEventType::ContractLoaded,
                None,
                json!({}),
                json!({"contract_id": contract.contract_id, "version": contract.version}),
            ))?;
        }
        let plans = initial_candidate_plans(self.contracts.as_ref(), goal, options);
        let mut plan_nodes = HashMap::new();
        for plan in plans {
            self.seed_action_plan(run_id, graph, &mut plan_nodes, plan)?;
        }
        Ok(())
    }

    fn seed_action_plan(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        plan_nodes: &mut HashMap<String, NodeId>,
        plan: ActionPlan,
    ) -> Result<()> {
        let plan_id = self.seed_plan_node(run_id, graph, plan_nodes, &plan)?;
        let mut step_ids = Vec::new();
        for candidate in plan.steps {
            step_ids.push(self.add_candidate_node(
                run_id,
                graph,
                candidate,
                Some(plan_id),
                None,
            )?);
        }
        for dependency in plan.dependencies {
            let Some(source) = step_ids.get(dependency.prerequisite_index).copied() else {
                continue;
            };
            let Some(target) = step_ids.get(dependency.dependent_index).copied() else {
                continue;
            };
            let kind = match dependency.kind {
                PlanDependencyKind::DependsOn => PlanEdgeKind::DependsOn,
                PlanDependencyKind::PreconditionFor => PlanEdgeKind::PreconditionFor,
            };
            graph.add_edge(source, target, kind, json!({}));
        }
        Ok(())
    }

    fn seed_plan_node(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        plan_nodes: &mut HashMap<String, NodeId>,
        plan: &ActionPlan,
    ) -> Result<NodeId> {
        let key = plan_key(&plan.source);
        if let Some(id) = plan_nodes.get(key).copied() {
            return Ok(id);
        }
        let id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Subgoal,
            status: PlanStatus::Open,
            label: format!("Plan: {}", plan_label(&plan.source)),
            payload: json!({
                "plan_id": key,
                "source": plan.source,
                "rationale": plan.rationale,
            }),
            action_ref: None,
            scores: ActionScores::default(),
        });
        graph.add_edge(NodeId(0), id, PlanEdgeKind::DecomposesTo, json!({}));
        self.append(trace_event(
            run_id,
            TraceEventType::NodeAdded,
            Some(id),
            json!({}),
            json!({
                "kind": "subgoal",
                "plan_id": key,
                "label": plan_label(&plan.source),
            }),
        ))?;
        plan_nodes.insert(key.to_string(), id);
        Ok(id)
    }

    fn context_for_selected(
        &self,
        graph: &PlanGraph,
        selected: &PlanNode,
        options: &RunOptions,
        beliefs: &BeliefGraph,
    ) -> RuntimeContext<'_> {
        let mut approvals = ApprovalStore::default();
        let mut beliefs = beliefs.clone();
        satisfy_selected_preconditions_from_graph(
            self.contracts.as_ref(),
            graph,
            selected,
            &mut beliefs,
        );
        if let Some(action_ref) = &selected.action_ref {
            if explicit_puffer_action(action_ref, options) {
                approvals.approve(selected.id);
                if let Some(action) = self
                    .contracts
                    .get_action(&action_ref.contract_id, &action_ref.action_name)
                {
                    for precondition in &action.preconditions {
                        beliefs.satisfy_precondition(
                            precondition,
                            "explicit Puffer tool request satisfies declared precondition",
                            json!({"action": action.name}),
                        );
                    }
                }
            }
        }
        RuntimeContext {
            contracts: self.contracts.as_ref(),
            approvals,
            beliefs,
        }
    }

    fn apply_rewrites(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        options: &RunOptions,
    ) -> Result<()> {
        let approved_nodes = graph
            .frontier_actions()
            .into_iter()
            .filter(|id| {
                graph
                    .node(*id)
                    .ok()
                    .and_then(|node| node.action_ref.as_ref())
                    .is_some_and(|action_ref| explicit_puffer_action(action_ref, options))
            })
            .collect::<HashSet<_>>();
        let ctx = RuleContext {
            contracts: self.contracts.as_ref(),
            repeated_failures: &self.repeated_failures,
            state_epoch: self.state_epoch,
            approved_nodes: &approved_nodes,
        };
        for (name, changes) in self.rewrite_engine.apply_all(graph, &ctx)? {
            if changes > 0 {
                self.append(trace_event(
                    run_id,
                    TraceEventType::RewriteApplied,
                    None,
                    json!({"rule": name}),
                    json!({"changes": changes}),
                ))?;
            }
        }
        Ok(())
    }

    fn apply_saturation(&mut self, run_id: RunId, graph: &mut PlanGraph) -> Result<()> {
        let report = saturate_guarded_substitutions(graph, self.contracts.as_ref())?;
        if report.can_replace_edges > 0 || report.pruned_actions > 0 {
            self.append(trace_event(
                run_id,
                TraceEventType::RewriteApplied,
                None,
                json!({"rule": "GuardedSubstitutionSaturation"}),
                json!({
                    "can_replace_edges": report.can_replace_edges,
                    "pruned_actions": report.pruned_actions,
                }),
            ))?;
        }
        Ok(())
    }

    fn add_candidate_node(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        mut candidate: ActionCandidate,
        support_source: Option<NodeId>,
        verifies: Option<NodeId>,
    ) -> Result<NodeId> {
        self.trace_stats
            .adjust_scores(&candidate.action_ref, &mut candidate.scores);
        let action_ref = candidate.action_ref.clone();
        let source = candidate.source.clone();
        let completion_role = candidate.completion_role.clone();
        let rationale = candidate.rationale.clone();
        let id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: format!("{}.{}", action_ref.contract_id, action_ref.action_name),
            payload: candidate.args,
            action_ref: Some(action_ref.clone()),
            scores: candidate.scores,
        });
        graph.add_edge(
            support_source.unwrap_or(NodeId(0)),
            id,
            PlanEdgeKind::Supports,
            json!({
                "source": source.clone(),
                "completion_role": completion_role.clone(),
                "rationale": rationale.clone(),
            }),
        );
        if let Some(target) = verifies {
            graph.add_edge(id, target, PlanEdgeKind::Verifies, json!({}));
        }
        let mut event = trace_event(
            run_id,
            TraceEventType::NodeAdded,
            Some(id),
            json!({}),
            json!({
                "kind": "action",
                "source": source,
                "completion_role": completion_role,
                "rationale": rationale,
            }),
        );
        event.contract_id = Some(action_ref.contract_id);
        event.action_name = Some(action_ref.action_name);
        self.append(event)?;
        Ok(id)
    }

    fn invocations_for_nodes(
        &self,
        run_id: RunId,
        graph: &PlanGraph,
        nodes: &[NodeId],
    ) -> Result<Vec<ActionInvocation>> {
        nodes
            .iter()
            .map(|node_id| self.invocation_for_node(run_id, graph, *node_id))
            .collect()
    }

    fn invocation_for_node(
        &self,
        run_id: RunId,
        graph: &PlanGraph,
        node_id: NodeId,
    ) -> Result<ActionInvocation> {
        let node = graph.node(node_id)?;
        let action_ref = node
            .action_ref
            .clone()
            .ok_or_else(|| anyhow!("selected action node has no action_ref"))?;
        Ok(ActionInvocation {
            run_id,
            plan_node_id: node_id,
            contract_id: action_ref.contract_id,
            action_name: action_ref.action_name,
            args: node.payload.clone(),
        })
    }

    fn workspace_snapshot_for_action(
        &self,
        graph: &PlanGraph,
        node_id: NodeId,
    ) -> Result<Option<filesystem_witness::WorkspaceSnapshot>> {
        let Some(node) = graph.node(node_id).ok() else {
            return Ok(None);
        };
        let Some(action_ref) = node.action_ref.as_ref() else {
            return Ok(None);
        };
        let Some(action) = self
            .contracts
            .get_action(&action_ref.contract_id, &action_ref.action_name)
        else {
            return Ok(None);
        };
        if action.side_effect_class == crate::contract::SideEffectClass::Unknown {
            return Ok(Some(snapshot_workspace(&self.workspace_root)?));
        }
        Ok(None)
    }

    fn attach_workspace_change_witness(
        &self,
        before: Option<filesystem_witness::WorkspaceSnapshot>,
        observation: &mut Observation,
    ) -> Result<()> {
        let Some(before) = before else {
            return Ok(());
        };
        let after = snapshot_workspace(&self.workspace_root)?;
        let witness = workspace_change_witness(&before, &after);
        if !witness_has_changes(&witness) {
            return Ok(());
        }
        if let Some(object) = observation.output.as_object_mut() {
            object.insert("filesystem_witness".to_string(), witness);
        }
        Ok(())
    }

    fn record_fresh_read_action(&mut self, action_ref: &ActionRef, args: &Value) {
        let Some(path_key) = self.read_file_path_key(action_ref, args) else {
            return;
        };
        let action_key = action_key_for_contracts(self.contracts.as_ref(), action_ref, args);
        self.fresh_read_action_keys.insert(action_key.clone());
        self.fresh_read_paths
            .entry(path_key)
            .or_default()
            .insert(action_key);
    }

    fn read_file_path_key(&self, action_ref: &ActionRef, args: &Value) -> Option<String> {
        let action = self
            .contracts
            .get_action(&action_ref.contract_id, &action_ref.action_name)?;
        action_intents(&action, args)
            .into_iter()
            .find(|intent| intent.normalized.intent == "read_file")
            .and_then(|intent| {
                intent
                    .normalized
                    .slots
                    .get("path")
                    .or_else(|| intent.normalized.slots.get("file_path"))
                    .cloned()
            })
            .map(|path| self.workspace_path_key(&path))
    }

    fn action_execution_fresh(&self, action_ref: &ActionRef, args: &Value) -> bool {
        let key = action_key_for_contracts(self.contracts.as_ref(), action_ref, args);
        self.executed_action_epochs
            .get(&key)
            .is_some_and(|epoch| *epoch == self.state_epoch)
            || self.fresh_read_action_keys.contains(&key)
    }

    fn invalidate_fresh_reads_for_observation(&mut self, output: &Value) {
        for path_key in self.changed_file_path_keys(output) {
            let Some(keys) = self.fresh_read_paths.remove(&path_key) else {
                continue;
            };
            for key in keys {
                self.fresh_read_action_keys.remove(&key);
            }
        }
    }

    fn changed_file_path_keys(&self, output: &Value) -> Vec<String> {
        let mut paths = Vec::new();
        collect_output_path_strings(output.get("filePath"), &mut paths);
        collect_output_path_strings(output.get("file_path"), &mut paths);
        collect_output_path_strings(
            output
                .get("structured_output")
                .and_then(|value| value.get("filePath")),
            &mut paths,
        );
        collect_output_path_strings(
            output
                .get("structured_output")
                .and_then(|value| value.get("file"))
                .and_then(|value| value.get("filePath")),
            &mut paths,
        );
        if let Some(witness) = output.get("filesystem_witness") {
            for key in ["created", "modified", "removed"] {
                collect_output_path_strings(witness.get(key), &mut paths);
            }
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|path| self.workspace_path_key(&path))
            .collect()
    }

    fn artifact_path_keys_for_action(
        &self,
        action_ref: &ActionRef,
        args: &Value,
        output: &Value,
    ) -> Vec<String> {
        let mut paths = Vec::new();
        if let Some(action) = self
            .contracts
            .get_action(&action_ref.contract_id, &action_ref.action_name)
        {
            collect_contract_path_arg_strings(&action, args, &mut paths);
        }
        collect_trusted_artifact_output_paths(output, &mut paths);
        if let Some(witness) = output.get("filesystem_witness") {
            for key in ["created", "modified", "removed"] {
                collect_output_path_strings(witness.get(key), &mut paths);
            }
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|path| self.workspace_path_key(&path))
            .collect()
    }

    fn quarantine_artifact_paths(&mut self, action_ref: &ActionRef, args: &Value, output: &Value) {
        for path in self.artifact_path_keys_for_action(action_ref, args, output) {
            self.quarantined_artifact_paths.insert(path);
        }
    }

    fn release_artifact_paths(&mut self, action_ref: &ActionRef, args: &Value, output: &Value) {
        for path in self.artifact_path_keys_for_action(action_ref, args, output) {
            self.quarantined_artifact_paths.remove(&path);
        }
    }

    fn workspace_path_key(&self, path: &str) -> String {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace_root.join(path)
        };
        path.canonicalize().unwrap_or(path).display().to_string()
    }

    fn process_observation(
        &mut self,
        run_id: RunId,
        step: usize,
        graph: &mut PlanGraph,
        beliefs: &mut BeliefGraph,
        goal: &str,
        options: &RunOptions,
        observation: Observation,
    ) -> Result<Option<Value>> {
        let node_id = observation.invocation.plan_node_id;
        let selected = graph.node(node_id)?.clone();
        let action_ref = selected
            .action_ref
            .clone()
            .ok_or_else(|| anyhow!("executed action node has no action_ref"))?;
        graph.node_mut(node_id)?.status = if observation.success {
            PlanStatus::Executed
        } else {
            PlanStatus::Failed
        };
        self.record_belief_observation(
            beliefs,
            &action_ref,
            &selected.payload,
            &observation.output,
            observation.success,
        );
        let progress = progress_evidence(
            self.contracts
                .get_action(&action_ref.contract_id, &action_ref.action_name)
                .as_ref(),
            &observation.output,
        );
        if observation.success && progress.changes_state {
            self.state_epoch = self.state_epoch.saturating_add(1);
            self.invalidate_fresh_reads_for_observation(&observation.output);
        }
        if observation.success {
            self.state_advancing_since_goal_check =
                self.state_advancing_since_goal_check.saturating_add(1);
        }
        write_preconditions::record_full_file_read_epoch(
            self.contracts.as_ref(),
            &action_ref,
            &selected.payload,
            observation.success,
            self.state_epoch,
            &self.workspace_root,
            &mut self.full_file_read_epochs,
        );
        if observation.success {
            let key =
                action_key_for_contracts(self.contracts.as_ref(), &action_ref, &selected.payload);
            self.executed_action_epochs.insert(key, self.state_epoch);
            self.record_fresh_read_action(&action_ref, &selected.payload);
        }
        let mut executed = trace_event(
            run_id,
            TraceEventType::ActionExecuted,
            Some(node_id),
            selected.payload.clone(),
            observation.output.clone(),
        );
        executed.contract_id = Some(action_ref.contract_id.clone());
        executed.action_name = Some(action_ref.action_name.clone());
        executed.success = Some(observation.success);
        executed.latency_ms = observation.latency_ms;
        executed.cost = observation.cost;
        self.append(executed)?;
        let observation_id =
            self.attach_observation(graph, node_id, &observation.output, observation.success);
        if observation.success {
            satisfy_dependent_preconditions(
                self.contracts.as_ref(),
                graph,
                beliefs,
                node_id,
                &observation.output,
            );
        }
        let verification = self.verify_observation(
            run_id,
            node_id,
            &action_ref,
            observation.success,
            &observation.output,
        )?;
        if observation.success
            && options.enable_observe_act_llm
            && self.review_generated_artifact_if_needed(
                run_id,
                graph,
                beliefs,
                goal,
                node_id,
                &action_ref,
                &selected.payload,
                &observation.output,
                &progress,
                options,
            )?
        {
            return Ok(None);
        }
        if observation.success && self.is_verification_action(graph, node_id) {
            if self.verification_action_verifies_terminal(graph, node_id) {
                if self.scheduler.choose_next_action(graph).is_some() {
                    return Ok(None);
                }
                if !self.goal_verified_or_expand(
                    run_id,
                    graph,
                    beliefs,
                    goal,
                    node_id,
                    &action_ref,
                    &selected.payload,
                    &observation.output,
                    &options,
                )? {
                    return Ok(None);
                }
                let artifact =
                    self.verified_success_artifact(graph, node_id, &observation.output)?;
                self.append(trace_event(
                    run_id,
                    TraceEventType::CompletionDeclared,
                    Some(node_id),
                    json!({"step": step}),
                    artifact.clone(),
                ))?;
                return Ok(Some(artifact));
            }
            if self.scheduler.choose_next_action(graph).is_some() {
                return Ok(None);
            }
        }
        if observation.success && self.should_schedule_verification(graph, node_id, &verification) {
            let added = self.add_verification_candidates(
                run_id,
                graph,
                node_id,
                &action_ref,
                &selected.payload,
                &observation.output,
            )?;
            if added > 0 {
                return Ok(None);
            }
        }
        if observation.success
            && model_unknown_terminal_without_progress(
                self.contracts
                    .get_action(&action_ref.contract_id, &action_ref.action_name)
                    .as_ref(),
                self.completion_role_for_node(graph, node_id)
                    .unwrap_or(CompletionRole::Support),
                model_proposed_node(graph, node_id),
                &progress,
            )
        {
            graph.node_mut(node_id)?.status = PlanStatus::Failed;
            self.record_failure(
                run_id,
                graph,
                beliefs,
                node_id,
                &action_ref,
                &selected.payload,
                &observation.output,
                FailureKind::NoProgress,
            )?;
            if options.enable_observe_act_llm {
                self.add_model_observe_act_candidates(
                    run_id,
                    graph,
                    goal,
                    observation_id,
                    node_id,
                    &action_ref,
                    &selected.payload,
                    false,
                    &observation.output,
                    &options,
                )?;
            }
            return Ok(None);
        }
        if observation.success
            && verification.passed
            && self.completion_role_for_node(graph, node_id) == Some(CompletionRole::Terminal)
            && self.should_verify_terminal_completion(graph, node_id, &progress)
        {
            if !self.goal_verified_or_expand(
                run_id,
                graph,
                beliefs,
                goal,
                node_id,
                &action_ref,
                &selected.payload,
                &observation.output,
                &options,
            )? {
                return Ok(None);
            }
            let artifact = success_artifact(&action_ref, &observation.output, &verification);
            self.append(trace_event(
                run_id,
                TraceEventType::CompletionDeclared,
                Some(node_id),
                json!({"step": step}),
                artifact.clone(),
            ))?;
            return Ok(Some(artifact));
        }
        if observation.success
            && verification.passed
            && options.enable_observe_act_llm
            && self.completion_role_for_node(graph, node_id) != Some(CompletionRole::Terminal)
            && self.state_advancing_since_goal_check >= FORCED_GOAL_CHECK_INTERVAL
        {
            self.state_advancing_since_goal_check = 0;
            if self.goal_verified_or_expand(
                run_id,
                graph,
                beliefs,
                goal,
                node_id,
                &action_ref,
                &selected.payload,
                &observation.output,
                &options,
            )? {
                let artifact = success_artifact(&action_ref, &observation.output, &verification);
                self.append(trace_event(
                    run_id,
                    TraceEventType::CompletionDeclared,
                    Some(node_id),
                    json!({"step": step, "forced_goal_check": true}),
                    artifact.clone(),
                ))?;
                return Ok(Some(artifact));
            }
        }
        if observation.success && !verification.passed {
            graph.node_mut(node_id)?.status = PlanStatus::Failed;
        }
        if observation.success
            && verification.passed
            && (progress.has_state_witness || progress.has_structural_witness)
        {
            self.resolve_repair_anchors(graph, node_id)?;
        }
        if !observation.success || !verification.passed {
            let failure_kind = if observation.success && !verification.passed {
                FailureKind::VerificationFailed
            } else {
                classify_failure(
                    self.contracts
                        .get_action(&action_ref.contract_id, &action_ref.action_name)
                        .as_ref(),
                    &observation.output,
                )
            };
            self.record_failure(
                run_id,
                graph,
                beliefs,
                node_id,
                &action_ref,
                &selected.payload,
                &observation.output,
                failure_kind.clone(),
            )?;
            if !observation.success && failed_terminal_completion_allowed(graph, node_id, options) {
                graph.node_mut(node_id)?.status = PlanStatus::Satisfied;
                let artifact =
                    failure_completion_artifact(&action_ref, &observation.output, &failure_kind);
                self.append(trace_event(
                    run_id,
                    TraceEventType::CompletionDeclared,
                    Some(node_id),
                    json!({"step": step, "expected_failure": true}),
                    artifact.clone(),
                ))?;
                return Ok(Some(artifact));
            }
        }
        if options.enable_observe_act_llm {
            self.add_model_observe_act_candidates(
                run_id,
                graph,
                goal,
                observation_id,
                node_id,
                &action_ref,
                &selected.payload,
                observation.success,
                &observation.output,
                &options,
            )?;
        }
        Ok(None)
    }

    fn attach_observation(
        &mut self,
        graph: &mut PlanGraph,
        action_id: NodeId,
        output: &Value,
        success: bool,
    ) -> NodeId {
        let observation_id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Observation,
            status: if success {
                PlanStatus::Satisfied
            } else {
                PlanStatus::Failed
            },
            label: "Observation".to_string(),
            payload: output.clone(),
            action_ref: None,
            scores: ActionScores::default(),
        });
        graph.add_edge(action_id, observation_id, PlanEdgeKind::Produces, json!({}));
        observation_id
    }

    fn record_belief_observation(
        &self,
        beliefs: &mut BeliefGraph,
        action_ref: &ActionRef,
        args: &Value,
        output: &Value,
        success: bool,
    ) {
        if success {
            beliefs.record_observation(
                action_ref.action_name.clone(),
                format!("{} succeeded", action_ref.action_name),
                output.clone(),
            );
            if let Some(action) = self
                .contracts
                .get_action(&action_ref.contract_id, &action_ref.action_name)
            {
                if progress_evidence(Some(&action), output).changes_state {
                    beliefs.mark_changed_state(
                        action_key_for_contracts(self.contracts.as_ref(), action_ref, args),
                        "action with possible side effects succeeded",
                        output.clone(),
                    );
                }
            }
        } else {
            beliefs.record_action_failure(
                action_ref.contract_id.clone(),
                action_ref.action_name.clone(),
                args,
                format!("{} failed", action_ref.action_name),
                output.clone(),
            );
        }
    }

    fn is_verification_action(&self, graph: &PlanGraph, node_id: NodeId) -> bool {
        graph
            .edges
            .iter()
            .any(|edge| edge.source == node_id && edge.kind == PlanEdgeKind::Verifies)
    }

    fn verification_action_verifies_terminal(&self, graph: &PlanGraph, node_id: NodeId) -> bool {
        graph
            .edges
            .iter()
            .filter(|edge| edge.source == node_id && edge.kind == PlanEdgeKind::Verifies)
            .any(|edge| {
                self.completion_role_for_node(graph, edge.target) == Some(CompletionRole::Terminal)
            })
    }

    fn should_schedule_verification(
        &self,
        graph: &PlanGraph,
        node_id: NodeId,
        verification: &VerificationOutcome,
    ) -> bool {
        verification.required
            && !verification.passed
            && !self.is_verification_action(graph, node_id)
            && !graph.edges.iter().any(|edge| {
                edge.target == node_id
                    && edge.kind == PlanEdgeKind::Verifies
                    && graph
                        .node(edge.source)
                        .ok()
                        .is_some_and(|node| node.kind == PlanNodeKind::Action)
            })
    }

    fn should_verify_terminal_completion(
        &self,
        graph: &PlanGraph,
        node_id: NodeId,
        progress: &ProgressEvidence,
    ) -> bool {
        progress.has_state_witness || self.is_verification_action(graph, node_id)
    }

    fn add_verification_candidates(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        node_id: NodeId,
        action_ref: &ActionRef,
        args: &Value,
        output: &Value,
    ) -> Result<usize> {
        let candidates =
            verification_candidates_for_action(self.contracts.as_ref(), action_ref, args, output);
        let mut added = 0;
        for candidate in candidates {
            self.add_candidate_node(run_id, graph, candidate, None, Some(node_id))?;
            added += 1;
        }
        Ok(added)
    }

    fn resolve_repair_anchors(&self, graph: &mut PlanGraph, action_id: NodeId) -> Result<()> {
        let anchors = graph
            .edges
            .iter()
            .filter(|edge| edge.target == action_id && edge.kind == PlanEdgeKind::Repairs)
            .map(|edge| edge.source)
            .collect::<Vec<_>>();
        for anchor in anchors {
            if let Some(node) = graph.nodes.get_mut(&anchor) {
                if matches!(node.kind, PlanNodeKind::Failure | PlanNodeKind::Repair)
                    && node.status == PlanStatus::Open
                {
                    node.status = PlanStatus::Satisfied;
                }
            }
            let dependent_repairs = graph
                .edges
                .iter()
                .filter(|edge| edge.source == anchor && edge.kind == PlanEdgeKind::Repairs)
                .map(|edge| edge.target)
                .collect::<Vec<_>>();
            for repair in dependent_repairs {
                if let Some(node) = graph.nodes.get_mut(&repair) {
                    if node.kind == PlanNodeKind::Repair && node.status == PlanStatus::Open {
                        node.status = PlanStatus::Satisfied;
                    }
                }
            }
        }
        Ok(())
    }

    fn completion_role_for_node(
        &self,
        graph: &PlanGraph,
        node_id: NodeId,
    ) -> Option<CompletionRole> {
        graph
            .edges
            .iter()
            .find(|edge| edge.target == node_id && edge.kind == PlanEdgeKind::Supports)
            .and_then(|edge| edge.payload.get("completion_role"))
            .and_then(Value::as_str)
            .and_then(parse_completion_role)
    }

    fn verified_success_artifact(
        &self,
        graph: &PlanGraph,
        verification_id: NodeId,
        verification_output: &Value,
    ) -> Result<Value> {
        let verified_action_id = graph
            .edges
            .iter()
            .find(|edge| edge.source == verification_id && edge.kind == PlanEdgeKind::Verifies)
            .map(|edge| edge.target)
            .unwrap_or(verification_id);
        let verified_action = graph.node(verified_action_id)?;
        let action_ref = verified_action
            .action_ref
            .as_ref()
            .ok_or_else(|| anyhow!("verified target has no action_ref"))?;
        Ok(json!({
            "completed": true,
            "contract_id": action_ref.contract_id,
            "action_name": action_ref.action_name,
            "verified_by": verification_id.0,
            "verification_required": true,
            "verification_passed": true,
            "verification_evidence": verification_output,
            "output": self.observation_output_for_action(graph, verified_action_id),
        }))
    }

    fn observation_output_for_action(&self, graph: &PlanGraph, action_id: NodeId) -> Option<Value> {
        graph
            .edges
            .iter()
            .find(|edge| edge.source == action_id && edge.kind == PlanEdgeKind::Produces)
            .and_then(|edge| graph.nodes.get(&edge.target))
            .map(|node| node.payload.clone())
    }

    fn no_executable_recovery_context(
        &self,
        graph: &PlanGraph,
        current_step: usize,
    ) -> (NodeId, &'static str, Value) {
        if let Some((id, node)) = latest_open_node(graph, PlanNodeKind::Failure) {
            return (
                id,
                "no_executable_actions_after_failure",
                json!({
                    "step": current_step,
                    "failure_node": id.0,
                    "failure": node.payload,
                }),
            );
        }
        if let Some((id, node)) = latest_open_node(graph, PlanNodeKind::Repair) {
            return (
                id,
                "no_executable_actions_after_failure",
                json!({
                    "step": current_step,
                    "repair_node": id.0,
                    "repair": node.payload,
                }),
            );
        }
        if graph_has_executed_non_support_action(graph) {
            return (
                NodeId(0),
                "no_executable_actions_after_model_work",
                json!({"step": current_step}),
            );
        }
        if graph_has_executed_support_action(graph) {
            return (
                NodeId(0),
                "no_executable_actions_after_support",
                json!({"step": current_step}),
            );
        }
        (
            NodeId(0),
            "no_executable_actions",
            json!({"step": current_step}),
        )
    }

    fn append(&self, event: TraceEvent) -> Result<()> {
        self.trace_store.append(&event)
    }
}

fn latest_open_node(graph: &PlanGraph, kind: PlanNodeKind) -> Option<(NodeId, &PlanNode)> {
    graph.nodes.iter().rev().find_map(|(id, node)| {
        (node.kind == kind && node.status == PlanStatus::Open).then_some((*id, node))
    })
}

fn graph_has_executed_non_support_action(graph: &PlanGraph) -> bool {
    graph.nodes.iter().any(|(id, node)| {
        node.kind == PlanNodeKind::Action
            && matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied)
            && completion_role_payload_for_node(graph, *id).is_some_and(|role| role != "support")
    })
}

fn graph_has_executed_support_action(graph: &PlanGraph) -> bool {
    graph.nodes.iter().any(|(id, node)| {
        node.kind == PlanNodeKind::Action
            && matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied)
            && completion_role_payload_for_node(graph, *id) == Some("support")
    })
}

fn completion_role_payload_for_node<'a>(graph: &'a PlanGraph, node_id: NodeId) -> Option<&'a str> {
    graph
        .edges
        .iter()
        .find(|edge| edge.target == node_id && edge.kind == PlanEdgeKind::Supports)
        .and_then(|edge| edge.payload.get("completion_role"))
        .and_then(Value::as_str)
}

fn collect_output_path_strings(value: Option<&Value>, paths: &mut Vec<String>) {
    match value {
        Some(Value::String(path)) if !path.trim().is_empty() => {
            paths.push(path.trim().to_string());
        }
        Some(Value::Array(items)) => {
            for item in items {
                collect_output_path_strings(Some(item), paths);
            }
        }
        _ => {}
    }
}

fn collect_contract_path_arg_strings(
    action: &ActionContract,
    args: &Value,
    paths: &mut Vec<String>,
) {
    for arg in contract_path_argument_names(action) {
        collect_output_path_strings(args.get(&arg), paths);
    }
}

fn contract_path_argument_names(action: &ActionContract) -> BTreeSet<String> {
    let mut args = BTreeSet::new();
    for intent in &action.semantic_intents {
        for (slot, arg) in intent.slots.iter().chain(intent.optional_slots.iter()) {
            if intent
                .slot_kinds
                .get(slot)
                .is_some_and(filesystem_slot_kind)
            {
                args.insert(arg.clone());
            }
        }
    }
    for safety in &action.structured_argument_safety {
        args.insert(structured_safety_source_arg(safety).to_string());
    }
    args
}

fn filesystem_slot_kind(kind: &SemanticSlotKind) -> bool {
    matches!(
        kind,
        SemanticSlotKind::FilesystemPath | SemanticSlotKind::FilesystemDirectory
    )
}

fn structured_safety_source_arg(safety: &StructuredArgumentSafetySpec) -> &str {
    match safety {
        StructuredArgumentSafetySpec::BlockPathPrefix { source_arg, .. }
        | StructuredArgumentSafetySpec::BlockParentTraversal { source_arg }
        | StructuredArgumentSafetySpec::RequireApprovalPathComponent { source_arg, .. } => {
            source_arg
        }
    }
}

fn collect_trusted_artifact_output_paths(output: &Value, paths: &mut Vec<String>) {
    collect_output_path_strings(output.get("filePath"), paths);
    collect_output_path_strings(output.get("file_path"), paths);
    collect_output_path_strings(
        output
            .get("structured_output")
            .and_then(|value| value.get("filePath")),
        paths,
    );
    collect_output_path_strings(
        output
            .get("structured_output")
            .and_then(|value| value.get("file_path")),
        paths,
    );
}

#[cfg(test)]
mod liveness_tests;
#[cfg(test)]
mod model_feedback_tests;
#[cfg(test)]
mod progress_tests;
#[cfg(test)]
mod retry_tests;
#[cfg(test)]
mod tests;
