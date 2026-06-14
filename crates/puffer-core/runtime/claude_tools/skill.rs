use crate::runtime::lambda_skill_activation::{
    allowed_tools_for_verified_skill, gate_for_verified_skill_activation, is_lambda_verified_skill,
};
use crate::AppState;
use anyhow::{anyhow, bail, Result};
use puffer_resources::{skill_by_name, LoadedResources};
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Write as _;

const COMMAND_NAME_TAG: &str = "command-name";

#[derive(Debug, Deserialize)]
struct SkillToolInput {
    skill: String,
    #[serde(default)]
    args: Option<String>,
}

fn normalize_skill_name(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Skill name cannot be empty");
    }
    let without_slash = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let without_alias = without_slash
        .strip_prefix("skill:")
        .unwrap_or(without_slash)
        .trim();
    if without_alias.is_empty() {
        bail!("Skill name cannot be empty");
    }
    Ok(without_alias.to_string())
}

fn parse_skill_tool_input(input: Value) -> Result<SkillToolInput> {
    let mut parsed: SkillToolInput = serde_json::from_value(input)?;
    parsed.skill = normalize_skill_name(&parsed.skill)?;
    Ok(parsed)
}

/// Extracts the normalized user-requested skill name from raw `Skill` tool JSON.
pub(crate) fn skill_name_from_tool_input(input: &str) -> Option<String> {
    let input = serde_json::from_str::<Value>(input).ok()?;
    parse_skill_tool_input(input)
        .ok()
        .map(|parsed| parsed.skill)
}

/// Executes Claude-style `Skill` tool input and installs any verified Lambda gate.
///
/// The returned inline command payload is suitable for model consumption in the
/// same turn.
pub fn execute_claude_skill_tool(
    state: &mut AppState,
    resources: &LoadedResources,
    input: Value,
) -> Result<String> {
    let parsed = parse_skill_tool_input(input)?;
    let normalized_skill = parsed.skill.clone();
    let skill = skill_by_name(resources, &normalized_skill)
        .ok_or_else(|| anyhow!("unknown skill `{normalized_skill}`"))?;

    if skill.value.disable_model_invocation {
        bail!(
            "skill `{}` cannot be used with Skill tool due to disable-model-invocation",
            skill.value.name
        );
    }
    if state.lambda_gate.is_some() && !is_lambda_verified_skill(&skill.value) {
        bail!(
            "active Lambda Skill gate cannot switch to unverified skill `{}`",
            skill.value.name
        );
    }
    if is_lambda_verified_skill(&skill.value) {
        state.lambda_gate = gate_for_verified_skill_activation(&skill.value)?;
        state.pending_lambda_host_call = None;
    } else {
        state.lambda_gate = None;
        state.pending_lambda_host_call = None;
    }

    let rendered = crate::skill_support::render_skill_prompt(
        skill,
        parsed.args.as_deref().unwrap_or_default(),
        "skill-tool",
    );
    let mut output = String::new();
    let _ = writeln!(
        &mut output,
        "<{COMMAND_NAME_TAG}>{}</{COMMAND_NAME_TAG}>",
        skill.value.name
    );
    let _ = writeln!(&mut output, "Skill {}", skill.value.name);
    let _ = writeln!(&mut output, "{}", skill.value.description);
    let allowed_tools = allowed_tools_for_verified_skill(&skill.value)?;
    if !allowed_tools.is_empty() {
        let _ = writeln!(&mut output, "allowed-tools: {}", allowed_tools.join(", "));
    }
    if let Some(verification) = skill.value.verification.as_ref() {
        let _ = writeln!(&mut output, "verified-skill: {}", verification.system);
        if let Some(source_path) = verification.source_path.as_deref() {
            let _ = writeln!(&mut output, "formal-source: {source_path}");
        }
        if let Some(generated_path) = verification.generated_path.as_deref() {
            let _ = writeln!(&mut output, "generated-descriptor: {generated_path}");
        }
        let mut stats = Vec::new();
        if let Some(tools) = verification.tools {
            stats.push(format!("tools={tools}"));
        }
        if let Some(actions) = verification.actions {
            stats.push(format!("actions={actions}"));
        }
        if !stats.is_empty() {
            let _ = writeln!(&mut output, "verified-stats: {}", stats.join(", "));
        }
    }
    let _ = writeln!(
        &mut output,
        "\n<skill name=\"{}\">\n{}\n</skill>",
        skill.value.name,
        rendered.trim()
    );
    Ok(output.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tests::state;
    use puffer_resources::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};
    use serde_json::json;

    fn sample_resources() -> LoadedResources {
        LoadedResources {
            skills: vec![
                LoadedItem {
                    value: SkillSpec {
                        name: "review-pr".to_string(),
                        description: "Review one pull request".to_string(),
                        content: "Review prompt body".to_string(),
                        disable_model_invocation: false,
                        ..SkillSpec::default()
                    },
                    source_info: SourceInfo {
                        path: "skills/review-pr/SKILL.md".into(),
                        kind: SourceKind::Builtin,
                    },
                },
                LoadedItem {
                    value: SkillSpec {
                        name: "hidden".to_string(),
                        description: "Hidden skill".to_string(),
                        content: "Top secret".to_string(),
                        disable_model_invocation: true,
                        ..SkillSpec::default()
                    },
                    source_info: SourceInfo {
                        path: "skills/hidden/SKILL.md".into(),
                        kind: SourceKind::Builtin,
                    },
                },
                LoadedItem {
                    value: SkillSpec {
                        name: "verified-ci".to_string(),
                        description: "Fix verified CI failures".to_string(),
                        content: "Verified prompt body".to_string(),
                        verification: Some(SkillVerificationSpec {
                            system: "lambda-skill".to_string(),
                            source_path: Some(
                                "fixtures/skills/verified-ci/skill.lskill".to_string(),
                            ),
                            generated_path: Some(
                                "fixtures/skills/verified-ci/out/GENERATED.SKILL.md".to_string(),
                            ),
                            host_catalogue_path: None,
                            compiler_path: None,
                            host_tool_bindings: Default::default(),
                            require_approval: false,
                            tools: Some(10),
                            actions: Some(2),
                        }),
                        ..SkillSpec::default()
                    },
                    source_info: SourceInfo {
                        path: "skills/verified-ci/skill.lskill".into(),
                        kind: SourceKind::Workspace,
                    },
                },
            ],
            ..LoadedResources::default()
        }
    }

    #[test]
    fn executes_skill_with_command_tag_and_args() {
        let mut state = state();
        let output = execute_claude_skill_tool(
            &mut state,
            &sample_resources(),
            json!({"skill": "/review-pr", "args": "123"}),
        )
        .unwrap();
        assert!(output.contains("<command-name>review-pr</command-name>"));
        assert!(output.contains("<skill name=\"review-pr\">"));
        assert!(output.contains("ARGUMENTS: 123"));
    }

    #[test]
    fn accepts_compatibility_skill_alias_prefix() {
        let mut state = state();
        let output = execute_claude_skill_tool(
            &mut state,
            &sample_resources(),
            json!({"skill": "/skill:review-pr"}),
        )
        .unwrap();
        assert!(output.contains("<command-name>review-pr</command-name>"));
    }

    #[test]
    fn parses_skill_name_from_tool_input() {
        assert_eq!(
            skill_name_from_tool_input(r#"{"skill":"review-pr"}"#),
            Some("review-pr".to_string())
        );
        assert_eq!(
            skill_name_from_tool_input(r#"{"skill":"/review-pr"}"#),
            Some("review-pr".to_string())
        );
        assert_eq!(
            skill_name_from_tool_input(r#"{"skill":"/skill:review-pr"}"#),
            Some("review-pr".to_string())
        );
    }

    #[test]
    fn skill_name_from_tool_input_rejects_invalid_input() {
        assert_eq!(skill_name_from_tool_input("not json"), None);
        assert_eq!(skill_name_from_tool_input(r#"{"skill":""}"#), None);
    }

    #[test]
    fn rejects_unknown_skill() {
        let mut state = state();
        let error = execute_claude_skill_tool(
            &mut state,
            &sample_resources(),
            json!({"skill": "does-not-exist"}),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unknown skill"));
    }

    #[test]
    fn rejects_skill_with_disabled_model_invocation() {
        let mut state = state();
        let error =
            execute_claude_skill_tool(&mut state, &sample_resources(), json!({"skill": "hidden"}))
                .unwrap_err()
                .to_string();
        assert!(error.contains("disable-model-invocation"));
    }

    #[test]
    fn rejects_verified_lambda_skill_without_gate_config() {
        let mut state = state();
        let error = execute_claude_skill_tool(
            &mut state,
            &sample_resources(),
            json!({"skill": "verified-ci"}),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("requires a precompiled host catalogue"));
    }
}
