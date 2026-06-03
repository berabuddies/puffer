use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

/// Persistent mapping from an external connector identity to an agent target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentProxyBinding {
    /// Connection that owns the external chat.
    pub connection_slug: String,
    /// External user or chat id.
    pub external_principal: String,
    /// Connector address used for replies, such as a Telegram chat id or
    /// Slack channel id. Defaults to `external_principal` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_target: Option<String>,
    /// Agent, team, or wildcard target selected by `/connect`.
    pub agent_target: String,
    /// Whether the binding is currently enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// Errors returned by [`AgentProxyStore`].
#[derive(Debug, Error)]
pub enum AgentProxyStoreError {
    /// I/O failed while reading or writing proxy state.
    #[error("agent proxy store io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON failed to parse or encode.
    #[error("agent proxy store json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Proxy binding input is invalid.
    #[error("invalid agent proxy binding: {0}")]
    Invalid(String),
    /// Proxy binding was not found.
    #[error("agent proxy binding `{0}` / `{1}` not found")]
    NotFound(String, String),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentProxyStoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    bindings: Vec<AgentProxyBinding>,
}

/// File-backed store for connector-to-agent proxy bindings.
pub struct AgentProxyStore {
    path: PathBuf,
    inner: Mutex<AgentProxyStoreFile>,
}

impl AgentProxyStore {
    /// Loads a proxy store. Missing files are treated as empty.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, AgentProxyStoreError> {
        let path = path.into();
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                AgentProxyStoreFile::default()
            } else {
                serde_json::from_str(&raw)?
            }
        } else {
            AgentProxyStoreFile::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Returns all proxy bindings sorted by connection and external principal.
    pub fn list(&self) -> Vec<AgentProxyBinding> {
        let mut bindings = self.inner.lock().unwrap().bindings.clone();
        bindings.sort_by(|a, b| {
            a.connection_slug
                .cmp(&b.connection_slug)
                .then_with(|| a.external_principal.cmp(&b.external_principal))
        });
        bindings
    }

    /// Returns one proxy binding by connection and external principal.
    pub fn get(
        &self,
        connection_slug: &str,
        external_principal: &str,
    ) -> Option<AgentProxyBinding> {
        self.inner
            .lock()
            .unwrap()
            .bindings
            .iter()
            .find(|binding| {
                binding.connection_slug == connection_slug
                    && binding.external_principal == external_principal
            })
            .cloned()
    }

    /// Inserts or replaces one proxy binding.
    pub fn upsert(
        &self,
        binding: AgentProxyBinding,
    ) -> Result<AgentProxyBinding, AgentProxyStoreError> {
        validate_proxy_binding(&binding)?;
        let mut guard = self.inner.lock().unwrap();
        guard.bindings.retain(|existing| {
            existing.connection_slug != binding.connection_slug
                || existing.external_principal != binding.external_principal
        });
        guard.bindings.push(binding.clone());
        guard.bindings.sort_by(|a, b| {
            a.connection_slug
                .cmp(&b.connection_slug)
                .then_with(|| a.external_principal.cmp(&b.external_principal))
        });
        write_proxy_store(&self.path, &*guard)?;
        Ok(binding)
    }

    /// Deletes one proxy binding.
    pub fn delete(
        &self,
        connection_slug: &str,
        external_principal: &str,
    ) -> Result<(), AgentProxyStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let before = guard.bindings.len();
        guard.bindings.retain(|binding| {
            binding.connection_slug != connection_slug
                || binding.external_principal != external_principal
        });
        if guard.bindings.len() == before {
            return Err(AgentProxyStoreError::NotFound(
                connection_slug.to_string(),
                external_principal.to_string(),
            ));
        }
        write_proxy_store(&self.path, &*guard)
    }

    /// Returns whether a connection has at least one enabled proxy binding.
    pub fn has_enabled_consumer(&self, connection_slug: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .bindings
            .iter()
            .any(|binding| binding.connection_slug == connection_slug && binding.enabled)
    }
}

/// Decision returned by an agent proxy implementation for one inbound event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentProxyDecision {
    /// Ignore the event.
    Ignore,
    /// Send a connector action without involving an agent.
    ConnectorAction {
        /// Connector action slug.
        action: String,
        /// Connector action input.
        input: Value,
    },
    /// Route the event text to an agent target.
    RouteToAgent {
        /// Agent or team target.
        target: String,
        /// Text to send to the agent.
        message: String,
        /// Binding that owns the external reply route.
        binding: AgentProxyBinding,
    },
    /// Store or update a persistent proxy binding.
    BindAgent {
        /// Binding to persist.
        binding: AgentProxyBinding,
        /// Optional connector reply.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply: Option<Value>,
    },
}

/// Interface for connectors that can proxy external chats to agents.
pub trait AgentProxy: Send + Sync {
    /// Handles connection/setup commands such as `/start` and `/connect`.
    fn connect_phase(&self, event: &Value) -> AgentProxyDecision;

    /// Handles ordinary messages after a proxy binding exists.
    fn agent_phase(&self, event: &Value, binding: &AgentProxyBinding) -> AgentProxyDecision;

    /// Converts an agent response into a connector action input.
    fn render_agent_reply(&self, agent_output: &str, binding: &AgentProxyBinding) -> Value;

    /// Builds the agent prompt for a routed message. Default forwards to a named agent.
    fn route_prompt(&self, target: &str, message: &str) -> String {
        format!("Route this external connector message to agent `{target}`.\n\n{message}")
    }
}

/// Returns a built-in agent proxy implementation for connectors that support it.
pub fn builtin_agent_proxy(connector_slug: &str) -> Option<Box<dyn AgentProxy>> {
    match connector_slug {
        "telegram-bot" => Some(Box::new(TelegramBotAgentProxy)),
        "lark-bot" => Some(Box::new(LarkBotAgentProxy)),
        _ => None,
    }
}

/// Handles one connector event against a built-in proxy and persists `/connect`.
pub fn handle_agent_proxy_event(
    connector_slug: &str,
    connection_slug: &str,
    event: &Value,
    store: &AgentProxyStore,
) -> Result<AgentProxyDecision, AgentProxyStoreError> {
    let Some(proxy) = builtin_agent_proxy(connector_slug) else {
        return Ok(AgentProxyDecision::Ignore);
    };
    let event = event_with_connection(event, connection_slug);
    let principal = external_principal(&event);
    let setup_decision = proxy.connect_phase(&event);
    let decision = match setup_decision {
        AgentProxyDecision::Ignore => match store.get(connection_slug, &principal) {
            Some(binding) => proxy.agent_phase(&event, &binding),
            None => AgentProxyDecision::Ignore,
        },
        AgentProxyDecision::BindAgent { mut binding, reply } => {
            binding.connection_slug = connection_slug.to_string();
            binding.external_principal = principal;
            let binding = store.upsert(binding)?;
            AgentProxyDecision::BindAgent { binding, reply }
        }
        other => other,
    };
    Ok(decision)
}

/// Agent proxy implementation for Telegram bot chats.
pub struct TelegramBotAgentProxy;

impl AgentProxy for TelegramBotAgentProxy {
    fn connect_phase(&self, event: &Value) -> AgentProxyDecision {
        command_decision(event, "telegram-bot")
    }

    fn agent_phase(&self, event: &Value, binding: &AgentProxyBinding) -> AgentProxyDecision {
        route_bound_message(event, binding)
    }

    fn render_agent_reply(&self, agent_output: &str, binding: &AgentProxyBinding) -> Value {
        let target = binding
            .reply_target
            .as_deref()
            .unwrap_or(&binding.external_principal);
        serde_json::json!({
            "to": target,
            "message": agent_output,
        })
    }
}

/// Agent proxy for Lark chats (`lark-cli`); same `/connect <agent>` flow as Telegram, replies to `chat_id`.
pub struct LarkBotAgentProxy;

impl AgentProxy for LarkBotAgentProxy {
    fn connect_phase(&self, event: &Value) -> AgentProxyDecision {
        command_decision(event, "lark-bot")
    }

    fn agent_phase(&self, event: &Value, binding: &AgentProxyBinding) -> AgentProxyDecision {
        route_bound_message(event, binding)
    }

    fn render_agent_reply(&self, agent_output: &str, binding: &AgentProxyBinding) -> Value {
        let target = binding
            .reply_target
            .as_deref()
            .unwrap_or(&binding.external_principal);
        serde_json::json!({
            "chat_id": target,
            "text": agent_output,
        })
    }

    /// The agent IS `target` and replies directly — it does not forward elsewhere.
    fn route_prompt(&self, target: &str, message: &str) -> String {
        format!(
            "You are `{target}`, an assistant chatting directly with a user in Lark. Reply to \
             their message below; your entire response is sent back to them as one chat message, \
             so write only the reply itself — do not route, forward, or mention other agents.\n\n\
             User message:\n{message}"
        )
    }
}

fn command_decision(event: &Value, connector_slug: &str) -> AgentProxyDecision {
    let text = event_text(event);
    let principal = external_principal(event);
    let target = reply_target(event).unwrap_or_else(|| principal.clone());
    if text.trim() == "/start" {
        return AgentProxyDecision::ConnectorAction {
            action: "send_message".to_string(),
            input: serde_json::json!({
                "to": target,
                "message": "Send /connect <agent-slug> to connect this chat to an agent.",
            }),
        };
    }
    let Some(agent_target) = text
        .trim()
        .strip_prefix("/connect ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return AgentProxyDecision::Ignore;
    };
    AgentProxyDecision::BindAgent {
        binding: AgentProxyBinding {
            connection_slug: event_connection_slug(event, connector_slug),
            external_principal: principal.clone(),
            reply_target: Some(target.clone()),
            agent_target: agent_target.to_string(),
            enabled: true,
        },
        reply: Some(serde_json::json!({
            "to": target,
            "message": format!("Connected to {agent_target}."),
        })),
    }
}

fn route_bound_message(event: &Value, binding: &AgentProxyBinding) -> AgentProxyDecision {
    if !binding.enabled {
        return AgentProxyDecision::Ignore;
    }
    let text = event_text(event);
    if text.trim().is_empty() {
        return AgentProxyDecision::Ignore;
    }
    let mut binding = binding.clone();
    if let Some(target) = reply_target(event) {
        binding.reply_target = Some(target);
    }
    AgentProxyDecision::RouteToAgent {
        target: binding.agent_target.clone(),
        message: text,
        binding,
    }
}

fn event_text(event: &Value) -> String {
    event
        .get("message")
        .or_else(|| event.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn external_principal(event: &Value) -> String {
    event
        .pointer("/person/id")
        .or_else(|| event.pointer("/from/id"))
        .or_else(|| event.get("from"))
        .and_then(string_value)
        .or_else(|| reply_target(event))
        .unwrap_or_else(|| "unknown".to_string())
}

fn reply_target(event: &Value) -> Option<String> {
    event
        .pointer("/chat/id")
        .or_else(|| event.get("chat_id"))
        .or_else(|| event.pointer("/channel/id"))
        .or_else(|| event.get("channel"))
        .or_else(|| event.pointer("/conversation/id"))
        .or_else(|| event.get("conversation"))
        .and_then(string_value)
        .filter(|target| !target.trim().is_empty())
}

fn string_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_i64().map(|id| id.to_string()))
        .or_else(|| value.as_u64().map(|id| id.to_string()))
}

fn event_connection_slug(event: &Value, connector_slug: &str) -> String {
    event
        .get("connection_slug")
        .or_else(|| event.get("connection"))
        .and_then(Value::as_str)
        .unwrap_or(connector_slug)
        .to_string()
}

fn event_with_connection(event: &Value, connection_slug: &str) -> Value {
    let mut event = event.clone();
    if let Some(object) = event.as_object_mut() {
        object
            .entry("connection_slug")
            .or_insert_with(|| Value::String(connection_slug.to_string()));
    }
    event
}

fn validate_proxy_binding(binding: &AgentProxyBinding) -> Result<(), AgentProxyStoreError> {
    validate_proxy_slug("connection slug", &binding.connection_slug)?;
    if binding.external_principal.trim().is_empty() {
        return Err(AgentProxyStoreError::Invalid(
            "external principal must not be empty".to_string(),
        ));
    }
    if binding.agent_target.trim().is_empty() {
        return Err(AgentProxyStoreError::Invalid(
            "agent target must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn validate_proxy_slug(label: &str, slug: &str) -> Result<(), AgentProxyStoreError> {
    if slug.is_empty()
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(AgentProxyStoreError::Invalid(format!(
            "{label} must be non-empty kebab-case ASCII"
        )));
    }
    Ok(())
}

fn write_proxy_store(path: &Path, store: &AgentProxyStoreFile) -> Result<(), AgentProxyStoreError> {
    let tmp = path.with_extension("tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, serde_json::to_vec_pretty(store)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoProxy;

    impl AgentProxy for EchoProxy {
        fn connect_phase(&self, _event: &Value) -> AgentProxyDecision {
            AgentProxyDecision::Ignore
        }

        fn agent_phase(&self, event: &Value, binding: &AgentProxyBinding) -> AgentProxyDecision {
            AgentProxyDecision::RouteToAgent {
                target: binding.agent_target.clone(),
                message: event["message"].as_str().unwrap_or("").to_string(),
                binding: binding.clone(),
            }
        }

        fn render_agent_reply(&self, agent_output: &str, binding: &AgentProxyBinding) -> Value {
            let target = binding
                .reply_target
                .as_deref()
                .unwrap_or(&binding.external_principal);
            serde_json::json!({"to": target, "message": agent_output})
        }
    }

    #[test]
    fn proxy_trait_covers_agent_phase_and_reply_rendering() {
        let proxy = EchoProxy;
        let binding = AgentProxyBinding {
            connection_slug: "telegram-bot".into(),
            external_principal: "u1".into(),
            reply_target: None,
            agent_target: "agent-1".into(),
            enabled: true,
        };

        assert_eq!(
            proxy.agent_phase(&serde_json::json!({"message":"hi"}), &binding),
            AgentProxyDecision::RouteToAgent {
                target: "agent-1".into(),
                message: "hi".into(),
                binding: binding.clone(),
            }
        );
        assert_eq!(
            proxy.render_agent_reply("ok", &binding),
            serde_json::json!({"to":"u1","message":"ok"})
        );
    }

    #[test]
    fn telegram_proxy_binds_connect_command() {
        let proxy = TelegramBotAgentProxy;
        let decision = proxy.connect_phase(&serde_json::json!({
            "message": "/connect agent-1",
            "from": {"id": 123},
            "connection_slug": "my-tg-bot"
        }));

        assert_eq!(
            decision,
            AgentProxyDecision::BindAgent {
                binding: AgentProxyBinding {
                    connection_slug: "my-tg-bot".into(),
                    external_principal: "123".into(),
                    reply_target: Some("123".into()),
                    agent_target: "agent-1".into(),
                    enabled: true,
                },
                reply: Some(serde_json::json!({
                    "to": "123",
                    "message": "Connected to agent-1.",
                })),
            }
        );
    }

    #[test]
    fn lark_route_prompt_makes_agent_the_target_not_a_forwarder() {
        let lark = LarkBotAgentProxy.route_prompt("assistant", "几点了");
        assert!(lark.contains("You are `assistant`"));
        assert!(!lark.to_lowercase().contains("route this external"));
        // Telegram keeps the default forwarding prompt (unchanged behavior).
        let tg = TelegramBotAgentProxy.route_prompt("assistant", "几点了");
        assert!(tg.contains("Route this external connector message to agent `assistant`"));
    }

    #[test]
    fn lark_proxy_binds_then_routes_and_replies_to_chat() {
        let temp = tempfile::tempdir().unwrap();
        let store = AgentProxyStore::load(temp.path().join("agent_proxy_bindings.json")).unwrap();

        // A Lark event payload as emitted by the lark-cli connector bridge.
        let connect = serde_json::json!({
            "message": "/connect agent-1",
            "chat_id": "oc_demo",
            "sender_open_id": "ou_user"
        });
        let decision = handle_agent_proxy_event("lark-bot", "lark-bot", &connect, &store).unwrap();
        assert!(matches!(decision, AgentProxyDecision::BindAgent { .. }));

        let chat = serde_json::json!({
            "message": "几点了",
            "chat_id": "oc_demo",
            "sender_open_id": "ou_user"
        });
        let decision = handle_agent_proxy_event("lark-bot", "lark-bot", &chat, &store).unwrap();
        let AgentProxyDecision::RouteToAgent {
            target, binding, ..
        } = decision
        else {
            panic!("expected RouteToAgent, got {decision:?}");
        };
        assert_eq!(target, "agent-1");

        // The bot reply targets the originating Lark chat.
        let reply = LarkBotAgentProxy.render_agent_reply("现在三点", &binding);
        assert_eq!(
            reply,
            serde_json::json!({"chat_id": "oc_demo", "text": "现在三点"})
        );
    }

    #[test]
    fn proxy_store_roundtrips_binding() {
        let temp = tempfile::tempdir().unwrap();
        let store = AgentProxyStore::load(temp.path().join("agent_proxy_bindings.json")).unwrap();
        store
            .upsert(AgentProxyBinding {
                connection_slug: "my-bot".into(),
                external_principal: "u1".into(),
                reply_target: Some("chat-1".into()),
                agent_target: "agent-1".into(),
                enabled: true,
            })
            .unwrap();

        let reopened =
            AgentProxyStore::load(temp.path().join("agent_proxy_bindings.json")).unwrap();
        assert_eq!(
            reopened.get("my-bot", "u1").unwrap().agent_target,
            "agent-1"
        );
        assert!(reopened.has_enabled_consumer("my-bot"));
    }

    #[test]
    fn handle_agent_proxy_event_binds_then_routes() {
        let temp = tempfile::tempdir().unwrap();
        let store = AgentProxyStore::load(temp.path().join("agent_proxy_bindings.json")).unwrap();

        let decision = handle_agent_proxy_event(
            "telegram-bot",
            "my-bot",
            &serde_json::json!({"message":"/connect agent-1","from":{"id":123}}),
            &store,
        )
        .unwrap();
        assert!(matches!(decision, AgentProxyDecision::BindAgent { .. }));

        let decision = handle_agent_proxy_event(
            "telegram-bot",
            "my-bot",
            &serde_json::json!({"message":"status?","from":{"id":123}}),
            &store,
        )
        .unwrap();
        assert_eq!(
            decision,
            AgentProxyDecision::RouteToAgent {
                target: "agent-1".into(),
                message: "status?".into(),
                binding: AgentProxyBinding {
                    connection_slug: "my-bot".into(),
                    external_principal: "123".into(),
                    reply_target: Some("123".into()),
                    agent_target: "agent-1".into(),
                    enabled: true,
                },
            }
        );
    }

    #[test]
    fn slack_bot_is_not_a_subscription_proxy_without_a_typed_stream() {
        let temp = tempfile::tempdir().unwrap();
        let store = AgentProxyStore::load(temp.path().join("agent_proxy_bindings.json")).unwrap();

        let decision = handle_agent_proxy_event(
            "slack-bot",
            "slack-work",
            &serde_json::json!({
                "text": "/connect agent-2",
                "person": {"id": "U123"},
                "channel": {"id": "C999"}
            }),
            &store,
        )
        .unwrap();

        assert_eq!(decision, AgentProxyDecision::Ignore);
        assert!(store.list().is_empty());
    }
}
