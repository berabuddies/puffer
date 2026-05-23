use super::top_panel::{should_show_puffer, TOP_PANEL_IMAGE_HEIGHT};
use puffer_core::{estimate_remaining_context_percent, running_background_task_count, AppState};
use puffer_provider_registry::{AuthStore, ProviderRegistry, StoredCredential};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use ratatui::text::{Line, Span};
use std::path::Path;

/// Builds the stacked summary rows used when the viewport is narrow.
pub(super) fn top_panel_compact_lines(
    state: &AppState,
    resources: &LoadedResources,
    auth_store: &AuthStore,
    tool_registry: &ToolRegistry,
    providers: &ProviderRegistry,
) -> Vec<Line<'static>> {
    let account = account_header_line(state, auth_store).unwrap_or_else(|| {
        let provider = current_provider(state);
        match auth_status(state, auth_store) {
            "api-key" => format!("{provider} via API key"),
            "oauth" => format!("{provider} via OAuth"),
            "missing" => format!("{provider} auth missing"),
            _ => "No active account".to_string(),
        }
    });
    let mut mode = vec![format!("effort {}", truncate(&state.effort_level, 14))];
    if state.fast_mode {
        mode.push("fast".to_string());
    }
    if state.vim_mode {
        mode.push("vim".to_string());
    }
    let tools = tool_status(tool_registry).executable;
    let remote = remote_label(state);
    let remaining = estimate_remaining_context_percent(state, providers);
    let ctx_pct = if remaining > 0 {
        format!(" · {remaining}% left")
    } else {
        String::new()
    };
    let cache_label = match (state.last_cache_hit_ratio, state.session_cache_hit_ratio) {
        (Some(last), Some(avg)) => {
            format!(" · cache {:.0}% (avg {:.0}%)", last * 100.0, avg * 100.0)
        }
        _ => String::new(),
    };
    let context = if remote == "local" {
        format!(
            "{} · {} msgs · {} wds · permissions ACL{}{}",
            path_tail(&state.cwd),
            state.transcript.len(),
            state.working_dirs.len(),
            ctx_pct,
            cache_label,
        )
    } else {
        format!(
            "{} · {} msgs · {} wds · {}{}{}",
            path_tail(&state.cwd),
            state.transcript.len(),
            state.working_dirs.len(),
            remote,
            ctx_pct,
            cache_label,
        )
    };

    vec![
        top_panel_line("Account", account, false),
        top_panel_line(
            "Model",
            format!(
                "{} · tools {tools}/{}",
                current_model(state),
                resources.tools.len(),
            ),
            false,
        ),
        top_panel_line(
            "Session",
            format!("{} · {}", session_name(state), state.session.id),
            false,
        ),
        top_panel_line("Mode", mode.join(" · "), false),
        top_panel_line("Context", context, false),
    ]
}

/// Returns the outer height needed for the top information box.
pub(super) fn top_panel_height(
    state: &AppState,
    resources: &LoadedResources,
    auth_store: &AuthStore,
    tool_registry: &ToolRegistry,
    providers: &ProviderRegistry,
    width: u16,
) -> u16 {
    let box_height = top_panel_compact_lines(state, resources, auth_store, tool_registry, providers)
        .len() as u16
        + 2;
    if should_show_puffer(width) {
        (TOP_PANEL_IMAGE_HEIGHT as u16).max(box_height)
    } else {
        box_height
    }
}

/// Builds the condensed footer status line shown below the composer.
pub(super) fn footer_status_line(state: &AppState, providers: &ProviderRegistry) -> String {
    let remaining = estimate_remaining_context_percent(state, providers);
    let bg_count = running_background_task_count(state);
    let bg_label = if bg_count > 0 {
        format!(
            " · {bg_count} bg task{}",
            if bg_count == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    };
    let cache_label = match (state.last_cache_hit_ratio, state.session_cache_hit_ratio) {
        (Some(last), Some(avg)) => {
            format!(" · cache {:.0}% (avg {:.0}%)", last * 100.0, avg * 100.0)
        }
        _ => String::new(),
    };
    format!(
        "{} · {}% left · {} · permissions ACL{}{}",
        current_model(state),
        remaining,
        footer_path(&state.cwd),
        cache_label,
        bg_label,
    )
}

/// Flattens the top-box summary into test-friendly lines.
#[cfg(test)]
pub(super) fn header_lines(
    state: &AppState,
    resources: &LoadedResources,
    auth_store: &AuthStore,
    tool_registry: &ToolRegistry,
    providers: &ProviderRegistry,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("Puffer Code")];
    lines.extend(top_panel_compact_lines(
        state,
        resources,
        auth_store,
        tool_registry,
        providers,
    ));
    lines
}

/// Builds the serialized session summary used in render tests.
#[cfg(test)]
pub(super) fn session_lines(state: &AppState) -> Vec<String> {
    let parent = state
        .session
        .parent_session_id
        .map(|value| short_id(&value.to_string()))
        .unwrap_or_else(|| "root".to_string());
    vec![
        format!("Name: {}", truncate(&session_name(state), 26)),
        format!("Id: {}", short_id(&state.session.id.to_string())),
        format!("Parent: {parent}"),
        format!("Dir: {}", truncate(&path_tail(&state.cwd), 26)),
        format!("Transcript: {} messages", state.transcript.len()),
        format!("Workdirs: {}", state.working_dirs.len()),
        format!(
            "Tags: {}",
            truncate(&format_tag_summary(&state.session.tags), 26)
        ),
        format!("Note: {}", state.session.note.as_deref().unwrap_or("-")),
    ]
}

/// Builds the test-only footer summary lines.
#[cfg(test)]
pub(super) fn footer_lines(state: &AppState, providers: &ProviderRegistry) -> Vec<Line<'static>> {
    vec![Line::from(footer_status_line(state, providers))]
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ToolStatus {
    executable: usize,
}

fn top_panel_line(label: &str, value: String, emphasize: bool) -> Line<'static> {
    let label = Span::styled(
        format!("{label:<10}"),
        ratatui::style::Style::reset().add_modifier(ratatui::style::Modifier::DIM),
    );
    let value = if emphasize {
        Span::styled(
            value,
            ratatui::style::Style::reset().add_modifier(ratatui::style::Modifier::BOLD),
        )
    } else {
        Span::styled(value, ratatui::style::Style::reset())
    };
    Line::from(vec![label, value]).patch_style(ratatui::style::Style::reset())
}

fn current_provider(state: &AppState) -> &str {
    state.current_provider.as_deref().unwrap_or("<unset>")
}

fn current_model(state: &AppState) -> &str {
    state.current_model.as_deref().unwrap_or("<unset>")
}

fn auth_status(state: &AppState, auth_store: &AuthStore) -> &'static str {
    match state
        .current_provider
        .as_deref()
        .and_then(|id| auth_store.get(id))
    {
        Some(StoredCredential::ApiKey { .. }) => "api-key",
        Some(StoredCredential::OAuth(_)) => "oauth",
        None if state.current_provider.is_some() => "missing",
        None => "n/a",
    }
}

fn account_header_line(state: &AppState, auth_store: &AuthStore) -> Option<String> {
    let provider_id = state.current_provider.as_deref()?;
    let StoredCredential::OAuth(credential) = auth_store.get(provider_id)? else {
        return None;
    };
    let mut parts = Vec::new();
    if let Some(email) = credential.email.as_deref() {
        parts.push(email.to_string());
    }
    if let Some(plan_type) = credential.plan_type.as_deref() {
        parts.push(format!("plan {}", format_metadata_value(plan_type)));
    }
    if let Some(organization_id) = credential.organization_id.as_deref() {
        parts.push(format!("org {}", organization_id));
    } else if let Some(account_id) = credential.account_id.as_deref() {
        parts.push(format!("acct {}", account_id));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn format_metadata_value(value: &str) -> String {
    value
        .split(['-', '_'])
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => {
                    let mut word = String::new();
                    word.extend(first.to_uppercase());
                    word.push_str(chars.as_str());
                    word
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn session_name(state: &AppState) -> String {
    state
        .session
        .display_name
        .as_deref()
        .or(state.session.slug.as_deref())
        .unwrap_or("untitled")
        .to_string()
}

fn remote_label(state: &AppState) -> String {
    match (
        state.remote_name.as_deref(),
        state.remote_environment.as_deref(),
    ) {
        (Some(name), Some(environment)) => format!("{name}@{environment}"),
        (Some(name), None) => name.to_string(),
        (None, Some(environment)) => environment.to_string(),
        (None, None) => "local".to_string(),
    }
}

fn path_tail(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

fn footer_path(path: &Path) -> String {
    let Some(home) = dirs::home_dir() else {
        return path.display().to_string();
    };
    if path == home {
        return "~".to_string();
    }
    if let Ok(relative) = path.strip_prefix(&home) {
        let suffix = relative.display().to_string();
        if suffix.is_empty() {
            "~".to_string()
        } else {
            format!("~/{suffix}")
        }
    } else {
        path.display().to_string()
    }
}

fn short_id(value: &str) -> String {
    value.chars().take(8).collect()
}

#[cfg(test)]
fn format_tag_summary(tags: &[String]) -> String {
    if tags.is_empty() {
        "-".to_string()
    } else {
        tags.join(",")
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let prefix = value.chars().take(max_chars - 3).collect::<String>();
    format!("{prefix}...")
}

fn tool_status(tool_registry: &ToolRegistry) -> ToolStatus {
    let mut status = ToolStatus::default();
    for _tool in tool_registry.tools() {
        status.executable += 1;
    }
    status
}
