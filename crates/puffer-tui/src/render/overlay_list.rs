use super::OverlayRow;
use crate::OverlayState;

pub(super) fn onboarding_fixed_line_count(overlay: &OverlayState) -> usize {
    7 + usize::from(onboarding_note(overlay).is_some())
}

pub(super) fn overlay_selection(overlay: &OverlayState) -> Option<usize> {
    match overlay {
        OverlayState::SessionPicker { selection, .. }
        | OverlayState::AgentPicker { selection, .. }
        | OverlayState::ModelPicker { selection, .. }
        | OverlayState::EffortPicker { selection, .. }
        | OverlayState::FastModePicker { selection, .. }
        | OverlayState::LoginPicker { selection, .. }
        | OverlayState::ProviderPicker { selection, .. }
        | OverlayState::AuthPicker { selection, .. }
        | OverlayState::LogoutPicker { selection, .. }
        | OverlayState::ThemePicker { selection, .. }
        | OverlayState::CommandPicker { selection, .. }
        | OverlayState::OnboardingTheme { selection, .. }
        | OverlayState::OnboardingProvider { selection, .. }
        | OverlayState::OnboardingAuth { selection, .. }
        | OverlayState::OnboardingModel { selection, .. }
        | OverlayState::AutoDreamSuggestion { selection, .. } => Some(*selection),
        OverlayState::PermissionPrompt { overlay } => Some(overlay.selection()),
        OverlayState::UserQuestionPrompt { overlay } => Some(overlay.selection()),
        OverlayState::ApiKeyPrompt { .. }
        | OverlayState::Help
        | OverlayState::Btw(..)
        | OverlayState::Session(..)
        | OverlayState::Status(..)
        | OverlayState::Text(..)
        | OverlayState::Usage(..)
        | OverlayState::OnboardingApiKey { .. } => None,
    }
}

pub(super) fn visible_overlay_rows(
    rows: Vec<OverlayRow>,
    selection: Option<usize>,
    max_rows: usize,
) -> Vec<OverlayRow> {
    if rows.len() <= max_rows || max_rows == 0 {
        return rows;
    }
    if max_rows <= 2 {
        let selection = selection.unwrap_or(0).min(rows.len().saturating_sub(1));
        let start = selection.min(rows.len().saturating_sub(max_rows));
        return rows[start..start + max_rows].to_vec();
    }
    let selection = selection.unwrap_or(0).min(rows.len().saturating_sub(1));
    let start = selection
        .saturating_sub(max_rows / 2)
        .min(rows.len().saturating_sub(max_rows));
    let end = start + max_rows;
    let hidden_above = start > 0;
    let hidden_below = end < rows.len();
    let mut visible = rows[start..end].to_vec();
    if hidden_above {
        visible[0] = OverlayRow {
            text: format!("... {} above", start),
            selected: false,
        };
    }
    if hidden_below {
        let last = visible.len() - 1;
        visible[last] = OverlayRow {
            text: format!("... {} more", rows.len() - end),
            selected: false,
        };
    }
    visible
}

fn onboarding_note(overlay: &OverlayState) -> Option<&str> {
    match overlay {
        OverlayState::ThemePicker { .. } => Some("You can run /theme later."),
        OverlayState::AuthPicker { provider_id, .. } => Some(provider_id.as_str()),
        OverlayState::ModelPicker {
            provider_id,
            onboarding: true,
            ..
        } => Some(provider_id.as_str()),
        OverlayState::EffortPicker {
            provider_id,
            onboarding: true,
            ..
        } => Some(provider_id.as_str()),
        OverlayState::FastModePicker {
            provider_id,
            onboarding: true,
            ..
        } => Some(provider_id.as_str()),
        _ => None,
    }
}
