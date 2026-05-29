use crate::{
    PipelineNode, WorkflowDefinition, WorkflowRun, WorkflowRunNode, WorkflowRunStatus,
    WorkflowStore,
};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;
use uuid::Uuid;

/// Node kind a workflow runner may execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// LLM-backed agent step (codex/claude/puffer).
    Agent,
    /// Direct tool invocation; the tool name lives in `tools[0]`,
    /// and the (interpolated) prompt is the tool input.
    Tool,
    /// Control-flow: collapses N upstream outputs into one JSON array
    /// in dependency order. Handled inline by the runner; no executor call.
    Merge,
    /// Control-flow: passes its upstream output through as-is for now.
    /// Full N-way parallel branching is not yet implemented.
    Fanout,
}

/// Context passed to an agent executor for one node.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Workflow slug.
    pub workflow_slug: String,
    /// Run id.
    pub run_id: String,
    /// Node id.
    pub node_id: String,
    /// What kind of step the executor must run.
    pub kind: NodeKind,
    /// Agent hint.
    pub agent: Option<String>,
    /// Model hint.
    pub model: Option<String>,
    /// Tool allowlist (for agent nodes) or `[tool_name]` for tool nodes.
    pub tools: Vec<String>,
    /// Environment values.
    pub env: BTreeMap<String, String>,
    /// Optional workflow working directory.
    pub working_dir: Option<String>,
    /// Interpolated prompt — for tool nodes, this is the tool input.
    pub prompt: String,
}

/// Result of executing one agent node.
#[derive(Debug, Clone)]
pub struct AgentExecution {
    /// Full output text.
    pub output: String,
}

/// Trait implemented by the caller to run a local Puffer agent.
pub trait AgentExecutor {
    /// Executes one workflow node and returns its output.
    fn execute(&mut self, context: ExecutionContext) -> Result<AgentExecution>;
}

/// Native DAG runner for local Puffer agent nodes.
pub struct DagRunner<'a, E> {
    store: &'a WorkflowStore,
    executor: E,
}

impl<'a, E: AgentExecutor> DagRunner<'a, E> {
    /// Creates a DAG runner that appends completed run records to `store`.
    pub fn new(store: &'a WorkflowStore, executor: E) -> Self {
        Self { store, executor }
    }

    /// Executes a workflow and appends the run record.
    pub fn run(
        mut self,
        definition: &WorkflowDefinition,
        trigger: Value,
        trigger_key: Option<String>,
    ) -> Result<WorkflowRun> {
        let started_at_ms = now_ms();
        let run_id = Uuid::new_v4().to_string();
        let mut statuses: BTreeMap<String, WorkflowRunNode> = definition
            .pipeline
            .nodes
            .iter()
            .map(|node| {
                (
                    node.id.clone(),
                    WorkflowRunNode {
                        id: node.id.clone(),
                        status: WorkflowRunStatus::Pending,
                        started_at_ms: None,
                        ended_at_ms: None,
                        output: None,
                        error: None,
                    },
                )
            })
            .collect();
        let mut outputs = BTreeMap::new();
        let mut completed = BTreeSet::new();
        let mut failed = false;

        loop {
            let ready = ready_nodes(&definition.pipeline.nodes, &completed, &statuses);
            if ready.is_empty() {
                break;
            }
            for node in ready {
                if has_failed_dependency(node, &statuses) {
                    mark_skipped(&mut statuses, &node.id, "dependency failed");
                    continue;
                }
                let prompt = interpolate_prompt(&node.prompt, &trigger, &outputs);
                let kind = node_kind(node);
                let record = statuses
                    .get_mut(&node.id)
                    .ok_or_else(|| anyhow!("missing run node `{}`", node.id))?;
                record.status = WorkflowRunStatus::Running;
                record.started_at_ms = Some(now_ms());
                let outcome = match kind {
                    NodeKind::Agent => {
                        let context = ExecutionContext {
                            workflow_slug: definition.slug.clone(),
                            run_id: run_id.clone(),
                            node_id: node.id.clone(),
                            kind,
                            agent: node.agent.clone(),
                            model: node.model.clone(),
                            tools: node.tools.clone(),
                            env: node.env.clone(),
                            working_dir: definition.pipeline.working_dir.clone(),
                            prompt,
                        };
                        self.executor.execute(context).map(|r| r.output)
                    }
                    _ => run_deterministic(kind, node, &prompt, &outputs),
                };
                match outcome {
                    Ok(output) => {
                        let preview = excerpt(&output);
                        outputs.insert(node.id.clone(), output);
                        record.status = WorkflowRunStatus::Completed;
                        record.output = Some(preview);
                        record.ended_at_ms = Some(now_ms());
                        completed.insert(node.id.clone());
                    }
                    Err(error) => {
                        record.status = WorkflowRunStatus::Failed;
                        record.error = Some(error.to_string());
                        record.ended_at_ms = Some(now_ms());
                        completed.insert(node.id.clone());
                        failed = true;
                    }
                }
            }
        }

        if failed {
            skip_downstream(&definition.pipeline.nodes, &mut statuses);
        }
        let nodes: Vec<_> = definition
            .pipeline
            .nodes
            .iter()
            .filter_map(|node| statuses.remove(&node.id))
            .collect();
        let status = if nodes
            .iter()
            .any(|node| node.status == WorkflowRunStatus::Failed)
        {
            WorkflowRunStatus::Failed
        } else if nodes
            .iter()
            .any(|node| node.status == WorkflowRunStatus::Skipped)
        {
            WorkflowRunStatus::Skipped
        } else {
            WorkflowRunStatus::Completed
        };
        let error = nodes
            .iter()
            .find(|node| node.status == WorkflowRunStatus::Failed)
            .and_then(|node| node.error.clone());
        let run = WorkflowRun {
            idx: 0,
            workflow_slug: definition.slug.clone(),
            run_id,
            trigger,
            status,
            started_at_ms,
            ended_at_ms: Some(now_ms()),
            nodes,
            error,
            trigger_key,
        };
        self.store.append_run(run)
    }
}

fn node_kind(node: &PipelineNode) -> NodeKind {
    // Order matters: an explicit `kind` field always wins. Otherwise the
    // `type` field only counts as a kind when it's one of the
    // deterministic kinds — for agent nodes `type` carries the provider
    // id (codex/claude/puffer), which must NOT collapse to "Agent" by
    // accident; that's exactly what we want here.
    if let Some(kind) = node
        .extra
        .get("kind")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return match kind {
            "tool" => NodeKind::Tool,
            "merge" => NodeKind::Merge,
            "fanout" => NodeKind::Fanout,
            _ => NodeKind::Agent,
        };
    }
    match node.node_type.as_deref() {
        Some("tool") => NodeKind::Tool,
        Some("merge") => NodeKind::Merge,
        Some("fanout") => NodeKind::Fanout,
        _ => NodeKind::Agent,
    }
}

/// Executes a non-Agent (deterministic) node inline. Returns the node's
/// output string. Trigger interpolation has already produced `prompt`.
fn run_deterministic(
    kind: NodeKind,
    node: &PipelineNode,
    prompt: &str,
    outputs: &BTreeMap<String, String>,
) -> Result<String> {
    match kind {
        NodeKind::Agent => Err(anyhow!(
            "run_deterministic called for agent node `{}`",
            node.id
        )),
        NodeKind::Tool => run_tool_node(node, prompt),
        NodeKind::Merge => Ok(merge_outputs(&node.depends_on, outputs)),
        NodeKind::Fanout => fanout_passthrough(&node.depends_on, outputs),
    }
}

fn run_tool_node(node: &PipelineNode, prompt: &str) -> Result<String> {
    let tool_name = node
        .tools
        .first()
        .ok_or_else(|| anyhow!("tool node `{}` is missing `tools[0]`", node.id))?;
    match tool_name.as_str() {
        "Bash" => run_bash(prompt, &node.env),
        other => Err(anyhow!(
            "tool `{other}` is not yet supported in deterministic workflow execution"
        )),
    }
}

fn run_bash(command: &str, env: &BTreeMap<String, String>) -> Result<String> {
    use std::process::Command;
    let mut cmd = Command::new("bash");
    cmd.arg("-lc").arg(command);
    for (key, value) in env {
        cmd.env(key, value);
    }
    let output = cmd
        .output()
        .map_err(|err| anyhow!("failed to launch bash: {err}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        return Err(anyhow!(
            "bash exited with status {}: {}",
            output.status,
            text.trim()
        ));
    }
    Ok(text)
}

fn merge_outputs(deps: &[String], outputs: &BTreeMap<String, String>) -> String {
    let values: Vec<Value> = deps
        .iter()
        .map(|id| match outputs.get(id) {
            Some(text) => {
                serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
            }
            None => Value::Null,
        })
        .collect();
    serde_json::to_string(&Value::Array(values)).unwrap_or_default()
}

fn fanout_passthrough(deps: &[String], outputs: &BTreeMap<String, String>) -> Result<String> {
    let dep = deps
        .first()
        .ok_or_else(|| anyhow!("fanout node must have one dependency"))?;
    Ok(outputs.get(dep).cloned().unwrap_or_default())
}

fn ready_nodes<'a>(
    nodes: &'a [PipelineNode],
    completed: &BTreeSet<String>,
    statuses: &BTreeMap<String, WorkflowRunNode>,
) -> Vec<&'a PipelineNode> {
    nodes
        .iter()
        .filter(|node| {
            matches!(
                statuses.get(&node.id).map(|record| record.status),
                Some(WorkflowRunStatus::Pending)
            ) && node.depends_on.iter().all(|dep| completed.contains(dep))
        })
        .collect()
}

fn has_failed_dependency(
    node: &PipelineNode,
    statuses: &BTreeMap<String, WorkflowRunNode>,
) -> bool {
    node.depends_on.iter().any(|dep| {
        statuses
            .get(dep)
            .map(|record| {
                matches!(
                    record.status,
                    WorkflowRunStatus::Failed | WorkflowRunStatus::Skipped
                )
            })
            .unwrap_or(false)
    })
}

fn mark_skipped(statuses: &mut BTreeMap<String, WorkflowRunNode>, node_id: &str, reason: &str) {
    if let Some(record) = statuses.get_mut(node_id) {
        record.status = WorkflowRunStatus::Skipped;
        record.error = Some(reason.to_string());
        record.ended_at_ms = Some(now_ms());
    }
}

fn skip_downstream(nodes: &[PipelineNode], statuses: &mut BTreeMap<String, WorkflowRunNode>) {
    loop {
        let to_skip: Vec<String> = nodes
            .iter()
            .filter(|node| {
                matches!(
                    statuses.get(&node.id).map(|record| record.status),
                    Some(WorkflowRunStatus::Pending)
                ) && has_failed_dependency(node, statuses)
            })
            .map(|node| node.id.clone())
            .collect();
        if to_skip.is_empty() {
            break;
        }
        for id in to_skip {
            mark_skipped(statuses, &id, "dependency failed");
        }
    }
}

fn interpolate_prompt(prompt: &str, trigger: &Value, outputs: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str("{{");
            rest = after;
            continue;
        };
        let token = after[..end].trim();
        out.push_str(&resolve_token(token, trigger, outputs));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn resolve_token(token: &str, trigger: &Value, outputs: &BTreeMap<String, String>) -> String {
    if let Some(path) = token.strip_prefix("trigger.") {
        return json_path(trigger, path).unwrap_or_default();
    }
    if let Some(path) = token.strip_prefix("nodes.") {
        let mut parts = path.splitn(2, '.');
        let Some(node_id) = parts.next() else {
            return String::new();
        };
        if parts.next() == Some("output") {
            return outputs.get(node_id).cloned().unwrap_or_default();
        }
    }
    String::new()
}

fn json_path(value: &Value, path: &str) -> Option<String> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(match current {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn excerpt(output: &str) -> String {
    const LIMIT: usize = 4000;
    if output.len() <= LIMIT {
        output.to_string()
    } else {
        format!("{}...", &output[..LIMIT])
    }
}

fn now_ms() -> i128 {
    OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RegisterOptions, TriggerSpec, WorkflowStore};
    use anyhow::bail;
    use serde_json::json;

    #[derive(Default)]
    struct FakeExecutor {
        calls: Vec<String>,
        fail: Option<String>,
    }

    impl AgentExecutor for FakeExecutor {
        fn execute(&mut self, context: ExecutionContext) -> Result<AgentExecution> {
            self.calls.push(context.node_id.clone());
            if self.fail.as_deref() == Some(context.node_id.as_str()) {
                bail!("boom");
            }
            Ok(AgentExecution {
                output: format!("{}:{}", context.node_id, context.prompt),
            })
        }
    }

    #[test]
    fn executes_dependencies_and_interpolation() {
        let temp = tempfile::tempdir().unwrap();
        let store = WorkflowStore::new(temp.path());
        let definition = crate::WorkflowDefinition::from_json(
            json!({
                "slug":"x",
                "trigger":{"type":"cron","cron":"* * * * *"},
                "pipeline":{"name":"x","nodes":[
                    {"id":"a","prompt":"{{ trigger.text }}"},
                    {"id":"b","depends_on":["a"],"prompt":"got {{ nodes.a.output }}"}
                ]}
            }),
            RegisterOptions::default(),
        )
        .unwrap();
        let run = DagRunner::new(&store, FakeExecutor::default())
            .run(&definition, json!({"text":"hi"}), None)
            .unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Completed);
        assert!(run.nodes[1].output.as_deref().unwrap().contains("a:hi"));
    }

    #[test]
    fn skips_downstream_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let store = WorkflowStore::new(temp.path());
        let definition = crate::WorkflowDefinition::from_json(
            json!({
                "name":"x",
                "nodes":[
                    {"id":"a","prompt":"go"},
                    {"id":"b","depends_on":["a"],"prompt":"after"}
                ]
            }),
            RegisterOptions {
                slug: Some("x".into()),
                trigger: Some(TriggerSpec::Cron {
                    cron: "* * * * *".into(),
                }),
            },
        )
        .unwrap();
        let run = DagRunner::new(
            &store,
            FakeExecutor {
                fail: Some("a".into()),
                ..FakeExecutor::default()
            },
        )
        .run(&definition, json!({}), None)
        .unwrap();
        assert_eq!(run.nodes[0].status, WorkflowRunStatus::Failed);
        assert_eq!(run.nodes[1].status, WorkflowRunStatus::Skipped);
    }

    fn make_node(id: &str) -> PipelineNode {
        PipelineNode {
            id: id.into(),
            node_type: None,
            agent: None,
            prompt: String::new(),
            model: None,
            tools: Vec::new(),
            env: BTreeMap::new(),
            depends_on: Vec::new(),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn node_kind_classifies_by_type_then_kind() {
        let mut agent = make_node("a");
        assert_eq!(node_kind(&agent), NodeKind::Agent);

        agent.node_type = Some("tool".into());
        assert_eq!(node_kind(&agent), NodeKind::Tool);
        agent.node_type = Some("merge".into());
        assert_eq!(node_kind(&agent), NodeKind::Merge);
        agent.node_type = Some("fanout".into());
        assert_eq!(node_kind(&agent), NodeKind::Fanout);

        // The `kind` extra wins over `type` when type isn't a known kind.
        agent.node_type = Some("codex".into());
        agent.extra.insert("kind".into(), json!("tool"));
        assert_eq!(node_kind(&agent), NodeKind::Tool);
    }

    #[test]
    fn merge_outputs_emits_json_array_in_dep_order() {
        let mut outputs = BTreeMap::new();
        outputs.insert("a".into(), "\"alpha\"".into());
        outputs.insert("b".into(), "{\"x\":1}".into());
        let deps = vec!["a".into(), "b".into()];
        let merged = merge_outputs(&deps, &outputs);
        let parsed: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed, json!(["alpha", { "x": 1 }]));
    }

    #[test]
    fn merge_outputs_falls_back_to_raw_string_when_not_json() {
        let mut outputs = BTreeMap::new();
        outputs.insert("a".into(), "not json".into());
        let merged = merge_outputs(&["a".into()], &outputs);
        let parsed: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed, json!(["not json"]));
    }

    #[test]
    fn fanout_passthrough_forwards_single_upstream() {
        let mut outputs = BTreeMap::new();
        outputs.insert("a".into(), "[1, 2, 3]".into());
        let out = fanout_passthrough(&["a".into()], &outputs).unwrap();
        assert_eq!(out, "[1, 2, 3]");
    }

    #[test]
    fn fanout_passthrough_rejects_zero_deps() {
        let outputs = BTreeMap::new();
        let err = fanout_passthrough(&[], &outputs).unwrap_err();
        assert!(err
            .to_string()
            .contains("fanout node must have one dependency"));
    }

    #[cfg(unix)]
    #[test]
    fn run_bash_executes_and_captures_stdout() {
        let env = BTreeMap::new();
        let out = run_bash("echo hello", &env).unwrap();
        assert_eq!(out.trim(), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn run_bash_surfaces_nonzero_exit_with_stderr() {
        let env = BTreeMap::new();
        let err = run_bash("echo nope >&2; exit 7", &env).unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("status"),
            "expected exit status in error: {text}"
        );
        assert!(text.contains("nope"), "expected stderr in error: {text}");
    }

    #[cfg(unix)]
    #[test]
    fn run_tool_node_rejects_unknown_tool_name() {
        let mut node = make_node("t");
        node.node_type = Some("tool".into());
        node.tools = vec!["Read".into()];
        let err = run_deterministic(NodeKind::Tool, &node, "{}", &BTreeMap::new()).unwrap_err();
        assert!(err.to_string().contains("tool `Read` is not yet supported"));
    }

    #[cfg(unix)]
    #[test]
    fn dag_runs_a_tool_node_inline_without_executor() {
        let temp = tempfile::tempdir().unwrap();
        let store = WorkflowStore::new(temp.path());
        let definition = crate::WorkflowDefinition::from_json(
            json!({
                "slug":"shellflow",
                "trigger":{"type":"cron","cron":"* * * * *"},
                "pipeline":{"name":"shellflow","nodes":[
                    {"id":"a","type":"tool","tools":["Bash"],"prompt":"printf '{{ trigger.msg }}'"}
                ]}
            }),
            RegisterOptions::default(),
        )
        .unwrap();
        // The FakeExecutor would write "a:..." if called — verify it's NOT called.
        let executor = FakeExecutor::default();
        let run = DagRunner::new(&store, executor)
            .run(&definition, json!({ "msg": "kept-deterministic" }), None)
            .unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Completed);
        let output = run.nodes[0].output.as_deref().unwrap();
        assert_eq!(output, "kept-deterministic");
    }

    #[test]
    fn dag_runs_a_merge_node_inline_collecting_dep_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let store = WorkflowStore::new(temp.path());
        let definition = crate::WorkflowDefinition::from_json(
            json!({
                "slug":"m",
                "trigger":{"type":"cron","cron":"* * * * *"},
                "pipeline":{"name":"m","nodes":[
                    {"id":"a","prompt":"alpha"},
                    {"id":"b","prompt":"beta"},
                    {"id":"m","type":"merge","prompt":"","depends_on":["a","b"]}
                ]}
            }),
            RegisterOptions::default(),
        )
        .unwrap();
        let run = DagRunner::new(&store, FakeExecutor::default())
            .run(&definition, json!({}), None)
            .unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Completed);
        let merged = run.nodes[2].output.as_deref().unwrap();
        let parsed: Value = serde_json::from_str(merged).unwrap();
        // The FakeExecutor emits "<id>:<prompt>"; check both upstreams landed.
        assert_eq!(parsed, json!(["a:alpha", "b:beta"]));
    }
}
