//! Wiring that boots the process-wide subscription manager and the
//! built-in subscriber discovery. Mirrors the connector wiring style.
//!
//! The subscription manager owns the in-process event bus, the spec
//! store on disk, the supervised subscriber children, and the router
//! task. Workflow tools (`SubscriptionCreate`, `TelegramLoginStart`, …)
//! reach into it through a `OnceLock` installed here.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::install_subscription_manager;
use puffer_provider_registry::{AuthStore, StoredCredential};
use puffer_subscriber_runtime::{Manifest, SubscriberCommand};
use puffer_subscriptions::{
    install_outbound, ClassifyDecision, Outbound, RemoteClassifier, SubscriptionManager,
    SubscriptionManagerBuilder,
};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::runtime::Runtime;

/// Owned wrapper around the dedicated Tokio runtime used by the
/// subscription manager and its supervised subscribers. Dropping it
/// after [`SubscriptionRuntime::shutdown`] joins all background tasks.
pub(crate) struct SubscriptionRuntime {
    runtime: Option<Runtime>,
    manager: Arc<SubscriptionManager>,
}

impl SubscriptionRuntime {
    /// Returns the manager handle.
    pub(crate) fn manager(&self) -> Arc<SubscriptionManager> {
        self.manager.clone()
    }

    /// Best-effort shutdown: stops router + every subscriber, then drops
    /// the underlying runtime.
    pub(crate) fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        shutdown_manager(&self.manager);
        runtime.shutdown_background();
    }
}

impl Drop for SubscriptionRuntime {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

/// Builds the subscription runtime, installs it as the process-wide
/// manager so workflow tools can find it, and (best-effort) starts the
/// bundled subscribers whose manifests are present on disk.
///
/// Failures are non-fatal: a broken subscription store or missing
/// manifest must not stop the rest of puffer from starting. Errors are
/// printed to stderr and the function still returns the manager
/// (empty) so workflow tools can report a meaningful error.
pub(crate) fn install(
    paths: &ConfigPaths,
    auth_store: &AuthStore,
    anthropic_base_url: Option<&str>,
) -> Result<SubscriptionRuntime> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .thread_name("puffer-subscriptions")
        .build()
        .context("failed to build subscription tokio runtime")?;
    let handle = runtime.handle().clone();

    let store_path = subscriptions_path(paths);
    let mut builder = SubscriptionManagerBuilder::new(&store_path);
    if let Some(classifier) = build_anthropic_classifier(auth_store, anthropic_base_url) {
        builder = builder.with_classifier(classifier);
    }
    let manager = Arc::new(
        builder
            .build(handle)
            .context("failed to build subscription manager")?,
    );

    install_subscription_manager(manager.clone())
        .context("failed to install subscription manager")?;
    install_outbound(Arc::new(ManagerOutbound {
        manager: Arc::downgrade(&manager),
    }))
    .context("failed to install subscription outbound")?;

    autostart_subscribers(&manager, paths);

    Ok(SubscriptionRuntime {
        runtime: Some(runtime),
        manager,
    })
}

fn shutdown_manager(manager: &Arc<SubscriptionManager>) {
    if tokio::runtime::Handle::try_current().is_ok() {
        let manager = manager.clone();
        let _ = std::thread::Builder::new()
            .name("puffer-subscriptions-shutdown".to_string())
            .spawn(move || manager.shutdown())
            .and_then(|thread| {
                thread.join().map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "subscription shutdown thread panicked",
                    )
                })
            });
        return;
    }
    manager.shutdown();
}

const DEFAULT_CLASSIFY_MODEL: &str = "claude-haiku-4-5";

/// Returns a classifier backed by Anthropic's `/v1/messages` endpoint
/// using the API key already in the auth store. Returns `None` when no
/// Anthropic API key is stored — the manager then falls back to the
/// default `NullClassifier`, and subscriptions with `classify_prompt`
/// will simply pass every event.
fn build_anthropic_classifier(
    auth_store: &AuthStore,
    anthropic_base_url: Option<&str>,
) -> Option<Arc<dyn puffer_subscriptions::Classifier>> {
    let key = match auth_store.providers.get("anthropic")? {
        StoredCredential::ApiKey { key } => key.clone(),
        StoredCredential::OAuth(_) => return None,
    };
    let base_url = anthropic_base_url
        .unwrap_or("https://api.anthropic.com")
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .ok()?;
    let callable = move |prompt: &str, model_hint: Option<&str>, text: &str| -> ClassifyDecision {
        let model = parse_model_hint(model_hint).unwrap_or(DEFAULT_CLASSIFY_MODEL);
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 4,
            "system": "You are a strict yes/no classifier. Reply with exactly `yes` or `no` and nothing else.",
            "messages": [{
                "role": "user",
                "content": format!(
                    "Question: {prompt}\n\nMessage:\n{text}\n\nAnswer with exactly 'yes' or 'no'."
                )
            }]
        });
        let request = client
            .post(format!("{base_url}/v1/messages"))
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .json(&body);
        let response = match request.send() {
            Ok(r) => r,
            Err(_) => return ClassifyDecision::Inconclusive,
        };
        let parsed: serde_json::Value = match response.json() {
            Ok(v) => v,
            Err(_) => return ClassifyDecision::Inconclusive,
        };
        decision_from_anthropic(&parsed)
    };
    Some(Arc::new(RemoteClassifier::new(callable)))
}

fn parse_model_hint(hint: Option<&str>) -> Option<&str> {
    let hint = hint?;
    if let Some((provider, model)) = hint.split_once('/') {
        if provider.eq_ignore_ascii_case("anthropic") {
            return Some(model);
        }
        return None;
    }
    Some(hint)
}

fn decision_from_anthropic(parsed: &serde_json::Value) -> ClassifyDecision {
    let text = parsed
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        })
        .and_then(|b| b.get("text").and_then(|t| t.as_str()))
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if text.starts_with('y') {
        ClassifyDecision::Pass
    } else if text.starts_with('n') {
        ClassifyDecision::Reject
    } else {
        ClassifyDecision::Inconclusive
    }
}

/// Outbound impl that translates `(platform, target, text)` action calls
/// into [`SubscriberCommand::SendMessage`] commands routed through the
/// subscriber that owns the platform.
///
/// Today this is a fixed mapping: `"telegram"` -> `"telegram-user"`. The
/// table is intentionally tiny — adding a new platform means adding one
/// entry plus a subscriber that handles `SendMessage`.
struct ManagerOutbound {
    manager: Weak<SubscriptionManager>,
}

impl Outbound for ManagerOutbound {
    fn send(&self, platform: &str, target: &str, text: &str) -> Result<String> {
        let subscriber_id = subscriber_for_platform(platform).ok_or_else(|| {
            anyhow::anyhow!("no subscriber is configured to send messages on platform `{platform}`")
        })?;
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("subscription manager is no longer running"))?;
        manager.send_command(
            subscriber_id,
            &SubscriberCommand::SendMessage {
                peer: target.to_string(),
                text: text.to_string(),
            },
        )?;
        Ok(format!(
            "queued send via {subscriber_id} -> {platform}:{target} ({} bytes)",
            text.len()
        ))
    }
}

fn subscriber_for_platform(platform: &str) -> Option<&'static str> {
    match platform {
        "telegram" | "telegram-user" => Some("telegram-user"),
        "email" => Some("email"),
        _ => None,
    }
}

fn subscriptions_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("subscriptions.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic;

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
        }
    }

    #[test]
    fn subscription_runtime_can_drop_inside_tokio_context() {
        let outer = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("outer runtime");
        let runtime = test_subscription_runtime();
        let result = panic::catch_unwind(|| {
            outer.block_on(async { drop(runtime) });
        });
        assert!(
            result.is_ok(),
            "dropping SubscriptionRuntime inside Tokio must not panic"
        );
    }
}

fn autostart_subscribers(manager: &SubscriptionManager, paths: &ConfigPaths) {
    let mut needed: Vec<String> = match manager.store().list() {
        list => list.into_iter().map(|spec| spec.source_topic).collect(),
    };
    needed.sort();
    needed.dedup();
    for topic in needed {
        match find_subscriber_manifest(paths, &topic) {
            Some(dir) => match Manifest::load(&dir) {
                Ok(manifest) => {
                    if let Err(error) = manager.start_subscriber(manifest) {
                        eprintln!("subscription: failed to start subscriber `{topic}`: {error:#}");
                    }
                }
                Err(error) => {
                    eprintln!("subscription: invalid manifest for `{topic}`: {error}");
                }
            },
            None => {
                eprintln!(
                    "subscription: no manifest installed for source `{topic}` referenced by an existing subscription; events will not flow until one is added"
                );
            }
        }
    }
}

fn find_subscriber_manifest(paths: &ConfigPaths, topic: &str) -> Option<PathBuf> {
    let workspace = paths.workspace_config_dir.join("subscribers").join(topic);
    if workspace.join("manifest.toml").exists() {
        return Some(workspace);
    }
    let user = paths.user_config_dir.join("subscribers").join(topic);
    if user.join("manifest.toml").exists() {
        return Some(user);
    }
    let bundled = bundled_resources_root().join("subscribers").join(topic);
    if bundled.join("manifest.toml").exists() {
        return Some(bundled);
    }
    None
}

fn bundled_resources_root() -> PathBuf {
    if let Some(env) = std::env::var_os("PUFFER_RESOURCES_DIR") {
        return PathBuf::from(env);
    }
    PathBuf::from("resources")
}
