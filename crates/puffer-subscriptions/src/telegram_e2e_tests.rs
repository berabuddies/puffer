use crate::{
    now_ms, ActionGraphNode, ActionSpec, BuiltinActionDispatcher, ConnectionRecord,
    ConnectorActionDefinition, ConnectorActionExecutor, ConnectorActionRequest,
    ConnectorPermissionDefinition, ConnectorTemplate, FilterSpec, SubscriptionManager,
    SubscriptionManagerBuilder, TaggedFilterSpec, WorkflowBindingSpec, WorkflowBindingStatus,
};
use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[test]
fn telegram_style_connector_runs_workflow_and_reply_action_end_to_end() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("telegram_connector.sh");
    let subscribe_log = temp.path().join("subscribe.log");
    let ack_log = temp.path().join("ack.log");
    let act_log = temp.path().join("act.log");
    write_telegram_connector_script(&script, &subscribe_log, &ack_log, &act_log);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    let dispatcher = Arc::new(BuiltinActionDispatcher::with_storage_root(temp.path()));
    let executor = Arc::new(ManagerBackedConnectorExecutor::default());
    dispatcher.set_connector_action_executor(executor.clone());
    let manager = Arc::new(
        SubscriptionManagerBuilder::new(temp.path().join("workflows.json"))
            .with_dispatcher(dispatcher)
            .build(runtime.handle().clone())
            .unwrap(),
    );
    assert!(executor.manager.set(manager.clone()).is_ok());

    manager
        .connector_store()
        .upsert(telegram_template(&script))
        .unwrap();
    manager
        .connection_store()
        .create(ConnectionRecord::authenticated(
            "tg-demo",
            "telegram-e2e",
            "Telegram demo connection",
        ))
        .unwrap();
    manager.store().create(telegram_workflow()).unwrap();
    manager.refresh_connection_consumers().unwrap();

    wait_for_file(&ack_log);
    wait_for_file(&act_log);

    let subscribe = std::fs::read_to_string(&subscribe_log).unwrap();
    assert!(subscribe.contains(r#""op":"subscribe""#));
    assert!(subscribe.contains(r#""connection":"tg-demo""#));
    let ack = std::fs::read_to_string(&ack_log).unwrap();
    assert!(ack.contains(r#""op":"ack""#));
    assert!(ack.contains(r#""event_id":"telegram-update-1""#));

    let connection = manager.connection_store().get("tg-demo").unwrap();
    assert_eq!(connection.cursor.as_deref(), Some("telegram-cursor-1"));

    let conn = Connection::open(temp.path().join("telegram.db")).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let text: String = conn
        .query_row("SELECT text FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(text, "deploy complete");

    let act = std::fs::read_to_string(&act_log).unwrap();
    let act: Value = serde_json::from_str(act.trim()).unwrap();
    assert_eq!(act["connection"], "tg-demo");
    assert_eq!(act["action"], "send_message");
    assert_eq!(act["input"]["to"], "chat-42");
    assert_eq!(act["input"]["message"], "Saved: deploy complete");

    manager.shutdown();
    drop(runtime);
}

#[derive(Default)]
struct ManagerBackedConnectorExecutor {
    manager: OnceLock<Arc<SubscriptionManager>>,
}

impl ConnectorActionExecutor for ManagerBackedConnectorExecutor {
    fn run_connector_action(
        &self,
        connector_slug: &str,
        action: &str,
        input: Value,
        trigger: Value,
    ) -> Result<String> {
        let manager = self
            .manager
            .get()
            .ok_or_else(|| anyhow::anyhow!("manager not installed"))?;
        let template = manager
            .connector_store()
            .get(connector_slug)
            .ok_or_else(|| anyhow::anyhow!("connector `{connector_slug}` not found"))?;
        let connection = input
            .get("connection_slug")
            .or_else(|| input.get("connection"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| {
                trigger
                    .get("connection_id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| connector_slug.to_string());
        let request = ConnectorActionRequest {
            connection,
            action: action.to_string(),
            input,
            idempotency_key: trigger
                .get("envelope_id")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        };
        let response = manager
            .run_connector_action(&template, &request)?
            .ok_or_else(|| anyhow::anyhow!("connector `{connector_slug}` has no act command"))?;
        if response.success {
            Ok(response.summary)
        } else {
            anyhow::bail!("{}", response.summary)
        }
    }
}

fn telegram_workflow() -> WorkflowBindingSpec {
    WorkflowBindingSpec {
        slug: "tg-save-and-reply".into(),
        description: "Save Telegram deploy messages and reply".into(),
        connection_slug: "tg-demo".into(),
        connector_slug: Some("telegram-e2e".into()),
        status: WorkflowBindingStatus::Enabled,
        filter: Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
            pattern: "deploy".into(),
            case_insensitive: true,
        })),
        classify_prompt: None,
        classify_model: None,
        action: ActionSpec::Graph {
            nodes: vec![
                ActionGraphNode {
                    id: "save".into(),
                    depends_on: Vec::new(),
                    action: ActionSpec::SqliteInsert {
                        path: "telegram.db".into(),
                        table: "messages".into(),
                    },
                },
                ActionGraphNode {
                    id: "reply".into(),
                    depends_on: vec!["save".into()],
                    action: ActionSpec::ConnectorAct {
                        connector_slug: "telegram-e2e".into(),
                        action: "send_message".into(),
                        input: json!({
                            "to": "{{payload.chat.id}}",
                            "message": "Saved: {{text}}"
                        }),
                    },
                },
            ],
        },
        created_at_ms: now_ms(),
    }
}

fn telegram_template(script: &Path) -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "telegram-e2e".into(),
        description: "Local Telegram-shaped E2E connector".into(),
        skill: "telegram".into(),
        binary: script.display().to_string(),
        command: vec!["sh".into(), script.display().to_string()],
        requires_auth: false,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: Value::Null,
        actions: BTreeMap::from([(
            "send_message".into(),
            ConnectorActionDefinition {
                slug: "send_message".into(),
                description: "Send a Telegram message".into(),
                input_schema: Value::Null,
                output_schema: Value::Null,
                permission: ConnectorPermissionDefinition {
                    category: "external_message_send".into(),
                    summary: "Send a Telegram message".into(),
                    external_side_effect: true,
                },
            },
        )]),
    }
}

fn write_telegram_connector_script(
    script: &Path,
    subscribe_log: &Path,
    ack_log: &Path,
    act_log: &Path,
) {
    std::fs::write(
        script,
        format!(
            r#"mode="$1"
if [ "$mode" = "auth-ok" ]; then
  printf '%s\n' true
  exit 0
fi
if [ "$mode" = "subscribe" ]; then
  IFS= read -r subscribe
  printf '%s\n' "$subscribe" > '{}'
  printf '%s\n' '{{"type":"event","id":"telegram-update-1","cursor":"telegram-cursor-1","payload":{{"kind":"message","message":"deploy complete","from":{{"id":"tony","name":"TonyKe"}},"chat":{{"id":"chat-42","name":"ETHSecurity"}}}}}}'
  IFS= read -r ack
  printf '%s\n' "$ack" > '{}'
  sleep 5
  exit 0
fi
if [ "$mode" = "act" ]; then
  connection="$2"
  action="$3"
  IFS= read -r input
  printf '{{"connection":"%s","action":"%s","input":%s}}\n' "$connection" "$action" "$input" >> '{}'
  printf '%s\n' '{{"success":true,"summary":"telegram mock sent reply","output":{{"message_id":"m-1"}},"retryable":false}}'
  exit 0
fi
echo "unsupported mode: $mode" >&2
exit 1
"#,
            subscribe_log.display(),
            ack_log.display(),
            act_log.display()
        ),
    )
    .unwrap();
}

fn wait_for_file(path: &PathBuf) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {}", path.display());
}
