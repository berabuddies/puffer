use super::*;
use crate::connection::ConnectionRecord;
use crate::spec::{ActionSpec, WorkflowBindingSpec, WorkflowBindingStatus};
use puffer_subscriber_runtime::SubscriberCommand;
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn agent_proxy_binding_counts_as_connection_consumer() {
    let temp = tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .build(runtime.handle().clone())
        .unwrap();
    manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            "my-bot",
            "telegram-bot",
            "demo",
        ))
        .unwrap();

    let decision = manager
        .handle_agent_proxy_event(
            "telegram-bot",
            "my-bot",
            &serde_json::json!({"message":"/connect agent-1","from":{"id":123}}),
        )
        .unwrap();

    assert!(matches!(decision, AgentProxyDecision::BindAgent { .. }));
    let connection = manager.connection_store().get("my-bot").unwrap();
    assert!(connection.has_consumer);
    assert_eq!(connection.state, ConnectionState::Active);

    let decision = manager
        .handle_agent_proxy_event(
            "telegram-bot",
            "my-bot",
            &serde_json::json!({"message":"status?","from":{"id":123}}),
        )
        .unwrap();
    assert_eq!(
        decision,
        AgentProxyDecision::RouteToAgent {
            target: "agent-1".into(),
            message: "status?".into(),
            binding: crate::proxy::AgentProxyBinding {
                connection_slug: "my-bot".into(),
                external_principal: "123".into(),
                reply_target: Some("123".into()),
                agent_target: "agent-1".into(),
                enabled: true,
            },
        }
    );

    manager.shutdown();
}

#[test]
fn start_subscriber_allows_immediate_control_command() {
    let temp = tempdir().unwrap();
    let subscriber_dir = temp.path().join("subscriber");
    std::fs::create_dir_all(&subscriber_dir).unwrap();
    std::fs::write(
        subscriber_dir.join("manifest.toml"),
        r#"manifest_version = 1
id = "test-subscriber"
kind = "subscriber"
topic = "test-topic"

[run]
cmd = ["sh", "run.sh"]
"#,
    )
    .unwrap();
    std::fs::write(
        subscriber_dir.join("run.sh"),
        r#"IFS= read -r _line || exit 0
printf '%s\n' '{"topic":"test-topic","kind":"message","text":"ready"}'
"#,
    )
    .unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .build(runtime.handle().clone())
        .unwrap();
    let mut rx = manager.bus().subscribe_topic("test-topic");
    let manifest = Manifest::load(&subscriber_dir).unwrap();

    manager.start_subscriber(manifest).unwrap();
    manager
        .send_command(
            "test-subscriber",
            &SubscriberCommand::Custom {
                op: "ping".into(),
                args: Value::Null,
            },
        )
        .unwrap();

    let envelope = runtime
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
        })
        .unwrap()
        .unwrap();
    assert_eq!(envelope.subscriber_id, "test-subscriber");
    assert_eq!(envelope.event.text, "ready");

    manager.shutdown();
}

#[test]
fn auth_refresh_does_not_degrade_connections_without_checker() {
    let temp = tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .build(runtime.handle().clone())
        .unwrap();
    manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            "work-email",
            "email",
            "demo",
        ))
        .unwrap();
    manager
        .store()
        .create(WorkflowBindingSpec {
            slug: "email-workflow".into(),
            description: String::new(),
            connection_slug: "work-email".into(),
            connector_slug: Some("email".into()),
            status: WorkflowBindingStatus::Enabled,
            filter: None,
            ignore_filters: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::RunWorkflow {
                slug: "demo".into(),
            },
            created_at_ms: 0,
        })
        .unwrap();

    manager.refresh_connection_consumers().unwrap();
    let notices = manager.refresh_connection_auth().unwrap();
    let connection = manager.connection_store().get("work-email").unwrap();

    assert!(notices.is_empty());
    assert_eq!(connection.state, ConnectionState::Active);
    assert!(!connection.auth_failure_notified);

    manager.shutdown();
}

#[test]
fn start_subscriber_passes_absolute_state_dir() {
    let temp = tempdir().unwrap();
    let subscriber_dir = temp.path().join("subscriber");
    std::fs::create_dir_all(&subscriber_dir).unwrap();
    std::fs::write(
        subscriber_dir.join("manifest.toml"),
        r#"manifest_version = 1
id = "state-subscriber"
kind = "subscriber"
topic = "state-topic"

[run]
cmd = ["sh", "run.sh"]

[state]
dir = "state"
"#,
    )
    .unwrap();
    std::fs::write(
        subscriber_dir.join("run.sh"),
        r#"printf '{"topic":"state-topic","kind":"state","text":"%s"}\n' "$PUFFER_SKILL_STATE_DIR"
"#,
    )
    .unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .build(runtime.handle().clone())
        .unwrap();
    let mut rx = manager.bus().subscribe_topic("state-topic");
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(temp.path()).unwrap();
    let manifest = Manifest::load("subscriber").unwrap();

    manager.start_subscriber(manifest).unwrap();
    std::env::set_current_dir(original_cwd).unwrap();

    let envelope = runtime
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
        })
        .unwrap()
        .unwrap();
    assert!(
        std::path::Path::new(&envelope.event.text).is_absolute(),
        "state dir should be absolute, got {}",
        envelope.event.text
    );

    manager.shutdown();
}

#[test]
fn send_command_and_wait_returns_terminal_event() {
    let temp = tempdir().unwrap();
    let subscriber_dir = temp.path().join("subscriber");
    std::fs::create_dir_all(&subscriber_dir).unwrap();
    std::fs::write(
        subscriber_dir.join("manifest.toml"),
        r#"manifest_version = 1
id = "wait-subscriber"
kind = "subscriber"
topic = "wait-topic"

[run]
cmd = ["sh", "run.sh"]
"#,
    )
    .unwrap();
    std::fs::write(
        subscriber_dir.join("run.sh"),
        r#"IFS= read -r _line || exit 0
printf '%s\n' '{"topic":"wait-topic","kind":"ignored","text":"first"}'
printf '%s\n' '{"topic":"wait-topic","kind":"login_error","text":"terminal","payload":{"error":"boom"}}'
"#,
    )
    .unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .build(runtime.handle().clone())
        .unwrap();
    let manifest = Manifest::load(&subscriber_dir).unwrap();

    manager.start_subscriber(manifest).unwrap();
    let envelope = manager
        .send_command_and_wait(
            "wait-subscriber",
            "wait-topic",
            &SubscriberCommand::Custom {
                op: "ping".into(),
                args: Value::Null,
            },
            &["login_awaiting_code", "login_error"],
            std::time::Duration::from_secs(2),
        )
        .unwrap();
    assert_eq!(envelope.event.kind, "login_error");
    assert_eq!(envelope.event.payload["error"], "boom");

    manager.shutdown();
}

#[test]
fn send_command_and_wait_resends_after_subscriber_restart() {
    let temp = tempdir().unwrap();
    let subscriber_dir = temp.path().join("subscriber");
    std::fs::create_dir_all(&subscriber_dir).unwrap();
    std::fs::write(
        subscriber_dir.join("manifest.toml"),
        r#"manifest_version = 1
id = "restart-subscriber"
kind = "subscriber"
topic = "restart-topic"

[run]
cmd = ["sh", "run.sh"]

[state]
dir = "state"
"#,
    )
    .unwrap();
    std::fs::write(
        subscriber_dir.join("run.sh"),
        r#"mkdir -p "$PUFFER_SKILL_STATE_DIR"
count_file="$PUFFER_SKILL_STATE_DIR/count"
count="$(cat "$count_file" 2>/dev/null || printf 0)"
IFS= read -r _line || exit 0
if [ "$count" = "0" ]; then
  printf 1 > "$count_file"
  exit 0
fi
printf '%s\n' '{"topic":"restart-topic","kind":"done","text":"retried"}'
"#,
    )
    .unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .build(runtime.handle().clone())
        .unwrap();
    let manifest = Manifest::load(&subscriber_dir).unwrap();

    manager.start_subscriber(manifest).unwrap();
    let envelope = manager
        .send_command_and_wait(
            "restart-subscriber",
            "restart-topic",
            &SubscriberCommand::Custom {
                op: "ping".into(),
                args: Value::Null,
            },
            &["done"],
            std::time::Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(envelope.event.kind, "done");
    assert_eq!(envelope.event.text, "retried");

    manager.shutdown();
}
