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
    assert!(slugs.contains(&"gmail-browser".to_string()));
    assert!(slugs.contains(&"gcal-browser".to_string()));
}

#[test]
fn suggested_connection_slugs_match_connect_defaults() {
    assert_eq!(suggested_connection_slug("telegram-login"), "telegram-user");
    assert_eq!(suggested_connection_slug("email"), "email");
    assert_eq!(suggested_connection_slug("gmail-browser"), "gmail-browser");
    assert_eq!(suggested_connection_slug("gcal-browser"), "gcal-browser");
    assert_eq!(suggested_connection_slug("discord-bot"), "discord-bot");
    assert_eq!(suggested_connection_slug("lark-app"), "lark-app");
    assert_eq!(suggested_connection_slug("matrix-bot"), "matrix-bot");
    assert_eq!(suggested_connection_slug("slack-login"), "slack-login");
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
    let email = builtin_connector_template("email").unwrap();
    let gmail = builtin_connector_template("gmail-browser").unwrap();
    let gcal = builtin_connector_template("gcal-browser").unwrap();

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
    assert!(gmail.can_subscribe);
    assert_eq!(
        gmail.subscriber.as_ref().unwrap().manifest_slug,
        "gmail-browser"
    );
    assert_eq!(
        email
            .actions
            .get("list_emails")
            .unwrap()
            .permission
            .category,
        "external_message_read"
    );
    assert!(
        !email
            .actions
            .get("list_emails")
            .unwrap()
            .permission
            .external_side_effect
    );
    assert_eq!(
        email
            .actions
            .get("draft_reply")
            .unwrap()
            .permission
            .category,
        "external_message_draft"
    );
    assert!(email.actions.contains_key("send_message"));
    assert!(email.actions.contains_key("send_email"));
    assert_eq!(
        gmail
            .actions
            .get("list_emails")
            .unwrap()
            .permission
            .category,
        "external_message_read"
    );
    assert!(gmail.actions.contains_key("send_email"));
    assert!(gmail.actions.contains_key("delete"));
    assert!(gcal.can_subscribe);
    assert_eq!(
        gcal.subscriber.as_ref().unwrap().manifest_slug,
        "gcal-browser"
    );
    assert_eq!(
        gcal.actions.get("get_detail").unwrap().permission.category,
        "external_calendar_read"
    );
    assert!(
        !gcal
            .actions
            .get("get_detail")
            .unwrap()
            .permission
            .external_side_effect
    );
    assert_eq!(
        gcal.actions.get("accept").unwrap().permission.category,
        "external_calendar_rsvp"
    );
    assert!(
        gcal.actions
            .get("accept")
            .unwrap()
            .permission
            .external_side_effect
    );
    assert!(gcal.actions.contains_key("deny"));
}

#[test]
fn serve_mode_connectors_do_not_claim_workflow_runtime_capabilities() {
    for slug in ["discord-bot", "matrix-bot"] {
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
