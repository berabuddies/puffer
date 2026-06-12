use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{
    compile_event_field_rule, connection_subscriber_manifest_dir, load_event_schema_from_dir,
    ConnectionRecord, EventFieldRule, EventOperator, EventSchema, FilterSpec, SubscriptionManager,
    TaggedFilterSpec, WorkflowBindingSpec,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct MonitorRuleAddParams {
    #[serde(alias = "connectionSlug")]
    connection_slug: String,
    mode: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    field: Option<String>,
    #[serde(default)]
    operator: Option<EventOperator>,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default = "default_true")]
    case_insensitive: bool,
}

#[derive(Debug, Deserialize)]
struct MonitorRuleDeleteParams {
    #[serde(alias = "connectionSlug")]
    connection_slug: String,
    mode: String,
    rule: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MonitorRuleMode {
    Include,
    Exclude,
}

/// Adds one include or exclude monitor rule and returns a refreshed snapshot.
pub(crate) fn handle_monitor_rule_add(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: MonitorRuleAddParams =
        serde_json::from_value(params.clone()).context("invalid monitor rule add params")?;
    let connection_slug = valid_connection_slug(&params.connection_slug)?;
    let mode = parse_rule_mode(&params.mode)?;
    let manager = subscription_manager()?;
    let mut binding = manager
        .store()
        .get(&monitor_slug(connection_slug))
        .with_context(|| format!("monitor `{connection_slug}` not found"))?;
    let rule = compile_rule(paths, manager.as_ref(), &binding, &params)?;
    match mode {
        MonitorRuleMode::Exclude => push_unique_rule(&mut binding.ignore_filters, rule),
        MonitorRuleMode::Include => {
            binding.filter = Some(append_include_filter(binding.filter.take(), rule));
        }
    }
    manager.store().upsert(binding)?;
    manager.refresh_connection_consumers()?;
    super::handle_workflow_list(paths)
}

/// Deletes one include or exclude monitor rule and returns a refreshed snapshot.
pub(crate) fn handle_monitor_rule_delete(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: MonitorRuleDeleteParams =
        serde_json::from_value(params.clone()).context("invalid monitor rule delete params")?;
    let connection_slug = valid_connection_slug(&params.connection_slug)?;
    let mode = parse_rule_mode(&params.mode)?;
    let target: FilterSpec =
        serde_json::from_value(params.rule).context("invalid monitor rule filter")?;
    let manager = subscription_manager()?;
    let mut binding = manager
        .store()
        .get(&monitor_slug(connection_slug))
        .with_context(|| format!("monitor `{connection_slug}` not found"))?;
    match mode {
        MonitorRuleMode::Exclude => remove_matching_rule(&mut binding.ignore_filters, &target),
        MonitorRuleMode::Include => {
            binding.filter = remove_from_include_filter(binding.filter.take(), &target);
        }
    }
    manager.store().upsert(binding)?;
    manager.refresh_connection_consumers()?;
    super::handle_workflow_list(paths)
}

pub(super) fn include_filters_json(filter: Option<&FilterSpec>) -> Value {
    Value::Array(flatten_include_filters(filter).into_iter().collect())
}

fn compile_rule(
    paths: &ConfigPaths,
    manager: &SubscriptionManager,
    binding: &WorkflowBindingSpec,
    params: &MonitorRuleAddParams,
) -> Result<FilterSpec> {
    match params
        .kind
        .as_deref()
        .unwrap_or("keyword")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "keyword" => keyword_filter(
            &params.keywords,
            params.operator.unwrap_or(EventOperator::Contains),
            params.case_insensitive,
        ),
        "field" => {
            let schema = monitor_rule_schema(paths, manager, binding)?
                .context("monitor rule schema not found for connection")?;
            let rule = EventFieldRule {
                field: params
                    .field
                    .clone()
                    .context("field monitor rule requires field")?,
                operator: params
                    .operator
                    .context("field monitor rule requires operator")?,
                value: params.value.clone(),
            };
            compile_event_field_rule(&schema, &rule)
        }
        other => anyhow::bail!("monitor rule kind `{other}` must be keyword or field"),
    }
}

fn keyword_filter(
    keywords: &[String],
    operator: EventOperator,
    case_insensitive: bool,
) -> Result<FilterSpec> {
    let mut seen = BTreeSet::new();
    let mut values = Vec::new();
    for keyword in keywords {
        let keyword = keyword.trim();
        if keyword.is_empty() || !seen.insert(keyword.to_string()) {
            continue;
        }
        values.push(keyword.to_string());
    }
    if values.is_empty() {
        anyhow::bail!("monitor rule keywords must not be empty");
    }
    let pattern = match operator {
        EventOperator::Contains => values
            .iter()
            .map(|value| escape_regex_literal(value))
            .collect::<Vec<_>>()
            .join("|"),
        EventOperator::Equals => format!(
            "^(?:{})$",
            values
                .iter()
                .map(|value| escape_regex_literal(value))
                .collect::<Vec<_>>()
                .join("|")
        ),
        EventOperator::Matches => keyword_regex_pattern(&values, case_insensitive)?,
        EventOperator::Exists => anyhow::bail!("message text rules do not support exists"),
    };
    let case_insensitive = match operator {
        EventOperator::Contains => true,
        _ => case_insensitive,
    };
    Ok(FilterSpec::Tagged(TaggedFilterSpec::Regex {
        pattern,
        case_insensitive,
    }))
}

fn keyword_regex_pattern(values: &[String], case_insensitive: bool) -> Result<String> {
    let pattern = values
        .iter()
        .map(|value| format!("(?:{value})"))
        .collect::<Vec<_>>()
        .join("|");
    let mut builder = regex::RegexBuilder::new(&pattern);
    builder.case_insensitive(case_insensitive);
    builder.build().context("invalid message text regex rule")?;
    Ok(pattern)
}

fn append_include_filter(existing: Option<FilterSpec>, rule: FilterSpec) -> FilterSpec {
    let Some(existing) = existing else {
        return rule;
    };
    if filter_json_eq(&existing, &rule) {
        return existing;
    }
    match existing {
        FilterSpec::Tagged(TaggedFilterSpec::Any { mut filters }) => {
            push_unique_rule(&mut filters, rule);
            FilterSpec::Tagged(TaggedFilterSpec::Any { filters })
        }
        other => FilterSpec::Tagged(TaggedFilterSpec::Any {
            filters: vec![other, rule],
        }),
    }
}

fn remove_from_include_filter(
    existing: Option<FilterSpec>,
    target: &FilterSpec,
) -> Option<FilterSpec> {
    let existing = existing?;
    if filter_json_eq(&existing, target) {
        return None;
    }
    match existing {
        FilterSpec::Tagged(TaggedFilterSpec::Any { mut filters }) => {
            remove_matching_rule(&mut filters, target);
            simplify_filters(filters)
        }
        other => Some(other),
    }
}

fn simplify_filters(mut filters: Vec<FilterSpec>) -> Option<FilterSpec> {
    match filters.len() {
        0 => None,
        1 => filters.pop(),
        _ => Some(FilterSpec::Tagged(TaggedFilterSpec::Any { filters })),
    }
}

fn push_unique_rule(filters: &mut Vec<FilterSpec>, rule: FilterSpec) {
    if !filters
        .iter()
        .any(|existing| filter_json_eq(existing, &rule))
    {
        filters.push(rule);
    }
}

fn remove_matching_rule(filters: &mut Vec<FilterSpec>, target: &FilterSpec) {
    filters.retain(|filter| !filter_json_eq(filter, target));
}

fn filter_json_eq(left: &FilterSpec, right: &FilterSpec) -> bool {
    serde_json::to_value(left).ok() == serde_json::to_value(right).ok()
}

fn flatten_include_filters(filter: Option<&FilterSpec>) -> Vec<Value> {
    match filter {
        Some(FilterSpec::Tagged(TaggedFilterSpec::Any { filters })) => filters
            .iter()
            .filter_map(|filter| serde_json::to_value(filter).ok())
            .collect(),
        Some(filter) => serde_json::to_value(filter).ok().into_iter().collect(),
        None => Vec::new(),
    }
}

pub(super) fn binding_monitor_rule_schema_json(
    paths: &ConfigPaths,
    binding: &WorkflowBindingSpec,
) -> Option<Value> {
    let manager = subscription_manager().ok()?;
    monitor_rule_schema(paths, manager.as_ref(), binding)
        .ok()
        .flatten()
        .and_then(|schema| serde_json::to_value(schema).ok())
}

pub(super) fn connection_monitor_rule_schema_json(
    paths: &ConfigPaths,
    manager: &SubscriptionManager,
    connection: &ConnectionRecord,
) -> Option<Value> {
    let template = manager.connector_store().get(&connection.connector_slug)?;
    let schema = monitor_rule_schema_for_connection(paths, connection, &template).ok()??;
    serde_json::to_value(schema).ok()
}

fn monitor_rule_schema(
    paths: &ConfigPaths,
    manager: &SubscriptionManager,
    binding: &WorkflowBindingSpec,
) -> Result<Option<EventSchema>> {
    if let Some(connection) = manager.connection_store().get(&binding.connection_slug) {
        let template = manager
            .connector_store()
            .get(&connection.connector_slug)
            .with_context(|| format!("connector `{}` not found", connection.connector_slug))?;
        return monitor_rule_schema_for_connection(paths, &connection, &template);
    }
    let Some(connector_slug) = binding.connector_slug.as_deref() else {
        return Ok(None);
    };
    let Some(template) = manager.connector_store().get(connector_slug) else {
        return Ok(None);
    };
    let connection =
        ConnectionRecord::authenticated(&binding.connection_slug, connector_slug, "monitor");
    monitor_rule_schema_for_connection(paths, &connection, &template)
}

fn monitor_rule_schema_for_connection(
    paths: &ConfigPaths,
    connection: &ConnectionRecord,
    template: &puffer_subscriptions::ConnectorTemplate,
) -> Result<Option<EventSchema>> {
    let roots = super::subscriber_manifest_roots(paths);
    if let Some(dir) = connection_subscriber_manifest_dir(&roots, connection, template) {
        if let Some(schema) = load_event_schema_from_dir(&dir)? {
            return Ok(Some(schema));
        }
    }
    for dir in connector_event_schema_dirs(paths, &template.slug) {
        if let Some(schema) = load_event_schema_from_dir(&dir)? {
            return Ok(Some(schema));
        }
    }
    Ok(None)
}

fn connector_event_schema_dirs(paths: &ConfigPaths, connector_slug: &str) -> [PathBuf; 3] {
    [
        paths
            .workspace_config_dir
            .join("connectors")
            .join(connector_slug),
        paths
            .user_config_dir
            .join("connectors")
            .join(connector_slug),
        paths
            .builtin_resources_dir
            .join("connectors")
            .join(connector_slug),
    ]
}

fn parse_rule_mode(mode: &str) -> Result<MonitorRuleMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "include" | "only_include" | "only-include" => Ok(MonitorRuleMode::Include),
        "exclude" | "ignore" => Ok(MonitorRuleMode::Exclude),
        _ => anyhow::bail!("monitor rule mode must be include or exclude"),
    }
}

fn valid_connection_slug(slug: &str) -> Result<&str> {
    let trimmed = slug.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
    {
        anyhow::bail!("invalid monitor rule connection slug");
    }
    Ok(trimmed)
}

fn escape_regex_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(
            ch,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn monitor_slug(connection_slug: &str) -> String {
    format!("monitor-{connection_slug}")
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
#[path = "monitor_rules_tests.rs"]
mod tests;
