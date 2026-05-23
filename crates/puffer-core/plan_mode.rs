use crate::plans::{plan_file_path, plan_has_user_content, read_plan_text};
use crate::AppState;
use anyhow::Result;
use puffer_resources::{render_prompt_for, LoadedResources};
use std::collections::BTreeMap;

const TURNS_BETWEEN_ATTACHMENTS: usize = 5;
const FULL_REMINDER_EVERY_N_ATTACHMENTS: usize = 5;

/// Enters plan mode and resets the Claude-style reminder cadence.
pub fn enter_plan_mode(state: &mut AppState) -> Result<()> {
    if state.plan_mode {
        return Ok(());
    }
    state.plan_mode = true;
    state.plan_mode_attachment_turns = 0;
    state.plan_mode_attachment_count = 0;
    state.plan_mode_needs_exit_attachment = false;
    state.plan_mode_needs_reentry_attachment =
        state.plan_mode_has_exited && active_plan_exists(state)?;
    Ok(())
}

/// Exits plan mode and schedules the one-shot post-exit reminder.
pub(crate) fn exit_plan_mode(state: &mut AppState) {
    state.plan_mode = false;
    state.plan_mode_attachment_turns = 0;
    state.plan_mode_attachment_count = 0;
    state.plan_mode_has_exited = true;
    state.plan_mode_needs_reentry_attachment = false;
    state.plan_mode_needs_exit_attachment = true;
}

/// Renders the next plan-mode reminder without mutating state.
pub(crate) fn preview_plan_mode_context_message(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Option<String>> {
    let mut preview_state = state.clone();
    render_plan_mode_context_message(&mut preview_state, resources, false)
}

/// Renders and consumes the next plan-mode reminder for the active turn.
pub(crate) fn take_plan_mode_context_message(
    state: &mut AppState,
    resources: &LoadedResources,
) -> Result<Option<String>> {
    render_plan_mode_context_message(state, resources, true)
}

fn render_plan_mode_context_message(
    state: &mut AppState,
    resources: &LoadedResources,
    consume: bool,
) -> Result<Option<String>> {
    if !state.plan_mode {
        return render_exit_attachment(state, resources, consume);
    }

    let mut sections = Vec::new();
    if should_emit_reentry_prompt(state)? {
        if let Some(prompt) = render_reentry_prompt(state, resources)? {
            sections.push(prompt);
        }
        if consume {
            state.plan_mode_needs_reentry_attachment = false;
        }
    }

    if should_attach_plan_mode_prompt(state) {
        if let Some(prompt) = render_main_plan_prompt(state, resources)? {
            sections.push(prompt);
        }
        if consume {
            state.plan_mode_attachment_count += 1;
            state.plan_mode_attachment_turns = 0;
        }
    } else if consume {
        state.plan_mode_attachment_turns += 1;
    }

    Ok((!sections.is_empty()).then(|| sections.join("\n\n")))
}

fn render_exit_attachment(
    state: &mut AppState,
    resources: &LoadedResources,
    consume: bool,
) -> Result<Option<String>> {
    if !state.plan_mode_needs_exit_attachment {
        return Ok(None);
    }
    let mut variables = BTreeMap::new();
    if active_plan_exists(state)? {
        variables.insert(
            "PLAN_REFERENCE".to_string(),
            format!(
                " The plan file is located at {} if you need to reference it.",
                plan_file_path(state)?.display()
            ),
        );
    }
    let rendered = render_prompt_for(
        resources,
        "plan-mode-exited",
        state.current_provider.as_deref(),
        state.current_model.as_deref(),
        &variables,
    )
    .map(|prompt| prompt.trim().to_string())
    .filter(|prompt| !prompt.is_empty());
    if consume {
        state.plan_mode_needs_exit_attachment = false;
    }
    Ok(rendered)
}

fn should_emit_reentry_prompt(state: &AppState) -> Result<bool> {
    Ok(state.plan_mode_needs_reentry_attachment && active_plan_exists(state)?)
}

fn should_attach_plan_mode_prompt(state: &AppState) -> bool {
    state.plan_mode_attachment_count == 0
        || state.plan_mode_attachment_turns >= TURNS_BETWEEN_ATTACHMENTS
}

fn render_main_plan_prompt(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Option<String>> {
    let prompt_id = if next_attachment_is_full(state) {
        "plan-mode-interview"
    } else {
        "plan-mode-sparse"
    };
    let mut variables = BTreeMap::new();
    variables.insert(
        "PLAN_FILE_PATH".to_string(),
        plan_file_path(state)?.display().to_string(),
    );
    if prompt_id == "plan-mode-interview" {
        variables.insert("PLAN_FILE_INFO".to_string(), plan_file_info(state)?);
        variables.insert(
            "READ_ONLY_TOOL_NAMES".to_string(),
            read_only_tool_names(resources),
        );
        variables.insert(
            "EXPLORE_AGENT_HINT".to_string(),
            if has_explore_agent(resources) {
                " You can use the explore agent type to parallelize complex searches without filling your context, though for straightforward queries direct tools are simpler.".to_string()
            } else {
                String::new()
            },
        );
    } else {
        variables.insert(
            "WORKFLOW_DESCRIPTION".to_string(),
            "Follow iterative workflow: explore codebase, interview user, write to plan incrementally.".to_string(),
        );
    }
    variables.insert(
        "ASK_USER_QUESTION_TOOL_NAME".to_string(),
        "AskUserQuestion".to_string(),
    );
    variables.insert(
        "EXIT_PLAN_MODE_TOOL_NAME".to_string(),
        "ExitPlanMode".to_string(),
    );
    Ok(render_prompt_for(
        resources,
        prompt_id,
        state.current_provider.as_deref(),
        state.current_model.as_deref(),
        &variables,
    )
    .map(|prompt| prompt.trim().to_string())
    .filter(|prompt| !prompt.is_empty()))
}

fn render_reentry_prompt(state: &AppState, resources: &LoadedResources) -> Result<Option<String>> {
    let variables = BTreeMap::from([
        (
            "PLAN_FILE_PATH".to_string(),
            plan_file_path(state)?.display().to_string(),
        ),
        (
            "EXIT_PLAN_MODE_TOOL_NAME".to_string(),
            "ExitPlanMode".to_string(),
        ),
    ]);
    Ok(render_prompt_for(
        resources,
        "plan-mode-reentry",
        state.current_provider.as_deref(),
        state.current_model.as_deref(),
        &variables,
    )
    .map(|prompt| prompt.trim().to_string())
    .filter(|prompt| !prompt.is_empty()))
}

fn next_attachment_is_full(state: &AppState) -> bool {
    (state.plan_mode_attachment_count + 1) % FULL_REMINDER_EVERY_N_ATTACHMENTS == 1
}

fn active_plan_exists(state: &AppState) -> Result<bool> {
    Ok(read_plan_text(state)?
        .as_deref()
        .map(plan_has_user_content)
        .unwrap_or(false))
}

fn plan_file_info(state: &AppState) -> Result<String> {
    let plan_path = plan_file_path(state)?;
    Ok(if active_plan_exists(state)? {
        format!(
            "A plan file already exists at {}. You can read it and make incremental edits using the Edit tool.",
            plan_path.display()
        )
    } else {
        format!(
            "No plan file exists yet. You should create your plan at {} using the Write tool.",
            plan_path.display()
        )
    })
}

fn read_only_tool_names(resources: &LoadedResources) -> String {
    let mut names = ["Read", "Glob", "Grep"]
        .into_iter()
        .filter(|candidate| {
            resources
                .tools
                .iter()
                .any(|tool| tool.value.id.eq_ignore_ascii_case(candidate))
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    if names.is_empty() {
        names = vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()];
    }
    names.join(", ")
}

fn has_explore_agent(resources: &LoadedResources) -> bool {
    resources
        .agents
        .iter()
        .any(|agent| agent.value.id.eq_ignore_ascii_case("explore"))
}
