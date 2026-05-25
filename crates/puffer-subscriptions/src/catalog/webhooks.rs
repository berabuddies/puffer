use super::schemas::*;
use super::ConnectorTemplate;
use serde_json::Value;
use std::collections::BTreeMap;

/// Returns the built-in serve-mode webhook connector presets.
pub(super) fn builtin_webhook_templates() -> Vec<ConnectorTemplate> {
    vec![
        webhook_preset_template(
            "alertmanager-webhook",
            "Prometheus Alertmanager webhook preset backed by puffer serve",
            "alertmanager-webhook",
            alertmanager_alert_output_schema(),
        ),
        webhook_preset_template(
            "asana-webhook",
            "Asana task, project, and story webhook preset backed by puffer serve",
            "asana-webhook",
            asana_event_output_schema(),
        ),
        webhook_preset_template(
            "datadog-webhook",
            "Datadog monitor and event webhook preset backed by puffer serve",
            "datadog-webhook",
            datadog_event_output_schema(),
        ),
        webhook_preset_template(
            "newrelic-webhook",
            "New Relic issue and alert webhook preset backed by puffer serve",
            "newrelic-webhook",
            newrelic_issue_output_schema(),
        ),
        webhook_preset_template(
            "opsgenie-webhook",
            "Opsgenie alert action webhook preset backed by puffer serve",
            "opsgenie-webhook",
            opsgenie_alert_output_schema(),
        ),
        webhook_preset_template(
            "github-webhook",
            "GitHub event webhook preset backed by puffer serve",
            "github-webhook",
            github_event_output_schema(),
        ),
        webhook_preset_template(
            "grafana-webhook",
            "Grafana Alerting webhook preset backed by puffer serve",
            "grafana-webhook",
            grafana_alert_output_schema(),
        ),
        webhook_preset_template(
            "gitlab-webhook",
            "GitLab issue, merge request, comment, and push webhook preset backed by puffer serve",
            "gitlab-webhook",
            gitlab_event_output_schema(),
        ),
        webhook_preset_template(
            "jira-webhook",
            "Jira issue and comment webhook preset backed by puffer serve",
            "jira-webhook",
            jira_event_output_schema(),
        ),
        webhook_preset_template(
            "linear-webhook",
            "Linear issue and project webhook preset backed by puffer serve",
            "linear-webhook",
            linear_event_output_schema(),
        ),
        webhook_preset_template(
            "pagerduty-webhook",
            "PagerDuty incident and service webhook preset backed by puffer serve",
            "pagerduty-webhook",
            pagerduty_event_output_schema(),
        ),
        webhook_preset_template(
            "sentry-webhook",
            "Sentry issue, event, and alert webhook preset backed by puffer serve",
            "sentry-webhook",
            sentry_event_output_schema(),
        ),
        webhook_preset_template(
            "shopify-webhook",
            "Shopify order, product, customer, and inventory webhook preset backed by puffer serve",
            "shopify-webhook",
            shopify_event_output_schema(),
        ),
        webhook_preset_template(
            "stripe-webhook",
            "Stripe invoice, payment, and billing webhook preset backed by puffer serve",
            "stripe-webhook",
            stripe_event_output_schema(),
        ),
        webhook_preset_template(
            "trello-webhook",
            "Trello board, card, list, and comment webhook preset backed by puffer serve",
            "trello-webhook",
            trello_event_output_schema(),
        ),
        generic_webhook_template(),
    ]
}

/// Returns whether a connector slug is one of the built-in webhook presets.
pub(super) fn is_builtin_webhook_slug(slug: &str) -> bool {
    matches!(
        slug,
        "alertmanager-webhook"
            | "asana-webhook"
            | "datadog-webhook"
            | "newrelic-webhook"
            | "opsgenie-webhook"
            | "github-webhook"
            | "grafana-webhook"
            | "gitlab-webhook"
            | "jira-webhook"
            | "linear-webhook"
            | "pagerduty-webhook"
            | "sentry-webhook"
            | "shopify-webhook"
            | "stripe-webhook"
            | "trello-webhook"
            | "webhook"
    )
}

fn webhook_preset_template(
    slug: &str,
    description: &str,
    skill: &str,
    output_schema: Value,
) -> ConnectorTemplate {
    ConnectorTemplate {
        slug: slug.to_string(),
        description: description.to_string(),
        skill: skill.to_string(),
        binary: "puffer connector webhook".to_string(),
        command: Vec::new(),
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        subscriber: None,
        output_schema,
        actions: BTreeMap::new(),
    }
}

fn generic_webhook_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "webhook".to_string(),
        description: "HTTP webhook connector configured through puffer serve".to_string(),
        skill: "webhook".to_string(),
        binary: "puffer connector webhook".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: BTreeMap::new(),
    }
}
