//! Subscription spec — what the agent writes when the user describes a task.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Component, Path};

/// A single subscription installed by the agent.
///
/// The shape is intentionally narrow: one source topic, one prefilter
/// (regex), one optional LLM judge prompt, one action. Composition lives
/// at the level of "create more subscriptions," not at the level of nested
/// configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SubscriptionSpec {
    /// Stable, unique id chosen by the agent. Used by management tools.
    pub id: String,
    /// Free-form description from the user prompt; surfaced in `/subscriptions`.
    #[serde(default)]
    pub description: String,
    /// Subscriber topic to subscribe to (e.g. `"telegram-user"`).
    pub source_topic: String,
    /// Status — `enabled` or `paused`. Paused subs receive no events.
    #[serde(default = "default_status")]
    pub status: SubscriptionStatus,
    /// Optional regex applied to [`Event::text`]. Match enables the rest
    /// of the pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefilter: Option<PrefilterSpec>,
    /// Optional LLM classify step. Present iff a "judge" should run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classify_prompt: Option<String>,
    /// Optional model hint for the classifier in the form
    /// `<provider>/<model>` (e.g. `anthropic/claude-haiku-4-5`,
    /// `openai/gpt-4o-mini`). The installed classifier impl decides what
    /// to do when this is missing — typically falling back to a default
    /// fast model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classify_model: Option<String>,
    /// The action to invoke when the message passes both filters.
    pub action: ActionSpec,
    /// Created-at, milliseconds since UNIX epoch.
    #[serde(default)]
    pub created_at_ms: i128,
}

fn default_status() -> SubscriptionStatus {
    SubscriptionStatus::Enabled
}

/// Status of a subscription. Persisted as snake_case strings for stable
/// JSON output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    /// Subscription is active.
    Enabled,
    /// Subscription is paused — events bypass it.
    Paused,
}

/// Prefilter regex variant. We expose only the regex form for the MVP;
/// future variants (e.g. `from_contains`) can be added without breaking
/// existing specs because of the tag.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PrefilterSpec {
    /// Matches when `pattern` matches anywhere in the event text.
    /// `case_insensitive` defaults to `true`.
    Regex {
        /// Rust regex syntax (no PCRE backrefs).
        pattern: String,
        /// Case-insensitive flag. Defaults to true.
        #[serde(default = "default_true")]
        case_insensitive: bool,
    },
}

fn default_true() -> bool {
    true
}

/// Action variants. Each carries its own params struct; the dispatcher
/// matches and executes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionSpec {
    /// Insert one row per matched event into a SQLite table.
    SqliteInsert {
        /// Path to the database file. Created if it doesn't exist.
        path: String,
        /// Target table. Created with a default schema if missing.
        table: String,
    },
    /// Forward the matched event to another connector. The MVP logs the
    /// intent; full forwarding ships in the next step alongside a
    /// connector egress trait.
    ForwardMessage {
        /// Destination platform id (e.g. `"telegram"`).
        platform: String,
        /// Destination chat handle or numeric id.
        target: String,
        /// Optional template; `{{text}}` and `{{payload.field}}` are
        /// substituted.
        #[serde(default)]
        template: Option<String>,
    },
    /// Trigger a registered native Puffer workflow.
    RunWorkflow {
        /// Workflow slug to run.
        slug: String,
    },
    /// Catch-all for future variants the agent may write before all
    /// dispatchers are implemented; treated as a no-op with a warning.
    #[serde(other)]
    Unknown,
}

/// Convenience helper: validate a spec's regex and action params at
/// install-time so we surface errors to the agent immediately rather than
/// at first event arrival.
pub fn validate_spec(spec: &SubscriptionSpec) -> Result<(), String> {
    if spec.id.trim().is_empty() {
        return Err("id must not be empty".into());
    }
    if spec.source_topic.trim().is_empty() {
        return Err("source_topic must not be empty".into());
    }
    if let Some(PrefilterSpec::Regex { pattern, .. }) = &spec.prefilter {
        regex::Regex::new(pattern).map_err(|e| format!("prefilter regex invalid: {e}"))?;
    }
    match &spec.action {
        ActionSpec::SqliteInsert { path, table } => {
            if path.trim().is_empty() {
                return Err("sqlite_insert.path must not be empty".into());
            }
            if !is_safe_relative_path(path) {
                return Err("sqlite_insert.path must be a safe relative path".into());
            }
            if !is_valid_sqlite_identifier(table) {
                return Err(format!(
                    "sqlite_insert.table `{table}` is not a valid SQL identifier"
                ));
            }
        }
        ActionSpec::ForwardMessage {
            platform, target, ..
        } => {
            if platform.trim().is_empty() {
                return Err("forward_message.platform must not be empty".into());
            }
            if target.trim().is_empty() {
                return Err("forward_message.target must not be empty".into());
            }
        }
        ActionSpec::RunWorkflow { slug } => {
            if slug.trim().is_empty() {
                return Err("run_workflow.slug must not be empty".into());
            }
        }
        ActionSpec::Unknown => {
            return Err("action.type is not recognized by this Puffer build".into());
        }
    }
    Ok(())
}

/// Allow only `[A-Za-z_][A-Za-z0-9_]*`. Conservative on purpose — table
/// names get inlined into SQL and we never want injection.
pub(crate) fn is_valid_sqlite_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_safe_relative_path(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.starts_with("~/") {
        return false;
    }
    let path = Path::new(trimmed);
    !path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

/// Render `template` against the matched event payload. Supported tokens:
/// `{{text}}` (the event's text), `{{payload.<field>}}` (a top-level field
/// in `payload`, JSON-stringified). Missing fields render as the empty
/// string.
pub fn render_template(template: &str, text: &str, payload: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str("{{");
            rest = after;
            continue;
        };
        let token = after[..end].trim();
        let replacement = if token == "text" {
            text.to_string()
        } else if let Some(field) = token.strip_prefix("payload.") {
            payload
                .get(field)
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        out.push_str(&replacement);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_spec_passes() {
        let spec = SubscriptionSpec {
            id: "ioc".into(),
            description: String::new(),
            source_topic: "telegram-user".into(),
            status: SubscriptionStatus::Enabled,
            prefilter: Some(PrefilterSpec::Regex {
                pattern: r"\bIoC\b".into(),
                case_insensitive: true,
            }),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::SqliteInsert {
                path: "x.db".into(),
                table: "ioc_messages".into(),
            },
            created_at_ms: 0,
        };
        validate_spec(&spec).unwrap();
    }

    #[test]
    fn rejects_bad_table_name() {
        let spec = SubscriptionSpec {
            id: "x".into(),
            description: String::new(),
            source_topic: "t".into(),
            status: SubscriptionStatus::Enabled,
            prefilter: None,
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::SqliteInsert {
                path: "x.db".into(),
                table: "drop table users--".into(),
            },
            created_at_ms: 0,
        };
        assert!(validate_spec(&spec).is_err());
    }

    #[test]
    fn rejects_absolute_or_traversal_sqlite_path() {
        for path in ["/tmp/x.db", "~/x.db", "../x.db"] {
            let spec = SubscriptionSpec {
                id: "x".into(),
                description: String::new(),
                source_topic: "t".into(),
                status: SubscriptionStatus::Enabled,
                prefilter: None,
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::SqliteInsert {
                    path: path.into(),
                    table: "ioc_messages".into(),
                },
                created_at_ms: 0,
            };
            assert!(validate_spec(&spec).is_err(), "{path} should be rejected");
        }
    }

    #[test]
    fn template_renders_text_and_payload_fields() {
        let result = render_template(
            "Hello {{payload.name}}: {{text}}",
            "world",
            &json!({"name": "Hongyi"}),
        );
        assert_eq!(result, "Hello Hongyi: world");
    }
}
