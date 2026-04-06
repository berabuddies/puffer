use crate::state::{
    load_agent_picker_entries, AuthPickerAction, AuthPickerEntry, ModelPickerEntry, OverlayState,
};
use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_core::AppState;
use puffer_provider_registry::{
    detect_import_candidates, AuthMode, AuthStore, ExternalImportCandidate, ExternalImportFamily,
    ExternalImportSource, ProviderDescriptor, ProviderRegistry,
};
use puffer_session_store::SessionStore;

/// Returns whether the current session still needs a selected provider/model pair.
pub(crate) fn needs_initial_provider_setup(
    state: &AppState,
    providers: &ProviderRegistry,
) -> bool {
    let Some(provider_id) = state.current_provider.as_deref() else {
        return true;
    };
    let Some(provider) = providers.provider(provider_id) else {
        return true;
    };
    if provider.models.is_empty() {
        return true;
    }
    let Some(model_selector) = state.current_model.as_deref() else {
        return false;
    };
    let Some((selected_provider, selected_model)) = model_selector.split_once('/') else {
        return false;
    };
    selected_provider != provider_id
        || !provider.models.iter().any(|model| model.id == selected_model)
}

/// Builds the initial onboarding overlay when provider/model setup is incomplete.
pub(crate) fn initial_overlay(
    state: &AppState,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> Result<Option<OverlayState>> {
    if !needs_initial_provider_setup(state, providers)
        && !missing_required_auth(state, providers, auth_store)
    {
        return Ok(None);
    }
    let paths = ConfigPaths::discover(&state.cwd);
    if !paths.has_user_config() {
        return Ok(Some(theme_picker()));
    }
    if let Some(provider_id) = state.current_provider.as_deref() {
        return provider_setup_overlay(providers, auth_store, provider_id);
    }
    Ok(provider_picker(providers, true))
}

/// Builds a command-driven picker overlay, including provider-scoped model selection.
pub(crate) fn overlay_from_command(
    state: &AppState,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
    submitted: &str,
) -> Result<Option<OverlayState>> {
    let without_slash = submitted.trim_start_matches('/');
    let (name, args) = without_slash
        .split_once(' ')
        .map(|(name, args)| (name, args.trim()))
        .unwrap_or((without_slash, ""));

    let overlay = match name {
        "resume" | "continue" if args.is_empty() => {
            let sessions = session_store.list_sessions()?;
            if sessions.is_empty() {
                None
            } else {
                Some(OverlayState::SessionPicker {
                    sessions,
                    selection: 0,
                })
            }
        }
        "model" if args.is_empty() => state
            .current_provider
            .as_deref()
            .and_then(|provider_id| model_picker(providers, provider_id, false)),
        "agents" if args.is_empty() => {
            let entries = load_agent_picker_entries(&state.cwd, state.current_model.as_deref())?;
            if entries.is_empty() {
                None
            } else {
                Some(OverlayState::AgentPicker {
                    entries,
                    selection: 0,
                })
            }
        }
        "login" if args.is_empty() => login_provider_picker(providers),
        "login" if !args.is_empty() => auth_picker(providers, auth_store, args, false)?,
        "logout" if args.is_empty() => logout_picker(providers, auth_store),
        "theme" if args.is_empty() => Some(theme_picker()),
        _ => None,
    };
    Ok(overlay)
}

/// Builds the next auth-or-model overlay for one provider.
pub(crate) fn provider_setup_overlay(
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    provider_id: &str,
) -> Result<Option<OverlayState>> {
    let Some(provider) = providers.provider(provider_id) else {
        return Ok(None);
    };
    if needs_auth_choice(provider, auth_store) {
        return auth_picker(providers, auth_store, provider_id, true);
    }
    Ok(model_picker(providers, provider_id, true))
}

/// Returns the first provider step used after the initial theme selection.
pub(crate) fn initial_provider_overlay(
    providers: &ProviderRegistry,
) -> Option<OverlayState> {
    provider_picker(providers, true)
}

/// Returns the previous overlay to show when the user presses Escape.
pub(crate) fn back_overlay(
    overlay: &OverlayState,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> Result<Option<OverlayState>> {
    let next = match overlay {
        OverlayState::ApiKeyPrompt {
            provider_id,
            onboarding,
            ..
        } => auth_picker(providers, auth_store, provider_id, *onboarding)?,
        OverlayState::AuthPicker { onboarding, .. } => provider_picker(providers, *onboarding),
        OverlayState::ProviderPicker { onboarding, .. } if *onboarding => Some(theme_picker()),
        OverlayState::ModelPicker { onboarding, .. } if *onboarding => provider_picker(providers, true),
        OverlayState::SessionPicker { .. }
        | OverlayState::AgentPicker { .. }
        | OverlayState::ModelPicker { .. }
        | OverlayState::ProviderPicker { .. }
        | OverlayState::LoginPicker { .. }
        | OverlayState::LogoutPicker { .. }
        | OverlayState::ThemePicker { .. } => None,
    };
    Ok(next)
}

fn missing_required_auth(
    state: &AppState,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> bool {
    let Some(provider_id) = state.current_provider.as_deref() else {
        return false;
    };
    let Some(provider) = providers.provider(provider_id) else {
        return false;
    };
    needs_auth_choice(provider, auth_store)
}

fn needs_auth_choice(provider: &ProviderDescriptor, auth_store: &AuthStore) -> bool {
    let supports_local_auth = provider.auth_modes.contains(&AuthMode::OAuth)
        || provider.auth_modes.contains(&AuthMode::ApiKey);
    supports_local_auth && !auth_store.has_auth(&provider.id)
}

fn provider_picker(providers: &ProviderRegistry, onboarding: bool) -> Option<OverlayState> {
    let mut providers = providers
        .providers()
        .filter(|provider| {
            !provider.models.is_empty() && (onboarding || !provider.auth_modes.is_empty())
        })
        .collect::<Vec<_>>();
    providers.sort_by_key(|provider| provider_rank(provider.id.as_str()));
    let entries = providers.into_iter().map(provider_entry).collect::<Vec<_>>();
    if entries.is_empty() {
        return None;
    }
    if onboarding {
        Some(OverlayState::ProviderPicker {
            entries,
            selection: 0,
            onboarding,
        })
    } else {
        Some(OverlayState::LoginPicker {
            entries,
            selection: 0,
        })
    }
}

fn login_provider_picker(providers: &ProviderRegistry) -> Option<OverlayState> {
    let mut providers = providers
        .providers()
        .filter(|provider| !provider.models.is_empty())
        .filter(|provider| !provider.auth_modes.is_empty())
        .collect::<Vec<_>>();
    providers.sort_by_key(|provider| provider_rank(provider.id.as_str()));
    let entries = providers
        .into_iter()
        .map(|provider| ModelPickerEntry {
            selector: provider.id.clone(),
            description: provider.display_name.clone(),
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        None
    } else {
        Some(OverlayState::LoginPicker {
            entries,
            selection: 0,
        })
    }
}

fn provider_entry(provider: &ProviderDescriptor) -> ModelPickerEntry {
    ModelPickerEntry {
        selector: provider.id.clone(),
        description: provider.display_name.clone(),
    }
}

fn auth_picker(
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    provider_id: &str,
    onboarding: bool,
) -> Result<Option<OverlayState>> {
    let Some(provider) = providers.provider(provider_id) else {
        return Ok(None);
    };

    let mut entries = Vec::new();
    if auth_store.has_auth(provider_id) {
        entries.push(AuthPickerEntry {
            label: "stored".to_string(),
            description: "Use the stored credentials in ~/.puffer/auth.json".to_string(),
            action: AuthPickerAction::UseStored,
        });
    }
    if provider.auth_modes.contains(&AuthMode::OAuth) {
        entries.push(AuthPickerEntry {
            label: "oauth".to_string(),
            description: "Sign in with your browser".to_string(),
            action: AuthPickerAction::OAuth,
        });
    }
    if provider.auth_modes.contains(&AuthMode::ApiKey) {
        entries.push(AuthPickerEntry {
            label: "api-key".to_string(),
            description: "Paste and store an API key".to_string(),
            action: AuthPickerAction::ApiKey,
        });
    }
    for candidate in import_candidates(provider)? {
        entries.push(AuthPickerEntry {
            label: import_label(&candidate),
            description: candidate.description.clone(),
            action: AuthPickerAction::Import(candidate),
        });
    }
    if entries.is_empty() {
        entries.push(AuthPickerEntry {
            label: "continue".to_string(),
            description: "This provider does not require stored credentials".to_string(),
            action: AuthPickerAction::NoneRequired,
        });
    }
    Ok(Some(OverlayState::AuthPicker {
        provider_id: provider.id.clone(),
        entries,
        selection: 0,
        onboarding,
    }))
}

fn import_candidates(provider: &ProviderDescriptor) -> Result<Vec<ExternalImportCandidate>> {
    let Some(family) = import_family(provider) else {
        return Ok(Vec::new());
    };
    detect_import_candidates(family)
}

fn import_family(provider: &ProviderDescriptor) -> Option<ExternalImportFamily> {
    match provider.default_api.as_str() {
        "anthropic-messages" => Some(ExternalImportFamily::Anthropic),
        "openai-responses"
        | "openai-completions"
        | "openai-codex-responses"
        | "azure-openai-responses" => Some(ExternalImportFamily::OpenAi),
        _ => None,
    }
}

fn import_label(candidate: &ExternalImportCandidate) -> String {
    match candidate.source {
        ExternalImportSource::Claude => "import-claude".to_string(),
        ExternalImportSource::Codex => "import-codex".to_string(),
    }
}

fn model_picker(
    providers: &ProviderRegistry,
    provider_id: &str,
    onboarding: bool,
) -> Option<OverlayState> {
    let provider = providers.provider(provider_id)?;
    let entries = provider
        .models
        .iter()
        .map(|model| ModelPickerEntry {
            selector: model.id.clone(),
            description: model.display_name.clone(),
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        None
    } else {
        Some(OverlayState::ModelPicker {
            provider_id: provider.id.clone(),
            entries,
            selection: 0,
            onboarding,
        })
    }
}

fn logout_picker(
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> Option<OverlayState> {
    let entries = auth_store
        .provider_ids()
        .map(|provider_id| ModelPickerEntry {
            selector: provider_id.to_string(),
            description: providers
                .provider(provider_id)
                .map(|provider| provider.display_name.clone())
                .unwrap_or_else(|| provider_id.to_string()),
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        None
    } else {
        Some(OverlayState::LogoutPicker {
            entries,
            selection: 0,
        })
    }
}

fn theme_picker() -> OverlayState {
    OverlayState::ThemePicker {
        entries: vec![
            ModelPickerEntry {
                selector: "puffer".to_string(),
                description: "Default Puffer text style".to_string(),
            },
            ModelPickerEntry {
                selector: "harbor".to_string(),
                description: "Calmer contrast for long sessions".to_string(),
            },
            ModelPickerEntry {
                selector: "sunrise".to_string(),
                description: "Warmer light-on-dark palette".to_string(),
            },
        ],
        selection: 0,
    }
}

fn provider_rank(provider_id: &str) -> (u8, &str) {
    match provider_id {
        "anthropic" => (0, provider_id),
        "openai" | "openai-codex" | "azure-openai-responses" => (1, provider_id),
        _ => (2, provider_id),
    }
}
