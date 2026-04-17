use super::emit_system;
use crate::runtime::{LlmJudgeConfig, LlmJudgeMode, ReflectionConfig, ReflectionLanguage};
use crate::AppState;
use anyhow::Result;
use puffer_session_store::SessionStore;

const USAGE: &str = "Usage: /reflect [on|off|toggle|status|lang <zh|en>|mode <confirm|independent>|llm <on|off>|help]\nControls the runtime reflection policy (stall/loop detection) for this session.";

/// Handles the `/reflect` slash command: toggles and configures the session-scoped
/// reflection policy used by `execute_user_prompt_*` for non-benchmark turns.
pub(crate) fn handle_reflect_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let trimmed = args.trim();
    if trimmed.is_empty()
        || matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "status" | "show" | "current" | "info"
        )
    {
        return emit_system(state, session_store, render_reflect_status(state));
    }
    if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "help" | "--help" | "-h"
    ) {
        return emit_system(state, session_store, USAGE.to_string());
    }

    let lower = trimmed.to_ascii_lowercase();
    let message = match lower.as_str() {
        "on" | "true" | "1" | "enable" => {
            if state.reflection_config.is_none() {
                state.reflection_config = Some(ReflectionConfig::default());
            }
            format!("Reflection turned on.\n{}", render_reflect_status(state))
        }
        "off" | "false" | "0" | "disable" => {
            state.reflection_config = None;
            "Reflection turned off.".to_string()
        }
        "toggle" => {
            if state.reflection_config.is_some() {
                state.reflection_config = None;
                "Reflection toggled off.".to_string()
            } else {
                state.reflection_config = Some(ReflectionConfig::default());
                format!("Reflection toggled on.\n{}", render_reflect_status(state))
            }
        }
        other if other.starts_with("lang") => {
            return apply_language(state, session_store, strip_prefix_arg(other, "lang"));
        }
        other if other.starts_with("mode") => {
            return apply_llm_mode(state, session_store, strip_prefix_arg(other, "mode"));
        }
        other if other.starts_with("llm") => {
            return apply_llm_toggle(state, session_store, strip_prefix_arg(other, "llm"));
        }
        _ => {
            return emit_system(
                state,
                session_store,
                format!("Unknown /reflect argument: {trimmed}\n{USAGE}"),
            );
        }
    };

    emit_system(state, session_store, message)
}

fn strip_prefix_arg<'a>(lower: &'a str, prefix: &str) -> &'a str {
    lower
        .strip_prefix(prefix)
        .map(str::trim)
        .unwrap_or_default()
}

fn apply_language(state: &mut AppState, session_store: &SessionStore, value: &str) -> Result<()> {
    let language = match value {
        "zh" | "chinese" | "cn" => ReflectionLanguage::Chinese,
        "en" | "english" => ReflectionLanguage::English,
        "" => {
            return emit_system(state, session_store, format!("Missing language. {USAGE}"));
        }
        other => {
            return emit_system(
                state,
                session_store,
                format!("Unsupported language {other:?}. Use zh or en."),
            );
        }
    };
    let config = state
        .reflection_config
        .get_or_insert_with(ReflectionConfig::default);
    config.language = language;
    emit_system(
        state,
        session_store,
        format!(
            "Reflection language set to {}.\n{}",
            language_label(language),
            render_reflect_status(state)
        ),
    )
}

fn apply_llm_mode(state: &mut AppState, session_store: &SessionStore, value: &str) -> Result<()> {
    let mode = match value {
        "confirm" | "confirm-code-judge" | "confirm_code_judge" => LlmJudgeMode::ConfirmCodeJudge,
        "independent" | "indep" => LlmJudgeMode::Independent,
        "" => {
            return emit_system(state, session_store, format!("Missing mode. {USAGE}"));
        }
        other => {
            return emit_system(
                state,
                session_store,
                format!("Unsupported mode {other:?}. Use confirm or independent."),
            );
        }
    };
    let config = state
        .reflection_config
        .get_or_insert_with(ReflectionConfig::default);
    let judge = config.llm_judge.get_or_insert_with(LlmJudgeConfig::default);
    judge.mode = mode;
    emit_system(
        state,
        session_store,
        format!(
            "Reflection LLM judge mode set to {}.\n{}",
            llm_mode_label(mode),
            render_reflect_status(state)
        ),
    )
}

fn apply_llm_toggle(state: &mut AppState, session_store: &SessionStore, value: &str) -> Result<()> {
    let enable = match value {
        "on" | "true" | "1" | "enable" => true,
        "off" | "false" | "0" | "disable" => false,
        "" => {
            return emit_system(state, session_store, format!("Missing llm toggle. {USAGE}"));
        }
        other => {
            return emit_system(
                state,
                session_store,
                format!("Unsupported llm toggle {other:?}. Use on or off."),
            );
        }
    };
    let config = state
        .reflection_config
        .get_or_insert_with(ReflectionConfig::default);
    if enable {
        if config.llm_judge.is_none() {
            config.llm_judge = Some(LlmJudgeConfig::default());
        }
    } else {
        config.llm_judge = None;
    }
    emit_system(
        state,
        session_store,
        format!(
            "Reflection LLM judge {}.\n{}",
            if enable { "enabled" } else { "disabled" },
            render_reflect_status(state)
        ),
    )
}

fn render_reflect_status(state: &AppState) -> String {
    let Some(config) = state.reflection_config.as_ref() else {
        return "Reflection is off.".to_string();
    };
    let mut lines = Vec::new();
    lines.push("Reflection is on.".to_string());
    lines.push(format!("- language: {}", language_label(config.language)));
    lines.push(format!(
        "- code_judge: {}",
        if config.code_judge.is_some() {
            "enabled"
        } else {
            "disabled"
        }
    ));
    match config.llm_judge.as_ref() {
        Some(judge) => {
            lines.push(format!(
                "- llm_judge: enabled (mode={}, model={}, effort={})",
                llm_mode_label(judge.mode),
                judge.model_selector.as_deref().unwrap_or("<default>"),
                judge.effort_level.as_deref().unwrap_or("<default>"),
            ));
        }
        None => lines.push("- llm_judge: disabled".to_string()),
    }
    lines.join("\n")
}

fn language_label(language: ReflectionLanguage) -> &'static str {
    match language {
        ReflectionLanguage::Chinese => "zh",
        ReflectionLanguage::English => "en",
    }
}

fn llm_mode_label(mode: LlmJudgeMode) -> &'static str {
    match mode {
        LlmJudgeMode::ConfirmCodeJudge => "confirm",
        LlmJudgeMode::Independent => "independent",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use tempfile::tempdir;

    fn setup() -> (AppState, SessionStore, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let paths = ConfigPaths::discover(tmp.path());
        ensure_workspace_dirs(&paths).unwrap();
        let store = SessionStore::from_paths(&paths).unwrap();
        let session = store.create_session(tmp.path().to_path_buf()).unwrap();
        let state = AppState::new(PufferConfig::default(), tmp.path().to_path_buf(), session);
        (state, store, tmp)
    }

    #[test]
    fn defaults_to_off() {
        let (mut state, store, _tmp) = setup();
        assert!(state.reflection_config.is_none());
        handle_reflect_command(&mut state, &store, "").unwrap();
        assert!(state.reflection_config.is_none());
    }

    #[test]
    fn on_then_off() {
        let (mut state, store, _tmp) = setup();
        handle_reflect_command(&mut state, &store, "on").unwrap();
        assert!(state.reflection_config.is_some());
        handle_reflect_command(&mut state, &store, "off").unwrap();
        assert!(state.reflection_config.is_none());
    }

    #[test]
    fn lang_implicitly_enables() {
        let (mut state, store, _tmp) = setup();
        handle_reflect_command(&mut state, &store, "lang en").unwrap();
        let config = state.reflection_config.as_ref().expect("enabled");
        assert_eq!(config.language, ReflectionLanguage::English);
    }

    #[test]
    fn mode_switches_llm_judge() {
        let (mut state, store, _tmp) = setup();
        handle_reflect_command(&mut state, &store, "on").unwrap();
        handle_reflect_command(&mut state, &store, "mode independent").unwrap();
        let judge = state
            .reflection_config
            .as_ref()
            .and_then(|config| config.llm_judge.as_ref())
            .expect("llm judge present");
        assert_eq!(judge.mode, LlmJudgeMode::Independent);
    }

    #[test]
    fn llm_off_keeps_code_judge() {
        let (mut state, store, _tmp) = setup();
        handle_reflect_command(&mut state, &store, "on").unwrap();
        handle_reflect_command(&mut state, &store, "llm off").unwrap();
        let config = state.reflection_config.as_ref().unwrap();
        assert!(config.llm_judge.is_none());
        assert!(config.code_judge.is_some());
    }

    #[test]
    fn unknown_argument_is_not_fatal() {
        let (mut state, store, _tmp) = setup();
        handle_reflect_command(&mut state, &store, "wat").unwrap();
        assert!(state.reflection_config.is_none());
    }
}
