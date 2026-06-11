//! Workflow trigger binding spec.
//!
//! The old implementation stored "subscriptions" keyed by subscriber topic.
//! The workflow redesign treats the same persisted rule as a binding from an
//! authorized connection plus a Puffer-side filter to one action graph entry.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::path::{Component, Path};

/// Backwards-compatible alias for older callers.
pub type SubscriptionSpec = WorkflowBindingSpec;

/// Backwards-compatible alias for older callers.
pub type SubscriptionStatus = WorkflowBindingStatus;

/// Backwards-compatible alias for older callers.
pub type PrefilterSpec = FilterSpec;

/// A workflow trigger binding.
///
/// The binding listens to one connection stream, applies a Puffer-owned
/// filter, then runs exactly one action. Multiple bindings can target the
/// same connection, making the connection stream act as a mux.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowBindingSpec {
    /// Stable workflow binding slug. Legacy JSON may call this `id`.
    #[serde(alias = "id")]
    pub slug: String,
    /// Free-form description surfaced in workflow/connection lists.
    #[serde(default)]
    pub description: String,
    /// Authorized connection slug. Legacy JSON may call this `source_topic`.
    #[serde(alias = "source_topic")]
    pub connection_slug: String,
    /// Connector template slug for compatibility with existing topic-based
    /// subscribers. When absent, the connection slug is matched directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_slug: Option<String>,
    /// Status. Paused bindings receive no events.
    #[serde(default = "default_status")]
    pub status: WorkflowBindingStatus,
    /// Optional Puffer-side filter. Legacy JSON may call this `prefilter`.
    #[serde(default, alias = "prefilter", skip_serializing_if = "Option::is_none")]
    pub filter: Option<FilterSpec>,
    /// Optional Puffer-side filters that suppress matching events before the
    /// action starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore_filters: Vec<FilterSpec>,
    /// Optional normalized contact ids. Empty means the binding receives all
    /// events on the connection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contact_ids: Vec<String>,
    /// Optional LLM classify step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classify_prompt: Option<String>,
    /// Optional model hint for the classifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classify_model: Option<String>,
    /// Action invoked when the event passes filtering.
    pub action: ActionSpec,
    /// Created-at, milliseconds since UNIX epoch.
    #[serde(default)]
    pub created_at_ms: i128,
}

fn default_status() -> WorkflowBindingStatus {
    WorkflowBindingStatus::Enabled
}

/// Status of a workflow binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBindingStatus {
    /// Binding is active.
    Enabled,
    /// Binding is paused.
    Paused,
}

/// Puffer-side filter syntax.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FilterSpec {
    /// Tagged filter form, for explicit regex or Puffer path expressions.
    Tagged(TaggedFilterSpec),
    /// Structured JSON shape matched as a subset of the event payload.
    Json(Value),
}

/// Explicit filter variants.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaggedFilterSpec {
    /// Matches event text using Rust regex syntax.
    Regex {
        /// Regex pattern.
        pattern: String,
        /// Case-insensitive flag. Defaults to true.
        #[serde(default = "default_true")]
        case_insensitive: bool,
    },
    /// Small Puffer path expression evaluated by Puffer, not the connector.
    ///
    /// Supported forms are `.path == value`, `.path =~ "regex"`,
    /// `.path | test("regex")`, and `.path | exists`.
    Jq {
        /// Puffer path expression.
        expression: String,
    },
    /// Matches when any nested filter matches the event.
    Any {
        /// Nested filters joined with logical OR.
        filters: Vec<FilterSpec>,
    },
}

fn default_true() -> bool {
    true
}

/// Action variants. Each carries its own params struct; the dispatcher
/// matches and executes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionSpec {
    /// Insert one row per matched event into a SQLite table.
    SqliteInsert {
        /// Path to the database file. Created if it does not exist.
        path: String,
        /// Target table. Created with a default schema if missing.
        table: String,
    },
    /// Append one record per matched event to a file.
    FileAppend {
        /// Output file path. Relative paths are rooted in subscription storage;
        /// absolute paths are limited to `/tmp`.
        path: String,
        /// Output format. Defaults to plain message text.
        #[serde(default = "default_file_append_format")]
        format: FileAppendFormat,
    },
    /// Legacy alias for `connector_act` with action `send_message`.
    ForwardMessage {
        /// Destination connector or legacy platform id.
        platform: String,
        /// Destination chat handle or numeric id.
        target: String,
        /// Optional template; `{{text}}` and `{{payload.field}}` are substituted.
        #[serde(default)]
        template: Option<String>,
    },
    /// Trigger a registered native Puffer workflow.
    RunWorkflow {
        /// Workflow slug to run.
        slug: String,
    },
    /// Invoke a connector action.
    ConnectorAct {
        /// Connector template slug.
        connector_slug: String,
        /// Action slug, for example `send_message`.
        action: String,
        /// Action input JSON. Strings may contain `{{text}}` and
        /// `{{payload.field}}` templates.
        #[serde(default)]
        input: Value,
    },
    /// Invoke a model-visible Puffer tool through the workflow runtime.
    ToolCall {
        /// Tool id, for example `Bash`.
        tool: String,
        /// Tool input JSON.
        #[serde(default)]
        input: Value,
    },
    /// Triage the event to an agent turn.
    TriageAgent {
        /// Prompt template for the agent.
        prompt: String,
        /// Optional model hint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Execute a linear action graph.
    Graph {
        /// Graph nodes.
        nodes: Vec<ActionGraphNode>,
    },
    /// Catch-all for future variants.
    #[serde(other)]
    Unknown,
}

/// File format used by [`ActionSpec::FileAppend`].
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileAppendFormat {
    /// Append only the message text plus a trailing newline.
    Text,
    /// Append one JSON object per event, including text and payload.
    Jsonl,
}

fn default_file_append_format() -> FileAppendFormat {
    FileAppendFormat::Text
}

/// One node in a workflow action graph.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActionGraphNode {
    /// Node id.
    pub id: String,
    /// Dependencies that must complete first.
    #[serde(default, alias = "dependsOn", skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Action to execute.
    #[serde(flatten)]
    pub action: ActionSpec,
}

/// Validates a workflow trigger binding.
pub fn validate_workflow_binding(spec: &WorkflowBindingSpec) -> Result<(), String> {
    validate_slug("slug", &spec.slug)?;
    validate_slug("connection_slug", &spec.connection_slug)?;
    if let Some(connector_slug) = &spec.connector_slug {
        validate_slug("connector_slug", connector_slug)?;
    }
    if let Some(filter) = &spec.filter {
        validate_filter(filter)?;
    }
    for filter in &spec.ignore_filters {
        validate_filter(filter)?;
    }
    validate_contact_ids(&spec.contact_ids)?;
    validate_action(&spec.action)
}

/// Backwards-compatible validation entry point.
pub fn validate_spec(spec: &WorkflowBindingSpec) -> Result<(), String> {
    validate_workflow_binding(spec)
}

/// Validates one workflow action or action graph.
pub fn validate_action_spec(action: &ActionSpec) -> Result<(), String> {
    validate_action(action)
}

fn validate_slug(label: &str, slug: &str) -> Result<(), String> {
    if slug.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if !slug
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(format!("{label} must be kebab-case ASCII"));
    }
    Ok(())
}

fn validate_filter(filter: &FilterSpec) -> Result<(), String> {
    match filter {
        FilterSpec::Tagged(TaggedFilterSpec::Regex { pattern, .. }) => {
            regex::Regex::new(pattern).map_err(|error| format!("filter regex invalid: {error}"))?;
        }
        FilterSpec::Tagged(TaggedFilterSpec::Jq { expression }) => {
            if expression.trim().is_empty() {
                return Err("jq filter expression must not be empty".into());
            }
            compile_jq_like(expression)?;
        }
        FilterSpec::Tagged(TaggedFilterSpec::Any { filters }) => {
            if filters.is_empty() {
                return Err("compound filter must contain at least one filter".into());
            }
            for filter in filters {
                validate_filter(filter)?;
            }
        }
        FilterSpec::Json(shape) => {
            if !shape.is_object() {
                return Err("json filter must be an object shape".into());
            }
        }
    }
    Ok(())
}

fn validate_contact_ids(contact_ids: &[String]) -> Result<(), String> {
    for contact_id in contact_ids {
        if crate::contacts::normalize_contact_id(contact_id).is_none() {
            return Err(format!("contact id `{contact_id}` is invalid"));
        }
    }
    Ok(())
}

fn validate_action(action: &ActionSpec) -> Result<(), String> {
    match action {
        ActionSpec::SqliteInsert { path, table } => {
            if path.trim().is_empty() {
                return Err("sqlite_insert.path must not be empty".into());
            }
            if !is_safe_relative_path(path) {
                return Err("sqlite_insert.path must be a safe relative path".into());
            }
            if !is_valid_sqlite_identifier(table) {
                return Err(format!(
                    "sqlite_insert.table `{table}` is not a valid SQL identifier"
                ));
            }
        }
        ActionSpec::FileAppend { path, .. } => {
            if path.trim().is_empty() {
                return Err("file_append.path must not be empty".into());
            }
            if !is_safe_file_append_path(path) {
                return Err(
                    "file_append.path must be a safe relative path or an absolute /tmp path".into(),
                );
            }
        }
        ActionSpec::ForwardMessage {
            platform, target, ..
        } => {
            if platform.trim().is_empty() {
                return Err("forward_message.platform must not be empty".into());
            }
            if target.trim().is_empty() {
                return Err("forward_message.target must not be empty".into());
            }
        }
        ActionSpec::RunWorkflow { slug } => validate_slug("run_workflow.slug", slug)?,
        ActionSpec::ConnectorAct {
            connector_slug,
            action,
            ..
        } => {
            validate_slug("connector_act.connector_slug", connector_slug)?;
            if action.trim().is_empty() {
                return Err("connector_act.action must not be empty".into());
            }
        }
        ActionSpec::ToolCall { tool, .. } => {
            if tool.trim().is_empty() {
                return Err("tool_call.tool must not be empty".into());
            }
        }
        ActionSpec::TriageAgent { prompt, .. } => {
            if prompt.trim().is_empty() {
                return Err("triage_agent.prompt must not be empty".into());
            }
        }
        ActionSpec::Graph { nodes } => validate_graph(nodes)?,
        ActionSpec::Unknown => {
            return Err("action.type is not recognized by this Puffer build".into());
        }
    }
    Ok(())
}

fn validate_graph(nodes: &[ActionGraphNode]) -> Result<(), String> {
    if nodes.is_empty() {
        return Err("graph.nodes must not be empty".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for node in nodes {
        if node.id.trim().is_empty() {
            return Err("graph node id must not be empty".into());
        }
        if !ids.insert(node.id.clone()) {
            return Err(format!("duplicate graph node id `{}`", node.id));
        }
        validate_action(&node.action)?;
    }
    for node in nodes {
        for dep in &node.depends_on {
            if !ids.contains(dep) {
                return Err(format!(
                    "graph node `{}` depends on unknown `{dep}`",
                    node.id
                ));
            }
        }
    }
    detect_graph_cycle(nodes)?;
    Ok(())
}

fn detect_graph_cycle(nodes: &[ActionGraphNode]) -> Result<(), String> {
    let deps = nodes
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                node.depends_on
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut visiting = std::collections::BTreeSet::new();
    let mut done = std::collections::BTreeSet::new();
    for node in nodes {
        visit_graph_node(node.id.as_str(), &deps, &mut visiting, &mut done)?;
    }
    Ok(())
}

fn visit_graph_node<'a>(
    id: &'a str,
    deps: &std::collections::BTreeMap<&'a str, Vec<&'a str>>,
    visiting: &mut std::collections::BTreeSet<&'a str>,
    done: &mut std::collections::BTreeSet<&'a str>,
) -> Result<(), String> {
    if done.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id) {
        return Err(format!("graph contains a dependency cycle at `{id}`"));
    }
    for dep in deps.get(id).into_iter().flatten() {
        visit_graph_node(dep, deps, visiting, done)?;
    }
    visiting.remove(id);
    done.insert(id);
    Ok(())
}

/// Evaluates a filter against event text and payload.
pub fn filter_matches(filter: Option<&FilterSpec>, text: &str, payload: &Value) -> bool {
    match filter {
        None => true,
        Some(FilterSpec::Json(shape)) => value_contains(payload, shape),
        Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
            pattern,
            case_insensitive,
        })) => {
            let mut builder = regex::RegexBuilder::new(pattern);
            builder.case_insensitive(*case_insensitive);
            builder.build().map(|re| re.is_match(text)).unwrap_or(false)
        }
        Some(FilterSpec::Tagged(TaggedFilterSpec::Jq { expression })) => {
            eval_jq_like(expression, text, payload).unwrap_or(false)
        }
        Some(FilterSpec::Tagged(TaggedFilterSpec::Any { filters })) => filters
            .iter()
            .any(|filter| filter_matches(Some(filter), text, payload)),
    }
}

fn value_contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .map(|actual_value| value_contains(actual_value, value))
                .unwrap_or(false)
        }),
        (Value::Array(actual), Value::Array(expected)) => expected.iter().all(|expected_value| {
            actual
                .iter()
                .any(|item| value_contains(item, expected_value))
        }),
        _ => actual == expected,
    }
}

#[derive(Debug, Clone)]
enum JqLikePredicate {
    Equals { path: String, value: Value },
    Regex { path: String, pattern: String },
    Exists { path: String },
}

fn compile_jq_like(expression: &str) -> Result<JqLikePredicate, String> {
    let expression = expression.trim();
    if let Some((path, value)) = expression.split_once("==") {
        return Ok(JqLikePredicate::Equals {
            path: parse_jq_path(path)?,
            value: parse_json_literal(value.trim())?,
        });
    }
    if let Some((path, value)) = expression.split_once("=~") {
        let pattern = parse_quoted(value.trim())?;
        regex::Regex::new(&pattern).map_err(|error| format!("jq regex invalid: {error}"))?;
        return Ok(JqLikePredicate::Regex {
            path: parse_jq_path(path)?,
            pattern,
        });
    }
    if let Some((path, call)) = expression.split_once("|") {
        let call = call.trim();
        if call == "exists" {
            return Ok(JqLikePredicate::Exists {
                path: parse_jq_path(path)?,
            });
        }
        let Some(inner) = call
            .strip_prefix("test(")
            .and_then(|rest| rest.strip_suffix(')'))
        else {
            return Err("jq filter supports only test(\"regex\") calls".into());
        };
        let pattern = parse_quoted(inner.trim())?;
        regex::Regex::new(&pattern).map_err(|error| format!("jq regex invalid: {error}"))?;
        return Ok(JqLikePredicate::Regex {
            path: parse_jq_path(path)?,
            pattern,
        });
    }
    Err("jq filter supports `.path == \"value\"`, `.path =~ \"regex\"`, or `.path | test(\"regex\")`".into())
}

fn eval_jq_like(expression: &str, text: &str, payload: &Value) -> Result<bool, String> {
    match compile_jq_like(expression)? {
        JqLikePredicate::Equals { path, value } => Ok(json_path_value(payload, text, &path)
            .as_deref()
            .is_some_and(|actual| actual == &value)),
        JqLikePredicate::Regex { path, pattern } => {
            let Some(value) = json_path_string(payload, text, &path) else {
                return Ok(false);
            };
            Ok(regex::Regex::new(&pattern)
                .map_err(|error| format!("jq regex invalid: {error}"))?
                .is_match(&value))
        }
        JqLikePredicate::Exists { path } => Ok(json_path_value(payload, text, &path)
            .as_deref()
            .is_some_and(json_value_exists)),
    }
}

fn parse_jq_path(path: &str) -> Result<String, String> {
    let path = path.trim();
    path.strip_prefix('.')
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "jq path must start with `.`".to_string())
}

fn parse_quoted(value: &str) -> Result<String, String> {
    serde_json::from_str::<String>(value)
        .map_err(|error| format!("jq literal must be a JSON string: {error}"))
}

fn parse_json_literal(value: &str) -> Result<Value, String> {
    serde_json::from_str::<Value>(value)
        .map_err(|error| format!("jq literal must be valid JSON: {error}"))
}

fn json_path_value<'a>(payload: &'a Value, text: &str, path: &str) -> Option<Cow<'a, Value>> {
    if path == "text" || path == "message" {
        return Some(Cow::Owned(Value::String(text.to_string())));
    }
    let mut current = payload;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(Cow::Borrowed(current))
}

fn json_path_string(payload: &Value, text: &str, path: &str) -> Option<String> {
    let value = json_path_value(payload, text, path)?;
    match value.as_ref() {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        other => Some(other.to_string()),
    }
}

fn json_value_exists(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

/// Allow only `[A-Za-z_][A-Za-z0-9_]*`. Conservative on purpose.
pub(crate) fn is_valid_sqlite_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_safe_relative_path(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.starts_with("~/") {
        return false;
    }
    let path = Path::new(trimmed);
    !path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn is_safe_file_append_path(path: &str) -> bool {
    let trimmed = path.trim();
    if is_safe_relative_path(trimmed) {
        return true;
    }
    let path = Path::new(trimmed);
    path.is_absolute()
        && path.starts_with("/tmp")
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
}

/// Render `template` against the matched event payload.
pub fn render_template(template: &str, text: &str, payload: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str("{{");
            rest = after;
            continue;
        };
        let token = after[..end].trim();
        let replacement = if token == "text" {
            text.to_string()
        } else if let Some(field) = token.strip_prefix("payload.") {
            payload_path_value(payload, field)
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        out.push_str(&replacement);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn payload_path_value<'a>(payload: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = payload;
    for part in path.split('.') {
        if part.is_empty() {
            return None;
        }
        current = match current {
            Value::Object(map) => map.get(part)?,
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Renders every string in `value` as a template against an event.
pub fn render_value_templates(value: &Value, text: &str, payload: &Value) -> Value {
    match value {
        Value::String(template) => Value::String(render_template(template, text, payload)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| render_value_templates(item, text, payload))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), render_value_templates(value, text, payload)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_binding_passes() {
        let spec = WorkflowBindingSpec {
            slug: "ioc".into(),
            description: String::new(),
            connection_slug: "telegram-user".into(),
            connector_slug: Some("telegram-login".into()),
            status: WorkflowBindingStatus::Enabled,
            filter: Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
                pattern: r"\bIoC\b".into(),
                case_insensitive: true,
            })),
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::SqliteInsert {
                path: "x.db".into(),
                table: "ioc_messages".into(),
            },
            created_at_ms: 0,
        };
        validate_workflow_binding(&spec).unwrap();
    }

    #[test]
    fn legacy_subscription_json_deserializes() {
        let spec: WorkflowBindingSpec = serde_json::from_value(json!({
            "id": "ioc-watch",
            "description": "demo",
            "source_topic": "telegram-user",
            "prefilter": {"type":"regex","pattern":"ioc"},
            "action": {"type":"run_workflow","slug":"ioc"}
        }))
        .unwrap();

        assert_eq!(spec.slug, "ioc-watch");
        assert_eq!(spec.connection_slug, "telegram-user");
        assert!(matches!(
            spec.filter,
            Some(FilterSpec::Tagged(TaggedFilterSpec::Regex { .. }))
        ));
    }

    #[test]
    fn rejects_bad_table_name() {
        let spec = WorkflowBindingSpec {
            slug: "x".into(),
            description: String::new(),
            connection_slug: "telegram-user".into(),
            connector_slug: None,
            status: WorkflowBindingStatus::Enabled,
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::SqliteInsert {
                path: "x.db".into(),
                table: "bad-name".into(),
            },
            created_at_ms: 0,
        };
        assert!(validate_workflow_binding(&spec).is_err());
    }

    #[test]
    fn file_append_accepts_tmp_path_and_rejects_other_absolute_paths() {
        let mut spec = WorkflowBindingSpec {
            slug: "x".into(),
            description: String::new(),
            connection_slug: "telegram-user".into(),
            connector_slug: None,
            status: WorkflowBindingStatus::Enabled,
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::FileAppend {
                path: "/tmp/msgs".into(),
                format: FileAppendFormat::Jsonl,
            },
            created_at_ms: 0,
        };
        validate_workflow_binding(&spec).unwrap();

        spec.action = ActionSpec::FileAppend {
            path: "/etc/msgs".into(),
            format: FileAppendFormat::Text,
        };
        assert!(validate_workflow_binding(&spec).is_err());
    }

    #[test]
    fn file_append_defaults_to_text_format() {
        let action: ActionSpec = serde_json::from_value(json!({
            "type": "file_append",
            "path": "/tmp/msgs"
        }))
        .unwrap();

        assert!(matches!(
            action,
            ActionSpec::FileAppend {
                format: FileAppendFormat::Text,
                ..
            }
        ));
    }

    #[test]
    fn json_filter_matches_payload_shape() {
        let filter = FilterSpec::Json(json!({"channel":{"name":"ETHSecurity"}}));
        let payload = json!({"channel":{"name":"ETHSecurity","id":1},"message":"gm"});

        assert!(filter_matches(Some(&filter), "gm", &payload));
        assert!(!filter_matches(
            Some(&filter),
            "gm",
            &json!({"channel":{"name":"Other"}})
        ));
    }

    #[test]
    fn compound_any_matches_any_keyword() {
        let filter = FilterSpec::Tagged(TaggedFilterSpec::Any {
            filters: vec![
                FilterSpec::Tagged(TaggedFilterSpec::Regex {
                    pattern: regex::escape("作业"),
                    case_insensitive: true,
                }),
                FilterSpec::Tagged(TaggedFilterSpec::Regex {
                    pattern: regex::escape("广告"),
                    case_insensitive: true,
                }),
            ],
        });

        assert!(filter_matches(Some(&filter), "今天有作业", &json!({})));
        assert!(filter_matches(Some(&filter), "这是广告", &json!({})));
        assert!(!filter_matches(Some(&filter), "正常消息", &json!({})));
    }

    #[test]
    fn jq_filter_supports_regex() {
        let filter = FilterSpec::Tagged(TaggedFilterSpec::Jq {
            expression: ".from | test(\"Tony\")".into(),
        });

        assert!(filter_matches(Some(&filter), "", &json!({"from":"TonyKe"})));
        assert!(!filter_matches(Some(&filter), "", &json!({"from":"Alice"})));
    }

    #[test]
    fn jq_filter_unescapes_json_string_literals() {
        let filter = FilterSpec::Tagged(TaggedFilterSpec::Jq {
            expression: r#".from == "Tony \"TK\"""#.into(),
        });

        assert!(filter_matches(
            Some(&filter),
            "",
            &json!({"from":"Tony \"TK\""})
        ));
    }

    #[test]
    fn render_value_templates_recurses() {
        let rendered = render_value_templates(
            &json!({"message":"got {{text}} from {{payload.from}}"}),
            "gm",
            &json!({"from":"Tony"}),
        );

        assert_eq!(rendered, json!({"message":"got gm from Tony"}));
    }

    #[test]
    fn render_template_supports_nested_payload_paths() {
        let rendered = render_template(
            "reply to {{payload.chat.id}} from {{payload.from.name}}",
            "gm",
            &json!({
                "chat": {"id": "chat-42"},
                "from": {"name": "TonyKe"}
            }),
        );

        assert_eq!(rendered, "reply to chat-42 from TonyKe");
    }
}
