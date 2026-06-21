use super::*;
use crate::action::ActionResult;
use crate::catalog::ConnectorTemplate;
use crate::connection::{ConnectionHealth, ConnectionHealthStatus, ConnectionRecord};
use crate::spec::{ActionSpec, WorkflowBindingSpec, WorkflowBindingStatus};
use puffer_subscriber_runtime::{Event, EventEnvelope, SubscriberCommand};
use serde_json::json;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use tempfile::tempdir;

static MONITOR_DIGEST_ENV_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();

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
fn monitor_events_flush_as_delayed_digest_batch() {
    struct RecordingDispatcher {
        batches: StdMutex<Vec<Vec<String>>>,
    }

    impl ActionDispatcher for RecordingDispatcher {
        fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
            panic!("monitor digest should use batch dispatch")
        }

        fn dispatch_batch(
            &self,
            _action: &ActionSpec,
            envelopes: &[EventEnvelope],
        ) -> ActionResult {
            self.batches.lock().unwrap().push(
                envelopes
                    .iter()
                    .map(|envelope| envelope.event.text.clone())
                    .collect(),
            );
            ActionResult::success("digest triaged")
        }
    }

    let temp = tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let dispatcher = Arc::new(RecordingDispatcher {
        batches: StdMutex::new(Vec::new()),
    });
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .with_dispatcher(dispatcher.clone())
        .with_monitor_digest_interval(std::time::Duration::from_millis(25))
        .build(runtime.handle().clone())
        .unwrap();
    manager
        .store()
        .create(WorkflowBindingSpec {
            slug: "monitor-telegram-user".into(),
            description: "Monitor telegram-user for actionable tasks".into(),
            connection_slug: "telegram-user".into(),
            connector_slug: Some("telegram-login".into()),
            status: WorkflowBindingStatus::Enabled,
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::TriageAgent {
                prompt: "triage".into(),
                model: None,
            },
            created_at_ms: 0,
        })
        .unwrap();
    let processor = ManagerConnectorEventProcessor::new(
        manager.store.clone(),
        manager.connection_store.clone(),
        manager.history_store.clone(),
        manager.proxy_store.clone(),
        manager.dispatcher.clone(),
        manager.classifier.clone(),
        manager.self_gate.clone(),
        manager.monitor_digest.clone(),
    );
    let envelopes = vec![
        manager_test_event("env-1", "please review the first update today"),
        manager_test_event("env-2", "can you confirm the second plan?"),
    ];

    processor
        .process_connector_events("telegram-login", "telegram-user", &envelopes)
        .unwrap();

    assert!(dispatcher.batches.lock().unwrap().is_empty());
    runtime.block_on(async {
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    });
    assert_eq!(
        dispatcher.batches.lock().unwrap().as_slice(),
        &[vec![
            "please review the first update today".to_string(),
            "can you confirm the second plan?".to_string()
        ]]
    );
    manager.shutdown();
}

#[test]
fn monitor_digest_interval_can_be_overridden_for_local_testing() {
    let _guard = MONITOR_DIGEST_ENV_LOCK
        .get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap();
    let previous = std::env::var_os(MONITOR_DIGEST_INTERVAL_SECONDS_ENV);
    std::env::set_var(MONITOR_DIGEST_INTERVAL_SECONDS_ENV, "300");

    assert_eq!(
        monitor_digest_interval_from_env(),
        std::time::Duration::from_secs(300)
    );

    if let Some(previous) = previous {
        std::env::set_var(MONITOR_DIGEST_INTERVAL_SECONDS_ENV, previous);
    } else {
        std::env::remove_var(MONITOR_DIGEST_INTERVAL_SECONDS_ENV);
    }
}

#[test]
fn monitor_digest_interval_defaults_to_one_minute() {
    let _guard = MONITOR_DIGEST_ENV_LOCK
        .get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap();
    let previous = std::env::var_os(MONITOR_DIGEST_INTERVAL_SECONDS_ENV);
    std::env::remove_var(MONITOR_DIGEST_INTERVAL_SECONDS_ENV);

    assert_eq!(
        monitor_digest_interval_from_env(),
        std::time::Duration::from_secs(60)
    );

    if let Some(previous) = previous {
        std::env::set_var(MONITOR_DIGEST_INTERVAL_SECONDS_ENV, previous);
    }
}

fn manager_test_event(envelope_id: &str, text: &str) -> EventEnvelope {
    EventEnvelope {
        envelope_id: envelope_id.into(),
        subscriber_id: "telegram-user".into(),
        received_at_ms: 0,
        event: Event {
            topic: "telegram-user".into(),
            kind: "message".into(),
            control: false,
            dedup_key: Some(envelope_id.into()),
            text: text.into(),
            payload: json!({ "message": text }),
        },
    }
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
            contact_ids: Vec::new(),
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

struct UnknownAuthChecker;

impl ConnectionAuthChecker for UnknownAuthChecker {
    fn check(
        &self,
        _manager: &SubscriptionManager,
        _template: &ConnectorTemplate,
        _connection_slug: &str,
    ) -> Result<Option<ConnectionAuthStatus>> {
        Ok(Some(ConnectionAuthStatus::Unknown))
    }
}

#[test]
fn control_health_event_marks_connection_degraded_and_ready_restores_active() {
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
            "telegram-user",
            "telegram-login",
            "Personal Telegram",
        ))
        .unwrap();
    manager
        .store()
        .create(test_binding(
            "telegram-monitor",
            "telegram-user",
            Vec::new(),
        ))
        .unwrap();
    manager.refresh_connection_consumers().unwrap();
    assert_eq!(
        manager
            .connection_store()
            .get("telegram-user")
            .unwrap()
            .state,
        ConnectionState::Active
    );

    manager.bus().publish(EventEnvelope {
        envelope_id: "offline".into(),
        subscriber_id: "telegram-user".into(),
        received_at_ms: 1_700_000_000_000,
        event: Event {
            topic: "telegram-user".into(),
            kind: "connection_health".into(),
            control: true,
            dedup_key: None,
            text: String::new(),
            payload: json!({
                "status": "retrying",
                "reason": "connect_failed",
                "detail": "read 0 bytes",
                "next_retry_at_ms": 1_700_000_010_000_i64,
            }),
        },
    });
    runtime.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(50)).await });

    let degraded = manager.connection_store().get("telegram-user").unwrap();
    assert_eq!(degraded.state, ConnectionState::Degraded);
    assert_eq!(
        degraded.health,
        Some(ConnectionHealth {
            status: ConnectionHealthStatus::Retrying,
            reason: Some("connect_failed".into()),
            detail: Some("read 0 bytes".into()),
            updated_at_ms: 1_700_000_000_000,
            next_retry_at_ms: Some(1_700_000_010_000),
        })
    );

    manager.bus().publish(EventEnvelope {
        envelope_id: "ready".into(),
        subscriber_id: "telegram-user".into(),
        received_at_ms: 1_700_000_020_000,
        event: Event {
            topic: "telegram-user".into(),
            kind: "ready".into(),
            control: true,
            dedup_key: None,
            text: String::new(),
            payload: json!({ "resumed": true, "recovered": true }),
        },
    });
    runtime.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(50)).await });

    let recovered = manager.connection_store().get("telegram-user").unwrap();
    assert_eq!(recovered.state, ConnectionState::Active);
    assert_eq!(
        recovered.health.as_ref().map(|health| health.status),
        Some(ConnectionHealthStatus::Ok)
    );

    manager.shutdown();
}

#[test]
fn auth_unknown_does_not_clear_degraded_health() {
    let temp = tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .with_connection_auth_checker(Arc::new(UnknownAuthChecker))
        .build(runtime.handle().clone())
        .unwrap();
    manager
        .connector_store()
        .upsert(ConnectorTemplate {
            slug: "telegram-login".into(),
            description: "Telegram".into(),
            skill: "telegram".into(),
            binary: "puffer".into(),
            command: Vec::new(),
            requires_auth: true,
            can_subscribe: true,
            can_proxy_agent: false,
            subscriber: None,
            output_schema: Value::Null,
            actions: BTreeMap::new(),
        })
        .unwrap();
    manager
        .connection_store()
        .create(ConnectionRecord {
            state: ConnectionState::Degraded,
            has_consumer: true,
            health: Some(ConnectionHealth {
                status: ConnectionHealthStatus::Retrying,
                reason: Some("connect_failed".into()),
                detail: Some("read 0 bytes".into()),
                updated_at_ms: 1_700_000_000_000,
                next_retry_at_ms: Some(1_700_000_010_000),
            }),
            ..ConnectionRecord::authenticated(
                "telegram-user",
                "telegram-login",
                "Personal Telegram",
            )
        })
        .unwrap();

    let notices = manager.refresh_connection_auth().unwrap();
    let connection = manager.connection_store().get("telegram-user").unwrap();

    assert!(notices.is_empty());
    assert_eq!(connection.state, ConnectionState::Degraded);
    assert_eq!(
        connection.health.as_ref().map(|health| health.status),
        Some(ConnectionHealthStatus::Retrying)
    );
    assert!(!connection.auth_failure_notified);

    manager.shutdown();
}

#[test]
fn connector_stream_restarts_when_contact_scope_changes() {
    let temp = tempdir().unwrap();
    let (script, log) = write_stream_logger(temp.path());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .build(runtime.handle().clone())
        .unwrap();
    manager
        .connector_store()
        .upsert(stream_connector_template(&script, &log))
        .unwrap();
    manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            "chat",
            "telegram-login",
            "test chat",
        ))
        .unwrap();

    manager
        .store()
        .upsert(test_binding(
            "chat-monitor",
            "chat",
            vec!["google@alice@example.com".into(), "telegram@alice".into()],
        ))
        .unwrap();
    manager.refresh_connection_consumers().unwrap();
    let commands = wait_for_subscribe_commands(&log, 1);
    assert_eq!(commands[0]["contact_ids"], json!(["telegram@alice"]));

    manager
        .store()
        .upsert(test_binding(
            "chat-monitor",
            "chat",
            vec!["telegram@bob".into()],
        ))
        .unwrap();
    manager.refresh_connection_consumers().unwrap();
    let commands = wait_for_subscribe_commands(&log, 2);
    assert_eq!(commands[1]["contact_ids"], json!(["telegram@bob"]));

    manager
        .store()
        .upsert(test_binding("chat-all", "chat", Vec::new()))
        .unwrap();
    manager.refresh_connection_consumers().unwrap();
    let commands = wait_for_subscribe_commands(&log, 3);
    assert!(commands[2].get("contact_ids").is_none());

    manager.shutdown();
}

#[test]
fn connector_stream_does_not_start_for_empty_owned_contact_scope() {
    let temp = tempdir().unwrap();
    let (script, log) = write_stream_logger(temp.path());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap();
    let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
        .build(runtime.handle().clone())
        .unwrap();
    manager
        .connector_store()
        .upsert(stream_connector_template(&script, &log))
        .unwrap();
    manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            "chat",
            "telegram-login",
            "test chat",
        ))
        .unwrap();

    manager
        .store()
        .upsert(test_binding(
            "chat-monitor",
            "chat",
            vec!["google@alice@example.com".into()],
        ))
        .unwrap();
    manager.refresh_connection_consumers().unwrap();

    assert!(read_subscribe_commands(&log).is_empty());

    manager.shutdown();
}

#[test]
fn no_command_connector_contacts_fall_back_to_history() {
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
            "work-gmail",
            "gmail-browser",
            "Work Gmail",
        ))
        .unwrap();
    let binding = WorkflowBindingSpec {
        slug: "gmail-monitor".into(),
        description: "gmail monitor".into(),
        connection_slug: "work-gmail".into(),
        connector_slug: Some("gmail-browser".into()),
        status: WorkflowBindingStatus::Enabled,
        filter: None,
        ignore_filters: Vec::new(),
        contact_ids: Vec::new(),
        classify_prompt: None,
        classify_model: None,
        action: ActionSpec::RunWorkflow {
            slug: "demo".into(),
        },
        created_at_ms: 0,
    };
    let envelope = EventEnvelope {
        envelope_id: "env-1".into(),
        subscriber_id: "work-gmail".into(),
        received_at_ms: 1_700_000_000_000,
        event: Event {
            topic: "work-gmail".into(),
            kind: "message".into(),
            control: false,
            dedup_key: None,
            text: "Alice sent the quarterly launch checklist".into(),
            payload: json!({
                "from": "Alice <Alice@Example.COM>",
                "subject": "Quarterly launch checklist"
            }),
        },
    };
    manager
        .history_store()
        .append_action_result(
            &binding,
            &envelope,
            &binding.action,
            &ActionResult::success("ok"),
            1_700_000_000_000,
            1_700_000_000_100,
        )
        .unwrap();

    let contacts = manager
        .list_connector_contacts("work-gmail", None, Some(10))
        .unwrap()
        .unwrap();
    assert_eq!(contacts[0].id, "google@alice@example.com");
    assert_eq!(contacts[0].name.as_deref(), Some("Alice"));
    let searched = manager
        .search_connector_contacts("work-gmail", "checklist".into(), Some(10))
        .unwrap()
        .unwrap();
    assert_eq!(searched[0].id, "google@alice@example.com");
    let (ids, context) = manager
        .connector_contact_context(
            "work-gmail",
            vec!["telegram@alice".into(), "google@alice@example.com".into()],
            Some(5),
        )
        .unwrap()
        .unwrap();
    assert_eq!(ids, vec!["google@alice@example.com"]);
    assert_eq!(context[0].text, "Alice sent the quarterly launch checklist");

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

fn write_stream_logger(dir: &Path) -> (PathBuf, PathBuf) {
    let script = dir.join("stream.sh");
    let log = dir.join("subscribes.ndjson");
    std::fs::write(
        &script,
        r#"log="$1"
IFS= read -r line || exit 0
printf '%s\n' "$line" >> "$log"
while IFS= read -r _line; do
  :
done
"#,
    )
    .unwrap();
    (script, log)
}

fn stream_connector_template(script: &Path, log: &Path) -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "telegram-login".into(),
        description: "Test stream connector".into(),
        skill: "test-stream".into(),
        binary: "sh".into(),
        command: vec![
            "sh".into(),
            script.display().to_string(),
            log.display().to_string(),
        ],
        requires_auth: false,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: Value::Null,
        actions: BTreeMap::new(),
    }
}

fn test_binding(
    slug: &str,
    connection_slug: &str,
    contact_ids: Vec<String>,
) -> WorkflowBindingSpec {
    WorkflowBindingSpec {
        slug: slug.into(),
        description: "test binding".into(),
        connection_slug: connection_slug.into(),
        connector_slug: Some("telegram-login".into()),
        status: WorkflowBindingStatus::Enabled,
        filter: None,
        ignore_filters: Vec::new(),
        contact_ids,
        classify_prompt: None,
        classify_model: None,
        action: ActionSpec::RunWorkflow {
            slug: "demo".into(),
        },
        created_at_ms: 0,
    }
}

fn wait_for_subscribe_commands(path: &Path, expected: usize) -> Vec<Value> {
    for _ in 0..200 {
        let commands = read_subscribe_commands(path);
        if commands.len() >= expected {
            return commands;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let commands = read_subscribe_commands(path);
    panic!(
        "timed out waiting for {expected} subscribe commands, got {}: {:?}",
        commands.len(),
        commands
    );
}

fn read_subscribe_commands(path: &Path) -> Vec<Value> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}
