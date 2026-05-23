use super::browser_policy::BrowserPolicySettings;
use super::browser_target::{BrowserActionCategory, BrowserPermissionContext};
use super::profile::EffectivePermissionProfile;

/// Enumerates the deterministic Browser evaluator outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserPermissionDecision {
    Allow,
    Deny,
    Ask,
}

/// Carries the evaluator outcome plus an optional human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserPermissionEvaluation {
    pub(crate) decision: BrowserPermissionDecision,
    pub(crate) reason: Option<String>,
}

/// Evaluates one Browser permission context against workspace Browser policy and typed grants.
pub(crate) fn evaluate_browser_permission(
    profile: &EffectivePermissionProfile,
    policy: &BrowserPolicySettings,
    context: &BrowserPermissionContext,
) -> BrowserPermissionEvaluation {
    if context.action.is_none() {
        return ask("browser action requires explicit approval");
    }

    if let Some(denied_reason) = hard_deny_reason(policy, context) {
        return BrowserPermissionEvaluation {
            decision: BrowserPermissionDecision::Deny,
            reason: Some(denied_reason),
        };
    }

    if profile.browser_context_is_session_granted(context) {
        return BrowserPermissionEvaluation {
            decision: BrowserPermissionDecision::Allow,
            reason: None,
        };
    }

    if context.target.is_none() {
        return ask("browser action requires page context because no target URL is available");
    }

    if explicit_allow_matches(policy, context) {
        return BrowserPermissionEvaluation {
            decision: BrowserPermissionDecision::Allow,
            reason: None,
        };
    }

    ask(default_ask_reason(context))
}

fn ask(reason: impl Into<String>) -> BrowserPermissionEvaluation {
    BrowserPermissionEvaluation {
        decision: BrowserPermissionDecision::Ask,
        reason: Some(reason.into()),
    }
}

fn hard_deny_reason(
    policy: &BrowserPolicySettings,
    context: &BrowserPermissionContext,
) -> Option<String> {
    let target = context.target.as_ref()?;
    if policy
        .deny_target_classes
        .contains(&target.target_class.as_slug().to_string())
    {
        return Some(format!(
            "browser policy denies target class `{}`",
            target.target_class.as_slug()
        ));
    }
    if matches!(context.action, Some(BrowserActionCategory::Evaluate))
        && policy
            .deny_evaluate_target_classes
            .contains(&target.target_class.as_slug().to_string())
    {
        return Some(format!(
            "browser policy denies evaluation on target class `{}`",
            target.target_class.as_slug()
        ));
    }
    if let Some(origin) = target.origin.as_deref() {
        if policy
            .deny_origins
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(origin))
        {
            return Some(format!("browser policy denies origin `{origin}`"));
        }
    }
    if let Some(domain) = target.registrable_domain.as_deref() {
        if policy
            .deny_domains
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(domain))
        {
            return Some(format!("browser policy denies domain `{domain}`"));
        }
    }
    None
}

fn explicit_allow_matches(
    policy: &BrowserPolicySettings,
    context: &BrowserPermissionContext,
) -> bool {
    let Some(target) = context.target.as_ref() else {
        return false;
    };
    if policy
        .allow_target_classes
        .contains(&target.target_class.as_slug().to_string())
    {
        return true;
    }
    if let Some(origin) = target.origin.as_deref() {
        if policy
            .allow_origins
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(origin))
        {
            return true;
        }
    }
    if let Some(domain) = target.registrable_domain.as_deref() {
        if policy
            .allow_domains
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(domain))
        {
            return true;
        }
    }
    false
}

fn default_ask_reason(context: &BrowserPermissionContext) -> &'static str {
    match context.action {
        Some(BrowserActionCategory::Inspect) => "browser inspection requires approval",
        Some(BrowserActionCategory::Navigate | BrowserActionCategory::Interact) => {
            "browser navigation and interaction require approval"
        }
        Some(BrowserActionCategory::Evaluate) => {
            "browser evaluation requires explicit approval because it executes page JavaScript"
        }
        None => "browser action requires explicit approval",
    }
}
