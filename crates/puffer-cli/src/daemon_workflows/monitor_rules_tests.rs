use super::super::handle_workflow_list;
use super::{handle_monitor_rule_add, handle_monitor_rule_delete};
use puffer_config::ConfigPaths;
use puffer_core::{install_subscription_manager, subscription_manager};
use puffer_subscriptions::{
    ActionSpec, ConnectionRecord, FilterSpec, SubscriptionManager, SubscriptionManagerBuilder,
    TaggedFilterSpec, WorkflowBindingSpec, WorkflowBindingStatus,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::{Arc, OnceLock};

struct TestManager {
    _runtime: tokio::runtime::Runtime,
    _tempdir: tempfile::TempDir,
    manager: Arc<SubscriptionManager>,
}

static TEST_MANAGER: OnceLock<TestManager> = OnceLock::new();

fn test_manager() -> Arc<SubscriptionManager> {
    if let Ok(manager) = subscription_manager() {
        return manager;
    }
    let state = TEST_MANAGER.get_or_init(|| {
        let tempdir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .thread_name("puffer-monitor-rules-test")
            .build()
            .unwrap();
        let manager = Arc::new(
            SubscriptionManagerBuilder::new(tempdir.path().join("subscriptions.json"))
                .build(runtime.handle().clone())
                .unwrap(),
        );
        let _ = install_subscription_manager(manager.clone());
        TestManager {
            _runtime: runtime,
            _tempdir: tempdir,
            manager,
        }
    });
    subscription_manager().unwrap_or_else(|_| state.manager.clone())
}

fn monitor_binding(slug: &str, connection_slug: &str) -> WorkflowBindingSpec {
    monitor_binding_for_connector(slug, connection_slug, "telegram-login")
}

fn monitor_binding_for_connector(
    slug: &str,
    connection_slug: &str,
    connector_slug: &str,
) -> WorkflowBindingSpec {
    WorkflowBindingSpec {
        slug: slug.to_string(),
        description: format!("Monitor {connection_slug} for actionable tasks"),
        connection_slug: connection_slug.to_string(),
        connector_slug: Some(connector_slug.to_string()),
        status: WorkflowBindingStatus::Enabled,
        filter: None,
        ignore_filters: Vec::new(),
        contact_ids: Vec::new(),
        classify_prompt: None,
        classify_model: None,
        action: ActionSpec::TriageAgent {
            prompt: "triage".to_string(),
            model: None,
        },
        created_at_ms: 0,
    }
}

fn config_paths_with_bundled_resources(root: &Path) -> ConfigPaths {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root");
    ConfigPaths {
        workspace_root: root.to_path_buf(),
        workspace_config_dir: root.join(".puffer"),
        user_config_dir: root.join("home").join(".puffer"),
        builtin_resources_dir: repo_root.join("resources"),
    }
}

#[test]
fn add_exclude_rule_persists_case_insensitive_keyword_filter_and_ignores_legacy_scope() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    let manager = test_manager();
    let connection_slug = "rule-exclude-telegram";
    manager
        .store()
        .upsert(monitor_binding(
            &format!("monitor-{connection_slug}"),
            connection_slug,
        ))
        .unwrap();

    let snapshot = handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": connection_slug,
            "mode": "exclude",
            "keywords": ["作业"],
            "scope": {"field": "chat_id", "value": 7},
            "case_insensitive": false
        }),
    )
    .unwrap();

    let binding = manager
        .store()
        .get(&format!("monitor-{connection_slug}"))
        .unwrap();
    assert!(matches!(
        binding.ignore_filters.as_slice(),
        [FilterSpec::Tagged(TaggedFilterSpec::Regex { pattern, case_insensitive })]
            if pattern == "作业" && *case_insensitive
    ));
    assert_eq!(
        snapshot["workflow_bindings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|binding| binding["connection_slug"] == connection_slug)
            .unwrap()["ignore_filters"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn add_include_rule_persists_filter_without_touching_other_monitor() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    let manager = test_manager();
    let connection_slug = "rule-include-telegram";
    let other_connection_slug = "rule-include-other";
    manager
        .store()
        .upsert(monitor_binding(
            &format!("monitor-{connection_slug}"),
            connection_slug,
        ))
        .unwrap();
    manager
        .store()
        .upsert(monitor_binding(
            &format!("monitor-{other_connection_slug}"),
            other_connection_slug,
        ))
        .unwrap();

    handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": connection_slug,
            "mode": "include",
            "keywords": ["review"],
            "case_insensitive": true
        }),
    )
    .unwrap();

    let binding = manager
        .store()
        .get(&format!("monitor-{connection_slug}"))
        .unwrap();
    assert!(matches!(
        binding.filter,
        Some(FilterSpec::Tagged(TaggedFilterSpec::Regex { .. }))
    ));
    let other = manager
        .store()
        .get(&format!("monitor-{other_connection_slug}"))
        .unwrap();
    assert!(other.filter.is_none());
    assert!(other.ignore_filters.is_empty());
}

#[test]
fn delete_rule_removes_exact_displayed_rule() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    let manager = test_manager();
    let connection_slug = "rule-delete-telegram";
    manager
        .store()
        .upsert(monitor_binding(
            &format!("monitor-{connection_slug}"),
            connection_slug,
        ))
        .unwrap();
    let snapshot = handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": connection_slug,
            "mode": "exclude",
            "keywords": ["noise"],
            "case_insensitive": true
        }),
    )
    .unwrap();
    let rule = snapshot["workflow_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["connection_slug"] == connection_slug)
        .unwrap()["ignore_filters"][0]
        .clone();

    handle_monitor_rule_delete(
        &paths,
        &json!({
            "connection_slug": connection_slug,
            "mode": "exclude",
            "rule": rule
        }),
    )
    .unwrap();

    let binding = manager
        .store()
        .get(&format!("monitor-{connection_slug}"))
        .unwrap();
    assert!(binding.ignore_filters.is_empty());
}

#[test]
fn workflow_snapshot_does_not_emit_scope_options() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    let manager = test_manager();
    let connection_slug = "rule-keyword-only-telegram";
    manager
        .store()
        .upsert(monitor_binding(
            &format!("monitor-{connection_slug}"),
            connection_slug,
        ))
        .unwrap();
    let task_path = paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_tasks.json");
    fs::create_dir_all(task_path.parent().unwrap()).unwrap();
    fs::write(
        &task_path,
        serde_json::to_string_pretty(&json!({
            "tasks": [{
                "task_id": "monitor-1",
                "subject": "Telegram group task",
                "description": "from group",
                "status": "pending",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": connection_slug,
                    "monitor_connector": "telegram-login",
                    "chat_id": 7,
                    "chat_title": "Puffer group",
                    "chat_kind": "group"
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let snapshot = handle_workflow_list(&paths).unwrap();
    let binding = snapshot["workflow_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["connection_slug"] == connection_slug)
        .unwrap();

    assert!(binding.get("scope_options").is_none());
}

#[test]
fn field_rule_snapshot_returns_schema_and_latest_filters() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = config_paths_with_bundled_resources(tempdir.path());
    let manager = test_manager();
    let connection_slug = "rule-snapshot-gmail";
    let _ = manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            connection_slug,
            "gmail-browser",
            "Gmail",
        ));
    manager
        .store()
        .upsert(monitor_binding_for_connector(
            &format!("monitor-{connection_slug}"),
            connection_slug,
            "gmail-browser",
        ))
        .unwrap();

    handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": connection_slug,
            "mode": "include",
            "kind": "field",
            "field": "message.subject",
            "operator": "contains",
            "value": "invoice"
        }),
    )
    .unwrap();
    let snapshot = handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": connection_slug,
            "mode": "exclude",
            "kind": "field",
            "field": "message.unread",
            "operator": "equals",
            "value": true
        }),
    )
    .unwrap();

    let binding = snapshot["workflow_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["connection_slug"] == connection_slug)
        .unwrap();
    assert_eq!(binding["include_filters"].as_array().unwrap().len(), 1);
    assert_eq!(binding["ignore_filters"].as_array().unwrap().len(), 1);
    assert_eq!(
        binding["include_filters"][0]["expression"],
        ".message.subject | test(\"(?i:invoice)\")"
    );
    assert_eq!(
        binding["ignore_filters"][0]["expression"],
        ".message.unread == true"
    );
    assert!(binding["monitor_rule_schema"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["path"] == "message.subject"));
}

#[test]
fn field_rules_require_schema_and_known_fields_but_keywords_do_not() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = config_paths_with_bundled_resources(tempdir.path());
    let manager = test_manager();
    let no_schema_connection = "rule-noschema-slack";
    let _ = manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            no_schema_connection,
            "slack-login",
            "Slack",
        ));
    manager
        .store()
        .upsert(monitor_binding_for_connector(
            &format!("monitor-{no_schema_connection}"),
            no_schema_connection,
            "slack-login",
        ))
        .unwrap();

    let error = handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": no_schema_connection,
            "mode": "include",
            "kind": "field",
            "field": "chat_kind",
            "operator": "equals",
            "value": "group"
        }),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("schema not found"));

    handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": no_schema_connection,
            "mode": "include",
            "kind": "keyword",
            "keywords": ["invoice"]
        }),
    )
    .unwrap();
    let no_schema_binding = manager
        .store()
        .get(&format!("monitor-{no_schema_connection}"))
        .unwrap();
    assert!(matches!(
        no_schema_binding.filter,
        Some(FilterSpec::Tagged(TaggedFilterSpec::Regex { .. }))
    ));

    let telegram_connection = "rule-unknown-telegram";
    let _ = manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            telegram_connection,
            "telegram-login",
            "Telegram",
        ));
    manager
        .store()
        .upsert(monitor_binding(
            &format!("monitor-{telegram_connection}"),
            telegram_connection,
        ))
        .unwrap();
    let error = handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": telegram_connection,
            "mode": "include",
            "kind": "field",
            "field": "message.subject",
            "operator": "contains",
            "value": "invoice"
        }),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("not declared"));
}

#[test]
fn command_backed_connectors_expose_monitor_rule_schemas() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = config_paths_with_bundled_resources(tempdir.path());
    let manager = test_manager();

    let telegram_bot_connection = "rule-schema-telegram-bot";
    let _ = manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            telegram_bot_connection,
            "telegram-bot",
            "Telegram bot",
        ));
    manager
        .store()
        .upsert(monitor_binding_for_connector(
            &format!("monitor-{telegram_bot_connection}"),
            telegram_bot_connection,
            "telegram-bot",
        ))
        .unwrap();
    let snapshot = handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": telegram_bot_connection,
            "mode": "exclude",
            "kind": "field",
            "field": "is_group",
            "operator": "equals",
            "value": true
        }),
    )
    .unwrap();
    let telegram_binding = snapshot["workflow_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["connection_slug"] == telegram_bot_connection)
        .unwrap();
    assert_eq!(
        telegram_binding["ignore_filters"][0]["expression"],
        ".is_group == true"
    );
    assert!(telegram_binding["monitor_rule_schema"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["path"] == "is_group"));

    let lark_connection = "rule-schema-lark";
    let _ = manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            lark_connection,
            "lark-login",
            "Lark",
        ));
    manager
        .store()
        .upsert(monitor_binding_for_connector(
            &format!("monitor-{lark_connection}"),
            lark_connection,
            "lark-login",
        ))
        .unwrap();
    let snapshot = handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": lark_connection,
            "mode": "include",
            "kind": "field",
            "field": "message_type",
            "operator": "equals",
            "value": "text"
        }),
    )
    .unwrap();
    let lark_binding = snapshot["workflow_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["connection_slug"] == lark_connection)
        .unwrap();
    assert_eq!(
        lark_binding["include_filters"][0]["expression"],
        ".message_type == \"text\""
    );
    assert!(lark_binding["monitor_rule_schema"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["path"] == "chat_type"));

    let lark_bot_connection = "rule-schema-lark-bot";
    let _ = manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            lark_bot_connection,
            "lark-bot",
            "Lark bot",
        ));
    manager
        .store()
        .upsert(monitor_binding_for_connector(
            &format!("monitor-{lark_bot_connection}"),
            lark_bot_connection,
            "lark-bot",
        ))
        .unwrap();
    let snapshot = handle_monitor_rule_add(
        &paths,
        &json!({
            "connection_slug": lark_bot_connection,
            "mode": "exclude",
            "kind": "field",
            "field": "chat_type",
            "operator": "equals",
            "value": "group"
        }),
    )
    .unwrap();
    let lark_bot_binding = snapshot["workflow_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["connection_slug"] == lark_bot_connection)
        .unwrap();
    assert_eq!(
        lark_bot_binding["ignore_filters"][0]["expression"],
        ".chat_type == \"group\""
    );
    assert!(lark_bot_binding["monitor_rule_schema"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["path"] == "message_type"));
}
