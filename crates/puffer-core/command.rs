use crate::command_helpers::{
    append_tool_invocations, append_trace_events, describe_context, describe_files_in_context,
    describe_git_diff, emit_system, execute_connect_flow, execute_skill_command,
    handle_agents_command, handle_branch_command, handle_config_command, handle_copy_command,
    handle_effort_command, handle_export_command, handle_fast_command, handle_genskill_command,
    handle_goal_command, handle_hooks_command, handle_ide_command, handle_keybindings_command,
    handle_mcp_command, handle_memory_command, handle_model_command, handle_monitor_command,
    handle_permissions_command, handle_plan_command, handle_plugin_command, handle_recap_command,
    handle_reflect_command, handle_remote_control_command, handle_remote_env_command,
    handle_resume_command, handle_sandbox_command, handle_session_command, handle_tag_command,
    handle_tasks_command, handle_telegram_command, handle_terminal_setup_command,
    handle_workflows_command, list_skills, persist_user_settings, record_command_checkpoint,
    reload_config_from_disk, remove_provider_credentials, render_login_guidance, rewind_transcript,
    run_doctor, run_provider_login_flow, supports_auth_mode,
};
use crate::{
    render_buddy_summary, render_cost_summary, render_status_summary, render_usage_summary,
    workspace_paths, AppState, MessageRole,
};
use anyhow::Result;
use puffer_provider_registry::{canonical_provider_id, AuthMode, AuthStore, ProviderRegistry};
use puffer_resources::{prompt_by_id, render_prompt_for, skill_by_name, LoadedResources};
use puffer_session_store::{SessionStore, TranscriptEvent};
use std::fmt::Write as _;

mod registry;
pub use registry::{command_surface, find_command, supported_commands, CommandKind, CommandSpec};

/// Dispatches a slash-command against the current application state.
pub fn dispatch_command(
    state: &mut AppState,
    commands: &[CommandSpec],
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    session_store: &SessionStore,
    raw_input: &str,
) -> Result<()> {
    let trimmed = raw_input.trim();
    let without_slash = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let (name, args) = without_slash
        .split_once(' ')
        .map(|(name, args)| (name, args.trim()))
        .unwrap_or((without_slash, ""));

    if let Some(skill_name) = name.strip_prefix("skill:") {
        session_store.append_event(
            state.session.id,
            TranscriptEvent::CommandInvoked {
                name: format!("skill:{skill_name}"),
                args: args.to_string(),
                actor: Some(state.user_actor()),
            },
        )?;
        execute_skill_command(
            state,
            resources,
            providers,
            auth_store,
            session_store,
            skill_name,
            args,
        )?;
        return record_command_checkpoint(
            state,
            session_store,
            format!("skill:{skill_name}").as_str(),
            args,
        );
    }

    let Some(command) = find_command(commands, name) else {
        if skill_by_name(resources, name).is_some_and(|skill| skill.value.user_invocable) {
            session_store.append_event(
                state.session.id,
                TranscriptEvent::CommandInvoked {
                    name: name.to_string(),
                    args: args.to_string(),
                    actor: Some(state.user_actor()),
                },
            )?;
            execute_skill_command(
                state,
                resources,
                providers,
                auth_store,
                session_store,
                name,
                args,
            )?;
            return record_command_checkpoint(state, session_store, name, args);
        }
        return emit_system(state, session_store, format!("Unknown command: /{name}"));
    };

    session_store.append_event(
        state.session.id,
        TranscriptEvent::CommandInvoked {
            name: command.name.to_string(),
            args: args.to_string(),
            actor: Some(state.user_actor()),
        },
    )?;

    match command.kind {
        CommandKind::Prompt => {
            execute_prompt_command(
                state,
                resources,
                providers,
                auth_store,
                session_store,
                command,
                args,
            )?;
            record_command_checkpoint(state, session_store, &command.name, args)
        }
        CommandKind::Local | CommandKind::Ui => {
            execute_local_command(
                state,
                commands,
                resources,
                providers,
                auth_store,
                session_store,
                command,
                args,
            )?;
            record_command_checkpoint(state, session_store, &command.name, args)
        }
    }
}

fn execute_prompt_command(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    session_store: &SessionStore,
    command: &CommandSpec,
    args: &str,
) -> Result<()> {
    let preparation = crate::command_helpers::prompt::prepare_prompt_command_specialization(
        state,
        session_store,
        &command.name,
        args,
    )?;
    if matches!(
        preparation,
        Some(crate::command_helpers::prompt::PromptCommandPreparation::HandledLocally)
    ) {
        return Ok(());
    }

    if let Some(crate::command_helpers::prompt::PromptCommandPreparation::SideQuestion(question)) =
        preparation.clone()
    {
        match crate::runtime::execute_side_question(
            state, resources, providers, auth_store, &question,
        ) {
            Ok(turn) => {
                state.push_message(MessageRole::Assistant, turn.assistant_text.clone());
                session_store.append_event(
                    state.session.id,
                    TranscriptEvent::AssistantMessage {
                        text: turn.assistant_text,
                        actor: Some(state.assistant_actor()),
                    },
                )?;
            }
            Err(error) => {
                emit_system(
                    state,
                    session_store,
                    format!("Prompt command /btw failed: {error}"),
                )?;
            }
        }
        return Ok(());
    }

    let uses_direct_prompt = matches!(
        preparation,
        Some(crate::command_helpers::prompt::PromptCommandPreparation::DirectPrompt(_))
    );
    let prompt = prompt_by_id(resources, &command.name);
    let tool_filter = prompt
        .map(|prompt| crate::runtime::build_request_tool_filter(&prompt.value.allowed_tools))
        .transpose()?
        .flatten();
    if let Some(prompt) = prompt {
        if let Some(model_override) = prompt
            .value
            .model_override
            .as_deref()
            .and_then(|model_id| providers.resolve_model(model_id))
        {
            state.current_provider = Some(model_override.provider.clone());
            state.current_model =
                Some(format!("{}/{}", model_override.provider, model_override.id));
        } else if let Some(provider_override) = prompt.value.provider_override.as_ref() {
            state.current_provider = Some(provider_override.clone());
        }
    }
    let mut variables = std::collections::BTreeMap::from([
        ("ARGUMENTS".to_string(), args.to_string()),
        ("CWD".to_string(), state.cwd.display().to_string()),
        (
            "PROVIDER".to_string(),
            state.current_provider.clone().unwrap_or_default(),
        ),
        (
            "MODEL".to_string(),
            state.current_model.clone().unwrap_or_default(),
        ),
        ("SESSION_ID".to_string(), state.session.id.to_string()),
        (
            "SESSION_SLUG".to_string(),
            state.session.slug.clone().unwrap_or_default(),
        ),
    ]);
    if let Some(crate::command_helpers::prompt::PromptCommandPreparation::VariableOverrides(
        extra_variables,
    )) = preparation.as_ref()
    {
        variables.extend(extra_variables.clone());
    }
    let mut rendered = match preparation {
        Some(crate::command_helpers::prompt::PromptCommandPreparation::PromptOverride(prompt)) => {
            prompt
        }
        Some(crate::command_helpers::prompt::PromptCommandPreparation::DirectPrompt(prompt)) => {
            prompt
        }
        Some(crate::command_helpers::prompt::PromptCommandPreparation::SideQuestion(_)) => {
            unreachable!("handled above")
        }
        Some(crate::command_helpers::prompt::PromptCommandPreparation::VariableOverrides(_))
        | None => render_prompt_for(
            resources,
            &command.name,
            state.current_provider.as_deref(),
            state.current_model.as_deref(),
            &variables,
        )
        .unwrap_or_else(|| format!("Prompt command /{} invoked with: {}", command.name, args)),
        Some(crate::command_helpers::prompt::PromptCommandPreparation::HandledLocally) => {
            unreachable!("handled above")
        }
    };
    if !uses_direct_prompt {
        if let Some(mode) = prompt.and_then(|prompt| prompt.value.mode.as_deref()) {
            rendered = format!("Command mode: {mode}\n\n{rendered}");
        }
    }

    if command.name == "compact" {
        return crate::command_helpers::prompt::execute_compact_prompt_command(
            state,
            resources,
            providers,
            auth_store,
            session_store,
            &rendered,
            tool_filter.as_ref(),
        );
    }

    state.push_message(MessageRole::User, rendered.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: rendered.clone(),
            actor: Some(state.user_actor()),
        },
    )?;

    match crate::runtime::execute_user_prompt_with_tool_filter(
        state,
        resources,
        providers,
        auth_store,
        &rendered,
        tool_filter.as_ref(),
    ) {
        Ok(turn) => {
            append_tool_invocations(state, session_store, &turn.tool_invocations)?;
            append_trace_events(session_store, state.session.id, &turn.reflection_traces);
            state.push_message(MessageRole::Assistant, turn.assistant_text.clone());
            session_store.append_event(
                state.session.id,
                TranscriptEvent::AssistantMessage {
                    text: turn.assistant_text,
                    actor: Some(state.assistant_actor()),
                },
            )?;
            if command.name == "statusline" {
                let _ = reload_config_from_disk(state);
            }
        }
        Err(error) => {
            let message = format!("Prompt command /{} failed: {error}", command.name);
            emit_system(state, session_store, message)?;
        }
    }

    Ok(())
}

fn execute_local_command(
    state: &mut AppState,
    commands: &[CommandSpec],
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    session_store: &SessionStore,
    command: &CommandSpec,
    args: &str,
) -> Result<()> {
    match command.name.as_str() {
        "help" => {
            let commands = command_surface(resources);
            let mut text = String::from("Supported commands:\n");
            for command in commands.iter().filter(|command| !command.hidden) {
                let _ = writeln!(&mut text, "/{:<16} {}", command.name, command.description);
            }
            let _ = writeln!(
                &mut text,
                "\nRun /skills to inspect loaded skill commands and aliases."
            );
            let _ = writeln!(
                &mut text,
                "\nResource counts: prompts={} tools={} internal_tools={} skills={} plugins={} mcp_servers={} ides={}",
                resources.prompts.len(),
                resources.tools.len(),
                resources.internal_tools.len(),
                resources.skills.len(),
                resources.plugins.len(),
                resources.mcp_servers.len(),
                resources.ides.len()
            );
            emit_system(state, session_store, text)
        }
        "status" => emit_system(
            state,
            session_store,
            render_status_summary(state, resources, providers, auth_store),
        ),
        "usage" => emit_system(
            state,
            session_store,
            render_usage_summary(state, commands, resources, providers, auth_store),
        ),
        "cost" => emit_system(state, session_store, render_cost_summary(state)),
        "connect" => match execute_connect_flow(state, resources, args) {
            Ok(turn) => emit_system(state, session_store, turn.assistant_text),
            Err(error) => emit_system(state, session_store, format!("/connect failed: {error}")),
        },
        "clear" => {
            session_store.append_transcript_clear(state.session.id)?;
            state.apply_transcript_rewrite(&puffer_session_store::TranscriptRewrite::Clear);
            emit_system(state, session_store, "Transcript cleared.".to_string())
        }
        "add-dir" => {
            let validation = workspace_paths::validate_directory_for_workspace(
                &state.cwd,
                &state.working_dirs,
                args,
            )?;
            match validation {
                workspace_paths::AddDirectoryValidation::Success { absolute_path } => {
                    if !state
                        .working_dirs
                        .iter()
                        .any(|existing| existing == &absolute_path)
                    {
                        state.working_dirs.push(absolute_path.clone());
                        state.working_dirs.sort();
                    }
                    emit_system(
                        state,
                        session_store,
                        workspace_paths::add_directory_help_message(
                            &workspace_paths::AddDirectoryValidation::Success { absolute_path },
                        ),
                    )
                }
                other => emit_system(
                    state,
                    session_store,
                    workspace_paths::add_directory_help_message(&other),
                ),
            }
        }
        "exit" => {
            state.should_exit = true;
            emit_system(state, session_store, "Exiting Puffer Code.".to_string())
        }
        "color" => {
            if args.is_empty() {
                emit_system(
                    state,
                    session_store,
                    format!("Current prompt color is {}.", state.prompt_color),
                )
            } else {
                state.prompt_color = args.to_string();
                emit_system(
                    state,
                    session_store,
                    format!("Prompt color set to {}.", state.prompt_color),
                )
            }
        }
        "effort" => handle_effort_command(state, providers, session_store, args),
        "fast" => handle_fast_command(state, session_store, args),
        "genskill" => {
            let message = handle_genskill_command(state, resources, providers, auth_store, args)?;
            emit_system(state, session_store, message)
        }
        "theme" => {
            if args.is_empty() {
                emit_system(
                    state,
                    session_store,
                    format!("Current theme is {}.", state.config.theme),
                )
            } else {
                state.config.theme = args.to_string();
                persist_user_settings(state)?;
                emit_system(
                    state,
                    session_store,
                    format!("Theme set to {}.", state.config.theme),
                )
            }
        }
        "vim" => {
            state.vim_mode = !state.vim_mode;
            state.config.editor_mode = if state.vim_mode {
                "vim".to_string()
            } else {
                "normal".to_string()
            };
            persist_user_settings(state)?;
            let message = if state.vim_mode {
                "Editor mode set to vim. Use Escape key to toggle between INSERT and NORMAL modes."
            } else {
                "Editor mode set to normal. Using standard (readline) keyboard bindings."
            };
            emit_system(state, session_store, message.to_string())
        }
        "model" => handle_model_command(state, providers, auth_store, session_store, args),
        "monitor" => match handle_monitor_command(state, resources, providers, auth_store, args) {
            Ok(message) => emit_system(state, session_store, message),
            Err(error) => emit_system(state, session_store, format!("/monitor failed: {error}")),
        },
        "permissions" => handle_permissions_command(state, resources, session_store, args),
        "hooks" => handle_hooks_command(state, resources, session_store, args),
        "tasks" => {
            handle_tasks_command(state, resources, providers, auth_store, session_store, args)
        }
        "workflows" => match handle_workflows_command(state, args) {
            Ok(message) => emit_system(state, session_store, message),
            Err(error) => emit_system(state, session_store, format!("/workflows failed: {error}")),
        },
        "telegram" => match handle_telegram_command(state, resources, args) {
            Ok(message) => emit_system(state, session_store, message),
            Err(error) => emit_system(state, session_store, format!("/telegram failed: {error}")),
        },
        "config" => handle_config_command(state, session_store, args),
        "context" => describe_context(state, resources, providers, session_store),
        "debug" => {
            // Rendered as overlay in TUI; fallback to system message for non-TUI.
            let text = crate::render_debug_context(state, resources, providers)?;
            emit_system(state, session_store, text)
        }
        "plan" => handle_plan_command(state, resources, providers, auth_store, session_store, args),
        "goal" => handle_goal_command(state, session_store, args),
        "reflect" => handle_reflect_command(state, session_store, args),
        "agents" => handle_agents_command(state, session_store, args),
        "memory" => handle_memory_command(state, session_store, args),
        "recap" => {
            handle_recap_command(state, resources, providers, auth_store, session_store, args)
        }
        "keybindings" => handle_keybindings_command(state, session_store),
        "remote-control" => handle_remote_control_command(state, session_store, args),
        "remote-env" => handle_remote_env_command(state, session_store, args),
        "sandbox" => handle_sandbox_command(state, session_store, args),
        "rename" => {
            let name = if args.is_empty() {
                format!("session-{}", &state.session.id.to_string()[..8])
            } else {
                args.to_string()
            };
            session_store.rename_session(state.session.id, name.clone())?;
            state.session.display_name = Some(name.clone());
            emit_system(state, session_store, format!("Session renamed to {name}."))
        }
        "export" => handle_export_command(state, session_store, args),
        "copy" => handle_copy_command(state, session_store, args),
        "diff" => describe_git_diff(state, session_store),
        "files" => describe_files_in_context(state, session_store),
        "doctor" => run_doctor(state, resources, providers, auth_store, session_store),
        "buddy" => emit_system(state, session_store, render_buddy_summary(state, resources)),
        "skills" => list_skills(state, resources, session_store),
        "plugin" => handle_plugin_command(state, resources, session_store, args),
        "reload-plugins" => {
            state.reload_resources_requested = true;
            emit_system(
                state,
                session_store,
                "Reloading plugin changes from disk for this session...".to_string(),
            )
        }
        "mcp" => handle_mcp_command(state, resources, session_store, args),
        "ide" => handle_ide_command(state, resources, session_store, args),
        "login" => {
            let provider_arg = if args.is_empty() {
                state.current_provider.as_deref().unwrap_or("anthropic")
            } else {
                args
            };
            let descriptor = providers.provider(provider_arg);
            let provider = descriptor
                .map(|provider_descriptor| provider_descriptor.id.clone())
                .unwrap_or_else(|| canonical_provider_id(provider_arg));
            if descriptor
                .map(|provider_descriptor| provider_descriptor.auth_modes.is_empty())
                .unwrap_or(false)
            {
                return emit_system(
                    state,
                    session_store,
                    format!("{provider} does not require stored credentials."),
                );
            }
            if supports_auth_mode(descriptor, AuthMode::OAuth) {
                return match run_provider_login_flow(state, auth_store, &provider) {
                    Ok(message) => emit_system(state, session_store, message),
                    Err(error) => emit_system(
                        state,
                        session_store,
                        format!(
                            "Login failed for {provider}: {error}\n\n{}",
                            render_login_guidance(
                                &provider,
                                descriptor,
                                auth_store.has_auth(&provider)
                            )
                        ),
                    ),
                };
            }
            emit_system(
                state,
                session_store,
                render_login_guidance(&provider, descriptor, auth_store.has_auth(&provider)),
            )
        }
        "logout" => {
            let provider_arg = if args.is_empty() {
                state.current_provider.as_deref().unwrap_or("anthropic")
            } else {
                args
            };
            let descriptor = providers.provider(provider_arg);
            let provider = descriptor
                .map(|provider_descriptor| provider_descriptor.id.clone())
                .unwrap_or_else(|| canonical_provider_id(provider_arg));
            if descriptor
                .map(|descriptor| descriptor.auth_modes.is_empty())
                .unwrap_or(false)
            {
                return emit_system(
                    state,
                    session_store,
                    format!("{} does not use stored credentials.", provider),
                );
            }
            let message = remove_provider_credentials(state, auth_store, provider.as_str())?;
            emit_system(state, session_store, message)
        }
        "session" => handle_session_command(state, session_store, args),
        "tag" => handle_tag_command(state, session_store, args),
        "resume" => handle_resume_command(state, session_store, args),
        "branch" => handle_branch_command(state, session_store, args),
        "loop" | "maximize" | "minimize" => {
            // Handled in TUI flow layer which has access to TuiState.
            // If we reach here the command was dispatched outside the TUI.
            emit_system(
                state,
                session_store,
                format!("/{} requires the interactive TUI.", command.name),
            )
        }
        "rewind" => rewind_transcript(state, session_store, args),
        "terminal-setup" => handle_terminal_setup_command(state, session_store),
        _ => emit_system(
            state,
            session_store,
            format!(
                "/{} is registered as a {:?} command but is not fully implemented yet.",
                command.name, command.kind
            ),
        ),
    }
}

#[cfg(test)]
mod tests;
