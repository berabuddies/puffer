use anyhow::{Context, Result};
use puffer_core::{
    command_surface, execute_user_turn_streaming, find_command, AppState, CommandKind,
};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::LoadedResources;
use serde_json::Value;
use std::path::Path;

const OPENAI_TITLE_MODEL: &str = "gpt-5.4-mini";
const ANTHROPIC_TITLE_MODEL: &str = "claude-haiku-4-5-20251001";

/// Returns whether a session without a user-set title should be auto-titled.
pub(crate) fn should_auto_title(
    display_name: Option<&str>,
    generated_title: Option<&str>,
    has_user_message: bool,
) -> bool {
    display_name
        .map(str::trim)
        .map(str::is_empty)
        .unwrap_or(true)
        && generated_title
            .map(str::trim)
            .map(str::is_empty)
            .unwrap_or(true)
        && !has_user_message
}

/// Returns whether this daemon turn should call the model-backed title path.
pub(crate) fn should_generate_title_for_turn(
    resources: &LoadedResources,
    display_name: Option<&str>,
    generated_title: Option<&str>,
    has_user_message: bool,
    disable_auto_title: bool,
    message: &str,
) -> bool {
    if disable_auto_title || !should_auto_title(display_name, generated_title, has_user_message) {
        return false;
    }

    match slash_command_name(message) {
        Some(name) => {
            let commands = command_surface(resources);
            find_command(&commands, name)
                .is_some_and(|command| matches!(command.kind, CommandKind::Prompt))
        }
        None => true,
    }
}

/// Generates a Claude-style session title using a small provider-family title model.
pub(crate) fn generate_title_with_model(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    message: &str,
) -> Result<Option<String>> {
    let Some(provider) = selected_provider(state, providers) else {
        return Ok(None);
    };
    let Some(model_id) = title_model_for_provider(provider) else {
        return Ok(None);
    };
    let mut title_state = state.clone();
    title_state.current_provider = Some(provider.id.clone());
    title_state.current_model = Some(format!("{}/{}", provider.id, model_id));
    title_state.transcript.clear();
    title_state.effort_level = "low".to_string();
    title_state.fast_mode = true;
    title_state.reflection_config = None;

    let mut title_resources = resources.clone();
    title_resources.tools.clear();

    let prompt = title_prompt(message, &state.cwd);
    let response = execute_user_turn_streaming(
        &mut title_state,
        &title_resources,
        providers,
        auth_store,
        &prompt,
        |_| {},
    )
    .with_context(|| {
        format!(
            "title model request failed for provider `{}` model `{}`",
            provider.id, model_id
        )
    })?;
    Ok(parse_generated_title(&response.assistant_text))
}

/// Builds a deterministic session title from the first user message.
pub(crate) fn title_from_first_message(message: &str) -> Option<String> {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return None;
    }

    const MAX_CHARS: usize = 60;
    let mut title = trimmed.chars().take(MAX_CHARS + 1).collect::<String>();
    if title.chars().count() > MAX_CHARS {
        title = title.chars().take(MAX_CHARS).collect::<String>();
        let without_partial_word = title
            .rsplit_once(' ')
            .map(|(prefix, _)| prefix)
            .filter(|prefix| prefix.chars().count() >= 20)
            .unwrap_or(title.as_str())
            .trim_end()
            .to_string();
        title = format!("{without_partial_word}...");
    }

    Some(title)
}

fn selected_provider<'a>(
    state: &AppState,
    providers: &'a ProviderRegistry,
) -> Option<&'a ProviderDescriptor> {
    state
        .current_model
        .as_deref()
        .and_then(|selector| providers.resolve_model(selector))
        .and_then(|model| providers.provider(&model.provider))
        .or_else(|| {
            state
                .current_provider
                .as_deref()
                .and_then(|provider_id| providers.provider(provider_id))
        })
        .or_else(|| providers.providers().next())
}

fn title_model_for_provider(provider: &ProviderDescriptor) -> Option<&'static str> {
    match provider.default_api.as_str() {
        "anthropic-messages" => Some(ANTHROPIC_TITLE_MODEL),
        api if api.contains("openai") => Some(OPENAI_TITLE_MODEL),
        _ => None,
    }
}

fn title_prompt(message: &str, cwd: &Path) -> String {
    let project = cwd
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("project");
    format!(
        "Generate a concise 3-7 word title for this coding session.\n\
Use sentence case. Do not end with punctuation. Do not mention tools, models, or providers.\n\
Do not copy the user's wording verbatim; summarize the intent as a task title.\n\
For vague questions about the current project, use the project name when it helps.\n\
\n\
Examples:\n\
- User: \"Whats up? What is this project\" / Project: \"puffer\" -> {{\"title\":\"Puffer project overview\"}}\n\
- User: \"Fix the login button on mobile\" / Project: \"app\" -> {{\"title\":\"Fix mobile login button\"}}\n\
- User: \"Add OAuth authentication\" / Project: \"app\" -> {{\"title\":\"Add OAuth authentication\"}}\n\
\n\
Project: {project}\n\
User message:\n{message}"
    )
}

fn parse_generated_title(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let title = parse_title_json(trimmed)
        .or_else(|| parse_title_json(strip_json_fence(trimmed)))
        .or_else(|| sanitize_title(trimmed))?;
    sanitize_title(&title)
}

fn parse_title_json(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    value
        .get("title")
        .and_then(Value::as_str)
        .and_then(sanitize_title)
}

fn strip_json_fence(text: &str) -> &str {
    let without_open = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text)
        .trim_start();
    without_open
        .strip_suffix("```")
        .unwrap_or(without_open)
        .trim()
}

fn sanitize_title(value: &str) -> Option<String> {
    let mut title = value.split_whitespace().collect::<Vec<_>>().join(" ");
    title = title
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`'))
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }
    if title.starts_with('{') || title.starts_with('[') {
        return None;
    }
    const MAX_CHARS: usize = 80;
    if title.chars().count() > MAX_CHARS {
        title = title.chars().take(MAX_CHARS).collect::<String>();
        title = title.trim_end().to_string();
    }
    title = title
        .trim_end_matches(|ch| matches!(ch, '.' | '!' | '?'))
        .trim_end()
        .to_string();
    if title.is_empty() {
        return None;
    }
    Some(title)
}

fn slash_command_name(message: &str) -> Option<&str> {
    let without_slash = message.trim_start().strip_prefix('/')?;
    Some(without_slash.split_whitespace().next().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_title_requires_empty_name_and_no_user_messages() {
        assert!(should_auto_title(None, None, false));
        assert!(should_auto_title(Some("  "), Some("  "), false));
        assert!(!should_auto_title(Some("Named"), None, false));
        assert!(!should_auto_title(None, Some("Generated"), false));
        assert!(!should_auto_title(None, None, true));
    }

    #[test]
    fn title_generation_skips_local_connector_command() {
        assert!(!should_generate_title_for_turn(
            &LoadedResources::default(),
            None,
            None,
            false,
            false,
            "/connect telegram-login personal"
        ));
    }

    #[test]
    fn title_generation_allows_prompt_command() {
        assert!(should_generate_title_for_turn(
            &LoadedResources::default(),
            None,
            None,
            false,
            false,
            "/commit"
        ));
    }

    #[test]
    fn title_generation_skips_unknown_slash_command() {
        assert!(!should_generate_title_for_turn(
            &LoadedResources::default(),
            None,
            None,
            false,
            false,
            "/unknown value"
        ));
    }

    #[test]
    fn title_generation_allows_regular_user_message() {
        assert!(should_generate_title_for_turn(
            &LoadedResources::default(),
            None,
            None,
            false,
            false,
            "Explain this repository"
        ));
    }

    #[test]
    fn title_from_first_message_collapses_whitespace() {
        assert_eq!(
            title_from_first_message("  Fix   the browser\nsession title  ").as_deref(),
            Some("Fix the browser session title")
        );
    }

    #[test]
    fn title_from_first_message_truncates_long_text() {
        assert_eq!(
            title_from_first_message(
                "Please investigate why the session title is never updated after sending a message"
            )
            .as_deref(),
            Some("Please investigate why the session title is never updated...")
        );
    }

    #[test]
    fn generated_title_parser_accepts_json_and_fences() {
        assert_eq!(
            parse_generated_title(r#"{"title":"Fix daemon title"}"#).as_deref(),
            Some("Fix daemon title")
        );
        assert_eq!(
            parse_generated_title("```json\n{\"title\":\"Add OAuth flow\"}\n```").as_deref(),
            Some("Add OAuth flow")
        );
    }

    #[test]
    fn generated_title_parser_rejects_empty_jsonish_text() {
        assert_eq!(parse_generated_title(r#"{"name":"No title"}"#), None);
        assert_eq!(parse_generated_title("   "), None);
    }

    #[test]
    fn title_prompt_discourages_echoes_for_project_questions() {
        let prompt = title_prompt(
            "Whats up? What is this project",
            std::path::Path::new("/Users/shou/puffer"),
        );
        assert!(prompt.contains("Do not copy"));
        assert!(prompt.contains("Puffer project overview"));
        assert!(prompt.contains("Project: puffer"));
    }

    #[test]
    fn generated_title_parser_trims_terminal_punctuation() {
        assert_eq!(
            parse_generated_title(r#"{"title":"Puffer project overview?"}"#).as_deref(),
            Some("Puffer project overview")
        );
    }
}
