use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// A registered Puffer workflow envelope around an AgentFlow-shaped pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowDefinition {
    /// Schema marker for future migrations.
    #[serde(default = "default_schema")]
    pub schema: String,
    /// Stable user-facing slug.
    pub slug: String,
    /// Whether triggers are allowed to fire this workflow.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Puffer-specific trigger metadata.
    pub trigger: TriggerSpec,
    /// AgentFlow-compatible graph payload where supported.
    pub pipeline: AgentFlowPipeline,
}

fn default_schema() -> String {
    "puffer.workflow.v1".to_string()
}

fn default_enabled() -> bool {
    true
}

/// Trigger variants supported by native workflows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerSpec {
    /// Five-field local-time cron expression.
    Cron {
        /// Cron expression in `min hour dom month dow` form.
        cron: String,
    },
    /// Event subscription trigger.
    Subscription {
        /// Subscriber topic, such as `telegram-user`.
        source_topic: String,
        /// Optional regex prefilter against event text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        /// Optional classifier prompt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        classify_prompt: Option<String>,
    },
    /// Connection trigger for the connector/workflow redesign.
    Connection {
        /// Authorized connection slug, such as `my-telegram`.
        connection_slug: String,
        /// Optional structured filter evaluated by Puffer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<Value>,
        /// Optional shorthand regex against event text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        /// Optional classifier prompt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        classify_prompt: Option<String>,
    },
}

/// Registration options supplied by CLI flags when importing raw pipelines.
#[derive(Debug, Clone, Default)]
pub struct RegisterOptions {
    /// Optional slug override.
    pub slug: Option<String>,
    /// Optional trigger to apply to a raw AgentFlow pipeline.
    pub trigger: Option<TriggerSpec>,
}

/// AgentFlow-shaped pipeline fields accepted by Puffer v1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentFlowPipeline {
    /// Human-readable workflow name.
    pub name: String,
    /// Optional working directory for agent nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Max runnable node count. The native runner validates this but may execute serially.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
    /// DAG nodes.
    pub nodes: Vec<PipelineNode>,
    /// Extra fields preserved for validation and forward compatibility.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A single local Puffer agent node in an AgentFlow-shaped pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PipelineNode {
    /// Stable node id used by dependencies and interpolation.
    pub id: String,
    /// Node type; omitted or `agent` is supported.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    /// Puffer agent identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Prompt template.
    pub prompt: String,
    /// Model hint passed to the agent executor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tool allowlist hint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Environment values made available to the node executor.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Dependency node ids.
    #[serde(default, alias = "dependsOn", skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Extra fields preserved for validation and diagnostics.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Overall run status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    /// The run has been created but not yet started.
    Pending,
    /// At least one node is currently running.
    Running,
    /// All runnable nodes completed successfully.
    Completed,
    /// A node failed.
    Failed,
    /// The run could not start or was skipped.
    Skipped,
}

/// Persisted workflow run record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowRun {
    /// Global monotonically assigned run index.
    pub idx: u64,
    /// Workflow slug.
    pub workflow_slug: String,
    /// Random run id.
    pub run_id: String,
    /// Trigger payload captured for interpolation and diagnostics.
    pub trigger: Value,
    /// Overall run status.
    pub status: WorkflowRunStatus,
    /// Start timestamp in milliseconds since UNIX epoch.
    pub started_at_ms: i128,
    /// End timestamp in milliseconds since UNIX epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<i128>,
    /// One record per pipeline node.
    pub nodes: Vec<WorkflowRunNode>,
    /// Optional failure message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Optional dedupe key, used by cron triggers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_key: Option<String>,
}

/// Persisted node execution record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowRunNode {
    /// Node id.
    pub id: String,
    /// Node status.
    pub status: WorkflowRunStatus,
    /// Start timestamp in milliseconds since UNIX epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i128>,
    /// End timestamp in milliseconds since UNIX epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<i128>,
    /// Output excerpt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Failure or skip message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WorkflowDefinition {
    /// Parses either a Puffer envelope or a raw AgentFlow pipeline plus options.
    pub fn from_json(value: Value, options: RegisterOptions) -> Result<Self> {
        if value.get("pipeline").is_some() {
            let mut definition: WorkflowDefinition = serde_json::from_value(value)
                .map_err(|error| anyhow!("invalid workflow: {error}"))?;
            if let Some(slug) = options.slug {
                definition.slug = slug;
            }
            if let Some(trigger) = options.trigger {
                definition.trigger = trigger;
            }
            validate_workflow(&definition)?;
            return Ok(definition);
        }

        let pipeline: AgentFlowPipeline = serde_json::from_value(value)
            .map_err(|error| anyhow!("invalid AgentFlow pipeline: {error}"))?;
        let slug = options
            .slug
            .clone()
            .unwrap_or_else(|| slugify(&pipeline.name));
        let trigger = options
            .trigger
            .ok_or_else(|| anyhow!("raw AgentFlow pipeline import requires a Puffer trigger"))?;
        let definition = WorkflowDefinition {
            schema: default_schema(),
            slug,
            enabled: true,
            trigger,
            pipeline,
        };
        validate_workflow(&definition)?;
        Ok(definition)
    }
}

/// Validates a workflow definition and rejects unsupported AgentFlow features.
pub fn validate_workflow(definition: &WorkflowDefinition) -> Result<()> {
    validate_slug(&definition.slug)?;
    match &definition.trigger {
        TriggerSpec::Cron { cron } => {
            crate::CronExpression::parse(cron)?;
        }
        TriggerSpec::Subscription {
            source_topic,
            pattern,
            ..
        } => {
            if source_topic.trim().is_empty() {
                return Err(anyhow!(
                    "subscription trigger source_topic must not be empty"
                ));
            }
            if let Some(pattern) = pattern {
                regex_like_check(pattern)?;
            }
        }
        TriggerSpec::Connection {
            connection_slug,
            filter,
            pattern,
            ..
        } => {
            if connection_slug.trim().is_empty() {
                return Err(anyhow!(
                    "connection trigger connection_slug must not be empty"
                ));
            }
            if let Some(pattern) = pattern {
                regex_like_check(pattern)?;
            }
            if matches!(filter, Some(Value::Null)) {
                return Err(anyhow!("connection trigger filter must not be null"));
            }
        }
    }

    let unsupported_pipeline = [
        "optimizer",
        "graph_optimization",
        "fanout",
        "fanouts",
        "merge",
        "remote",
        "targets",
        "python",
        "shell",
        "sync",
    ];
    for key in unsupported_pipeline {
        if definition.pipeline.extra.contains_key(key) {
            return Err(anyhow!("unsupported AgentFlow pipeline feature `{key}`"));
        }
    }
    if matches!(definition.pipeline.concurrency, Some(0)) {
        return Err(anyhow!("pipeline.concurrency must be greater than zero"));
    }
    if definition.pipeline.nodes.is_empty() {
        return Err(anyhow!("pipeline.nodes must not be empty"));
    }

    let mut ids = BTreeSet::new();
    for node in &definition.pipeline.nodes {
        validate_node(node)?;
        if !ids.insert(node.id.clone()) {
            return Err(anyhow!("duplicate node id `{}`", node.id));
        }
    }
    for node in &definition.pipeline.nodes {
        for dep in &node.depends_on {
            if !ids.contains(dep) {
                return Err(anyhow!(
                    "node `{}` depends on unknown node `{dep}`",
                    node.id
                ));
            }
        }
    }
    detect_cycle(&definition.pipeline.nodes)?;
    Ok(())
}

fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty()
        || !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(anyhow!("workflow slug must be non-empty kebab-case ASCII"));
    }
    Ok(())
}

/// Recognized node kinds. Everything that is not `Agent` is **deterministic** —
/// executed inline by the runner, no provider/LLM involved.
///
/// For agent nodes, the `type` field carries the provider id (e.g. `codex`,
/// `claude`, `puffer`) — that's NOT a kind. So `type` is only consulted
/// when it spells out an explicit non-agent kind; otherwise the node is an
/// agent and the kind defaults to "agent".
fn node_kind_str(node: &PipelineNode) -> &str {
    if let Some(value) = node
        .extra
        .get("kind")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return value;
    }
    match node.node_type.as_deref() {
        Some(value) if DETERMINISTIC_KINDS.contains(&value) => value,
        _ => "agent",
    }
}

const DETERMINISTIC_KINDS: &[&str] = &["tool", "merge", "fanout"];
const KNOWN_KINDS: &[&str] = &["agent", "tool", "merge", "fanout"];

fn validate_node(node: &PipelineNode) -> Result<()> {
    if node.id.trim().is_empty() {
        return Err(anyhow!("node id must not be empty"));
    }
    // AgentFlow control-flow primitive — the runner has no implementation
    // for it, so reject it before `node_kind_str` collapses it to "agent".
    if node.node_type.as_deref() == Some("sync") {
        return Err(anyhow!(
            "unsupported AgentFlow control-flow node `{}` type `sync`",
            node.id
        ));
    }
    let kind = node_kind_str(node);
    if !KNOWN_KINDS.contains(&kind) {
        return Err(anyhow!("unsupported node `{}` kind `{kind}`", node.id));
    }
    if let Some(target) = node.extra.get("target") {
        if !is_local_target(target) {
            return Err(anyhow!(
                "node `{}` uses an unsupported remote target",
                node.id
            ));
        }
    }
    for key in ["optimizer", "schedule", "on_failure_restart", "sync"] {
        if node.extra.contains_key(key) {
            return Err(anyhow!(
                "node `{}` uses unsupported feature `{key}`",
                node.id
            ));
        }
    }
    // Agent nodes must carry a prompt; deterministic nodes use `prompt` as
    // their argument input and may legitimately leave it empty (e.g. merge).
    if kind == "agent" && node.prompt.trim().is_empty() {
        return Err(anyhow!("node `{}` prompt must not be empty", node.id));
    }
    Ok(())
}

fn is_local_target(target: &Value) -> bool {
    match target {
        Value::String(s) => s == "local",
        Value::Object(map) => map
            .get("type")
            .or_else(|| map.get("kind"))
            .and_then(Value::as_str)
            .map(|s| s == "local")
            .unwrap_or(false),
        _ => false,
    }
}

fn detect_cycle(nodes: &[PipelineNode]) -> Result<()> {
    let deps: BTreeMap<&str, Vec<&str>> = nodes
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                node.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let mut visiting = BTreeSet::new();
    let mut done = BTreeSet::new();
    for node in nodes {
        visit(node.id.as_str(), &deps, &mut visiting, &mut done)?;
    }
    Ok(())
}

fn visit<'a>(
    id: &'a str,
    deps: &BTreeMap<&'a str, Vec<&'a str>>,
    visiting: &mut BTreeSet<&'a str>,
    done: &mut BTreeSet<&'a str>,
) -> Result<()> {
    if done.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id) {
        return Err(anyhow!(
            "workflow graph contains a dependency cycle at `{id}`"
        ));
    }
    for dep in deps.get(id).into_iter().flatten() {
        visit(dep, deps, visiting, done)?;
    }
    visiting.remove(id);
    done.insert(id);
    Ok(())
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn regex_like_check(pattern: &str) -> Result<()> {
    if pattern.trim().is_empty() {
        return Err(anyhow!("subscription trigger pattern must not be empty"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_envelope() {
        let definition = WorkflowDefinition::from_json(
            json!({
                "slug": "daily-review",
                "trigger": {"type":"cron","cron":"0 9 * * *"},
                "pipeline": {"name":"Daily Review","nodes":[{"id":"a","agent":"default","prompt":"go"}]}
            }),
            RegisterOptions::default(),
        )
        .unwrap();
        assert_eq!(definition.slug, "daily-review");
    }

    #[test]
    fn imports_raw_pipeline_with_trigger() {
        let definition = WorkflowDefinition::from_json(
            json!({"name":"Raw Flow","nodes":[{"id":"a","prompt":"go"}]}),
            RegisterOptions {
                slug: None,
                trigger: Some(TriggerSpec::Cron {
                    cron: "*/5 * * * *".into(),
                }),
            },
        )
        .unwrap();
        assert_eq!(definition.slug, "raw-flow");
    }

    #[test]
    fn parses_connection_trigger() {
        let definition = WorkflowDefinition::from_json(
            json!({
                "slug": "connection-flow",
                "trigger": {
                    "type": "connection",
                    "connection_slug": "my-telegram",
                    "filter": {"person": {"name": "TonyKe"}}
                },
                "pipeline": {"name":"Connection Flow","nodes":[{"id":"a","prompt":"go"}]}
            }),
            RegisterOptions::default(),
        )
        .unwrap();
        assert!(matches!(definition.trigger, TriggerSpec::Connection { .. }));
    }

    #[test]
    fn rejects_unsupported_node_kind() {
        // `shell` as a `type` is no longer rejected — it's just a tag on an
        // agent node since tool nodes cover that ground now. The unsupported
        // path is for explicit `kind` values outside KNOWN_KINDS.
        let err = WorkflowDefinition::from_json(
            json!({
                "slug":"x",
                "trigger":{"type":"cron","cron":"* * * * *"},
                "pipeline":{"name":"x","nodes":[
                    {"id":"a","kind":"bogus","prompt":"x"}
                ]}
            }),
            RegisterOptions::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn rejects_sync_control_flow_node() {
        // `sync` is an AgentFlow control-flow primitive the runner doesn't
        // implement; it remains explicitly rejected even though
        // `python`/`shell` were dropped as separate categories.
        let err = WorkflowDefinition::from_json(
            json!({
                "slug":"x",
                "trigger":{"type":"cron","cron":"* * * * *"},
                "pipeline":{"name":"x","nodes":[
                    {"id":"a","type":"sync","prompt":"x"}
                ]}
            }),
            RegisterOptions::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn accepts_provider_id_as_node_type_for_agent_nodes() {
        // Agent nodes carry their provider id in the `type` field (codex /
        // claude / puffer) — schema must not reject this as an unknown
        // kind. Regression guard for the
        // `workflow_save_upserts_definition_and_returns_snapshot`
        // daemon-side test that uses `type:"puffer"`.
        WorkflowDefinition::from_json(
            json!({
                "slug":"x",
                "trigger":{"type":"cron","cron":"* * * * *"},
                "pipeline":{"name":"x","nodes":[
                    {"id":"reply","type":"puffer","prompt":"Draft a reply."}
                ]}
            }),
            RegisterOptions::default(),
        )
        .expect("agent node with provider id type must be accepted");
    }
}
