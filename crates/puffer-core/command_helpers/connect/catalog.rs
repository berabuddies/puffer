use super::{ask_searchable_choice, call_tool};
use crate::AppState;
use anyhow::{anyhow, bail, Result};
use puffer_resources::LoadedResources;
use puffer_subscriptions::ConnectorTemplate;
use serde_json::{json, Value};

/// Asks the user to choose a connector from the searchable connector catalog.
pub(crate) fn ask_connector_slug(
    state: &mut AppState,
    resources: &LoadedResources,
) -> Result<String> {
    let options = connector_catalog_options(state, resources)?;
    ask_searchable_choice(
        state,
        resources,
        "Connector",
        "Which connector should Puffer connect?",
        &options,
    )
}

/// Resolves an exact or partial connector slug before `/connect` asks for setup details.
pub(crate) fn resolve_connector_slug(
    state: &mut AppState,
    resources: &LoadedResources,
    raw_slug: &str,
) -> Result<String> {
    let raw_slug = raw_slug.trim();
    if raw_slug.is_empty() {
        return ask_connector_slug(state, resources);
    }
    if local_builtin_slug_exists(raw_slug) {
        return Ok(raw_slug.to_string());
    }
    let options = match connector_catalog_options(state, resources) {
        Ok(options) => options,
        Err(_) => return Ok(raw_slug.to_string()),
    };
    if options.iter().any(|(slug, _)| slug == raw_slug) {
        return Ok(raw_slug.to_string());
    }
    let matches = matching_connector_options(&options, raw_slug);
    match matches.len() {
        0 => Ok(raw_slug.to_string()),
        1 => Ok(matches[0].0.clone()),
        _ => ask_searchable_choice(
            state,
            resources,
            "Connector",
            &format!("Which connector matches `{raw_slug}`?"),
            &matches,
        ),
    }
}

fn connector_catalog_options(
    state: &mut AppState,
    resources: &LoadedResources,
) -> Result<Vec<(String, String)>> {
    match call_tool(state, resources, "ConnectorList", json!({})) {
        Ok(output) => connector_options(&output),
        Err(error)
            if error
                .to_string()
                .contains("subscription runtime is not running") =>
        {
            Ok(builtin_connector_options())
        }
        Err(error) => Err(error),
    }
}

fn builtin_connector_options() -> Vec<(String, String)> {
    puffer_subscriptions::builtin_connector_templates()
        .into_iter()
        .map(|template| {
            let description = connector_template_description(&template);
            (template.slug, description)
        })
        .collect()
}

fn local_builtin_slug_exists(slug: &str) -> bool {
    puffer_subscriptions::builtin_connector_templates()
        .into_iter()
        .any(|template| template.slug == slug)
}

fn connector_options(output: &Value) -> Result<Vec<(String, String)>> {
    let connectors = output
        .get("connectors")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("ConnectorList did not return a connectors array"))?;
    if connectors.is_empty() {
        bail!("no connector templates are available");
    }
    let options = connectors
        .iter()
        .filter_map(|connector| {
            let slug = connector.get("connector_slug").and_then(Value::as_str)?;
            Some((slug.to_string(), connector_option_description(connector)))
        })
        .collect::<Vec<_>>();
    if options.is_empty() {
        bail!("ConnectorList did not return any connector slugs");
    }
    Ok(options)
}

fn connector_option_description(connector: &Value) -> String {
    let description = connector
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Connector template");
    let mut details = Vec::new();
    if connector
        .get("requires_auth")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        details.push("auth".to_string());
    } else {
        details.push("no auth".to_string());
    }
    if connector
        .get("can_subscribe")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        details.push("subscribe".to_string());
    }
    if connector
        .get("can_proxy_agent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        details.push("agent proxy".to_string());
    }
    if let Some(skill) = connector
        .get("skill")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(format!("skill={skill}"));
    }
    let actions = connector_action_slugs(connector);
    if !actions.is_empty() {
        details.push(format!("actions={}", action_summary(&actions)));
    }
    let runtime_hints = connector_runtime_hints(connector);
    if !runtime_hints.is_empty() {
        details.push(format!("runtime={}", action_summary(&runtime_hints)));
    }
    format!("{description} ({})", details.join("; "))
}

fn connector_template_description(template: &ConnectorTemplate) -> String {
    let mut details = Vec::new();
    if template.requires_auth {
        details.push("auth".to_string());
    } else {
        details.push("no auth".to_string());
    }
    if template.can_subscribe {
        details.push("subscribe".to_string());
    }
    if template.can_proxy_agent {
        details.push("agent proxy".to_string());
    }
    if !template.skill.trim().is_empty() {
        details.push(format!("skill={}", template.skill.trim()));
    }
    let actions = template.actions.keys().cloned().collect::<Vec<_>>();
    if !actions.is_empty() {
        details.push(format!("actions={}", action_summary(&actions)));
    }
    let runtime_hints = connector_template_runtime_hints(template);
    if !runtime_hints.is_empty() {
        details.push(format!("runtime={}", action_summary(&runtime_hints)));
    }
    format!("{} ({})", template.description, details.join("; "))
}

fn connector_action_slugs(connector: &Value) -> Vec<String> {
    if let Some(actions) = connector.get("actions").and_then(Value::as_object) {
        return actions.keys().cloned().collect();
    }
    connector
        .get("action_slugs")
        .and_then(Value::as_array)
        .map(|actions| {
            actions
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn connector_runtime_hints(connector: &Value) -> Vec<String> {
    connector
        .get("runtime_hints")
        .and_then(Value::as_array)
        .map(|hints| {
            hints
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn connector_template_runtime_hints(template: &ConnectorTemplate) -> Vec<String> {
    let mut hints = Vec::new();
    if template.subscriber.is_some() {
        hints.push("subscriber");
    }
    if template.command_argv().is_some() {
        hints.push("command");
    }
    if template.binary.starts_with("puffer internal-tool") {
        hints.push("internal-tool");
    }
    if matches!(
        template.slug.as_str(),
        "telegram-bot"
            | "discord-bot"
            | "matrix-bot"
            | "asana-webhook"
            | "github-webhook"
            | "gitlab-webhook"
            | "jira-webhook"
            | "linear-webhook"
            | "pagerduty-webhook"
            | "sentry-webhook"
            | "shopify-webhook"
            | "stripe-webhook"
            | "trello-webhook"
            | "webhook"
    ) {
        hints.push("serve");
    }
    hints.into_iter().map(str::to_string).collect()
}

fn action_summary(actions: &[String]) -> String {
    let mut actions = actions.to_vec();
    actions.sort();
    actions.join(",")
}

fn matching_connector_options(options: &[(String, String)], query: &str) -> Vec<(String, String)> {
    let terms = search_terms(query);
    if terms.is_empty() {
        return options.to_vec();
    }
    options
        .iter()
        .filter(|(slug, description)| {
            let haystack = connector_search_text(slug, description);
            terms.iter().all(|term| haystack.contains(term))
        })
        .cloned()
        .collect()
}

fn connector_search_text(slug: &str, description: &str) -> String {
    format!("{} {}", slug, description).to_ascii_lowercase()
}

fn search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{with_user_question_prompt_handler, UserQuestionPromptResponse};
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use serde_json::{json, Map};
    use std::sync::{Arc, Mutex};

    fn temp_state() -> AppState {
        let tempdir = tempfile::tempdir().unwrap();
        let cwd = tempdir.keep();
        let session = SessionMetadata {
            id: uuid::Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: cwd.clone(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        AppState::new(PufferConfig::default(), cwd, session)
    }

    #[test]
    fn matching_connector_options_matches_multiple_terms() {
        let options = vec![
            (
                "telegram-login".to_string(),
                "Telegram personal account".to_string(),
            ),
            ("slack-login".to_string(), "Slack user account".to_string()),
            ("slack-app".to_string(), "Slack Socket Mode app".to_string()),
        ];

        let matches = matching_connector_options(&options, "slack app");

        assert_eq!(matches, vec![options[2].clone()]);
    }

    #[test]
    fn builtin_connector_options_include_skill_and_action_metadata() {
        let options = builtin_connector_options();
        let telegram = options
            .iter()
            .find(|(slug, _)| slug == "telegram-login")
            .expect("telegram-login option");

        assert!(telegram.1.contains("skill=telegram"));
        assert!(telegram.1.contains("runtime=internal-tool,subscriber"));
        assert!(telegram.1.contains("vote_poll"));
        assert_eq!(
            matching_connector_options(&options, "vote poll"),
            vec![telegram.clone()]
        );
    }

    #[test]
    fn connector_options_include_action_slugs_from_connector_list_rows() {
        let output = json!({
            "connectors": [
                {
                    "connector_slug": "demo-chat",
                    "description": "Demo chat connector",
                    "skill": "demo",
                    "requires_auth": false,
                    "can_subscribe": true,
                    "can_proxy_agent": false,
                    "runtime_hints": ["command"],
                    "actions": {
                        "send_message": {},
                        "react": {}
                    }
                }
            ]
        });

        let options = connector_options(&output).expect("options");

        assert!(options[0].1.contains("skill=demo"));
        assert!(options[0].1.contains("send_message"));
        assert!(options[0].1.contains("runtime=command"));
        assert_eq!(
            matching_connector_options(&options, "send message"),
            options
        );
    }

    #[test]
    fn resolve_connector_slug_accepts_unique_partial_builtin() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug = resolve_connector_slug(&mut state, &resources, "matrix").expect("slug");

        assert_eq!(slug, "matrix-bot");
    }

    #[test]
    fn resolve_connector_slug_accepts_unique_action_term_match() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug = resolve_connector_slug(&mut state, &resources, "vote poll").expect("slug");

        assert_eq!(slug, "telegram-login");
    }

    #[test]
    fn resolve_connector_slug_accepts_unique_runtime_term_match() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug =
            resolve_connector_slug(&mut state, &resources, "serve http webhook").expect("slug");

        assert_eq!(slug, "webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_github_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug = resolve_connector_slug(&mut state, &resources, "github event").expect("slug");

        assert_eq!(slug, "github-webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_asana_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug =
            resolve_connector_slug(&mut state, &resources, "asana task project").expect("slug");

        assert_eq!(slug, "asana-webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_gitlab_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug =
            resolve_connector_slug(&mut state, &resources, "gitlab merge request").expect("slug");

        assert_eq!(slug, "gitlab-webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_jira_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug =
            resolve_connector_slug(&mut state, &resources, "jira issue comment").expect("slug");

        assert_eq!(slug, "jira-webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_linear_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug = resolve_connector_slug(&mut state, &resources, "linear issue").expect("slug");

        assert_eq!(slug, "linear-webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_pagerduty_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug =
            resolve_connector_slug(&mut state, &resources, "pagerduty incident").expect("slug");

        assert_eq!(slug, "pagerduty-webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_stripe_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug =
            resolve_connector_slug(&mut state, &resources, "stripe invoice payment").expect("slug");

        assert_eq!(slug, "stripe-webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_sentry_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug =
            resolve_connector_slug(&mut state, &resources, "sentry issue alert").expect("slug");

        assert_eq!(slug, "sentry-webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_shopify_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug =
            resolve_connector_slug(&mut state, &resources, "shopify order product").expect("slug");

        assert_eq!(slug, "shopify-webhook");
    }

    #[test]
    fn resolve_connector_slug_accepts_trello_webhook_terms() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let slug =
            resolve_connector_slug(&mut state, &resources, "trello board card").expect("slug");

        assert_eq!(slug, "trello-webhook");
    }

    #[test]
    fn resolve_connector_slug_asks_when_partial_match_is_ambiguous() {
        let mut state = temp_state();
        let resources = LoadedResources::default();
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let request_log = Arc::clone(&requests);

        let slug = with_user_question_prompt_handler(
            move |request| {
                request_log.lock().unwrap().push(request.questions.clone());
                UserQuestionPromptResponse {
                    answers: Map::from_iter([(
                        "Which connector matches `slack`?".to_string(),
                        json!("slack-login"),
                    )]),
                    annotations: Map::new(),
                }
            },
            || resolve_connector_slug(&mut state, &resources, "slack"),
        )
        .expect("slug");

        assert_eq!(slug, "slack-login");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0][0]["searchable"], true);
        let options = requests[0][0]["options"].as_array().expect("options");
        assert!(options.iter().any(|option| option["label"] == "slack-app"));
        assert!(options
            .iter()
            .any(|option| option["label"] == "slack-login"));
    }
}
