//! Action dispatchers — what happens to an event after it passes the
//! prefilter and classifier.

use crate::spec::{render_template, ActionSpec};
use anyhow::{Context, Result};
use puffer_subscriber_runtime::EventEnvelope;
use rusqlite::{params, Connection};
use serde_json::json;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static GLOBAL_OUTBOUND: OnceLock<Arc<dyn Outbound>> = OnceLock::new();
static GLOBAL_WORKFLOW_RUNNER: OnceLock<Arc<dyn WorkflowActionRunner>> = OnceLock::new();

/// Installs the process-wide outbound implementation. Returns
/// `Err(_)` if a different outbound has already been installed.
pub fn install_outbound(outbound: Arc<dyn Outbound>) -> Result<()> {
    GLOBAL_OUTBOUND
        .set(outbound)
        .map_err(|_| anyhow::anyhow!("outbound already installed"))
}

/// Installs the process-wide workflow action runner. Returns `Err(_)` if
/// a different runner has already been installed.
pub fn install_workflow_runner(runner: Arc<dyn WorkflowActionRunner>) -> Result<()> {
    GLOBAL_WORKFLOW_RUNNER
        .set(runner)
        .map_err(|_| anyhow::anyhow!("workflow runner already installed"))
}

fn global_outbound() -> Option<Arc<dyn Outbound>> {
    GLOBAL_OUTBOUND.get().cloned()
}

fn global_workflow_runner() -> Option<Arc<dyn WorkflowActionRunner>> {
    GLOBAL_WORKFLOW_RUNNER.get().cloned()
}

/// Outcome of an action invocation. The router records the success bit
/// and a short message for diagnostics.
#[derive(Debug, Clone)]
pub struct ActionResult {
    /// Whether the action succeeded.
    pub success: bool,
    /// One-line summary suitable for logs and `/subscriptions status`.
    pub summary: String,
}

/// Trait for delivering an outbound message through whichever subscriber
/// is responsible for the named platform. The MVP impl, installed by
/// `puffer-cli`, routes `("telegram", peer, text)` to the running
/// `telegram-user` subscriber via [`puffer_subscriber_runtime::SubscriberCommand::SendMessage`].
///
/// Implementations are synchronous from the dispatcher's view; if they
/// need to drive a Tokio runtime they own that internally.
pub trait Outbound: Send + Sync {
    /// Sends `text` to `target` on `platform`. Returns a one-line
    /// human-readable summary on success.
    fn send(&self, platform: &str, target: &str, text: &str) -> Result<String>;
}

/// Trait for triggering native Puffer workflows from subscription actions.
pub trait WorkflowActionRunner: Send + Sync {
    /// Runs `slug` with `trigger` as the interpolation payload.
    fn run_workflow(&self, slug: &str, trigger: serde_json::Value) -> Result<String>;
}

/// Dispatcher trait — one method per invocation. Implementations may keep
/// connection pools, etc., behind interior mutability.
pub trait ActionDispatcher: Send + Sync {
    /// Executes `action` for `envelope` and returns the result.
    fn dispatch(&self, action: &ActionSpec, envelope: &EventEnvelope) -> ActionResult;
}

/// Built-in dispatcher for the MVP action set.
///
/// `sqlite_insert` connects to the configured database (creating it on
/// demand), creates a default table schema if missing, and inserts one
/// row per matched event.
///
/// `forward_message` calls into the installed [`Outbound`] impl. When no
/// outbound has been installed (e.g. the binary is running without
/// connector wiring) the action returns a clear error so the agent can
/// surface it to the user.
pub struct BuiltinActionDispatcher {
    sqlite_pool: Mutex<Vec<(PathBuf, Connection)>>,
    storage_root: PathBuf,
    /// Per-instance outbound override, used by tests. Production code
    /// installs the outbound process-globally via [`install_outbound`]
    /// so any dispatcher instance picks it up.
    outbound: OnceLock<Arc<dyn Outbound>>,
    workflow_runner: OnceLock<Arc<dyn WorkflowActionRunner>>,
}

impl Default for BuiltinActionDispatcher {
    fn default() -> Self {
        Self {
            sqlite_pool: Mutex::new(Vec::new()),
            storage_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            outbound: OnceLock::new(),
            workflow_runner: OnceLock::new(),
        }
    }
}

impl BuiltinActionDispatcher {
    /// Constructs an empty dispatcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Constructs a dispatcher rooted at `storage_root` for relative file actions.
    pub fn with_storage_root(storage_root: impl Into<PathBuf>) -> Self {
        Self {
            storage_root: storage_root.into(),
            ..Self::default()
        }
    }

    /// Installs an outbound implementation on this specific dispatcher
    /// instance. Tests use this to inject a recording outbound. Production
    /// code should prefer the process-wide [`install_outbound`].
    pub fn set_outbound(&self, outbound: Arc<dyn Outbound>) {
        let _ = self.outbound.set(outbound);
    }

    /// Installs a workflow runner on this dispatcher instance.
    pub fn set_workflow_runner(&self, runner: Arc<dyn WorkflowActionRunner>) {
        let _ = self.workflow_runner.set(runner);
    }

    fn resolved_outbound(&self) -> Option<Arc<dyn Outbound>> {
        self.outbound.get().cloned().or_else(global_outbound)
    }

    fn resolved_workflow_runner(&self) -> Option<Arc<dyn WorkflowActionRunner>> {
        self.workflow_runner
            .get()
            .cloned()
            .or_else(global_workflow_runner)
    }

    fn sqlite_insert(
        &self,
        path: &str,
        table: &str,
        envelope: &EventEnvelope,
    ) -> Result<ActionResult> {
        let absolute = resolve_sqlite_path(&self.storage_root, path)?;
        let mut pool = self.sqlite_pool.lock().unwrap();
        if !pool.iter().any(|(p, _)| p == &absolute) {
            if let Some(parent) = absolute.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let conn = Connection::open(&absolute)
                .with_context(|| format!("open sqlite database {}", absolute.display()))?;
            pool.push((absolute.clone(), conn));
        }
        let conn = &pool
            .iter()
            .find(|(p, _)| p == &absolute)
            .expect("just inserted")
            .1;
        // The CREATE/INSERT SQL inlines `table` — we validate it as a
        // strict identifier in spec::validate_spec, so this is safe.
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {table} (\
                envelope_id TEXT PRIMARY KEY, \
                received_at_ms INTEGER NOT NULL, \
                subscriber_id TEXT NOT NULL, \
                topic TEXT NOT NULL, \
                kind TEXT NOT NULL, \
                dedup_key TEXT, \
                text TEXT NOT NULL, \
                payload TEXT NOT NULL\
            );"
        ))
        .with_context(|| format!("create table {table}"))?;
        let payload =
            serde_json::to_string(&envelope.event.payload).unwrap_or_else(|_| "null".into());
        conn.execute(
            &format!(
                "INSERT OR IGNORE INTO {table} \
                  (envelope_id, received_at_ms, subscriber_id, topic, kind, dedup_key, text, payload) \
                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            ),
            params![
                envelope.envelope_id,
                envelope.received_at_ms as i64,
                envelope.subscriber_id,
                envelope.event.topic,
                envelope.event.kind,
                envelope.event.dedup_key,
                envelope.event.text,
                payload,
            ],
        )
        .with_context(|| format!("insert into {table}"))?;
        Ok(ActionResult {
            success: true,
            summary: format!("inserted into {} ({})", absolute.display(), table),
        })
    }

    fn forward_message(
        &self,
        platform: &str,
        target: &str,
        template: Option<&str>,
        envelope: &EventEnvelope,
    ) -> ActionResult {
        let rendered = template
            .map(|t| render_template(t, &envelope.event.text, &envelope.event.payload))
            .unwrap_or_else(|| envelope.event.text.clone());
        match self.resolved_outbound() {
            Some(outbound) => match outbound.send(platform, target, &rendered) {
                Ok(summary) => ActionResult {
                    success: true,
                    summary,
                },
                Err(error) => ActionResult {
                    success: false,
                    summary: format!("forward_message to {platform}:{target} failed: {error:#}"),
                },
            },
            None => ActionResult {
                success: false,
                summary: format!(
                    "forward_message: no outbound is installed; spec needs {platform}:{target} but the running puffer process cannot deliver messages"
                ),
            },
        }
    }

    fn run_workflow(&self, slug: &str, envelope: &EventEnvelope) -> ActionResult {
        let trigger = json!({
            "type": "subscription",
            "envelope_id": envelope.envelope_id,
            "subscriber_id": envelope.subscriber_id,
            "received_at_ms": envelope.received_at_ms,
            "topic": envelope.event.topic,
            "kind": envelope.event.kind,
            "dedup_key": envelope.event.dedup_key,
            "text": envelope.event.text,
            "payload": envelope.event.payload,
        });
        match self.resolved_workflow_runner() {
            Some(runner) => match runner.run_workflow(slug, trigger) {
                Ok(summary) => ActionResult {
                    success: true,
                    summary,
                },
                Err(error) => ActionResult {
                    success: false,
                    summary: format!("run_workflow `{slug}` failed: {error:#}"),
                },
            },
            None => ActionResult {
                success: false,
                summary: format!(
                    "run_workflow: no workflow runner is installed; `{slug}` cannot be run"
                ),
            },
        }
    }
}

impl ActionDispatcher for BuiltinActionDispatcher {
    fn dispatch(&self, action: &ActionSpec, envelope: &EventEnvelope) -> ActionResult {
        match action {
            ActionSpec::SqliteInsert { path, table } => {
                match self.sqlite_insert(path, table, envelope) {
                    Ok(result) => result,
                    Err(error) => ActionResult {
                        success: false,
                        summary: format!("sqlite_insert failed: {error:#}"),
                    },
                }
            }
            ActionSpec::ForwardMessage {
                platform,
                target,
                template,
            } => self.forward_message(platform, target, template.as_deref(), envelope),
            ActionSpec::RunWorkflow { slug } => self.run_workflow(slug, envelope),
            ActionSpec::Unknown => ActionResult {
                success: false,
                summary: "action.type unknown — agent wrote a spec this Puffer build cannot run"
                    .into(),
            },
        }
    }
}

fn resolve_sqlite_path(storage_root: &Path, path: &str) -> Result<PathBuf> {
    let raw = path.trim();
    if raw.starts_with("~/") {
        anyhow::bail!("sqlite_insert.path must be relative to subscription storage");
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() || has_parent_component(candidate) {
        anyhow::bail!("sqlite_insert.path must be a safe relative path");
    }
    Ok(storage_root.join(candidate))
}

fn has_parent_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriber_runtime::Event;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    fn envelope(text: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope {
            envelope_id: "env-1".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                dedup_key: None,
                text: text.into(),
                payload,
            },
        }
    }

    #[test]
    fn sqlite_insert_creates_table_and_inserts_row() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("x.db");
        let dispatcher = BuiltinActionDispatcher::with_storage_root(dir.path());
        let action = ActionSpec::SqliteInsert {
            path: "x.db".to_string(),
            table: "ioc_messages".into(),
        };
        let result = dispatcher.dispatch(&action, &envelope("hello", json!({"chat":"@x"})));
        assert!(result.success, "{}", result.summary);
        let conn = Connection::open(&db).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ioc_messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn sqlite_insert_rejects_absolute_and_traversal_paths() {
        let dir = tempdir().unwrap();
        let dispatcher = BuiltinActionDispatcher::with_storage_root(dir.path());
        for path in [
            "/tmp/puffer-subscriptions.db",
            "~/puffer.db",
            "../puffer.db",
        ] {
            let action = ActionSpec::SqliteInsert {
                path: path.to_string(),
                table: "ioc_messages".into(),
            };
            let result = dispatcher.dispatch(&action, &envelope("hello", json!({})));
            assert!(!result.success, "{path} should be rejected");
        }
    }

    struct RecordingOutbound {
        calls: StdMutex<Vec<(String, String, String)>>,
    }

    impl Outbound for RecordingOutbound {
        fn send(&self, platform: &str, target: &str, text: &str) -> Result<String> {
            self.calls.lock().unwrap().push((
                platform.to_string(),
                target.to_string(),
                text.to_string(),
            ));
            Ok(format!("recorded {platform}:{target}"))
        }
    }

    #[test]
    fn forward_message_calls_installed_outbound_with_rendered_template() {
        let dispatcher = BuiltinActionDispatcher::new();
        let outbound = Arc::new(RecordingOutbound {
            calls: StdMutex::new(Vec::new()),
        });
        dispatcher.set_outbound(outbound.clone());
        let action = ActionSpec::ForwardMessage {
            platform: "telegram".into(),
            target: "@hongyi_zhang".into(),
            template: Some("Zara: {{text}} ({{payload.subject}})".into()),
        };
        let result = dispatcher.dispatch(
            &action,
            &envelope("50% off jeans", json!({"subject": "Sale today"})),
        );
        assert!(result.success, "{}", result.summary);
        let calls = outbound.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "telegram");
        assert_eq!(calls[0].1, "@hongyi_zhang");
        assert_eq!(calls[0].2, "Zara: 50% off jeans (Sale today)");
    }

    #[test]
    fn forward_message_without_outbound_reports_actionable_error() {
        let dispatcher = BuiltinActionDispatcher::new();
        let action = ActionSpec::ForwardMessage {
            platform: "telegram".into(),
            target: "@hongyi_zhang".into(),
            template: None,
        };
        let result = dispatcher.dispatch(&action, &envelope("hi", json!({})));
        assert!(!result.success);
        assert!(result.summary.contains("no outbound is installed"));
    }

    struct RecordingWorkflowRunner {
        calls: StdMutex<Vec<(String, serde_json::Value)>>,
    }

    impl WorkflowActionRunner for RecordingWorkflowRunner {
        fn run_workflow(&self, slug: &str, trigger: serde_json::Value) -> Result<String> {
            self.calls.lock().unwrap().push((slug.to_string(), trigger));
            Ok(format!("ran {slug}"))
        }
    }

    #[test]
    fn run_workflow_dispatches_subscription_trigger() {
        let dispatcher = BuiltinActionDispatcher::new();
        let runner = Arc::new(RecordingWorkflowRunner {
            calls: StdMutex::new(Vec::new()),
        });
        dispatcher.set_workflow_runner(runner.clone());
        let action = ActionSpec::RunWorkflow {
            slug: "daily-review".into(),
        };
        let result = dispatcher.dispatch(&action, &envelope("hello", json!({"chat":"@x"})));
        assert!(result.success, "{}", result.summary);
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0].0, "daily-review");
        assert_eq!(calls[0].1["type"], "subscription");
        assert_eq!(calls[0].1["text"], "hello");
    }
}
