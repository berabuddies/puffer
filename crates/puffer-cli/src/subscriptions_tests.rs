use super::*;
use puffer_subscriptions::{
    ActionSpec, ConnectorSubscriberTemplate, ConnectorTemplate, WorkflowBindingSpec,
    WorkflowBindingStatus,
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
