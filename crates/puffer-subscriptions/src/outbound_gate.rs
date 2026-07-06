//! Single outbound gate: the ONLY policy deciding whether an outbound
//! connector action may run without a human-approved draft (plan-05,
//! agentenv/monorepo#767). Every send initiation path must call
//! [`evaluate`]; the execute RPC runs post-approval and does not.

use crate::catalog::{
    builtin_connector_template, ConnectorPermissionDefinition, ConnectorTemplate,
};

/// Who is initiating the outbound action.
#[derive(Debug, Clone)]
pub enum SendOrigin {
    /// The model decided to send (agent session, task session, background task).
    LlmInitiated {
        session_id: String,
        turn_id: Option<String>,
        task_id: Option<String>,
    },
    /// A user-configured automation fired (subscription rule / workflow node).
    /// Configuring the rule is standing approval; audited but not gated.
    RuleAutomation { rule_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    Allowed { reason: &'static str },
    RequiresDraft,
}

/// Category whitelist for external side effects that stay ungated.
/// Membership is declared in the builtin catalog, never inferred from slugs.
const UNGATED_EXTERNAL_CATEGORIES: &[&str] = &["external_reaction"];

pub fn evaluate(
    origin: &SendOrigin,
    connector_slug: &str,
    template: &ConnectorTemplate,
    action_slug: &str,
) -> GateDecision {
    match origin {
        SendOrigin::RuleAutomation { .. } => GateDecision::Allowed {
            reason: "rule_automation",
        },
        SendOrigin::LlmInitiated { .. } => {
            if action_requires_human_review(connector_slug, template, action_slug) {
                GateDecision::RequiresDraft
            } else {
                GateDecision::Allowed {
                    reason: "no_external_side_effect",
                }
            }
        }
    }
}

/// Effective permission = user template merged against the builtin floor:
/// a user override may tighten but never weaken the builtin contract.
pub fn effective_action_permission(
    connector_slug: &str,
    template: &ConnectorTemplate,
    action_slug: &str,
) -> Option<ConnectorPermissionDefinition> {
    let declared = template
        .actions
        .get(action_slug)
        .map(|action| action.permission.clone());
    let floor = builtin_connector_template(connector_slug).and_then(|builtin| {
        builtin
            .actions
            .get(action_slug)
            .map(|a| a.permission.clone())
    });
    match (declared, floor) {
        (Some(declared), Some(floor)) => Some(ConnectorPermissionDefinition {
            external_side_effect: declared.external_side_effect || floor.external_side_effect,
            // The floor category wins whenever it is stricter (i.e. gated).
            category: if permission_is_gated(&floor.category, floor.external_side_effect) {
                floor.category
            } else {
                declared.category
            },
            summary: declared.summary,
        }),
        (declared, floor) => declared.or(floor),
    }
}

pub fn action_requires_human_review(
    connector_slug: &str,
    template: &ConnectorTemplate,
    action_slug: &str,
) -> bool {
    effective_action_permission(connector_slug, template, action_slug)
        .map(|permission| {
            permission_is_gated(&permission.category, permission.external_side_effect)
        })
        .unwrap_or(false)
}

fn permission_is_gated(category: &str, external_side_effect: bool) -> bool {
    external_side_effect && !UNGATED_EXTERNAL_CATEGORIES.contains(&category)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::builtin_connector_template;

    fn tg() -> crate::catalog::ConnectorTemplate {
        builtin_connector_template("telegram-login").unwrap()
    }

    #[test]
    fn llm_send_message_requires_draft() {
        let origin = SendOrigin::LlmInitiated {
            session_id: "s1".into(),
            turn_id: None,
            task_id: None,
        };
        assert!(matches!(
            evaluate(&origin, "telegram-login", &tg(), "send_message"),
            GateDecision::RequiresDraft
        ));
    }

    #[test]
    fn rule_automation_is_allowed() {
        let origin = SendOrigin::RuleAutomation {
            rule_id: "r1".into(),
        };
        assert!(matches!(
            evaluate(&origin, "telegram-login", &tg(), "send_message"),
            GateDecision::Allowed { .. }
        ));
    }

    #[test]
    fn llm_read_action_is_allowed() {
        let origin = SendOrigin::LlmInitiated {
            session_id: "s1".into(),
            turn_id: None,
            task_id: None,
        };
        assert!(matches!(
            evaluate(&origin, "telegram-login", &tg(), "read_history"),
            GateDecision::Allowed { .. }
        ));
    }

    #[test]
    fn weakened_user_template_still_gated_by_builtin_floor() {
        // A user template weakens send_message permissions: the builtin floor must win.
        let mut weakened = tg();
        let action = weakened.actions.get_mut("send_message").unwrap();
        action.permission.category = "other".into();
        action.permission.external_side_effect = false;
        let origin = SendOrigin::LlmInitiated {
            session_id: "s1".into(),
            turn_id: None,
            task_id: None,
        };
        assert!(matches!(
            evaluate(&origin, "telegram-login", &weakened, "send_message"),
            GateDecision::RequiresDraft
        ));
    }

    #[test]
    fn external_reaction_category_is_whitelisted() {
        // Light actions explicitly marked external_reaction in the catalog stay ungated.
        let template = tg();
        if let Some(react) = template.actions.get("react") {
            assert_eq!(react.permission.category, "external_reaction");
            let origin = SendOrigin::LlmInitiated {
                session_id: "s1".into(),
                turn_id: None,
                task_id: None,
            };
            assert!(matches!(
                evaluate(&origin, "telegram-login", &template, "react"),
                GateDecision::Allowed { .. }
            ));
        }
    }
}
