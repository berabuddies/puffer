//! Supervision for connector `subscribe` processes.

use crate::catalog::ConnectorTemplate;
use crate::connection::{ConnectionState, ConnectionStore};
use crate::protocol::{ConnectorSubscribeCommand, ConnectorSubscribeFrame};
use anyhow::{Context, Result};
use puffer_subscriber_runtime::{read_lines, write_line, Event, EventBus, EventEnvelope};
use serde_json::Value;
use std::process::Stdio;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Synchronous event processor used by connector streams before acking.
pub trait ConnectorEventProcessor: Send + Sync {
    /// Processes one connector event. Returning an error prevents cursor ack.
    fn process_connector_event(
        &self,
        connector_slug: &str,
        connection_slug: &str,
        envelope: &EventEnvelope,
    ) -> Result<()>;
}

/// Handle for one running connector subscription process.
pub struct ConnectorStreamHandle {
    /// Connection slug owned by this stream.
    pub connection_slug: String,
    shutdown_tx: watch::Sender<bool>,
    join: Option<JoinHandle<()>>,
}

impl ConnectorStreamHandle {
    /// Spawns a connector `subscribe` process when the template has a command.
    pub async fn spawn(
        template: ConnectorTemplate,
        connection_slug: String,
        cursor: Option<String>,
        bus: EventBus,
        connection_store: Arc<ConnectionStore>,
        processor: Option<Arc<dyn ConnectorEventProcessor>>,
    ) -> Result<Option<Self>> {
        let Some(argv) = template.command_argv() else {
            return Ok(None);
        };
        let Some((program, fixed_args)) = argv.split_first() else {
            return Ok(None);
        };
        let mut command = Command::new(program);
        command
            .args(fixed_args)
            .arg("subscribe")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("spawn connector stream `{}`", template.slug))?;
        let mut stdin = child
            .stdin
            .take()
            .context("connector stream stdin missing")?;
        let stdout = child
            .stdout
            .take()
            .context("connector stream stdout missing")?;
        let stderr = child
            .stderr
            .take()
            .context("connector stream stderr missing")?;
        write_line(
            &mut stdin,
            &ConnectorSubscribeCommand::Subscribe {
                connection: connection_slug.clone(),
                cursor,
            },
        )
        .await?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let join = tokio::spawn(run_stream(
            template.slug,
            connection_slug.clone(),
            stdin,
            stdout,
            stderr,
            child,
            bus,
            connection_store,
            processor,
            shutdown_rx,
        ));
        Ok(Some(Self {
            connection_slug,
            shutdown_tx,
            join: Some(join),
        }))
    }

    /// Stops the connector stream by signalling shutdown and awaiting exit.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }

    /// Returns whether the underlying stream task has reached a terminal state.
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_some_and(|join| join.is_finished())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_stream(
    connector_slug: String,
    connection_slug: String,
    mut stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    mut child: tokio::process::Child,
    bus: EventBus,
    connection_store: Arc<ConnectionStore>,
    processor: Option<Arc<dyn ConnectorEventProcessor>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut stdout_rx = read_lines(stdout);
    let mut stderr_rx = read_lines(stderr);
    let mut shutdown_requested = false;
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                shutdown_requested = true;
                let _ = child.start_kill();
                break;
            }
            maybe = stdout_rx.recv() => {
                let Some(line) = maybe else { break; };
                match serde_json::from_str::<ConnectorSubscribeFrame>(&line) {
                    Ok(frame) => handle_frame(
                        &connector_slug,
                        &connection_slug,
                        frame,
                        &bus,
                        &connection_store,
                        &mut stdin,
                        processor.as_deref(),
                    ).await,
                    Err(error) => tracing::warn!(
                        connector = %connector_slug,
                        connection = %connection_slug,
                        %error,
                        line = %line.chars().take(256).collect::<String>(),
                        "connector stream emitted invalid frame"
                    ),
                }
            }
            maybe = stderr_rx.recv() => {
                if let Some(line) = maybe {
                    tracing::info!(
                        connector = %connector_slug,
                        connection = %connection_slug,
                        "{}",
                        line
                    );
                }
            }
        }
    }
    let _ = child.wait().await;
    if !shutdown_requested {
        let _ = connection_store.update(&connection_slug, |record| {
            if record.state != ConnectionState::Disabled {
                record.state = ConnectionState::Degraded;
            }
        });
    }
}

async fn handle_frame(
    connector_slug: &str,
    connection_slug: &str,
    frame: ConnectorSubscribeFrame,
    bus: &EventBus,
    connection_store: &ConnectionStore,
    stdin: &mut tokio::process::ChildStdin,
    processor: Option<&dyn ConnectorEventProcessor>,
) {
    match frame {
        ConnectorSubscribeFrame::Event {
            id,
            cursor,
            payload,
        } => {
            let now_ms = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
            let envelope = EventEnvelope {
                envelope_id: uuid::Uuid::new_v4().to_string(),
                subscriber_id: connection_slug.to_string(),
                received_at_ms: now_ms,
                event: Event {
                    topic: connection_slug.to_string(),
                    kind: payload_kind(&payload),
                    control: false,
                    dedup_key: Some(id.clone()),
                    text: payload_text(&payload),
                    payload,
                },
            };
            let processed = match processor {
                Some(processor) => {
                    processor.process_connector_event(connector_slug, connection_slug, &envelope)
                }
                None => {
                    bus.publish(envelope);
                    Ok(())
                }
            };
            match processed {
                Ok(()) => {
                    let _ = connection_store.update(connection_slug, |record| {
                        record.cursor = Some(cursor.clone());
                    });
                    let _ = write_line(
                        stdin,
                        &ConnectorSubscribeCommand::Ack {
                            cursor,
                            event_id: id,
                        },
                    )
                    .await;
                }
                Err(error) => {
                    tracing::warn!(
                        connector = %connector_slug,
                        connection = %connection_slug,
                        %error,
                        "connector event processing failed; leaving cursor unacked"
                    );
                }
            }
        }
        ConnectorSubscribeFrame::Checkpoint { cursor } => {
            let _ = connection_store.update(connection_slug, |record| {
                record.cursor = Some(cursor);
            });
        }
        ConnectorSubscribeFrame::Health { status, detail } => {
            let degraded = status != "ok";
            let _ = connection_store.update(connection_slug, |record| {
                if degraded {
                    record.state = ConnectionState::Degraded;
                } else if record.state == ConnectionState::Degraded {
                    record.state = ConnectionState::Authenticated;
                    record.set_has_consumer(record.has_consumer);
                }
            });
            if degraded {
                tracing::warn!(
                    connector = %connector_slug,
                    connection = %connection_slug,
                    status = %status,
                    detail = ?detail,
                    "connector stream health degraded"
                );
            }
        }
    }
}

fn payload_kind(payload: &Value) -> String {
    payload
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("message")
        .to_string()
}

fn payload_text(payload: &Value) -> String {
    payload
        .get("message")
        .or_else(|| payload.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ConnectorTemplate;
    use crate::connection::ConnectionRecord;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn template(script: &std::path::Path) -> ConnectorTemplate {
        ConnectorTemplate {
            slug: "demo".into(),
            description: "demo".into(),
            skill: "demo".into(),
            binary: script.display().to_string(),
            command: vec!["sh".into(), script.display().to_string()],
            requires_auth: true,
            can_subscribe: true,
            can_proxy_agent: false,
            subscriber: None,
            output_schema: Value::Null,
            actions: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn subscribe_process_publishes_events_and_persists_cursor() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("connector.sh");
        std::fs::write(
            &script,
            r#"IFS= read -r _subscribe
printf '%s\n' '{"type":"event","id":"e1","cursor":"c1","payload":{"message":"gm","from":"Tony"}}'
IFS= read -r _ack
"#,
        )
        .unwrap();
        let store = Arc::new(ConnectionStore::load(temp.path().join("connections.json")).unwrap());
        store
            .create(ConnectionRecord::authenticated("conn", "demo", "demo"))
            .unwrap();
        let bus = EventBus::new();
        let mut rx = bus.subscribe_topic("conn");
        let handle = ConnectorStreamHandle::spawn(
            template(&script),
            "conn".into(),
            None,
            bus,
            store.clone(),
            None,
        )
        .await
        .unwrap()
        .unwrap();

        let envelope = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(envelope.event.text, "gm");
        assert_eq!(store.get("conn").unwrap().cursor.as_deref(), Some("c1"));

        handle.shutdown().await;
    }

    struct RecordingProcessor {
        calls: AtomicUsize,
    }

    impl ConnectorEventProcessor for RecordingProcessor {
        fn process_connector_event(
            &self,
            _connector_slug: &str,
            _connection_slug: &str,
            envelope: &EventEnvelope,
        ) -> Result<()> {
            assert_eq!(envelope.event.text, "gm");
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn subscribe_process_acks_after_processor_success() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("connector.sh");
        let ack_path = temp.path().join("ack.txt");
        std::fs::write(
            &script,
            format!(
                r#"IFS= read -r _subscribe
printf '%s\n' '{{"type":"event","id":"e1","cursor":"c1","payload":{{"message":"gm"}}}}'
IFS= read -r ack
printf '%s\n' "$ack" > '{}'
"#,
                ack_path.display()
            ),
        )
        .unwrap();
        let store = Arc::new(ConnectionStore::load(temp.path().join("connections.json")).unwrap());
        store
            .create(ConnectionRecord::authenticated("conn", "demo", "demo"))
            .unwrap();
        let processor = Arc::new(RecordingProcessor {
            calls: AtomicUsize::new(0),
        });
        let handle = ConnectorStreamHandle::spawn(
            template(&script),
            "conn".into(),
            None,
            EventBus::new(),
            store.clone(),
            Some(processor.clone()),
        )
        .await
        .unwrap()
        .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if ack_path.exists() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(processor.calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.get("conn").unwrap().cursor.as_deref(), Some("c1"));
        assert!(std::fs::read_to_string(ack_path)
            .unwrap()
            .contains("\"op\":\"ack\""));

        handle.shutdown().await;
    }
}
