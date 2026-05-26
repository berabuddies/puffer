use crate::{builtin_connector_template, builtin_connector_templates, suggested_connection_slug};

#[test]
fn builtins_cover_required_initial_connectors() {
    let slugs = builtin_connector_templates()
        .into_iter()
        .map(|template| template.slug)
        .collect::<Vec<_>>();

    assert!(slugs.contains(&"telegram-bot".to_string()));
    assert!(slugs.contains(&"telegram-login".to_string()));
    assert!(slugs.contains(&"discord-bot".to_string()));
    assert!(slugs.contains(&"lark-app".to_string()));
    assert!(slugs.contains(&"lark-login".to_string()));
    assert!(slugs.contains(&"matrix-bot".to_string()));
    assert!(slugs.contains(&"slack-app".to_string()));
    assert!(slugs.contains(&"slack-login".to_string()));
    assert!(slugs.contains(&"slack-bot".to_string()));
    assert!(slugs.contains(&"email".to_string()));
    assert!(slugs.contains(&"alertmanager-webhook".to_string()));
    assert!(slugs.contains(&"asana-webhook".to_string()));
    assert!(slugs.contains(&"datadog-webhook".to_string()));
    assert!(slugs.contains(&"newrelic-webhook".to_string()));
    assert!(slugs.contains(&"opsgenie-webhook".to_string()));
    assert!(slugs.contains(&"azure-devops-webhook".to_string()));
    assert!(slugs.contains(&"bitbucket-webhook".to_string()));
    assert!(slugs.contains(&"figma-webhook".to_string()));
    assert!(slugs.contains(&"github-webhook".to_string()));
    assert!(slugs.contains(&"grafana-webhook".to_string()));
    assert!(slugs.contains(&"gitlab-webhook".to_string()));
    assert!(slugs.contains(&"jira-webhook".to_string()));
    assert!(slugs.contains(&"linear-webhook".to_string()));
    assert!(slugs.contains(&"pagerduty-webhook".to_string()));
    assert!(slugs.contains(&"sentry-webhook".to_string()));
    assert!(slugs.contains(&"shopify-webhook".to_string()));
    assert!(slugs.contains(&"stripe-webhook".to_string()));
    assert!(slugs.contains(&"trello-webhook".to_string()));
    assert!(slugs.contains(&"vercel-webhook".to_string()));
    assert!(slugs.contains(&"webhook".to_string()));
}

#[test]
fn suggested_connection_slugs_match_connect_defaults() {
    assert_eq!(suggested_connection_slug("telegram-login"), "telegram-user");
    assert_eq!(suggested_connection_slug("email"), "email");
    assert_eq!(suggested_connection_slug("discord-bot"), "discord-bot");
    assert_eq!(suggested_connection_slug("lark-app"), "lark-app");
    assert_eq!(suggested_connection_slug("matrix-bot"), "matrix-bot");
    assert_eq!(suggested_connection_slug("slack-login"), "slack-login");
    assert_eq!(
        suggested_connection_slug("alertmanager-webhook"),
        "alertmanager-webhook"
    );
    assert_eq!(suggested_connection_slug("asana-webhook"), "asana-webhook");
    assert_eq!(
        suggested_connection_slug("datadog-webhook"),
        "datadog-webhook"
    );
    assert_eq!(
        suggested_connection_slug("newrelic-webhook"),
        "newrelic-webhook"
    );
    assert_eq!(
        suggested_connection_slug("opsgenie-webhook"),
        "opsgenie-webhook"
    );
    assert_eq!(
        suggested_connection_slug("azure-devops-webhook"),
        "azure-devops-webhook"
    );
    assert_eq!(
        suggested_connection_slug("bitbucket-webhook"),
        "bitbucket-webhook"
    );
    assert_eq!(suggested_connection_slug("figma-webhook"), "figma-webhook");
    assert_eq!(
        suggested_connection_slug("github-webhook"),
        "github-webhook"
    );
    assert_eq!(
        suggested_connection_slug("grafana-webhook"),
        "grafana-webhook"
    );
    assert_eq!(
        suggested_connection_slug("gitlab-webhook"),
        "gitlab-webhook"
    );
    assert_eq!(suggested_connection_slug("jira-webhook"), "jira-webhook");
    assert_eq!(
        suggested_connection_slug("linear-webhook"),
        "linear-webhook"
    );
    assert_eq!(
        suggested_connection_slug("pagerduty-webhook"),
        "pagerduty-webhook"
    );
    assert_eq!(
        suggested_connection_slug("sentry-webhook"),
        "sentry-webhook"
    );
    assert_eq!(
        suggested_connection_slug("shopify-webhook"),
        "shopify-webhook"
    );
    assert_eq!(
        suggested_connection_slug("stripe-webhook"),
        "stripe-webhook"
    );
    assert_eq!(
        suggested_connection_slug("trello-webhook"),
        "trello-webhook"
    );
    assert_eq!(
        suggested_connection_slug("vercel-webhook"),
        "vercel-webhook"
    );
    assert_eq!(suggested_connection_slug("webhook"), "webhook");
    assert_eq!(suggested_connection_slug("custom-feed"), "custom-feed");
}

#[test]
fn builtins_define_host_enforced_action_permissions() {
    let telegram = builtin_connector_template("telegram-login").unwrap();
    let action = telegram.actions.get("send_message").unwrap();
    let vote = telegram.actions.get("vote_poll").unwrap();
    let update_group = telegram.actions.get("update_group_title").unwrap();
    let lark = builtin_connector_template("lark-login").unwrap();
    let slack = builtin_connector_template("slack-login").unwrap();

    assert_eq!(action.permission.category, "external_message_send");
    assert!(action.permission.external_side_effect);
    assert_eq!(
        telegram.subscriber.as_ref().unwrap().manifest_slug,
        "telegram-user"
    );
    assert_eq!(vote.permission.category, "external_message_interaction");
    assert!(vote.permission.external_side_effect);
    assert_eq!(update_group.permission.category, "external_chat_admin");
    assert!(lark.actions.contains_key("send_message"));
    assert_eq!(
        lark.actions.get("react").unwrap().permission.category,
        "external_message_interaction"
    );
    assert!(!lark.can_subscribe);
    assert!(slack.actions.contains_key("send_message"));
    assert_eq!(
        slack.actions.get("react").unwrap().permission.category,
        "external_message_interaction"
    );
    assert!(
        !builtin_connector_template("slack-app")
            .unwrap()
            .can_subscribe
    );
    assert!(
        !builtin_connector_template("slack-app")
            .unwrap()
            .can_proxy_agent
    );
    assert!(
        !builtin_connector_template("slack-bot")
            .unwrap()
            .can_subscribe
    );
    assert!(
        !builtin_connector_template("slack-bot")
            .unwrap()
            .can_proxy_agent
    );
    assert!(builtin_connector_template("slack-bot")
        .unwrap()
        .actions
        .is_empty());
    assert!(!slack.actions.contains_key("vote_poll"));
    assert!(!slack.actions.contains_key("update_group_title"));
}

#[test]
fn serve_mode_connectors_do_not_claim_workflow_runtime_capabilities() {
    for slug in [
        "discord-bot",
        "matrix-bot",
        "alertmanager-webhook",
        "asana-webhook",
        "datadog-webhook",
        "newrelic-webhook",
        "opsgenie-webhook",
        "azure-devops-webhook",
        "bitbucket-webhook",
        "figma-webhook",
        "github-webhook",
        "grafana-webhook",
        "gitlab-webhook",
        "jira-webhook",
        "linear-webhook",
        "pagerduty-webhook",
        "sentry-webhook",
        "shopify-webhook",
        "stripe-webhook",
        "trello-webhook",
        "vercel-webhook",
        "webhook",
    ] {
        let template = builtin_connector_template(slug).unwrap();
        assert!(!template.can_subscribe, "{slug} should not claim triggers");
        assert!(
            !template.can_proxy_agent,
            "{slug} should not claim subscription proxy routing"
        );
        assert!(
            template.actions.is_empty(),
            "{slug} should not advertise ConnectorAct support"
        );
        assert!(
            template.command_argv().is_none(),
            "{slug} should not advertise connector protocol command support"
        );
    }
}
