use super::claude_tools::skill::skill_name_from_tool_input;
use super::ToolInvocation;
use puffer_resources::{skill_by_name, LoadedResources};

const REMINDER_PREFIX: &str = "The activated skill requires tool-driven work before completion.";
const FAILURE_PREFIX: &str = "No work was started";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingSkill {
    name: String,
    reminder_sent: bool,
}

/// Tracks whether an activated skill requires follow-up tool work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SkillActionObligation {
    pending: Option<PendingSkill>,
}

/// Describes how the loop should handle a no-tool model response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoToolDecision {
    Complete,
    ContinueWithReminder(String),
    FailNotStarted(String),
}

impl SkillActionObligation {
    /// Observes executed tool invocations and updates skill action state.
    pub(crate) fn observe_invocations(
        &mut self,
        resources: &LoadedResources,
        invocations: &[ToolInvocation],
    ) {
        for invocation in invocations {
            if invocation.tool_id == "Skill" {
                self.observe_skill_invocation(resources, invocation);
            } else {
                self.pending = None;
            }
        }
    }

    /// Decides how to handle a model response that requested no tools.
    pub(crate) fn no_tool_decision(&mut self) -> NoToolDecision {
        let Some(pending) = self.pending.as_mut() else {
            return NoToolDecision::Complete;
        };

        if pending.reminder_sent {
            let name = pending.name.clone();
            self.pending = None;
            return NoToolDecision::FailNotStarted(failure_message(&name));
        }

        pending.reminder_sent = true;
        NoToolDecision::ContinueWithReminder(reminder_message(&pending.name))
    }

    /// Returns a failure message if an obligation is still pending.
    pub(crate) fn fail_if_pending(&mut self) -> Option<String> {
        let pending = self.pending.take()?;
        Some(failure_message(&pending.name))
    }

    fn observe_skill_invocation(
        &mut self,
        resources: &LoadedResources,
        invocation: &ToolInvocation,
    ) {
        if !invocation.success {
            return;
        }
        let Some(name) = skill_name_from_tool_input(&invocation.input) else {
            return;
        };
        let Some(skill) = skill_by_name(resources, &name) else {
            return;
        };
        if skill.value.requires_action {
            self.pending = Some(PendingSkill {
                name: skill.value.name.clone(),
                reminder_sent: false,
            });
        }
    }
}

fn reminder_message(skill_name: &str) -> String {
    format!(
        "{REMINDER_PREFIX} The `{skill_name}` skill is active. Start the work now by calling an appropriate tool; do not reply with promises or progress-only text."
    )
}

fn failure_message(skill_name: &str) -> String {
    format!(
        "{FAILURE_PREFIX} after activating the `{skill_name}` skill. This skill requires a follow-up tool call before completion."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::{LoadedItem, LoadedResources, SkillSpec, SourceInfo, SourceKind};
    use serde_json::json;

    fn resources() -> LoadedResources {
        LoadedResources {
            skills: vec![
                skill("short-drama-generation", true),
                skill("review-pr", false),
                skill("video-drama", true),
            ],
            ..LoadedResources::default()
        }
    }

    fn skill(name: &str, requires_action: bool) -> LoadedItem<SkillSpec> {
        LoadedItem {
            value: SkillSpec {
                name: name.to_string(),
                description: format!("{name} description"),
                content: format!("{name} body"),
                requires_action,
                ..SkillSpec::default()
            },
            source_info: SourceInfo {
                path: format!("skills/{name}/SKILL.md").into(),
                kind: SourceKind::Builtin,
            },
        }
    }

    fn invocation(tool_id: &str, input: &str, success: bool) -> ToolInvocation {
        ToolInvocation {
            call_id: format!("call-{tool_id}"),
            tool_id: tool_id.to_string(),
            input: input.to_string(),
            output: String::new(),
            success,
            metadata: json!({}),
            terminate: false,
        }
    }

    fn skill_invocation(name: &str, success: bool) -> ToolInvocation {
        invocation("Skill", &format!(r#"{{"skill":"{name}"}}"#), success)
    }

    #[test]
    fn requires_action_skill_activates_pending() {
        let resources = resources();
        let mut obligation = SkillActionObligation::default();

        obligation.observe_invocations(
            &resources,
            &[skill_invocation("short-drama-generation", true)],
        );

        assert!(matches!(
            obligation.no_tool_decision(),
            NoToolDecision::ContinueWithReminder(message)
                if message.contains("short-drama-generation")
        ));
    }

    #[test]
    fn non_requires_action_skill_does_nothing() {
        let resources = resources();
        let mut obligation = SkillActionObligation::default();

        obligation.observe_invocations(&resources, &[skill_invocation("review-pr", true)]);

        assert!(matches!(
            obligation.no_tool_decision(),
            NoToolDecision::Complete
        ));
    }

    #[test]
    fn failed_skill_invocation_does_nothing() {
        let resources = resources();
        let mut obligation = SkillActionObligation::default();

        obligation.observe_invocations(
            &resources,
            &[skill_invocation("short-drama-generation", false)],
        );

        assert!(matches!(
            obligation.no_tool_decision(),
            NoToolDecision::Complete
        ));
    }

    #[test]
    fn first_no_tool_returns_reminder() {
        let resources = resources();
        let mut obligation = SkillActionObligation::default();
        obligation.observe_invocations(
            &resources,
            &[skill_invocation("short-drama-generation", true)],
        );

        match obligation.no_tool_decision() {
            NoToolDecision::ContinueWithReminder(message) => {
                assert!(message.contains("short-drama-generation"));
                assert!(!message.contains("No work was started"));
            }
            other => panic!("expected reminder, got {other:?}"),
        }
    }

    #[test]
    fn second_no_tool_returns_fail() {
        let resources = resources();
        let mut obligation = SkillActionObligation::default();
        obligation.observe_invocations(
            &resources,
            &[skill_invocation("short-drama-generation", true)],
        );
        assert!(matches!(
            obligation.no_tool_decision(),
            NoToolDecision::ContinueWithReminder(_)
        ));

        match obligation.no_tool_decision() {
            NoToolDecision::FailNotStarted(message) => {
                assert!(message.contains("No work was started"));
                assert!(message.contains("short-drama-generation"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[test]
    fn pending_failure_can_be_forced_when_turn_budget_is_exhausted() {
        let resources = resources();
        let mut obligation = SkillActionObligation::default();
        obligation.observe_invocations(
            &resources,
            &[skill_invocation("short-drama-generation", true)],
        );

        let message = obligation
            .fail_if_pending()
            .expect("pending obligation should fail");

        assert!(message.contains("No work was started"));
        assert!(message.contains("short-drama-generation"));
        assert!(matches!(
            obligation.no_tool_decision(),
            NoToolDecision::Complete
        ));
    }

    #[test]
    fn non_skill_invocation_satisfies_pending() {
        let resources = resources();
        let mut obligation = SkillActionObligation::default();
        obligation.observe_invocations(
            &resources,
            &[skill_invocation("short-drama-generation", true)],
        );
        obligation.observe_invocations(&resources, &[invocation("Write", "{}", false)]);

        assert!(matches!(
            obligation.no_tool_decision(),
            NoToolDecision::Complete
        ));
    }

    #[test]
    fn same_batch_skill_then_write_satisfies_pending() {
        let resources = resources();
        let mut obligation = SkillActionObligation::default();
        obligation.observe_invocations(
            &resources,
            &[
                skill_invocation("short-drama-generation", true),
                invocation("Write", "{}", true),
            ],
        );

        assert!(matches!(
            obligation.no_tool_decision(),
            NoToolDecision::Complete
        ));
    }

    #[test]
    fn second_requires_action_skill_replaces_pending_name() {
        let resources = resources();
        let mut obligation = SkillActionObligation::default();
        obligation.observe_invocations(
            &resources,
            &[
                skill_invocation("short-drama-generation", true),
                skill_invocation("video-drama", true),
            ],
        );

        assert!(matches!(
            obligation.no_tool_decision(),
            NoToolDecision::ContinueWithReminder(message) if message.contains("video-drama")
                && !message.contains("short-drama-generation")
        ));
    }
}
