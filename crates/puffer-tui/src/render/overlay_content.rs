use crate::state::AuthPickerEntry;
use crate::{ModelPickerEntry, OverlayState};
use puffer_session_store::SessionSummary;
use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OverlayRow {
    pub(super) text: String,
    pub(super) selected: bool,
}

pub(super) fn short_id(value: &str) -> String {
    value.chars().take(8).collect()
}

pub(super) fn overlay_title(overlay: &OverlayState) -> Cow<'_, str> {
    match overlay {
        OverlayState::SessionPicker { .. } => Cow::Borrowed("Resume Session"),
        OverlayState::AgentPicker { .. } => Cow::Borrowed("Select Agent"),
        OverlayState::ModelPicker { .. } => Cow::Borrowed("Select Model"),
        OverlayState::EffortPicker { .. } => Cow::Borrowed("Select Effort"),
        OverlayState::FastModePicker { .. } => Cow::Borrowed("Fast Mode"),
        OverlayState::ProviderPicker { .. } => Cow::Borrowed("Select Provider"),
        OverlayState::AuthPicker { .. } => Cow::Borrowed("Select Login Method"),
        OverlayState::ApiKeyPrompt { .. } => Cow::Borrowed("Enter API Key"),
        OverlayState::LoginPicker { .. } => Cow::Borrowed("Select Provider"),
        OverlayState::LogoutPicker { .. } => Cow::Borrowed("Logout Provider"),
        OverlayState::ThemePicker { .. } => Cow::Borrowed("Select Theme"),
        OverlayState::CommandPicker { title, .. } => Cow::Borrowed(title.as_str()),
        OverlayState::Help => Cow::Borrowed("Help"),
        OverlayState::PermissionPrompt { .. } => Cow::Borrowed("Permission Needed"),
        OverlayState::UserQuestionPrompt { overlay } => Cow::Owned(overlay.title()),
        OverlayState::Btw(..) => Cow::Borrowed("BTW"),
        OverlayState::Session(..) => Cow::Borrowed("Session"),
        OverlayState::Status(..) => Cow::Borrowed("Status"),
        OverlayState::Text(..) => Cow::Borrowed("Panel"),
        OverlayState::OnboardingTheme { .. } => Cow::Borrowed("Select Theme"),
        OverlayState::OnboardingProvider { .. } => Cow::Borrowed("Select Provider"),
        OverlayState::OnboardingAuth { .. } => Cow::Borrowed("Select Login Method"),
        OverlayState::OnboardingModel { .. } => Cow::Borrowed("Select Model"),
        OverlayState::OnboardingApiKey { .. } => Cow::Borrowed("Enter API Key"),
        OverlayState::Usage(..) => Cow::Borrowed("Usage"),
        OverlayState::AutoDreamSuggestion { .. } => Cow::Borrowed("AutoDream Suggestion"),
    }
}

pub(super) fn overlay_rows(overlay: &OverlayState) -> Vec<OverlayRow> {
    match overlay {
        OverlayState::SessionPicker {
            sessions,
            selection,
        } => sessions
            .iter()
            .enumerate()
            .map(|(index, session)| OverlayRow {
                selected: index == *selection,
                text: render_session_entry(session),
            })
            .collect(),
        OverlayState::AgentPicker { entries, selection }
        | OverlayState::ModelPicker {
            entries, selection, ..
        }
        | OverlayState::EffortPicker {
            entries, selection, ..
        }
        | OverlayState::FastModePicker {
            entries, selection, ..
        }
        | OverlayState::ProviderPicker {
            entries, selection, ..
        }
        | OverlayState::LoginPicker { entries, selection }
        | OverlayState::LogoutPicker { entries, selection }
        | OverlayState::ThemePicker { entries, selection }
        | OverlayState::CommandPicker {
            entries, selection, ..
        }
        | OverlayState::OnboardingTheme { entries, selection }
        | OverlayState::OnboardingProvider { entries, selection }
        | OverlayState::OnboardingAuth {
            entries, selection, ..
        }
        | OverlayState::OnboardingModel {
            entries, selection, ..
        } => entries
            .iter()
            .enumerate()
            .map(|(index, entry)| OverlayRow {
                selected: index == *selection,
                text: render_model_entry(entry),
            })
            .collect(),
        OverlayState::AuthPicker {
            entries, selection, ..
        } => entries
            .iter()
            .enumerate()
            .map(|(index, entry)| OverlayRow {
                selected: index == *selection,
                text: render_auth_entry(entry),
            })
            .collect(),
        OverlayState::AutoDreamSuggestion {
            skill_name,
            purpose: _,
            selection,
        } => vec![
            OverlayRow {
                selected: *selection == 0,
                text: format!("Create skill draft: {skill_name}"),
            },
            OverlayRow {
                selected: *selection == 1,
                text: "Dismiss".to_string(),
            },
        ],
        OverlayState::ApiKeyPrompt { value, .. } => vec![
            OverlayRow {
                selected: false,
                text: "Paste an API key and press Enter.".to_string(),
            },
            OverlayRow {
                selected: true,
                text: format!("key  {}", masked_secret(value)),
            },
        ],
        OverlayState::OnboardingApiKey { input, .. } => vec![
            OverlayRow {
                selected: false,
                text: "Paste an API key and press Enter.".to_string(),
            },
            OverlayRow {
                selected: true,
                text: format!("key  {}", masked_secret(input)),
            },
        ],
        OverlayState::UserQuestionPrompt { overlay } => overlay
            .rows()
            .into_iter()
            .map(|(selected, text)| OverlayRow { selected, text })
            .collect(),
        OverlayState::PermissionPrompt { .. }
        | OverlayState::Help
        | OverlayState::Btw(..)
        | OverlayState::Session(..)
        | OverlayState::Status(..)
        | OverlayState::Text(..)
        | OverlayState::Usage(..) => Vec::new(),
    }
}

pub(super) fn masked_secret(value: &str) -> String {
    if value.is_empty() {
        return "<empty>".to_string();
    }
    "*".repeat(value.chars().count().min(32))
}

pub(super) fn render_model_entry(entry: &ModelPickerEntry) -> String {
    if entry.description.trim().is_empty() {
        return entry.selector.clone();
    }
    if entry
        .selector
        .eq_ignore_ascii_case(entry.description.trim())
    {
        return entry.description.clone();
    }
    format!("{}  {}", entry.selector, entry.description)
}

fn render_auth_entry(entry: &AuthPickerEntry) -> String {
    format!("{}  {}", entry.label, entry.description)
}

fn render_session_entry(session: &SessionSummary) -> String {
    let mut context = vec![path_tail(&session.cwd)];
    if let Some(slug) = session
        .slug
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        context.push(format!("slug={slug}"));
    }
    if !session.tags.is_empty() {
        context.push(format!("#{}", session.tags.join(" #")));
    }
    format!(
        "{}  {}  {}",
        short_id(&session.id.to_string()),
        session_title(session),
        context.join("  ")
    )
}

fn session_title(session: &SessionSummary) -> &str {
    session
        .display_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            session
                .generated_title
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or("<unnamed>")
}

fn path_tail(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;

    #[test]
    fn render_session_entry_uses_generated_title_when_display_name_missing() {
        let session = SessionSummary {
            id: Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap(),
            display_name: None,
            generated_title: Some("Fix session naming".to_string()),
            cwd: PathBuf::from("/workspace/puffer"),
            created_at_ms: 1,
            updated_at_ms: 2,
            event_count: 3,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };

        let row = render_session_entry(&session);

        assert!(row.contains("Fix session naming"));
        assert!(!row.contains("<unnamed>"));
    }
}
