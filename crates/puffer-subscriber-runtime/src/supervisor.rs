//! Subscriber process supervisor.
//!
//! Owns the child process lifecycle: spawn, read stdout into the event bus,
//! expose stdin as a [`CommandSender`], mirror stderr into tracing, and on
//! exit apply an exponential-backoff restart loop until explicitly stopped.
//!
//! The supervisor runs entirely inside the Tokio runtime it is spawned on.
//! Callers hold a [`SubscriberHandle`] which exposes the subscriber id, a
//! command sender for control messages, and a shutdown trigger.

use crate::bus::EventBus;
use crate::codec::read_lines;
use crate::command::CommandSender;
use crate::event::{Event, EventEnvelope};
use crate::manifest::Manifest;
use anyhow::{Context, Result};
use std::process::Stdio;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Configuration for how a single subscriber is supervised.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Starting backoff when the child exits; doubled on repeated failures
    /// up to `max_backoff`.
    pub min_backoff: Duration,
    /// Upper bound on the backoff duration.
    pub max_backoff: Duration,
    /// Whether to automatically restart the child when it exits. Set to
    /// false for one-shot subscribers (most shouldn't be).
    pub restart_on_exit: bool,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            min_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            restart_on_exit: true,
        }
    }
}

/// Handle returned by [`SubscriberSupervisor::spawn`]. Holds the subscriber
/// id, the control channel for stdin commands, and a shutdown trigger.
pub struct SubscriberHandle {
    /// Subscriber manifest id.
    pub id: String,
    /// Control channel: send [`crate::SubscriberCommand`] values to the
    /// child's stdin.
    pub commands: CommandSender,
    shutdown_tx: watch::Sender<bool>,
    join: Option<JoinHandle<()>>,
}

impl SubscriberHandle {
    /// Fires the shutdown signal and awaits supervisor task exit.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.join.take() {
            let _ = handle.await;
        }
    }
}

/// Starts subscribers based on their [`Manifest`]. One static method — the
/// supervisor carries no per-subscriber state beyond what's captured in the
/// spawned task closure.
pub struct SubscriberSupervisor;

impl SubscriberSupervisor {
    /// Spawns the subscriber described by `manifest`, wiring stdout onto
    /// `bus`, exposing stdin via the returned handle's [`CommandSender`],
    /// and applying the restart policy in `config`.
    pub fn spawn(
        manifest: Manifest,
        bus: EventBus,
        config: SupervisorConfig,
    ) -> Result<SubscriberHandle> {
        let state_dir = manifest
            .ensure_state_dir()
            .with_context(|| format!("failed to create state dir for `{}`", manifest.spec.id))?;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let id = manifest.spec.id.clone();
        let topic = manifest.topic().to_string();
        let commands = CommandSender::disconnected();

        let commands_for_task = commands.clone();
        let join = tokio::spawn(run_loop(
            manifest,
            topic,
            state_dir,
            bus,
            commands_for_task,
            shutdown_rx,
            config,
        ));

        Ok(SubscriberHandle {
            id,
            commands,
            shutdown_tx,
            join: Some(join),
        })
    }
}

async fn run_loop(
    manifest: Manifest,
    topic: String,
    state_dir: Option<std::path::PathBuf>,
    bus: EventBus,
    commands: CommandSender,
    mut shutdown_rx: watch::Receiver<bool>,
    config: SupervisorConfig,
) {
    let id = manifest.spec.id.clone();
    let mut backoff = config.min_backoff;
    loop {
        if *shutdown_rx.borrow() {
            break;
        }
        match spawn_once(&manifest, &topic, state_dir.as_deref(), &bus, &commands).await {
            Ok(exit_status) => {
                tracing::info!(%id, code = ?exit_status, "subscriber exited");
                if !config.restart_on_exit {
                    break;
                }
            }
            Err(error) => {
                tracing::warn!(%id, %error, "subscriber spawn failed");
                if !config.restart_on_exit {
                    break;
                }
            }
        }
        commands.replace(None).await;
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown_rx.changed() => break,
        }
        backoff = (backoff * 2).min(config.max_backoff);
    }
    commands.replace(None).await;
}

async fn spawn_once(
    manifest: &Manifest,
    topic: &str,
    state_dir: Option<&std::path::Path>,
    bus: &EventBus,
    commands: &CommandSender,
) -> Result<std::process::ExitStatus> {
    let program = &manifest.spec.run.cmd[0];
    let args = &manifest.spec.run.cmd[1..];
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(&manifest.dir)
        .env("PUFFER_SKILL_ID", &manifest.spec.id)
        .env("PUFFER_SKILL_TOPIC", topic)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = state_dir {
        cmd.env("PUFFER_SKILL_STATE_DIR", dir);
    }
    for entry in &manifest.spec.run.env {
        cmd.env(&entry.name, &entry.value);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn subscriber `{}`", manifest.spec.id))?;
    let stdout = child.stdout.take().context("child stdout missing")?;
    let stderr = child.stderr.take().context("child stderr missing")?;
    if let Some(stdin) = child.stdin.take() {
        commands.replace(Some(stdin)).await;
    }

    let subscriber_id = manifest.spec.id.clone();
    let default_topic = topic.to_string();
    let bus_for_stdout = bus.clone();
    let stdout_task = tokio::spawn(async move {
        let mut rx = read_lines(stdout);
        while let Some(line) = rx.recv().await {
            match serde_json::from_str::<Event>(&line) {
                Ok(mut event) => {
                    if event.topic.is_empty() {
                        event.topic = default_topic.clone();
                    }
                    let now_ms = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
                    let envelope = EventEnvelope {
                        envelope_id: uuid::Uuid::new_v4().to_string(),
                        subscriber_id: subscriber_id.clone(),
                        received_at_ms: now_ms,
                        event,
                    };
                    bus_for_stdout.publish(envelope);
                }
                Err(error) => {
                    tracing::warn!(
                        %subscriber_id,
                        %error,
                        line = %line.chars().take(256).collect::<String>(),
                        "subscriber emitted invalid ndjson line"
                    );
                }
            }
        }
    });

    let stderr_id = manifest.spec.id.clone();
    let stderr_task = tokio::spawn(async move {
        let mut rx = read_lines(stderr);
        while let Some(line) = rx.recv().await {
            tracing::info!(subscriber = %stderr_id, "{}", line);
        }
    });

    let status = child.wait().await.context("waiting on subscriber exit")?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    Ok(status)
}
