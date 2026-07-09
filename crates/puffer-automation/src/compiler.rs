use crate::{
    automation_spec_hash, AgentEnvNodeRef, AutomationFlowSpec, AutomationRecord,
    AutomationStepSpec, AutomationTriggerSpec, CompiledWorkflowRole,
};
pub use puffer_workflow::{AgentEnvWorkflowDefinition, AgentEnvWorkflowEdge, AgentEnvWorkflowNode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Internal runtime plan produced from one user-facing Automation record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationCompilePlan {
    pub automation_id: String,
    pub revision: u64,
    pub spec_hash: String,
    pub workflows: Vec<CompiledWorkflowDefinition>,
    pub puffer_bindings: Vec<CompiledPufferBindingPlan>,
}

/// One AgentEnv workflow definition artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledWorkflowDefinition {
    pub role: CompiledWorkflowRole,
    pub definition: Value,
    pub definition_hash: String,
}

/// One Puffer-side connector binding artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledPufferBindingPlan {
    pub trigger_id: String,
    pub binding_slug: String,
    pub connection_slug: String,
    pub connector_slug: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AutomationCompileError {
    #[error("invalid automation spec hash: {0}")]
    Hash(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("failed to serialize AgentEnv workflow definition: {0}")]
    Serialize(String),
}

/// Compiles a persisted Automation into AgentEnv workflow artifacts plus
/// Puffer-owned binding plans. User-facing Puffer-owned steps such as
/// `puffer_agent` and `puffer_connector_action` are runtime boundaries; they
/// split the AgentEnv graph into supported node segments plus daemon-executed
/// steps.
pub fn compile_automation(
    record: &AutomationRecord,
) -> Result<AutomationCompilePlan, AutomationCompileError> {
    let spec_hash = automation_spec_hash(&record.spec).map_err(AutomationCompileError::Hash)?;
    let mut workflows = Vec::new();
    workflows.push(compile_root_workflow(record)?);
    workflows.extend(compile_connector_continuation_workflows(&record.spec.flow)?);
    workflows.extend(compile_loop_body_workflows(&record.spec.flow)?);

    let puffer_bindings = record
        .spec
        .triggers
        .iter()
        .filter_map(|trigger| match trigger {
            AutomationTriggerSpec::PufferConnection {
                id,
                connection_slug,
                connector_slug,
                ..
            } => Some(CompiledPufferBindingPlan {
                trigger_id: id.clone(),
                binding_slug: automation_binding_slug(&record.id, id),
                connection_slug: connection_slug.clone(),
                connector_slug: connector_slug.clone(),
            }),
            AutomationTriggerSpec::AgentEnvNode { .. } => None,
        })
        .collect();

    Ok(AutomationCompilePlan {
        automation_id: record.id.clone(),
        revision: record.revision,
        spec_hash,
        workflows,
        puffer_bindings,
    })
}

fn compile_root_workflow(
    record: &AutomationRecord,
) -> Result<CompiledWorkflowDefinition, AutomationCompileError> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut previous_ids = Vec::new();

    for trigger in &record.spec.triggers {
        match trigger {
            AutomationTriggerSpec::AgentEnvNode { id, node, .. } => {
                let span = append_agentenv_nodes_for_automation(&mut nodes, id, node)?;
                edges.extend(span.internal_edges);
                previous_ids.extend(span.exit_ids);
            }
            AutomationTriggerSpec::PufferConnection { .. } => {}
        }
    }

    let mut seen_loop = None::<String>;
    let mut seen_boundary = None::<String>;
    for step in &record.spec.flow.steps {
        match step {
            AutomationStepSpec::AgentEnvNode { id, node, .. } => {
                if is_puffer_runtime_boundary(node) {
                    seen_boundary = Some(id.clone());
                    continue;
                }
                if seen_boundary.is_some() {
                    continue;
                }
                if let Some(loop_id) = seen_loop {
                    return Err(AutomationCompileError::Unsupported(format!(
                        "loop continuation compilation is not implemented yet; step `{id}` follows loop `{loop_id}`"
                    )));
                }
                let span = append_agentenv_nodes_for_automation(&mut nodes, id, node)?;
                for previous_id in &previous_ids {
                    for entry_id in &span.entry_ids {
                        edges.push(workflow_edge(previous_id, entry_id));
                    }
                }
                edges.extend(span.internal_edges);
                previous_ids = span.exit_ids;
            }
            AutomationStepSpec::Agent { id, .. } => {
                // The iterative agent runs entirely in the Puffer daemon. It is a
                // runtime boundary: nothing after it belongs in the root workflow.
                seen_boundary = Some(id.clone());
            }
            AutomationStepSpec::Loop { id, .. } => {
                if seen_boundary.is_some() {
                    continue;
                }
                if seen_loop.is_some() {
                    return Err(AutomationCompileError::Unsupported(
                        "multiple loop steps in one Automation are not implemented yet".into(),
                    ));
                }
                seen_loop = Some(id.clone());
            }
        }
    }

    compiled_workflow(
        CompiledWorkflowRole::Root,
        AgentEnvWorkflowDefinition { nodes, edges },
    )
}

fn compile_connector_continuation_workflows(
    flow: &AutomationFlowSpec,
) -> Result<Vec<CompiledWorkflowDefinition>, AutomationCompileError> {
    // Puffer-owned steps are runtime boundaries. The compiler emits AgentEnv
    // workflows only for the segments that run after each boundary.
    let mut workflows = Vec::new();
    let mut current_boundary_id = None::<String>;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut previous_ids = Vec::new();

    for step in &flow.steps {
        match step {
            AutomationStepSpec::AgentEnvNode { id, node, .. }
                if is_puffer_runtime_boundary(node) =>
            {
                push_connector_continuation_workflow(
                    &mut workflows,
                    &mut current_boundary_id,
                    &mut nodes,
                    &mut edges,
                    &mut previous_ids,
                )?;
                current_boundary_id = Some(id.clone());
            }
            AutomationStepSpec::AgentEnvNode { id, node, .. } => {
                if current_boundary_id.is_none() {
                    continue;
                }
                let span = append_agentenv_nodes_for_automation(&mut nodes, id, node)?;
                for previous_id in &previous_ids {
                    for entry_id in &span.entry_ids {
                        edges.push(workflow_edge(previous_id, entry_id));
                    }
                }
                edges.extend(span.internal_edges);
                previous_ids = span.exit_ids;
            }
            AutomationStepSpec::Agent { id, .. } => {
                // Close any open continuation segment; the agent boundary starts a
                // fresh segment for whatever AgentEnv nodes follow it.
                push_connector_continuation_workflow(
                    &mut workflows,
                    &mut current_boundary_id,
                    &mut nodes,
                    &mut edges,
                    &mut previous_ids,
                )?;
                current_boundary_id = Some(id.clone());
            }
            AutomationStepSpec::Loop { .. } => {
                push_connector_continuation_workflow(
                    &mut workflows,
                    &mut current_boundary_id,
                    &mut nodes,
                    &mut edges,
                    &mut previous_ids,
                )?;
            }
        }
    }
    push_connector_continuation_workflow(
        &mut workflows,
        &mut current_boundary_id,
        &mut nodes,
        &mut edges,
        &mut previous_ids,
    )?;
    Ok(workflows)
}

fn push_connector_continuation_workflow(
    workflows: &mut Vec<CompiledWorkflowDefinition>,
    current_connector_id: &mut Option<String>,
    nodes: &mut Vec<AgentEnvWorkflowNode>,
    edges: &mut Vec<AgentEnvWorkflowEdge>,
    previous_ids: &mut Vec<String>,
) -> Result<(), AutomationCompileError> {
    let Some(step_id) = current_connector_id.take() else {
        return Ok(());
    };
    if nodes.is_empty() {
        edges.clear();
        previous_ids.clear();
        return Ok(());
    }
    workflows.push(compiled_workflow(
        CompiledWorkflowRole::Continuation { step_id },
        AgentEnvWorkflowDefinition {
            nodes: std::mem::take(nodes),
            edges: std::mem::take(edges),
        },
    )?);
    previous_ids.clear();
    Ok(())
}

fn compile_loop_body_workflows(
    flow: &AutomationFlowSpec,
) -> Result<Vec<CompiledWorkflowDefinition>, AutomationCompileError> {
    let mut workflows = Vec::new();
    for step in &flow.steps {
        if let AutomationStepSpec::Loop { id, body, .. } = step {
            validate_loop_body_connector_terminal_suffix(id, body)?;
            workflows.push(compile_loop_body_workflow(id, body)?);
        }
    }
    Ok(workflows)
}

fn validate_loop_body_connector_terminal_suffix(
    loop_id: &str,
    body: &AutomationFlowSpec,
) -> Result<(), AutomationCompileError> {
    let mut terminal_connector_suffix = None::<String>;
    for step in &body.steps {
        match step {
            AutomationStepSpec::AgentEnvNode { id, node, .. } => {
                if is_puffer_connector_action(node) {
                    terminal_connector_suffix.get_or_insert_with(|| id.clone());
                } else if let Some(connector_id) = &terminal_connector_suffix {
                    return Err(AutomationCompileError::Unsupported(format!(
                        "automation loop `{loop_id}` body step `{id}` cannot follow connector action `{connector_id}`; connector actions run as the terminal loop-body suffix"
                    )));
                }
            }
            AutomationStepSpec::Agent { id, .. } => {
                return Err(AutomationCompileError::Unsupported(format!(
                    "automation loop `{loop_id}` body agent step `{id}` is not supported; the iterative agent owns its own loop"
                )));
            }
            AutomationStepSpec::Loop { id, .. } => {
                if let Some(connector_id) = &terminal_connector_suffix {
                    return Err(AutomationCompileError::Unsupported(format!(
                        "automation loop `{loop_id}` body loop `{id}` cannot follow connector action `{connector_id}`; connector actions run as the terminal loop-body suffix"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn compile_loop_body_workflow(
    step_id: &str,
    body: &AutomationFlowSpec,
) -> Result<CompiledWorkflowDefinition, AutomationCompileError> {
    validate_loop_body_connector_terminal_suffix(step_id, body)?;

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut previous_ids = Vec::<String>::new();

    for step in &body.steps {
        match step {
            AutomationStepSpec::AgentEnvNode { id, node, .. } => {
                if is_puffer_runtime_boundary(node) {
                    previous_ids.clear();
                    continue;
                }
                let span = append_loop_body_agentenv_nodes_for_automation(&mut nodes, id, node)?;
                for previous_id in &previous_ids {
                    for entry_id in &span.entry_ids {
                        edges.push(workflow_edge(previous_id, entry_id));
                    }
                }
                edges.extend(span.internal_edges);
                previous_ids = span.exit_ids;
            }
            AutomationStepSpec::Agent { id, .. } => {
                return Err(AutomationCompileError::Unsupported(format!(
                    "automation loop `{step_id}` body agent step `{id}` is not supported; the iterative agent owns its own loop"
                )));
            }
            AutomationStepSpec::Loop { id, .. } => {
                return Err(AutomationCompileError::Unsupported(format!(
                    "nested loop compilation is not implemented yet; loop `{id}` is inside loop `{step_id}`"
                )));
            }
        }
    }

    compiled_workflow(
        CompiledWorkflowRole::LoopBody {
            step_id: step_id.to_string(),
        },
        AgentEnvWorkflowDefinition { nodes, edges },
    )
}

fn agentenv_node(id: &str, node: &AgentEnvNodeRef) -> AgentEnvWorkflowNode {
    AgentEnvWorkflowNode {
        id: id.to_string(),
        node_type: node.node_type.clone(),
        name: node.name.clone(),
        config: node.config.clone(),
        trusted: node.trusted,
        position: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentEnvNodeSpan {
    entry_ids: Vec<String>,
    exit_ids: Vec<String>,
    internal_edges: Vec<AgentEnvWorkflowEdge>,
}

fn append_agentenv_nodes_for_automation(
    nodes: &mut Vec<AgentEnvWorkflowNode>,
    id: &str,
    node: &AgentEnvNodeRef,
) -> Result<AgentEnvNodeSpan, AutomationCompileError> {
    match node.node_type.as_str() {
        "puffer_agent" => Ok(AgentEnvNodeSpan {
            entry_ids: Vec::new(),
            exit_ids: Vec::new(),
            internal_edges: Vec::new(),
        }),
        "tool_capability" => {
            Err(AutomationCompileError::Unsupported(format!(
                "Automation step `{id}` uses Puffer-only node `{}`; compile it to an AgentEnv-supported node such as `transform_js` before preparing the runtime",
                node.node_type
            )))
        }
        "puffer_connector_action" => Ok(AgentEnvNodeSpan {
            entry_ids: Vec::new(),
            exit_ids: Vec::new(),
            internal_edges: Vec::new(),
        }),
        _ => {
            nodes.push(agentenv_node(id, node));
            Ok(AgentEnvNodeSpan {
                entry_ids: vec![id.to_string()],
                exit_ids: vec![id.to_string()],
                internal_edges: Vec::new(),
            })
        }
    }
}

fn is_puffer_connector_action(node: &AgentEnvNodeRef) -> bool {
    node.node_type == "puffer_connector_action"
}

fn is_puffer_runtime_boundary(node: &AgentEnvNodeRef) -> bool {
    matches!(
        node.node_type.as_str(),
        "puffer_agent" | "puffer_connector_action"
    )
}

fn append_loop_body_agentenv_nodes_for_automation(
    nodes: &mut Vec<AgentEnvWorkflowNode>,
    id: &str,
    node: &AgentEnvNodeRef,
) -> Result<AgentEnvNodeSpan, AutomationCompileError> {
    match node.node_type.as_str() {
        "puffer_agent" => Ok(AgentEnvNodeSpan {
            entry_ids: Vec::new(),
            exit_ids: Vec::new(),
            internal_edges: Vec::new(),
        }),
        "tool_capability" => {
            Err(AutomationCompileError::Unsupported(format!(
                "Automation step `{id}` uses Puffer-only node `{}`; compile it to an AgentEnv-supported node such as `transform_js` before preparing the runtime",
                node.node_type
            )))
        }
        "puffer_connector_action" => Ok(AgentEnvNodeSpan {
            entry_ids: Vec::new(),
            exit_ids: Vec::new(),
            internal_edges: Vec::new(),
        }),
        _ => {
            nodes.push(agentenv_node(id, node));
            Ok(AgentEnvNodeSpan {
                entry_ids: vec![id.to_string()],
                exit_ids: vec![id.to_string()],
                internal_edges: Vec::new(),
            })
        }
    }
}

fn workflow_edge(source: &str, target: &str) -> AgentEnvWorkflowEdge {
    AgentEnvWorkflowEdge {
        source: source.to_string(),
        target: target.to_string(),
        condition_script: None,
    }
}

fn compiled_workflow(
    role: CompiledWorkflowRole,
    definition: AgentEnvWorkflowDefinition,
) -> Result<CompiledWorkflowDefinition, AutomationCompileError> {
    let definition = serde_json::to_value(definition)
        .map_err(|error| AutomationCompileError::Serialize(error.to_string()))?;
    let definition_hash = stable_json_hash(&definition)?;
    Ok(CompiledWorkflowDefinition {
        role,
        definition,
        definition_hash,
    })
}

fn stable_json_hash(value: &Value) -> Result<String, AutomationCompileError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| AutomationCompileError::Serialize(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn automation_binding_slug(automation_id: &str, trigger_id: &str) -> String {
    format!("automation-{automation_id}-{trigger_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AutomationFlowSpec, AutomationLoopInput, AutomationLoopSpec, AutomationReviewSpec,
        AutomationSource, AutomationSpec, AutomationStatus, AUTOMATION_SPEC_VERSION,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    fn node(node_type: &str) -> AgentEnvNodeRef {
        AgentEnvNodeRef {
            node_type: node_type.to_string(),
            name: Some(node_type.to_string()),
            trusted: Some(true),
            config: BTreeMap::new(),
        }
    }

    fn record(flow: AutomationFlowSpec, triggers: Vec<AutomationTriggerSpec>) -> AutomationRecord {
        AutomationRecord {
            id: "reply-helper".into(),
            status: AutomationStatus::Enabled,
            revision: 7,
            spec: AutomationSpec {
                spec_version: AUTOMATION_SPEC_VERSION,
                name: "Reply helper".into(),
                description: None,
                source: AutomationSource::Blank,
                instructions: "Draft a reply.".into(),
                run_location: Default::default(),
                triggers,
                flow,
                review: AutomationReviewSpec::default(),
            },
            runtime: Default::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn puffer_connection_trigger() -> AutomationTriggerSpec {
        AutomationTriggerSpec::PufferConnection {
            id: "incoming".into(),
            connection_slug: "telegram-user".into(),
            connector_slug: Some("telegram-login".into()),
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            summary: None,
        }
    }

    fn assert_no_agentenv_managed_agent_nodes(plan: &AutomationCompilePlan) {
        for workflow in &plan.workflows {
            let nodes = workflow.definition["nodes"].as_array().unwrap();
            assert!(nodes.iter().all(|node| {
                !matches!(
                    node.get("type").and_then(Value::as_str),
                    Some("managed_agent_create" | "managed_agent_call")
                )
            }));
        }
    }

    #[test]
    fn linear_automation_compiles_one_root_workflow() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![
                    AutomationStepSpec::AgentEnvNode {
                        id: "draft".into(),
                        node: node("llm"),
                        summary: None,
                    },
                    AutomationStepSpec::AgentEnvNode {
                        id: "format".into(),
                        node: node("transform"),
                        summary: None,
                    },
                ],
            },
            vec![puffer_connection_trigger()],
        );

        let plan = compile_automation(&record).unwrap();

        assert_eq!(plan.workflows.len(), 1);
        assert_eq!(plan.workflows[0].role, CompiledWorkflowRole::Root);
        assert_eq!(
            plan.workflows[0].definition["nodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            plan.workflows[0].definition["edges"],
            json!([
                {"source": "draft", "target": "format"}
            ])
        );
    }

    #[test]
    fn step_edges_are_linear_without_self_loop() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![
                    AutomationStepSpec::AgentEnvNode {
                        id: "a".into(),
                        node: node("a"),
                        summary: None,
                    },
                    AutomationStepSpec::AgentEnvNode {
                        id: "b".into(),
                        node: node("b"),
                        summary: None,
                    },
                ],
            },
            vec![puffer_connection_trigger()],
        );

        let plan = compile_automation(&record).unwrap();
        let edges = plan.workflows[0].definition["edges"].as_array().unwrap();

        assert!(edges.iter().all(|edge| edge["source"] != edge["target"]));
    }

    #[test]
    fn puffer_connection_trigger_generates_binding_plan() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "draft".into(),
                    node: node("llm"),
                    summary: None,
                }],
            },
            vec![AutomationTriggerSpec::PufferConnection {
                id: "incoming".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                filter: None,
                ignore_filters: Vec::new(),
                contact_ids: Vec::new(),
                summary: None,
            }],
        );

        let plan = compile_automation(&record).unwrap();

        assert_eq!(plan.puffer_bindings.len(), 1);
        assert_eq!(
            plan.puffer_bindings[0].binding_slug,
            "automation-reply-helper-incoming"
        );
        assert_eq!(plan.puffer_bindings[0].connection_slug, "telegram-user");
        assert_eq!(plan.workflows[0].definition["nodes"][0]["id"], "draft");
        assert_eq!(plan.workflows[0].definition["nodes"][0]["type"], "llm");
        assert!(plan.workflows[0].definition["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["type"] != "webhook"));
        assert_eq!(plan.workflows[0].definition["edges"], json!([]));
    }

    #[test]
    fn agentenv_trigger_becomes_root_start_node() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "draft".into(),
                    node: node("llm"),
                    summary: None,
                }],
            },
            vec![AutomationTriggerSpec::AgentEnvNode {
                id: "webhook".into(),
                node: node("webhook"),
                summary: None,
            }],
        );

        let plan = compile_automation(&record).unwrap();

        assert_eq!(
            plan.workflows[0].definition["nodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            plan.workflows[0].definition["edges"],
            json!([
                {"source": "webhook", "target": "draft"}
            ])
        );
    }

    #[test]
    fn puffer_agent_is_left_for_puffer_runner() {
        let mut agent = node("puffer_agent");
        agent.name = Some("Workflow Agent".into());
        agent.config.insert(
            "instructions".into(),
            json!("Use these product-level agent instructions."),
        );

        let record = record(
            AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "agent".into(),
                    node: agent,
                    summary: None,
                }],
            },
            vec![puffer_connection_trigger()],
        );

        let plan = compile_automation(&record).unwrap();
        let root = plan
            .workflows
            .iter()
            .find(|workflow| workflow.role == CompiledWorkflowRole::Root)
            .expect("root workflow");

        assert!(root.definition["nodes"].as_array().unwrap().is_empty());
        assert_no_agentenv_managed_agent_nodes(&plan);
    }

    #[test]
    fn puffer_agent_splits_agentenv_continuation() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![
                    AutomationStepSpec::AgentEnvNode {
                        id: "prepare".into(),
                        node: node("transform_js"),
                        summary: None,
                    },
                    AutomationStepSpec::AgentEnvNode {
                        id: "agent".into(),
                        node: node("puffer_agent"),
                        summary: None,
                    },
                    AutomationStepSpec::AgentEnvNode {
                        id: "format".into(),
                        node: node("transform_js"),
                        summary: None,
                    },
                ],
            },
            vec![puffer_connection_trigger()],
        );

        let plan = compile_automation(&record).unwrap();
        let root = plan
            .workflows
            .iter()
            .find(|workflow| workflow.role == CompiledWorkflowRole::Root)
            .expect("root workflow");
        let continuation = plan
            .workflows
            .iter()
            .find(|workflow| {
                workflow.role
                    == (CompiledWorkflowRole::Continuation {
                        step_id: "agent".into(),
                    })
            })
            .expect("agent continuation workflow");

        assert_eq!(root.definition["nodes"][0]["id"], "prepare");
        assert_eq!(root.definition["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(continuation.definition["nodes"][0]["id"], "format");
        assert_no_agentenv_managed_agent_nodes(&plan);
    }

    #[test]
    fn loop_body_puffer_agent_does_not_emit_helper_or_template_call() {
        let mut agent = node("puffer_agent");
        agent.config.insert("agentId".into(), json!("agent-1"));
        agent.config.insert("content".into(), json!("Continue."));

        let record = record(
            AutomationFlowSpec {
                steps: vec![AutomationStepSpec::Loop {
                    id: "retry".into(),
                    loop_spec: AutomationLoopSpec::ForEach {
                        input: AutomationLoopInput::Trigger,
                        item_alias: "item".into(),
                        max_iterations: Some(3),
                    },
                    body: AutomationFlowSpec {
                        steps: vec![AutomationStepSpec::AgentEnvNode {
                            id: "agent".into(),
                            node: agent,
                            summary: None,
                        }],
                    },
                    summary: None,
                }],
            },
            vec![puffer_connection_trigger()],
        );

        let plan = compile_automation(&record).unwrap();
        let loop_body = plan
            .workflows
            .iter()
            .find(|workflow| {
                workflow.role
                    == (CompiledWorkflowRole::LoopBody {
                        step_id: "retry".into(),
                    })
            })
            .expect("loop body workflow");

        assert!(!plan
            .workflows
            .iter()
            .any(|workflow| matches!(workflow.role, CompiledWorkflowRole::Helper { .. })));
        assert!(loop_body.definition["nodes"].as_array().unwrap().is_empty());
        assert_no_agentenv_managed_agent_nodes(&plan);
    }

    #[test]
    fn unsupported_tool_capability_fails_before_runtime_submission() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "agent".into(),
                    node: node("tool_capability"),
                    summary: None,
                }],
            },
            vec![puffer_connection_trigger()],
        );

        let error = compile_automation(&record).unwrap_err();

        assert!(error.to_string().contains("Puffer-only node"));
        assert!(error.to_string().contains("tool_capability"));
    }

    #[test]
    fn puffer_connector_action_is_left_for_puffer_runner() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![
                    AutomationStepSpec::AgentEnvNode {
                        id: "agent".into(),
                        node: node("transform_js"),
                        summary: None,
                    },
                    AutomationStepSpec::AgentEnvNode {
                        id: "connector".into(),
                        node: node("puffer_connector_action"),
                        summary: None,
                    },
                ],
            },
            vec![puffer_connection_trigger()],
        );

        let plan = compile_automation(&record).unwrap();
        let root = plan
            .workflows
            .iter()
            .find(|workflow| matches!(workflow.role, CompiledWorkflowRole::Root))
            .expect("root workflow");

        assert_eq!(root.definition["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(root.definition["nodes"][0]["id"], "agent");
    }

    #[test]
    fn agentenv_steps_after_connector_action_compile_as_continuation() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![
                    AutomationStepSpec::AgentEnvNode {
                        id: "before".into(),
                        node: node("transform_js"),
                        summary: None,
                    },
                    AutomationStepSpec::AgentEnvNode {
                        id: "connector".into(),
                        node: node("puffer_connector_action"),
                        summary: None,
                    },
                    AutomationStepSpec::AgentEnvNode {
                        id: "after".into(),
                        node: node("transform_js"),
                        summary: None,
                    },
                ],
            },
            vec![puffer_connection_trigger()],
        );

        let plan = compile_automation(&record).unwrap();
        let root = plan
            .workflows
            .iter()
            .find(|workflow| matches!(workflow.role, CompiledWorkflowRole::Root))
            .expect("root workflow");
        let continuation = plan
            .workflows
            .iter()
            .find(|workflow| {
                matches!(
                    &workflow.role,
                    CompiledWorkflowRole::Continuation { step_id } if step_id == "connector"
                )
            })
            .expect("connector continuation");

        assert_eq!(root.definition["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(root.definition["nodes"][0]["id"], "before");
        assert_eq!(
            continuation.definition["nodes"].as_array().unwrap().len(),
            1
        );
        assert_eq!(continuation.definition["nodes"][0]["id"], "after");
    }

    #[test]
    fn loop_body_puffer_connector_action_is_left_for_puffer_runner() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![AutomationStepSpec::Loop {
                    id: "retry".into(),
                    loop_spec: AutomationLoopSpec::ForEach {
                        input: AutomationLoopInput::Trigger,
                        item_alias: "item".into(),
                        max_iterations: Some(3),
                    },
                    body: AutomationFlowSpec {
                        steps: vec![
                            AutomationStepSpec::AgentEnvNode {
                                id: "attempt".into(),
                                node: node("transform_js"),
                                summary: None,
                            },
                            AutomationStepSpec::AgentEnvNode {
                                id: "connector".into(),
                                node: node("puffer_connector_action"),
                                summary: None,
                            },
                            AutomationStepSpec::AgentEnvNode {
                                id: "notify".into(),
                                node: node("puffer_connector_action"),
                                summary: None,
                            },
                        ],
                    },
                    summary: None,
                }],
            },
            vec![puffer_connection_trigger()],
        );

        let plan = compile_automation(&record).unwrap();
        let loop_body = plan
            .workflows
            .iter()
            .find(|workflow| {
                workflow.role
                    == (CompiledWorkflowRole::LoopBody {
                        step_id: "retry".into(),
                    })
            })
            .expect("loop body workflow");

        assert_eq!(loop_body.definition["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(loop_body.definition["nodes"][0]["id"], "attempt");
    }

    #[test]
    fn loop_body_agentenv_step_after_puffer_connector_action_fails_before_compile() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![AutomationStepSpec::Loop {
                    id: "retry".into(),
                    loop_spec: AutomationLoopSpec::ForEach {
                        input: AutomationLoopInput::Trigger,
                        item_alias: "item".into(),
                        max_iterations: Some(3),
                    },
                    body: AutomationFlowSpec {
                        steps: vec![
                            AutomationStepSpec::AgentEnvNode {
                                id: "attempt".into(),
                                node: node("transform_js"),
                                summary: None,
                            },
                            AutomationStepSpec::AgentEnvNode {
                                id: "connector".into(),
                                node: node("puffer_connector_action"),
                                summary: None,
                            },
                            AutomationStepSpec::AgentEnvNode {
                                id: "after".into(),
                                node: node("transform_js"),
                                summary: None,
                            },
                        ],
                    },
                    summary: None,
                }],
            },
            vec![puffer_connection_trigger()],
        );

        let error = compile_automation(&record).unwrap_err();

        assert!(error.to_string().contains("after"));
        assert!(error.to_string().contains("connector"));
        assert!(error.to_string().contains("terminal loop-body suffix"));
    }

    #[test]
    fn loop_compiles_root_and_loop_body_without_backedge() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![
                    AutomationStepSpec::AgentEnvNode {
                        id: "prepare".into(),
                        node: node("prepare"),
                        summary: None,
                    },
                    AutomationStepSpec::Loop {
                        id: "retry".into(),
                        loop_spec: AutomationLoopSpec::ForEach {
                            input: AutomationLoopInput::Trigger,
                            item_alias: "item".into(),
                            max_iterations: Some(3),
                        },
                        body: AutomationFlowSpec {
                            steps: vec![AutomationStepSpec::AgentEnvNode {
                                id: "attempt".into(),
                                node: node("attempt"),
                                summary: None,
                            }],
                        },
                        summary: None,
                    },
                ],
            },
            vec![puffer_connection_trigger()],
        );

        let plan = compile_automation(&record).unwrap();

        assert_eq!(plan.workflows.len(), 2);
        assert_eq!(
            plan.workflows[1].role,
            CompiledWorkflowRole::LoopBody {
                step_id: "retry".into()
            }
        );
        for workflow in &plan.workflows {
            for edge in workflow.definition["edges"].as_array().unwrap() {
                assert_ne!(edge["source"], edge["target"]);
            }
        }
    }

    #[test]
    fn definition_hash_is_stable() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "draft".into(),
                    node: node("llm"),
                    summary: None,
                }],
            },
            vec![puffer_connection_trigger()],
        );

        let first = compile_automation(&record).unwrap();
        let second = compile_automation(&record).unwrap();

        assert_eq!(
            first.workflows[0].definition_hash,
            second.workflows[0].definition_hash
        );
    }

    #[test]
    fn unsupported_loop_continuation_returns_clear_error() {
        let record = record(
            AutomationFlowSpec {
                steps: vec![
                    AutomationStepSpec::Loop {
                        id: "retry".into(),
                        loop_spec: AutomationLoopSpec::ForEach {
                            input: AutomationLoopInput::Trigger,
                            item_alias: "item".into(),
                            max_iterations: Some(3),
                        },
                        body: AutomationFlowSpec {
                            steps: vec![AutomationStepSpec::AgentEnvNode {
                                id: "attempt".into(),
                                node: node("attempt"),
                                summary: None,
                            }],
                        },
                        summary: None,
                    },
                    AutomationStepSpec::AgentEnvNode {
                        id: "after".into(),
                        node: node("after"),
                        summary: None,
                    },
                ],
            },
            vec![puffer_connection_trigger()],
        );

        let error = compile_automation(&record).unwrap_err();

        assert!(error
            .to_string()
            .contains("loop continuation compilation is not implemented yet"));
    }
}
