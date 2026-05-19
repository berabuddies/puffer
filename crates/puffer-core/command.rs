use crate::command_helpers::{
    append_tool_invocations, append_trace_events, describe_context, describe_files_in_context,
    describe_git_diff, emit_system, execute_skill_command, handle_agents_command,
    handle_branch_command, handle_config_command, handle_copy_command, handle_effort_command,
    handle_export_command, handle_fast_command, handle_goal_command, handle_hooks_command,
    handle_ide_command, handle_keybindings_command, handle_mcp_command, handle_memory_command,
    handle_model_command, handle_permissions_command, handle_plan_command, handle_plugin_command,
    handle_recap_command, handle_reflect_command, handle_remote_control_command,
    handle_remote_env_command, handle_resume_command, handle_sandbox_command,
    handle_session_command, handle_tag_command, handle_tasks_command,
    handle_terminal_setup_command, list_skills, persist_user_settings, record_command_checkpoint,
    reload_config_from_disk, remove_provider_credentials, render_login_guidance, rewind_transcript,
    run_doctor, run_provider_login_flow, should_hide_terminal_setup_command, supports_auth_mode,
    terminal_setup_command_description,
};
use crate::{
    render_buddy_summary, render_cost_summary, render_status_summary, render_usage_summary,
    workspace_paths, AppState, MessageRole,
};
use anyhow::Result;
use puffer_provider_registry::{canonical_provider_id, AuthMode, AuthStore, ProviderRegistry};
use puffer_resources::{prompt_by_id, render_prompt_for, skill_by_name, LoadedResources};
use puffer_session_store::{SessionStore, TranscriptEvent};
use serde::Serialize;
use std::fmt::Write as _;

/// Distinguishes prompt-backed, local, and UI-oriented slash commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CommandKind {
    Prompt,
    Local,
    Ui,
}

/// Declares one built-in slash command supported by Puffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandSpec {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub argument_hint: Option<String>,
    pub kind: CommandKind,
    pub hidden: bool,
}

/// Returns the full built-in slash-command surface supported by Puffer.
pub fn supported_commands() -> Vec<CommandSpec> {
    vec![
        cmd(
            "add-dir",
            &[],
            "Add a new working directory",
            Some("<path>"),
            CommandKind::Local,
        ),
        cmd(
            "agents",
            &[],
            "Manage agent configurations",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "branch",
            &["fork"],
            "Create a branch of the current conversation at this point",
            Some("[name]"),
            CommandKind::Local,
        ),
        cmd(
            "btw",
            &[],
            "Ask a quick side question without interrupting the main conversation",
            Some("<question>"),
            CommandKind::Local,
        ),
        cmd(
            "buddy",
            &[],
            "Show or interact with Clawd",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "clear",
            &["reset", "new"],
            "Clear conversation history and free up context",
            None,
            CommandKind::Local,
        ),
        cmd(
            "color",
            &[],
            "Set the prompt bar color for this session",
            Some("<color|default>"),
            CommandKind::Ui,
        ),
        cmd(
            "commit",
            &[],
            "Create a git commit",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "compact",
            &[],
            "Summarize the conversation to preserve context budget",
            Some("<optional custom summarization instructions>"),
            CommandKind::Prompt,
        ),
        cmd(
            "config",
            &["settings"],
            "Open config panel",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "context",
            &[],
            "Show current context usage",
            None,
            CommandKind::Local,
        ),
        cmd(
            "copy",
            &[],
            "Copy the latest assistant output",
            None,
            CommandKind::Local,
        ),
        cmd(
            "cost",
            &[],
            "Show the total cost and duration of the current session",
            None,
            CommandKind::Local,
        ),
        cmd(
            "diff",
            &[],
            "View uncommitted changes and per-turn diffs",
            None,
            CommandKind::Local,
        ),
        cmd(
            "debug",
            &[],
            "Show the full raw context sent to the model",
            None,
            CommandKind::Local,
        ),
        cmd(
            "doctor",
            &[],
            "Diagnose and verify your Puffer Code installation and settings",
            None,
            CommandKind::Local,
        ),
        cmd(
            "effort",
            &[],
            "Set effort level for model usage",
            Some("[minimal|low|medium|high|xhigh|max|auto]"),
            CommandKind::Ui,
        ),
        cmd("exit", &["quit"], "Exit the REPL", None, CommandKind::Local),
        cmd(
            "export",
            &[],
            "Export the current conversation to a file or clipboard",
            Some("[filename]"),
            CommandKind::Local,
        ),
        cmd(
            "files",
            &[],
            "List all files currently in context",
            None,
            CommandKind::Local,
        ),
        cmd(
            "fast",
            &[],
            "Toggle fast mode",
            Some("[on|off]"),
            CommandKind::Ui,
        ),
        cmd(
            "goal",
            &["objective"],
            "Set or view the goal for a long-running task",
            Some("[<text> | clear | budget <N> | status]"),
            CommandKind::Local,
        ),
        cmd(
            "help",
            &["?"],
            "Show help and available commands",
            None,
            CommandKind::Local,
        ),
        cmd(
            "hooks",
            &[],
            "View hook configurations for tool events",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "ide",
            &[],
            "Manage IDE integrations and show status",
            Some("[open]"),
            CommandKind::Ui,
        ),
        cmd(
            "init",
            &[],
            "Initialize project guidance and defaults",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "keybindings",
            &[],
            "Open or create your keybindings configuration file",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "login",
            &[],
            "Sign in to a provider",
            Some("[provider]"),
            CommandKind::Ui,
        ),
        cmd(
            "loop",
            &[],
            "Run a prompt on a recurring interval until a condition is met",
            Some("<interval> <prompt> | stop"),
            CommandKind::Local,
        ),
        cmd(
            "logout",
            &[],
            "Sign out from a provider",
            Some("[provider]"),
            CommandKind::Ui,
        ),
        cmd(
            "maximize",
            &["max"],
            "Iteratively maximize a metric via repeated prompt execution",
            Some("<metric> <prompt>"),
            CommandKind::Local,
        ),
        cmd(
            "mcp",
            &[],
            "Manage MCP servers",
            Some("[enable|disable [server-name]]"),
            CommandKind::Ui,
        ),
        cmd(
            "minimize",
            &["min"],
            "Iteratively minimize a metric via repeated prompt execution",
            Some("<metric> <prompt>"),
            CommandKind::Local,
        ),
        cmd("memory", &[], "Edit memory files", None, CommandKind::Ui),
        cmd(
            "recap",
            &[],
            "Summarize the session in 1-2 sentences (transient, not persisted)",
            None,
            CommandKind::Local,
        ),
        cmd(
            "model",
            &[],
            "Select the active model",
            Some("[provider/model]"),
            CommandKind::Ui,
        ),
        cmd(
            "permissions",
            &["allowed-tools"],
            "Manage allow & deny tool permission rules",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "plan",
            &[],
            "Enable plan mode or view the current session plan",
            Some("[open|<description>]"),
            CommandKind::Local,
        ),
        cmd(
            "plugin",
            &["plugins", "marketplace"],
            "Manage Puffer plugins",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "pr-comments",
            &[],
            "Get comments from a GitHub pull request",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "reflect",
            &[],
            "Toggle runtime reflection (stall/loop detection) for this session",
            Some("[on|off|toggle|status|lang <zh|en>|mode <confirm|independent>|llm <on|off>]"),
            CommandKind::Local,
        ),
        cmd(
            "reload-plugins",
            &[],
            "Activate pending plugin changes in the current session",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "remote-control",
            &["rc"],
            "Connect this terminal for remote-control sessions",
            Some("[name]"),
            CommandKind::Ui,
        ),
        cmd(
            "remote-env",
            &[],
            "Configure the remote environment for this session",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "rename",
            &[],
            "Rename the current conversation",
            Some("[name]"),
            CommandKind::Local,
        ),
        cmd(
            "resume",
            &["continue"],
            "Resume a previous conversation",
            Some("[conversation id or search term]"),
            CommandKind::Ui,
        ),
        cmd(
            "rewind",
            &["checkpoint"],
            "Restore the code and/or conversation to a previous point",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "review",
            &[],
            "Review the current worktree or pull request",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "sandbox",
            &[],
            "Manage sandbox policies",
            Some("exclude \"command pattern\""),
            CommandKind::Ui,
        ),
        cmd(
            "security-review",
            &[],
            "Complete a security review of the pending changes on the current branch",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "session",
            &["remote"],
            "Show remote session URL and QR code",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "skills",
            &[],
            "List available skills",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "skill:<name>",
            &[],
            "Run a loaded skill by slash-safe skill name",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "status",
            &[],
            "Show current session configuration and status",
            None,
            CommandKind::Local,
        ),
        cmd(
            "statusline",
            &[],
            "Set up Claude Code's status line UI",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "tag",
            &[],
            "Toggle a searchable tag on the current session",
            Some("<tag-name>"),
            CommandKind::Ui,
        ),
        cmd(
            "tasks",
            &["bashes"],
            "List and manage background tasks",
            None,
            CommandKind::Ui,
        ),
        cmd_hidden(
            "terminal-setup",
            &[],
            terminal_setup_command_description(),
            None,
            CommandKind::Local,
            should_hide_terminal_setup_command(),
        ),
        cmd("theme", &[], "Change the theme", None, CommandKind::Local),
        cmd(
            "usage",
            &[],
            "Show plan usage limits",
            None,
            CommandKind::Local,
        ),
        cmd(
            "vim",
            &[],
            "Toggle between Vim and Normal editing modes",
            None,
            CommandKind::Local,
        ),
    ]
}

/// Returns the full slash-command surface, including loaded user-invocable skills.
pub fn command_surface(resources: &LoadedResources) -> Vec<CommandSpec> {
    let mut commands = supported_commands();
    let reserved_names = commands
        .iter()
        .flat_map(|command| {
            std::iter::once(command.name.as_str()).chain(command.aliases.iter().map(String::as_str))
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut skills = resources
        .skills
        .iter()
        .filter(|skill| skill.value.user_invocable)
        .filter(|skill| !reserved_names.contains(skill.value.name.as_str()))
        .map(|skill| CommandSpec {
            name: skill.value.name.clone(),
            aliases: vec![format!("skill:{}", skill.value.name)],
            description: skill.value.description.clone(),
            argument_hint: skill.value.argument_hint.clone(),
            kind: CommandKind::Prompt,
            hidden: false,
        })
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    commands.extend(skills);
    commands
}

/// Finds a command by canonical name or alias.
pub fn find_command<'a>(commands: &'a [CommandSpec], name: &str) -> Option<&'a CommandSpec> {
    commands
        .iter()
        .find(|command| command.name == name || command.aliases.iter().any(|alias| alias == name))
}

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
                "\nResource counts: prompts={} tools={} skills={} plugins={} mcp_servers={} ides={}",
                resources.prompts.len(),
                resources.tools.len(),
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
        "permissions" => handle_permissions_command(state, resources, session_store, args),
        "hooks" => handle_hooks_command(state, resources, session_store, args),
        "tasks" => handle_tasks_command(state, session_store, args),
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

fn cmd(
    name: &'static str,
    aliases: &'static [&'static str],
    description: impl Into<String>,
    argument_hint: Option<&'static str>,
    kind: CommandKind,
) -> CommandSpec {
    cmd_hidden(name, aliases, description, argument_hint, kind, false)
}

fn cmd_hidden(
    name: &'static str,
    aliases: &'static [&'static str],
    description: impl Into<String>,
    argument_hint: Option<&'static str>,
    kind: CommandKind,
    hidden: bool,
) -> CommandSpec {
    CommandSpec {
        name: name.to_string(),
        aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
        description: description.into(),
        argument_hint: argument_hint.map(str::to_string),
        kind,
        hidden,
    }
}

#[cfg(test)]
mod tests;
