use anyhow::{Context, Result};
use puffer_core::{AppState, MessageRole};
use puffer_provider_registry::ProviderRegistry;
use serde_json::{json, Value};
use std::io::Write as _;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const STATUS_LINE_TIMEOUT_MS: u64 = 500;

/// Refreshes the configured status line command output when the input snapshot changes.
pub(crate) fn refresh_status_line(
    state: &mut AppState,
    providers: &ProviderRegistry,
) -> Result<()> {
    if !state.statusline_enabled {
        state.status_line_text = None;
        state.set_status_line_signature(None);
        return Ok(());
    }
    let Some(config) = state
        .config
        .ui
        .status_line
        .as_ref()
        .filter(|config| !config.command.trim().is_empty())
    else {
        state.status_line_text = None;
        state.set_status_line_signature(None);
        return Ok(());
    };
    let command = config.command.clone();

    let input = build_status_line_input(state, providers);
    let signature = format!("{}\0{}", command, input);
    if state.status_line_signature() == Some(signature.as_str()) {
        return Ok(());
    }

    state.set_status_line_signature(Some(signature));
    state.status_line_text =
        run_status_line_command(&state.cwd, &command, &input).unwrap_or_default();
    Ok(())
}

fn build_status_line_input(state: &AppState, providers: &ProviderRegistry) -> String {
    let user_messages = state
        .transcript
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .count();
    let assistant_messages = state
        .transcript
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .count();
    let system_messages = state
        .transcript
        .iter()
        .filter(|message| message.role == MessageRole::System)
        .count();
    serde_json::to_string_pretty(&json!({
        "session_id": state.session.id,
        "session_name": state.session.display_name,
        "transcript_path": Value::Null,
        "cwd": state.cwd,
        "provider": state.current_provider,
        "model": {
            "id": model_id(state.current_model.as_deref()),
            "display_name": model_display_name(state.current_model.as_deref()),
        },
        "workspace": {
            "current_dir": state.cwd,
            "project_dir": state.session.cwd,
            "added_dirs": state.working_dirs,
        },
        "version": env!("CARGO_PKG_VERSION"),
        "output_style": {
            "name": "default",
        },
        "context_window": context_window_json(state, providers),
        "rate_limits": Value::Null,
        "vim": if state.vim_mode {
            json!({ "mode": "NORMAL" })
        } else {
            Value::Null
        },
        "agent": Value::Null,
        "worktree": Value::Null,
        "app": {
            "version": env!("CARGO_PKG_VERSION"),
        },
        "ui": {
            "theme": state.config.theme,
            "fast_mode": state.fast_mode,
            "plan_mode": state.plan_mode,
            "sandbox_mode": state.sandbox_mode,
            "vim_mode": state.vim_mode,
        },
        "transcript": {
            "message_count": state.transcript.len(),
            "user_messages": user_messages,
            "assistant_messages": assistant_messages,
            "system_messages": system_messages,
        },
        "remote": {
            "name": state.remote_name,
            "environment": state.remote_environment,
            "session_id": state.remote_session_id,
            "status": state.remote_session_status,
            "url": state.remote_session_url,
        }
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

fn context_window_json(state: &AppState, providers: &ProviderRegistry) -> Value {
    let remaining_pct = puffer_core::estimate_remaining_context_percent(state, providers);
    let context_window_size = state
        .current_model
        .as_deref()
        .and_then(|selector| providers.resolve_model(selector))
        .map(|model| model.context_window);
    match context_window_size {
        Some(size) if size > 0 => {
            let used_pct = 100u32.saturating_sub(remaining_pct);
            json!({
                "context_window_size": size,
                "used_percentage": used_pct,
                "remaining_percentage": remaining_pct,
            })
        }
        _ => json!({
            "context_window_size": Value::Null,
            "used_percentage": Value::Null,
            "remaining_percentage": Value::Null,
        }),
    }
}

fn model_id(model_id: Option<&str>) -> String {
    model_id
        .and_then(|model_id| model_id.rsplit('/').next())
        .unwrap_or("<unset>")
        .to_string()
}

fn model_display_name(model: Option<&str>) -> String {
    model_id(model)
}

fn run_status_line_command(
    cwd: &std::path::Path,
    command: &str,
    input: &str,
) -> Result<Option<String>> {
    let mut child = Command::new(puffer_tools::detected_shell())
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start status line command in {}", cwd.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }
    let output = wait_for_output(child, STATUS_LINE_TIMEOUT_MS)?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rendered = stdout
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if rendered.is_empty() {
        Ok(None)
    } else {
        Ok(Some(rendered))
    }
}

fn wait_for_output(mut child: std::process::Child, timeout_ms: u64) -> Result<Output> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if child.try_wait()?.is_some() {
            return child
                .wait_with_output()
                .context("failed to read status line output");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return child
                .wait_with_output()
                .context("failed to read timed out status line output");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::{build_status_line_input, refresh_status_line};
    use puffer_config::{PufferConfig, StatusLineConfig};
    use puffer_core::AppState;
    use puffer_provider_registry::ProviderRegistry;
    use puffer_session_store::SessionMetadata;
    use serde_json::Value;
    use std::path::PathBuf;
    use uuid::Uuid;

    #[test]
    fn refresh_status_line_executes_configured_command() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut config = PufferConfig::default();
        config.ui.status_line = Some(StatusLineConfig {
            command: r#"cat >/dev/null; printf 'openai gpt-5'"#.to_string(),
            padding: 0,
        });
        let mut state = AppState::new(
            config,
            tempdir.path().to_path_buf(),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: Some("dockyard".to_string()),
                generated_title: None,
                cwd: tempdir.path().to_path_buf(),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );
        state.current_provider = Some("openai".to_string());
        state.current_model = Some("openai/gpt-5".to_string());

        refresh_status_line(&mut state, &ProviderRegistry::new()).unwrap();

        assert_eq!(state.status_line_text.as_deref(), Some("openai gpt-5"));
        assert!(state.status_line_signature().is_some());
    }

    #[test]
    fn refresh_status_line_skips_command_when_disabled() {
        let tempdir = tempfile::tempdir().unwrap();
        let marker = tempdir.path().join("statusline-ran");
        let mut config = PufferConfig::default();
        config.ui.status_line = Some(StatusLineConfig {
            command: format!("printf ran > {}", marker.display()),
            padding: 0,
        });
        let mut state = AppState::new(
            config,
            tempdir.path().to_path_buf(),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: Some("dockyard".to_string()),
                generated_title: None,
                cwd: tempdir.path().to_path_buf(),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );
        state.statusline_enabled = false;
        state.status_line_text = Some("stale".to_string());
        state.set_status_line_signature(Some("sig".to_string()));

        refresh_status_line(&mut state, &ProviderRegistry::new()).unwrap();

        assert!(state.status_line_text.is_none());
        assert!(state.status_line_signature().is_none());
        assert!(!marker.exists());
    }

    #[test]
    fn refresh_status_line_clears_output_when_not_configured() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut state = AppState::new(
            PufferConfig::default(),
            tempdir.path().to_path_buf(),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: None,
                generated_title: None,
                cwd: PathBuf::from(tempdir.path()),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );
        state.status_line_text = Some("stale".to_string());
        state.set_status_line_signature(Some("sig".to_string()));

        refresh_status_line(&mut state, &ProviderRegistry::new()).unwrap();

        assert!(state.status_line_text.is_none());
        assert!(state.status_line_signature().is_none());
    }

    #[test]
    fn refresh_status_line_exposes_claude_compatible_fields() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut state = AppState::new(
            PufferConfig::default(),
            tempdir.path().to_path_buf(),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: None,
                generated_title: None,
                cwd: tempdir.path().to_path_buf(),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );
        state.current_model = Some("openai/gpt-5".to_string());
        state.vim_mode = true;

        let input: Value =
            serde_json::from_str(&build_status_line_input(&state, &ProviderRegistry::new()))
                .unwrap();

        assert_eq!(input["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(input["output_style"]["name"], "default");
        assert_eq!(input["model"]["id"], "gpt-5");
        assert_eq!(input["vim"]["mode"], "NORMAL");
        assert!(input["context_window"].is_object());
    }
}
