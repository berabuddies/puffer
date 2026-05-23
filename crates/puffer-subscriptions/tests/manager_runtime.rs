use puffer_subscriber_runtime::{Manifest, SubscriberCommand};
use puffer_subscriptions::SubscriptionManagerBuilder;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn send_command_and_wait_can_run_inside_manager_runtime() {
    let temp = tempfile::tempdir().unwrap();
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
printf '%s\n' '{"topic":"wait-topic","kind":"send_complete","control":true,"payload":{"peer":"@alice"}}'
"#,
    )
    .unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    let manager = Arc::new(
        SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
            .build(runtime.handle().clone())
            .unwrap(),
    );
    let manifest = Manifest::load(&subscriber_dir).unwrap();

    manager.start_subscriber(manifest).unwrap();
    let manager_for_task = manager.clone();
    let envelope = runtime
        .block_on(async move {
            manager_for_task.send_command_and_wait(
                "wait-subscriber",
                "wait-topic",
                &SubscriberCommand::Custom {
                    op: "ping".into(),
                    args: Value::Null,
                },
                &["send_complete"],
                Duration::from_secs(2),
            )
        })
        .unwrap();

    assert_eq!(envelope.event.kind, "send_complete");
    assert!(envelope.event.control);
    assert_eq!(envelope.event.payload["peer"], "@alice");

    manager.shutdown();
}
