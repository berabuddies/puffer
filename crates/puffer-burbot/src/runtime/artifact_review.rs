use super::artifact_context::generated_artifact_context;
use super::filesystem_witness::file_sha256;
use super::model_loop_support::{
    accumulated_image_context, artifact_review_history_summary_excluding, compact_value,
    frontier_summary, verified_target_summary,
};
use super::progress::{model_proposed_node, ProgressEvidence};
use super::stale::prune_stale_model_siblings_after_artifact_review_failed;
use super::BurbotRuntime;
use crate::belief::BeliefGraph;
use crate::contract::{ActionContract, CapabilityContract, ContractRegistry, SideEffectClass};
use crate::failure::FailureKind;
use crate::graph::{ActionRef, PlanGraph, PlanStatus};
use crate::ids::{NodeId, RunId};
use crate::llm::{retryable_openai_error, review_generated_artifact, GoalVerificationResult};
use crate::planner::CandidateSource;
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use crate::semantics::read_only_side_effect;
use crate::trace::{trace_event, TraceEventType};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const ARTIFACT_REVIEW_MIN_CONFIDENCE: f64 = 0.5;

impl BurbotRuntime {
    pub(super) fn review_generated_artifact_if_needed(
        &mut self,
        run_id: RunId,
        graph: &mut PlanGraph,
        beliefs: &mut BeliefGraph,
        goal: &str,
        action_id: NodeId,
        action_ref: &ActionRef,
        args: &Value,
        output: &Value,
        progress: &ProgressEvidence,
        options: &super::RunOptions,
    ) -> Result<bool> {
        let Some(model) = options.model.as_deref() else {
            return Ok(false);
        };
        if !self.should_review_generated_artifact(graph, action_id, action_ref, output, progress) {
            return Ok(false);
        }
        let contract = self.artifact_review_contract()?;
        let context = json!({
            "phase": "generated_artifact_review",
            "action_id": action_id.0,
            "action_ref": action_ref,
            "args": compact_value(args),
            "output": compact_value(output),
            "completion_role": self.completion_role_for_node(graph, action_id),
            "verified_target": verified_target_summary(graph, action_id),
            "current_artifacts": current_artifact_records(&self.workspace_root, output),
            "generated_artifacts": generated_artifact_context(graph, self.contracts.as_ref(), Some(action_id)),
            "frontier": frontier_summary(graph),
            "history": artifact_review_history_summary_excluding(graph, self.contracts.as_ref(), Some(action_id)),
        });
        let image_context = accumulated_image_context(graph, Some(output));
        self.append(trace_event(
            run_id,
            TraceEventType::ArtifactReviewPerformed,
            Some(action_id),
            context.clone(),
            json!({"started": true}),
        ))?;
        let decision = match review_generated_artifact(
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
                let error_string = error.to_string();
                let restored = restore_rejected_artifact_write(&self.workspace_root, args, output)?;
                self.quarantine_artifact_paths(action_ref, args, output);
                self.append(trace_event(
                    run_id,
                    TraceEventType::ArtifactReviewPerformed,
                    Some(action_id),
                    context.clone(),
                    json!({
                        "accepted": false,
                        "confidence": 0.0,
                        "error": error_string.clone(),
                        "retryable": retryable,
                        "restored": restored,
                        "pending_review": retryable,
                    }),
                ))?;
                graph.node_mut(action_id)?.status = PlanStatus::Failed;
                self.record_failure(
                    run_id,
                    graph,
                    beliefs,
                    action_id,
                    action_ref,
                    args,
                    &json!({
                        "artifact_review": {
                            "accepted": false,
                            "satisfied": false,
                            "confidence": 0.0,
                            "error": error_string.clone(),
                            "retryable": retryable,
                            "pending_review": retryable,
                            "restored": restored,
                        },
                        "review_context": context,
                    }),
                    FailureKind::Contract(
                        if retryable {
                            "artifact_review_unavailable"
                        } else {
                            "artifact_review_failed"
                        }
                        .to_string(),
                    ),
                )?;
                if retryable {
                    self.add_model_retry_candidate(
                        run_id,
                        graph,
                        action_id,
                        "artifact_reviewer_unavailable",
                        &error_string,
                    )?;
                } else {
                    for event in prune_stale_model_siblings_after_artifact_review_failed(
                        run_id,
                        graph,
                        self.contracts.as_ref(),
                        action_id,
                    ) {
                        self.append(event)?;
                    }
                }
                return Ok(true);
            }
        };
        let accepted = artifact_review_accepts(&decision);
        self.append(trace_event(
            run_id,
            TraceEventType::ArtifactReviewPerformed,
            Some(action_id),
            context.clone(),
            json!({
                "accepted": accepted,
                "satisfied": decision.satisfied,
                "confidence": decision.confidence,
                "missing_evidence": decision.missing_evidence.clone(),
                "suggested_candidates": decision.suggested_candidates.len(),
                "rejected_suggested_candidates": decision.rejected_suggested_candidates.clone(),
            }),
        ))?;
        if accepted {
            self.release_artifact_paths(action_ref, args, output);
            return Ok(false);
        }
        let restored = restore_rejected_artifact_write(&self.workspace_root, args, output)?;
        self.quarantine_artifact_paths(action_ref, args, output);
        graph.node_mut(action_id)?.status = PlanStatus::Failed;
        self.record_failure(
            run_id,
            graph,
            beliefs,
            action_id,
            action_ref,
            args,
            &json!({
                "artifact_review": {
                    "accepted": false,
                    "satisfied": decision.satisfied,
                    "confidence": decision.confidence,
                    "missing_evidence": decision.missing_evidence.clone(),
                    "restored": restored,
                },
                "review_context": context,
            }),
            FailureKind::Contract("artifact_review_failed".to_string()),
        )?;
        for event in prune_stale_model_siblings_after_artifact_review_failed(
            run_id,
            graph,
            self.contracts.as_ref(),
            action_id,
        ) {
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
            self.add_recovery_model_candidates(
                run_id,
                graph,
                goal,
                action_id,
                if outcome.is_non_advancing() {
                    "artifact_review_non_advancing_candidates"
                } else {
                    "artifact_review_no_candidates"
                },
                json!({
                    "artifact_review": "not_accepted",
                    "proposal_outcome": {
                        "added": outcome.added,
                        "proposed": outcome.proposed,
                        "skipped_wait": outcome.skipped_wait,
                        "skipped_duplicate": outcome.skipped_duplicate,
                        "skipped_dependency": outcome.skipped_dependency,
                        "skipped_policy": outcome.skipped_policy,
                        "duplicate_candidates": outcome.skipped_duplicate_candidates,
                        "policy_skipped_candidates": outcome.skipped_policy_candidates,
                    },
                }),
                Some(output),
                options,
            )?;
        }
        Ok(true)
    }

    fn should_review_generated_artifact(
        &self,
        graph: &PlanGraph,
        action_id: NodeId,
        action_ref: &ActionRef,
        output: &Value,
        progress: &ProgressEvidence,
    ) -> bool {
        if !model_proposed_node(graph, action_id) {
            return false;
        }
        let Some(action) = self
            .contracts
            .get_action(&action_ref.contract_id, &action_ref.action_name)
        else {
            return false;
        };
        if read_only_side_effect(&action.side_effect_class)
            || !(progress.has_state_witness || progress.has_structural_witness)
        {
            return false;
        }
        model_authored_artifact_action(&action, output)
    }

    fn artifact_review_contract(&self) -> Result<CapabilityContract> {
        self.contracts
            .active_contracts()
            .into_iter()
            .find(|contract| contract.contract_id == PUFFER_TOOLS_CONTRACT_ID)
            .ok_or_else(|| anyhow!("missing puffer.tools contract for artifact review"))
    }
}

fn artifact_review_accepts(decision: &GoalVerificationResult) -> bool {
    decision.satisfied && decision.confidence >= ARTIFACT_REVIEW_MIN_CONFIDENCE
}

fn model_authored_artifact_action(action: &ActionContract, output: &Value) -> bool {
    match action.side_effect_class {
        SideEffectClass::LocalWrite
        | SideEffectClass::ExternalWrite
        | SideEffectClass::DestructiveWrite => true,
        SideEffectClass::Unknown => output_has_created_or_modified_workspace_witness(output),
        _ => false,
    }
}

fn restore_rejected_artifact_write(
    workspace_root: &Path,
    args: &Value,
    output: &Value,
) -> Result<bool> {
    let mut restored = false;
    let Some(structured_output) = output.get("structured_output").and_then(Value::as_object) else {
        return restore_rejected_witness_creates(workspace_root, output);
    };
    let path = structured_output
        .get("filePath")
        .or_else(|| structured_output.get("file_path"))
        .and_then(Value::as_str)
        .or_else(|| args.get("file_path").and_then(Value::as_str))
        .or_else(|| args.get("path").and_then(Value::as_str));
    if let Some(path) = path {
        let path = workspace_path(workspace_root, path);
        let rejected_content = structured_output.get("content").and_then(Value::as_str);
        if let Some(rejected_content) = rejected_content {
            let Ok(current) = fs::read_to_string(&path) else {
                return restore_rejected_witness_creates(workspace_root, output);
            };
            if current != rejected_content {
                return restore_rejected_witness_creates(workspace_root, output);
            }
        }
        match structured_output.get("originalFile") {
            Some(Value::String(original)) => {
                fs::write(&path, original)?;
                restored = true;
            }
            Some(Value::Null) | None
                if structured_output
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind == "create") =>
            {
                fs::remove_file(&path)?;
                restored = true;
            }
            _ => {}
        }
    }
    Ok(restore_rejected_witness_creates(workspace_root, output)? || restored)
}

fn workspace_path(workspace_root: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

fn output_has_created_or_modified_workspace_witness(output: &Value) -> bool {
    output
        .get("filesystem_witness")
        .is_some_and(witness_has_created_or_modified)
}

fn witness_has_created_or_modified(witness: &Value) -> bool {
    ["created", "modified"].iter().any(|key| {
        witness
            .get(*key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    })
}

fn current_artifact_records(workspace_root: &Path, output: &Value) -> Value {
    let mut records = Vec::new();
    if let Some(witness) = output.get("filesystem_witness") {
        collect_witness_artifact_records(workspace_root, witness, "created", &mut records);
        collect_witness_artifact_records(workspace_root, witness, "modified", &mut records);
    }
    if let Some(path) = output
        .get("structured_output")
        .and_then(|value| value.get("filePath").or_else(|| value.get("file_path")))
        .and_then(Value::as_str)
    {
        records.push(artifact_record_for_path(
            workspace_root,
            path,
            "structured_output",
        ));
    }
    json!({
        "items": records,
    })
}

fn collect_witness_artifact_records(
    workspace_root: &Path,
    witness: &Value,
    kind: &'static str,
    records: &mut Vec<Value>,
) {
    if let Some(details) = witness
        .get("details")
        .and_then(|value| value.get(kind))
        .and_then(Value::as_array)
    {
        for detail in details {
            let Some(path) = detail.get("path").and_then(Value::as_str) else {
                continue;
            };
            let mut record = artifact_record_for_path(workspace_root, path, kind);
            if let Some(object) = record.as_object_mut() {
                object.insert("witness".to_string(), detail.clone());
            }
            records.push(record);
        }
        return;
    }
    if let Some(paths) = witness.get(kind).and_then(Value::as_array) {
        for path in paths.iter().filter_map(Value::as_str) {
            records.push(artifact_record_for_path(workspace_root, path, kind));
        }
    }
}

fn artifact_record_for_path(workspace_root: &Path, path: &str, kind: &'static str) -> Value {
    let path_buf = workspace_path(workspace_root, path);
    let metadata = fs::metadata(&path_buf).ok();
    let exists = metadata.as_ref().is_some_and(|metadata| metadata.is_file());
    json!({
        "path": path,
        "kind": kind,
        "exists": exists,
        "len": metadata.as_ref().map(|metadata| metadata.len()),
        "sha256": metadata
            .as_ref()
            .filter(|metadata| metadata.is_file())
            .and_then(|_| file_sha256(&path_buf).ok()),
        "content_preview": read_text_preview(&path_buf),
    })
}

fn read_text_preview(path: &Path) -> Option<String> {
    const MAX_PREVIEW_BYTES: usize = 16 * 1024;
    let bytes = fs::read(path).ok()?;
    if bytes.iter().take(MAX_PREVIEW_BYTES).any(|byte| *byte == 0) {
        return None;
    }
    let take = bytes.len().min(MAX_PREVIEW_BYTES);
    Some(String::from_utf8_lossy(&bytes[..take]).into_owned())
}

fn restore_rejected_witness_creates(workspace_root: &Path, output: &Value) -> Result<bool> {
    let Some(witness) = output.get("filesystem_witness") else {
        return Ok(false);
    };
    let mut restored = false;
    for detail in witness_created_details(witness) {
        let Some(path) = detail.get("path").and_then(Value::as_str) else {
            continue;
        };
        let Some(after_hash) = detail
            .get("after")
            .and_then(|value| value.get("sha256"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let path = workspace_path(workspace_root, path);
        if !path.is_file() {
            continue;
        }
        if file_sha256(&path).ok().as_deref() != Some(after_hash) {
            continue;
        }
        fs::remove_file(path)?;
        restored = true;
    }
    Ok(restored)
}

fn witness_created_details(witness: &Value) -> Vec<Value> {
    if let Some(details) = witness
        .get("details")
        .and_then(|value| value.get("created"))
        .and_then(Value::as_array)
    {
        return details.clone();
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, Idempotency,
        InMemoryContractRegistry, Reversibility, RiskLevel, TrustLevel, VerificationSpec,
    };
    use crate::graph::{PlanEdgeKind, PlanNode, PlanNodeKind};
    use crate::trace::JsonlTraceStore;
    use serde_json::json;

    fn action(side_effect_class: SideEffectClass) -> ActionContract {
        ActionContract {
            name: "Tool".to_string(),
            description: "tool".to_string(),
            input_schema: json!({"type": "object", "additionalProperties": true}),
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
    fn downstream_action_does_not_defer_generated_artifact_review() {
        let workspace = tempfile::tempdir().unwrap();
        let mut write = action(SideEffectClass::LocalWrite);
        write.name = "Write".to_string();
        let mut bash = action(SideEffectClass::Unknown);
        bash.name = "Bash".to_string();
        let mut registry = InMemoryContractRegistry::default();
        registry
            .register(CapabilityContract {
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                version: "0.1.0".to_string(),
                status: ContractStatus::Active,
                trust_level: TrustLevel::Sandboxed,
                description: "tools".to_string(),
                actions: vec![write, bash],
                global_constraints: Vec::new(),
                forbidden_uses: Vec::new(),
                local_rules: Vec::new(),
                examples: Vec::new(),
                contract_hash: None,
            })
            .unwrap();
        let runtime = BurbotRuntime::new(
            registry,
            workspace.path().into(),
            JsonlTraceStore::new(workspace.path().join("traces")),
        )
        .unwrap();
        let mut graph = PlanGraph::from_goal("ship generated artifact".to_string());
        let write_id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Executed,
            label: "Write".to_string(),
            payload: json!({
                "file_path": workspace.path().join("artifact.py").display().to_string(),
                "content": "print('generated')",
            }),
            action_ref: Some(ActionRef {
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                action_name: "Write".to_string(),
            }),
            scores: Default::default(),
        });
        let run_id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: "Bash".to_string(),
            payload: json!({"command": "python3 artifact.py"}),
            action_ref: Some(ActionRef {
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                action_name: "Bash".to_string(),
            }),
            scores: Default::default(),
        });
        graph.add_edge(
            NodeId(0),
            write_id,
            PlanEdgeKind::Supports,
            json!({
                "source": "model_observation_proposal",
                "completion_role": "repair",
            }),
        );
        graph.add_edge(write_id, run_id, PlanEdgeKind::DependsOn, json!({}));
        let progress = ProgressEvidence {
            changes_state: true,
            has_state_witness: true,
            has_structural_witness: true,
            has_output_witness: true,
        };

        assert!(runtime.should_review_generated_artifact(
            &graph,
            write_id,
            &ActionRef {
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                action_name: "Write".to_string(),
            },
            &json!({
                "structured_output": {
                    "filePath": workspace.path().join("artifact.py").display().to_string()
                }
            }),
            &progress,
        ));
    }

    #[test]
    fn rejected_generated_update_restores_original_file() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("artifact.txt");
        fs::write(&path, "bad replacement").unwrap();

        let restored = restore_rejected_artifact_write(
            workspace.path(),
            &json!({"file_path": path.display().to_string()}),
            &json!({
                "structured_output": {
                    "type": "update",
                    "filePath": path.display().to_string(),
                    "content": "bad replacement",
                    "originalFile": "original content"
                }
            }),
        )
        .unwrap();

        assert!(restored);
        assert_eq!(fs::read_to_string(path).unwrap(), "original content");
    }

    #[test]
    fn rejected_generated_create_removes_file_from_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("artifact.txt");
        fs::write(&path, "created content").unwrap();

        let restored = restore_rejected_artifact_write(
            workspace.path(),
            &json!({"file_path": path.display().to_string()}),
            &json!({
                "structured_output": {
                    "type": "create",
                    "filePath": path.display().to_string(),
                    "content": "created content",
                    "originalFile": null
                }
            }),
        )
        .unwrap();

        assert!(restored);
        assert!(!path.exists());
    }

    #[test]
    fn rejected_generated_write_does_not_overwrite_changed_file() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("artifact.txt");
        fs::write(&path, "newer content").unwrap();

        let restored = restore_rejected_artifact_write(
            workspace.path(),
            &json!({"file_path": path.display().to_string()}),
            &json!({
                "structured_output": {
                    "type": "update",
                    "filePath": path.display().to_string(),
                    "content": "bad replacement",
                    "originalFile": "original content"
                }
            }),
        )
        .unwrap();

        assert!(!restored);
        assert_eq!(fs::read_to_string(path).unwrap(), "newer content");
    }

    #[test]
    fn unknown_command_execution_is_not_a_direct_reviewable_artifact() {
        assert!(!model_authored_artifact_action(
            &action(SideEffectClass::Unknown),
            &json!({"structured_output": {"success": true}}),
        ));
    }

    #[test]
    fn local_write_with_content_is_a_direct_reviewable_artifact() {
        assert!(model_authored_artifact_action(
            &action(SideEffectClass::LocalWrite),
            &json!({"structured_output": {"filePath": "/tmp/generated.c"}}),
        ));
    }

    #[test]
    fn unknown_command_with_filesystem_witness_is_reviewable_artifact() {
        assert!(model_authored_artifact_action(
            &action(SideEffectClass::Unknown),
            &json!({"filesystem_witness": {"created": ["generated.c"], "modified": [], "removed": []}}),
        ));
    }

    #[test]
    fn verification_role_unknown_action_with_witness_gets_artifact_review() {
        let workspace = tempfile::tempdir().unwrap();
        let mut bash = action(SideEffectClass::Unknown);
        bash.name = "Bash".to_string();
        let mut registry = InMemoryContractRegistry::default();
        registry
            .register(CapabilityContract {
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                version: "0.1.0".to_string(),
                status: ContractStatus::Active,
                trust_level: TrustLevel::Sandboxed,
                description: "tools".to_string(),
                actions: vec![bash],
                global_constraints: Vec::new(),
                forbidden_uses: Vec::new(),
                local_rules: Vec::new(),
                examples: Vec::new(),
                contract_hash: None,
            })
            .unwrap();
        let runtime = BurbotRuntime::new(
            registry,
            workspace.path().into(),
            JsonlTraceStore::new(workspace.path().join("traces")),
        )
        .unwrap();
        let mut graph = PlanGraph::from_goal("review generated verifier output".to_string());
        let action_id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Executed,
            label: "Bash".to_string(),
            payload: json!({"command": "opaque"}),
            action_ref: Some(ActionRef {
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                action_name: "Bash".to_string(),
            }),
            scores: Default::default(),
        });
        graph.add_edge(
            NodeId(0),
            action_id,
            PlanEdgeKind::Supports,
            json!({
                "source": "model_observation_proposal",
                "completion_role": "verification",
            }),
        );
        let progress = ProgressEvidence {
            changes_state: true,
            has_state_witness: true,
            has_structural_witness: true,
            has_output_witness: true,
        };

        assert!(runtime.should_review_generated_artifact(
            &graph,
            action_id,
            &ActionRef {
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                action_name: "Bash".to_string(),
            },
            &json!({
                "filesystem_witness": {
                    "created": [workspace.path().join("artifact.txt").display().to_string()],
                    "modified": [],
                    "removed": []
                }
            }),
            &progress,
        ));
    }
}
