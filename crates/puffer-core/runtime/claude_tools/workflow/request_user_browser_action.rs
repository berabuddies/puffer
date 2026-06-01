use crate::AppState;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Deserialize)]
struct RequestUserBrowserActionInput {
    description: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    connector_slug: Option<String>,
    #[serde(default)]
    connection_slug: Option<String>,
}

/// Executes `requestuserbrowseraction` by asking the user for browser-action confirmation.
pub fn execute_request_user_browser_action(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: RequestUserBrowserActionInput =
        serde_json::from_value(input).context("invalid requestuserbrowseraction input")?;
    let description = parsed.description.trim();
    if description.is_empty() {
        bail!("requestuserbrowseraction requires a non-empty description");
    }
    let question = browser_action_question(description, parsed.url.as_deref());
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "source".to_string(),
        Value::String("requestuserbrowseraction".to_string()),
    );
    if let Some(connector_slug) = parsed
        .connector_slug
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert(
            "connector_slug".to_string(),
            Value::String(connector_slug.to_string()),
        );
    }
    if let Some(connection_slug) = parsed
        .connection_slug
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert(
            "connection_slug".to_string(),
            Value::String(connection_slug.to_string()),
        );
    }
    let ask_output = super::ask_user_question::execute_ask_user_question(
        state,
        cwd,
        json!({
            "questions": [{
                "type": "choice",
                "header": "Browser",
                "question": question,
                "options": [
                    {
                        "label": "Done",
                        "description": "I completed the requested browser action."
                    },
                    {
                        "label": "Cannot complete",
                        "description": "I could not complete the requested browser action."
                    }
                ]
            }],
            "metadata": metadata
        }),
    )?;
    let ask_value: Value = serde_json::from_str(&ask_output)
        .context("parse requestuserbrowseraction prompt output")?;
    Ok(serde_json::to_string_pretty(&json!({
        "description": description,
        "url": parsed.url,
        "prompt": ask_value,
    }))?)
}

fn browser_action_question(description: &str, url: Option<&str>) -> String {
    let Some(url) = url.map(str::trim).filter(|value| !value.is_empty()) else {
        return description.to_string();
    };
    format!("{description}\n\nURL: {url}")
}
