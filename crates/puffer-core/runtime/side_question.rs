use super::TurnExecution;
use crate::AppState;
use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;

const SIDE_QUESTION_SYSTEM_REMINDER: &str = r#"<system-reminder>This is a side question from the user. You must answer this question directly in a single response.

IMPORTANT CONTEXT:
- You are a separate, lightweight agent spawned to answer this one question
- The main agent is NOT interrupted - it continues working independently in the background
- You share the conversation context but are a completely separate instance
- Do NOT reference being interrupted or what you were "previously doing" - that framing is incorrect

CRITICAL CONSTRAINTS:
- You have NO tools available - you cannot read files, run commands, search, or take any actions
- This is a one-off response - there will be no follow-up turns
- You can ONLY provide information based on what you already know from the conversation context
- NEVER say things like "Let me try...", "I'll now...", "Let me check...", or promise to take any action
- If you don't know the answer, say so - do not offer to look it up or investigate

Simply answer the question with the information you have.</system-reminder>"#;

/// Executes one side question against a cloned session state with all tools removed.
pub(super) fn execute_side_question(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    question: &str,
) -> Result<TurnExecution> {
    let mut side_state = state.clone();
    let mut side_resources = resources.clone();
    side_resources.tools.clear();
    let prompt = build_side_question_prompt(question);
    super::execute_user_prompt_with_options(
        &mut side_state,
        &side_resources,
        providers,
        auth_store,
        &prompt,
        super::TurnRequestOptions::default(),
    )
}

fn build_side_question_prompt(question: &str) -> String {
    format!("{SIDE_QUESTION_SYSTEM_REMINDER}\n\n{}", question.trim())
}

#[cfg(test)]
mod tests {
    use super::build_side_question_prompt;
    use puffer_resources::{LoadedItem, LoadedResources, SourceInfo, SourceKind, ToolSpec};
    use std::path::PathBuf;

    #[test]
    fn side_question_prompt_wraps_question_with_one_off_constraints() {
        let prompt = build_side_question_prompt("What changed?");
        assert!(prompt.contains("side question from the user"));
        assert!(prompt.contains("You have NO tools available"));
        assert!(prompt.ends_with("What changed?"));
    }

    #[test]
    fn side_question_execution_removes_tool_resources() {
        let mut resources = LoadedResources::default();
        resources.tools.push(LoadedItem {
            value: ToolSpec {
                id: "Bash".to_string(),
                name: "Bash".to_string(),
                description: "shell".to_string(),
                handler: "builtin:bash".to_string(),
                aliases: Vec::new(),
                handler_args: Vec::new(),
                approval_policy: Some("on-request".to_string()),
                sandbox_policy: Some("workspace-write".to_string()),
                shared_lib: None,
                enabled_if: None,
                input_schema: None,
                contract: None,
                metadata: Default::default(),
                display: Default::default(),
            },
            source_info: SourceInfo {
                path: PathBuf::from("tools/bash.yaml"),
                kind: SourceKind::Builtin,
            },
        });

        let mut side_resources = resources.clone();
        side_resources.tools.clear();

        assert_eq!(resources.tools.len(), 1);
        assert!(side_resources.tools.is_empty());
    }
}
