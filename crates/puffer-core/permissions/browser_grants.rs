use super::browser_target::{BrowserActionCategory, BrowserPermissionContext};

/// Enumerates the typed Browser session grant scopes supported by `puffer-core`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum BrowserGrantScopeKind {
    AllowOnce,
    AllowOriginSession,
    AllowDomainSession,
    AllowActionOnOriginSession,
    AllowTabSession,
}

/// Stores one scoped Browser session grant derived from an approved tool call.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum BrowserGrantCategory {
    AllowOnce {
        root_session_id: String,
        action: BrowserActionCategory,
        target_origin: Option<String>,
        target_domain: Option<String>,
        tab_id: Option<String>,
    },
    AllowOriginSession {
        root_session_id: String,
        target_origin: String,
    },
    AllowDomainSession {
        root_session_id: String,
        target_domain: String,
    },
    AllowActionOnOriginSession {
        root_session_id: String,
        action: BrowserActionCategory,
        target_origin: String,
    },
    AllowTabSession {
        root_session_id: String,
        tab_id: String,
    },
}

/// Returns the Browser typed grants implied by an approved Browser call.
pub(crate) fn browser_grant_categories(
    context: &BrowserPermissionContext,
) -> Vec<BrowserGrantCategory> {
    browser_grant_categories_for_scope(context, suggested_browser_grant_scope(context))
}

/// Returns the Browser typed grants implied by an approved Browser call and scope.
pub(crate) fn browser_grant_categories_for_scope(
    context: &BrowserPermissionContext,
    scope: BrowserGrantScopeKind,
) -> Vec<BrowserGrantCategory> {
    let Some(action) = context.action else {
        return Vec::new();
    };
    let target = context.target.as_ref();
    match scope {
        BrowserGrantScopeKind::AllowOnce => {
            vec![BrowserGrantCategory::AllowOnce {
                root_session_id: context.root_session_id.clone(),
                action,
                target_origin: target.and_then(|target| target.origin.clone()),
                target_domain: target.and_then(|target| target.registrable_domain.clone()),
                tab_id: context.tab_id.clone(),
            }]
        }
        BrowserGrantScopeKind::AllowOriginSession => {
            if let Some(origin) = target.and_then(|target| target.origin.clone()) {
                vec![BrowserGrantCategory::AllowOriginSession {
                    root_session_id: context.root_session_id.clone(),
                    target_origin: origin,
                }]
            } else {
                Vec::new()
            }
        }
        BrowserGrantScopeKind::AllowDomainSession => {
            if let Some(domain) = target.and_then(|target| target.registrable_domain.clone()) {
                vec![BrowserGrantCategory::AllowDomainSession {
                    root_session_id: context.root_session_id.clone(),
                    target_domain: domain,
                }]
            } else {
                Vec::new()
            }
        }
        BrowserGrantScopeKind::AllowActionOnOriginSession => {
            if let Some(origin) = target.and_then(|target| target.origin.clone()) {
                vec![BrowserGrantCategory::AllowActionOnOriginSession {
                    root_session_id: context.root_session_id.clone(),
                    action,
                    target_origin: origin,
                }]
            } else {
                Vec::new()
            }
        }
        BrowserGrantScopeKind::AllowTabSession => {
            if let Some(tab_id) = context.tab_id.clone() {
                vec![BrowserGrantCategory::AllowTabSession {
                    root_session_id: context.root_session_id.clone(),
                    tab_id,
                }]
            } else {
                Vec::new()
            }
        }
    }
}

/// Returns true when the supplied typed grants allow the Browser context.
pub(crate) fn browser_context_matches_grants(
    context: &BrowserPermissionContext,
    mut has_grant: impl FnMut(&BrowserGrantCategory) -> bool,
) -> bool {
    let Some(action) = context.action else {
        return false;
    };
    let target = context.target.as_ref();

    let allow_once = BrowserGrantCategory::AllowOnce {
        root_session_id: context.root_session_id.clone(),
        action,
        target_origin: target.and_then(|target| target.origin.clone()),
        target_domain: target.and_then(|target| target.registrable_domain.clone()),
        tab_id: context.tab_id.clone(),
    };

    let candidates = [
        target
            .and_then(|target| target.origin.clone())
            .map(|origin| BrowserGrantCategory::AllowActionOnOriginSession {
                root_session_id: context.root_session_id.clone(),
                action,
                target_origin: origin,
            }),
        target
            .and_then(|target| target.origin.clone())
            .map(|origin| BrowserGrantCategory::AllowOriginSession {
                root_session_id: context.root_session_id.clone(),
                target_origin: origin,
            }),
        target
            .and_then(|target| target.registrable_domain.clone())
            .map(|domain| BrowserGrantCategory::AllowDomainSession {
                root_session_id: context.root_session_id.clone(),
                target_domain: domain,
            }),
        context
            .tab_id
            .clone()
            .map(|tab_id| BrowserGrantCategory::AllowTabSession {
                root_session_id: context.root_session_id.clone(),
                tab_id,
            }),
        allow_once_reusable(&allow_once).then_some(allow_once),
    ];

    candidates.iter().flatten().any(|grant| has_grant(grant))
}

fn allow_once_reusable(grant: &BrowserGrantCategory) -> bool {
    match grant {
        BrowserGrantCategory::AllowOnce {
            target_origin,
            target_domain,
            tab_id,
            ..
        } => target_origin.is_some() || target_domain.is_some() || tab_id.is_some(),
        _ => false,
    }
}

/// Returns the default Browser session scope suggested for one approval prompt.
pub(crate) fn suggested_browser_grant_scope(
    context: &BrowserPermissionContext,
) -> BrowserGrantScopeKind {
    if context.action == Some(BrowserActionCategory::Evaluate) {
        if context.tab_id.is_some() {
            return BrowserGrantScopeKind::AllowTabSession;
        }
        if context
            .target
            .as_ref()
            .and_then(|target| target.origin.as_ref())
            .is_some()
        {
            return BrowserGrantScopeKind::AllowActionOnOriginSession;
        }
        return BrowserGrantScopeKind::AllowOnce;
    }

    if matches!(context.action, Some(BrowserActionCategory::Navigate))
        && context.tab_id.is_none()
        && context
            .target
            .as_ref()
            .is_some_and(|target| target.registrable_domain.is_some())
    {
        return BrowserGrantScopeKind::AllowDomainSession;
    }

    if context.action == Some(BrowserActionCategory::Interact)
        && context.tab_id.is_some()
        && context
            .target
            .as_ref()
            .is_some_and(|target| target.origin.is_some() && !browser_target_is_high_risk(target))
    {
        return BrowserGrantScopeKind::AllowOriginSession;
    }

    if context.tab_id.is_some() {
        return BrowserGrantScopeKind::AllowTabSession;
    }
    if context
        .target
        .as_ref()
        .and_then(|target| target.origin.as_ref())
        .is_some()
    {
        return BrowserGrantScopeKind::AllowOriginSession;
    }
    if context
        .target
        .as_ref()
        .and_then(|target| target.registrable_domain.as_ref())
        .is_some()
    {
        return BrowserGrantScopeKind::AllowDomainSession;
    }
    BrowserGrantScopeKind::AllowOnce
}

fn browser_target_is_high_risk(target: &super::browser_target::BrowserTarget) -> bool {
    if target.target_class != super::browser_target::BrowserTargetClass::OpenWeb {
        return true;
    }
    if target.scheme == "http"
        && target
            .host
            .as_deref()
            .is_some_and(|host| !is_local_like_host(host))
    {
        return true;
    }
    if target
        .host
        .as_deref()
        .is_some_and(|host| host.parse::<std::net::IpAddr>().is_ok())
    {
        return true;
    }
    if target.port.is_some_and(|port| {
        (target.scheme == "http" && port != 80) || (target.scheme == "https" && port != 443)
    }) {
        return true;
    }
    target
        .host
        .as_deref()
        .map(host_looks_encoded_or_punycode)
        .unwrap_or(false)
}

fn is_local_like_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified(),
            std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
        })
}

fn host_looks_encoded_or_punycode(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    lower.contains("xn--")
        || host.chars().filter(|ch| ch.is_ascii_digit()).count() >= 8
        || host.matches('-').count() >= 4
}
