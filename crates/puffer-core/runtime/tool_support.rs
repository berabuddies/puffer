use anyhow::{anyhow, bail, Result};
use puffer_provider_openai::{
    OpenAIChatCompletionTool, OpenAIChatCompletionToolFunction, OpenAIResponsesTool,
};
use puffer_provider_registry::ProviderDescriptor;
use puffer_tools::{ToolDefinition, ToolRegistry};
use serde_json::{json, Value};

pub(super) fn anthropic_tool_definitions(
    registry: &ToolRegistry,
    provider: &ProviderDescriptor,
    model_id: &str,
) -> Vec<Value> {
    registry
        .definitions()
        .filter(|definition| should_expose_tool(definition, provider, model_id))
        .map(|definition| {
            json!({
                "name": definition.id,
                "description": definition.description,
                "input_schema": definition.input_schema.as_json_schema(),
            })
        })
        .collect()
}

pub(super) fn openai_tool_definitions(
    registry: &ToolRegistry,
    provider: &ProviderDescriptor,
    model_id: &str,
) -> Vec<OpenAIResponsesTool> {
    registry
        .definitions()
        .filter(|definition| should_expose_tool(definition, provider, model_id))
        .map(|definition| OpenAIResponsesTool {
            kind: "function".to_string(),
            name: definition.id.clone(),
            description: definition.description.clone(),
            parameters: definition.input_schema.as_json_schema(),
            filters: None,
            user_location: None,
            external_web_access: None,
        })
        .collect()
}

pub(super) fn openai_chat_completion_tools(
    registry: &ToolRegistry,
    provider: &ProviderDescriptor,
    model_id: &str,
) -> Vec<OpenAIChatCompletionTool> {
    registry
        .definitions()
        .filter(|definition| should_expose_tool(definition, provider, model_id))
        .map(|definition| OpenAIChatCompletionTool {
            kind: "function".to_string(),
            function: OpenAIChatCompletionToolFunction {
                name: definition.id.clone(),
                description: definition.description.clone(),
                parameters: definition.input_schema.as_json_schema(),
            },
        })
        .collect()
}

pub(super) fn enforce_tool_permission(registry: &ToolRegistry, tool_id: &str) -> Result<()> {
    let definition = registry
        .definition(tool_id)
        .ok_or_else(|| anyhow!("tool `{tool_id}` is not available in this runtime"))?;
    if let Some(reason) = tool_permission_denied_reason(definition) {
        bail!("tool `{tool_id}` is disabled by policy: {reason}");
    }
    Ok(())
}

pub(super) fn is_provider_web_search_tool(definition: &ToolDefinition) -> bool {
    definition.handler == "provider:web_search"
}

pub(super) fn supports_web_search(provider: &ProviderDescriptor, model_id: &str) -> bool {
    let model = model_id.to_ascii_lowercase();
    match provider.default_api.as_str() {
        "anthropic-messages" => {
            model.contains("claude-opus-4")
                || model.contains("claude-sonnet-4")
                || model.contains("claude-haiku-4")
        }
        "openai-responses" | "openai-codex-responses" | "azure-openai-responses" => {
            model.starts_with("gpt-5") || model.starts_with("o4-") || model.contains("codex")
        }
        _ => false,
    }
}

pub(super) fn merge_tool_output(stdout: String, stderr: String) -> String {
    if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    }
}

fn should_expose_tool(
    definition: &ToolDefinition,
    provider: &ProviderDescriptor,
    model_id: &str,
) -> bool {
    tool_permission_denied_reason(definition).is_none()
        && (!is_provider_web_search_tool(definition) || supports_web_search(provider, model_id))
}

fn tool_permission_denied_reason(definition: &ToolDefinition) -> Option<&'static str> {
    if definition
        .policy
        .approval_policy
        .as_deref()
        .is_some_and(policy_value_disables_tool)
    {
        return Some("approval policy marks it disabled");
    }
    if definition
        .enabled_if
        .as_deref()
        .is_some_and(enabled_if_value_disables_tool)
    {
        return Some("enabled_if expression currently resolves to disabled");
    }
    None
}

fn policy_value_disables_tool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "disabled" | "deny" | "never"
    )
}

fn enabled_if_value_disables_tool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "disabled" | "deny" | "false" | "never" | "off"
    )
}
