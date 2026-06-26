use super::*;
use puffer_subscriptions::{
    ActionSpec, ConnectionAuthStatus, ConnectorSubscriberTemplate, ConnectorTemplate,
    WorkflowBindingSpec, WorkflowBindingStatus,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};

fn test_subscription_runtime() -> SubscriptionRuntime {
    let temp = tempfile::tempdir().expect("temp dir");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .thread_name("puffer-subscriptions-test")
        .build()
        .expect("subscription runtime");
    let manager = Arc::new(
        SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
            .build(runtime.handle().clone())
            .expect("subscription manager"),
    );
    SubscriptionRuntime {
        runtime: Some(runtime),
        manager,
        auth_stop: Arc::new(AtomicBool::new(false)),
        auth_thread: None,
    }
}

#[test]
fn subscription_runtime_can_drop_inside_tokio_context() {
    let outer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("outer runtime");
    let runtime = test_subscription_runtime();
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        outer.block_on(async { drop(runtime) });
    }));
    assert!(
        result.is_ok(),
        "dropping SubscriptionRuntime inside Tokio must not panic"
    );
}

#[test]
fn parse_reply_to_accepts_supported_shapes() {
    assert_eq!(
        parse_reply_to(&serde_json::json!({"reply_to": 42})).unwrap(),
        Some(42)
    );
    assert_eq!(
        parse_reply_to(&serde_json::json!({"reply_to": "42"})).unwrap(),
        Some(42)
    );
    assert_eq!(
        parse_reply_to(&serde_json::json!({"reply_to": {"message_id": 42}})).unwrap(),
        Some(42)
    );
    assert_eq!(parse_reply_to(&serde_json::json!({})).unwrap(), None);
}

#[test]
fn parse_reply_to_rejects_invalid_shape() {
    let error = parse_reply_to(&serde_json::json!({"reply_to": {"peer": "x"}}))
        .expect_err("missing message id should fail");
    assert!(error.to_string().contains("reply_to"));
}

#[test]
fn parse_media_attachments_accepts_common_shapes() {
    let media = parse_media_attachments(&serde_json::json!({
        "media": [
            "/tmp/a.jpg",
            {
                "path": "/tmp/b.pdf",
                "caption": "report",
                "kind": "file",
                "mime_type": "application/pdf",
                "thumbnail": "/tmp/thumb.jpg"
            }
        ]
    }))
    .unwrap();

    assert_eq!(media.len(), 2);
    assert_eq!(media[0].path, "/tmp/a.jpg");
    assert_eq!(media[1].caption.as_deref(), Some("report"));
    assert_eq!(media[1].kind, Some(SendMediaKind::File));
    assert_eq!(media[1].mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(media[1].thumbnail.as_deref(), Some("/tmp/thumb.jpg"));
}

#[test]
fn parse_media_attachments_accepts_audio_video_aliases() {
    let media = parse_media_attachments(&serde_json::json!({
        "media": [
            {"path": "/tmp/a.mp3", "kind": "audio"},
            {"path": "/tmp/b.mp4", "kind": "video"}
        ]
    }))
    .unwrap();

    assert_eq!(media[0].kind, Some(SendMediaKind::Document));
    assert_eq!(media[1].kind, Some(SendMediaKind::Document));
}

#[test]
fn send_message_via_subscriber_accepts_schema_recipient_aliases() {
    let runtime = test_subscription_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    let authorization = send_authorization_for_message(
        "draft-test",
        1,
        "send_message",
        "123456789",
        "deployment is finished",
        "request-1",
    );
    let error = send_message_via_subscriber(
        &runtime.manager,
        &paths,
        "telegram-login",
        "telegram-user",
        &serde_json::json!({
            "chat_id": 123456789,
            "text": "deployment is finished"
        }),
        authorization,
    )
    .expect_err("missing subscriber should fail after input aliases are accepted");

    let error = error.to_string();
    assert!(
        error.contains("subscriber") && !error.contains("requires"),
        "unexpected validation error: {error}"
    );
}

#[test]
fn send_message_via_subscriber_rejects_mismatched_send_authorization() {
    let runtime = test_subscription_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    let authorization = send_authorization_for_message(
        "draft-test",
        1,
        "send_message",
        "123456789",
        "different body",
        "request-1",
    );
    let error = send_message_via_subscriber(
        &runtime.manager,
        &paths,
        "telegram-login",
        "telegram-user",
        &serde_json::json!({
            "chat_id": 123456789,
            "text": "deployment is finished"
        }),
        authorization,
    )
    .expect_err("changed content must not be sent with another draft's authorization");

    assert!(
        error.to_string().contains("send authorization"),
        "unexpected validation error: {error:#}"
    );
}

#[test]
fn send_message_target_accepts_advertised_recipient_aliases() {
    for (field, value, expected) in [
        ("to", serde_json::json!("alice"), "alice"),
        ("target", serde_json::json!("bob"), "bob"),
        ("channel", serde_json::json!("#ops"), "#ops"),
        ("chat_id", serde_json::json!(123456789), "123456789"),
        ("open_id", serde_json::json!("ou_123"), "ou_123"),
        ("user", serde_json::json!("u_123"), "u_123"),
        ("receive_id", serde_json::json!("rid_123"), "rid_123"),
    ] {
        let mut input = serde_json::Map::new();
        input.insert(field.to_string(), value);
        assert_eq!(
            send_message_target(&Value::Object(input)).as_deref(),
            Some(expected),
            "recipient alias {field} should be accepted"
        );
    }
}

#[test]
fn send_message_aliases_skip_empty_preferred_values() {
    assert_eq!(
        send_message_target(&serde_json::json!({"to": " ", "chat_id": 42})).as_deref(),
        Some("42")
    );
    assert_eq!(
        send_message_text(&serde_json::json!({"message": " ", "text": "hello"})).as_deref(),
        Some("hello")
    );
}

#[test]
fn telegram_connector_actions_use_subscriber_fallback() {
    assert!(is_telegram_connector("telegram-login"));
    assert!(is_telegram_action("vote_poll"));
    assert!(is_telegram_action("update_group_title"));
    assert!(is_telegram_action("send_story"));
    assert!(!is_telegram_action("send_message"));
}

#[test]
fn telegram_subscriber_id_tracks_connection_slug() {
    assert_eq!(telegram_subscriber_id("telegram-login"), "telegram-user");
    assert_eq!(telegram_subscriber_id("tg-alt"), "tg-alt");
    assert!(validate_telegram_connection_slug("tg-alt").is_ok());
    assert!(validate_telegram_connection_slug("TG Alt").is_err());
}

#[test]
fn telegram_auth_checker_retries_transient_false_response() {
    let runtime = test_subscription_runtime();
    let subscriber_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        subscriber_dir.path().join("manifest.toml"),
        r#"manifest_version = 1
id = "telegram-user"
kind = "subscriber"
topic = "telegram-user"

[run]
cmd = ["sh", "run.sh"]

[state]
dir = "state"
"#,
    )
    .unwrap();
    std::fs::write(
        subscriber_dir.path().join("run.sh"),
        r#"mkdir -p "$PUFFER_SKILL_STATE_DIR"
count_file="$PUFFER_SKILL_STATE_DIR/auth-count"
while IFS= read -r _line; do
  count="$(cat "$count_file" 2>/dev/null || printf 0)"
  if [ "$count" = "0" ]; then
    printf 1 > "$count_file"
    printf '%s\n' '{"topic":"telegram-user","kind":"auth_ok","control":true,"payload":{"ok":false,"authenticated":false}}'
  else
    printf '%s\n' '{"topic":"telegram-user","kind":"auth_ok","control":true,"payload":{"ok":true,"authenticated":true}}'
    exit 0
  fi
done
"#,
    )
    .unwrap();
    let manifest = puffer_subscriber_runtime::Manifest::load(subscriber_dir.path()).unwrap();
    runtime.manager().start_subscriber(manifest).unwrap();

    let checker = TelegramConnectionAuthChecker;
    let result = puffer_subscriptions::ConnectionAuthChecker::check(
        &checker,
        &runtime.manager(),
        &telegram_login_template(),
        "telegram-user",
    )
    .unwrap();

    assert_eq!(result, Some(ConnectionAuthStatus::Healthy));
    runtime.shutdown();
}

#[test]
fn autostart_topics_skip_paused_bindings() {
    let runtime = test_subscription_runtime();
    runtime
        .manager()
        .store()
        .create(workflow_binding(
            "paused",
            "work-email",
            Some("email"),
            WorkflowBindingStatus::Paused,
        ))
        .unwrap();
    let paths = temp_config_paths();

    assert!(autostart_topics(&runtime.manager(), &paths).is_empty());

    runtime.shutdown();
}

#[test]
fn autostart_topics_use_manifest_connector_topic_for_legacy_subscribers() {
    let runtime = test_subscription_runtime();
    runtime
        .manager()
        .connection_store()
        .create(puffer_subscriptions::ConnectionRecord::authenticated(
            "work-email",
            "email",
            "Work email",
        ))
        .unwrap();
    runtime
        .manager()
        .store()
        .create(workflow_binding(
            "email-workflow",
            "work-email",
            Some("email"),
            WorkflowBindingStatus::Enabled,
        ))
        .unwrap();
    let paths = temp_config_paths();
    let email_dir = paths
        .builtin_resources_dir
        .join("subscribers")
        .join("email");
    std::fs::create_dir_all(&email_dir).unwrap();
    std::fs::write(email_dir.join("manifest.toml"), "").unwrap();

    assert_eq!(autostart_topics(&runtime.manager(), &paths), vec!["email"]);

    runtime.shutdown();
}

#[test]
fn autostart_topics_use_connection_topic_for_configured_subscribers() {
    let runtime = test_subscription_runtime();
    runtime
        .manager()
        .connection_store()
        .create(puffer_subscriptions::ConnectionRecord::authenticated(
            "personal", "shared", "Personal",
        ))
        .unwrap();
    runtime
        .manager()
        .connector_store()
        .upsert(configured_subscriber_template())
        .unwrap();
    runtime
        .manager()
        .store()
        .create(workflow_binding(
            "shared-workflow",
            "personal",
            Some("shared"),
            WorkflowBindingStatus::Enabled,
        ))
        .unwrap();
    let paths = temp_config_paths();
    let manifest_dir = paths
        .builtin_resources_dir
        .join("subscribers")
        .join("shared-login");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::write(manifest_dir.join("manifest.toml"), "").unwrap();

    assert_eq!(
        autostart_topics(&runtime.manager(), &paths),
        vec!["personal"]
    );

    runtime.shutdown();
}

#[test]
fn builtin_auth_checker_reports_gmail_browser_auth_required_state() {
    let runtime = test_subscription_runtime();
    let paths = temp_config_paths();
    crate::gmail_browser::save_config(
        &paths,
        &paths.workspace_root,
        "work",
        vec!["me@example.com".to_string()],
    )
    .unwrap();
    let checker = BuiltinConnectionAuthChecker {
        paths: paths.clone(),
    };
    let template = gmail_browser_template();

    assert_eq!(
        puffer_subscriptions::ConnectionAuthChecker::check(
            &checker,
            &runtime.manager(),
            &template,
            "work"
        )
        .unwrap(),
        Some(ConnectionAuthStatus::Healthy)
    );

    let dir = crate::gmail_browser::state_dir(&paths, "work");
    std::fs::write(
        dir.join("auth_state.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "auth_required_accounts": {
                "me@example.com": {
                    "url": "https://accounts.google.com/signin",
                    "title": "Sign in - Google Accounts",
                    "updated_at_ms": 1
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    assert_eq!(
        puffer_subscriptions::ConnectionAuthChecker::check(
            &checker,
            &runtime.manager(),
            &template,
            "work"
        )
        .unwrap(),
        Some(ConnectionAuthStatus::Broken)
    );

    runtime.shutdown();
}

#[test]
fn autostart_manifest_instantiates_connector_subscriber_without_connection_row() {
    let runtime = test_subscription_runtime();
    runtime
        .manager()
        .connector_store()
        .upsert(configured_subscriber_template())
        .unwrap();
    runtime
        .manager()
        .store()
        .create(workflow_binding(
            "shared-workflow",
            "personal",
            Some("shared"),
            WorkflowBindingStatus::Enabled,
        ))
        .unwrap();
    let paths = temp_config_paths();
    let manifest_dir = paths
        .builtin_resources_dir
        .join("subscribers")
        .join("shared-login");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::write(
        manifest_dir.join("manifest.toml"),
        r#"manifest_version = 1
id = "shared-login"
kind = "subscriber"

[run]
cmd = ["puffer", "__subscriber", "shared-login"]

[state]
dir = "state"
"#,
    )
    .unwrap();

    let manifest = autostart_manifest(&runtime.manager(), &paths, "personal")
        .unwrap()
        .unwrap();

    assert_eq!(manifest.spec.id, "personal");
    assert_eq!(manifest.topic(), "personal");
    assert_eq!(
        manifest.spec.state.unwrap().dir,
        paths
            .user_config_dir
            .join("shared-accounts/personal")
            .to_string_lossy()
    );

    runtime.shutdown();
}

fn workflow_binding(
    slug: &str,
    connection_slug: &str,
    connector_slug: Option<&str>,
    status: WorkflowBindingStatus,
) -> WorkflowBindingSpec {
    WorkflowBindingSpec {
        slug: slug.to_string(),
        description: String::new(),
        connection_slug: connection_slug.to_string(),
        connector_slug: connector_slug.map(ToString::to_string),
        status,
        filter: None,
        ignore_filters: Vec::new(),
        contact_ids: Vec::new(),
        classify_prompt: None,
        classify_model: None,
        action: ActionSpec::RunWorkflow {
            slug: "demo".to_string(),
        },
        created_at_ms: 0,
    }
}

fn configured_subscriber_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "shared".to_string(),
        description: "Shared connector".to_string(),
        skill: "shared".to_string(),
        binary: "shared".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: Some(ConnectorSubscriberTemplate {
            manifest_slug: "shared-login".to_string(),
            state_root: Some("shared-accounts".to_string()),
            display_name: Some("Shared".to_string()),
        }),
        output_schema: Value::Null,
        actions: BTreeMap::new(),
    }
}

fn gmail_browser_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: crate::gmail_browser::CONNECTOR_SLUG.to_string(),
        description: "Gmail browser".to_string(),
        skill: "gmail".to_string(),
        binary: "gmail".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: Value::Null,
        actions: BTreeMap::new(),
    }
}

fn telegram_login_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "telegram-login".to_string(),
        description: "Telegram login".to_string(),
        skill: "telegram".to_string(),
        binary: "telegram".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: true,
        subscriber: Some(ConnectorSubscriberTemplate {
            manifest_slug: "telegram-user".to_string(),
            state_root: Some("telegram-accounts".to_string()),
            display_name: Some("Telegram".to_string()),
        }),
        output_schema: Value::Null,
        actions: BTreeMap::new(),
    }
}

fn temp_config_paths() -> ConfigPaths {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    ConfigPaths {
        workspace_root: root.join("workspace"),
        workspace_config_dir: root.join("workspace").join(".puffer"),
        user_config_dir: root.join("home").join(".puffer"),
        builtin_resources_dir: root.join("resources"),
    }
}
