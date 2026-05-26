#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriptions::ConnectionState;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn trigger_template() -> ConnectorTemplate {
        ConnectorTemplate {
            slug: "demo-chat".to_string(),
            description: "Demo chat".to_string(),
            skill: "demo".to_string(),
            binary: "demo".to_string(),
            command: vec!["true".to_string()],
            requires_auth: false,
            can_subscribe: true,
            can_proxy_agent: false,
            subscriber: None,
            output_schema: json!({}),
            actions: BTreeMap::new(),
        }
    }

    #[test]
    fn workflow_connections_show_monitor_command_for_trigger_ready_connections() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let context = ConnectorContext {
            connectors: vec![trigger_template()],
            connections: vec![ConnectionRecord {
                slug: "demo-main".to_string(),
                connector_slug: "demo-chat".to_string(),
                description: "Demo main channel".to_string(),
                state: ConnectionState::Authenticated,
                has_consumer: false,
                cursor: None,
                auth_failure_notified: false,
            }],
            error: None,
        };
        let mut out = String::new();

        write_connections(&mut out, &context, &roots, "");

        assert!(out.contains("filters: trigger-ready | no-trigger | draft | monitor | repair"));
        assert!(out.contains(
            "trigger=ready repair=/connect demo-chat demo-main draft=/workflows new demo-main-workflow demo-main monitor=/monitor demo-main"
        ));
    }

    #[test]
    fn workflow_list_labels_subscription_triggers_as_subscriptions() {
        let workflow = WorkflowDefinition {
            schema: "puffer.workflow.v1".to_string(),
            slug: "agent-review".to_string(),
            enabled: false,
            trigger: TriggerSpec::Subscription {
                source_topic: "workspace.task.created".to_string(),
                pattern: Some("review".to_string()),
                classify_prompt: None,
            },
            pipeline: puffer_workflow::AgentFlowPipeline {
                name: "Agent review".to_string(),
                working_dir: None,
                concurrency: Some(1),
                nodes: vec![puffer_workflow::PipelineNode {
                    id: "codex".to_string(),
                    node_type: Some("codex".to_string()),
                    agent: Some("Codex".to_string()),
                    prompt: "Review the event.".to_string(),
                    model: None,
                    tools: Vec::new(),
                    env: BTreeMap::new(),
                    depends_on: Vec::new(),
                    extra: BTreeMap::new(),
                }],
                extra: BTreeMap::new(),
            },
        };
        let mut out = String::new();

        write_workflows(&mut out, &[workflow], &[]);

        assert!(out.contains("trigger=subscription:workspace.task.created"));
    }

    #[test]
    fn workflow_connections_omit_monitor_command_for_non_trigger_connections() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut template = trigger_template();
        template.can_subscribe = false;
        template.command.clear();
        let context = ConnectorContext {
            connectors: vec![template],
            connections: vec![ConnectionRecord {
                slug: "demo-main".to_string(),
                connector_slug: "demo-chat".to_string(),
                description: "Demo main channel".to_string(),
                state: ConnectionState::Authenticated,
                has_consumer: false,
                cursor: None,
                auth_failure_notified: false,
            }],
            error: None,
        };
        let mut out = String::new();

        write_connections(&mut out, &context, &roots, "");

        assert!(out.contains("trigger=unavailable"));
        assert!(out.contains("repair=/connect demo-chat demo-main"));
        assert!(!out.contains("draft=/workflows new"));
        assert!(!out.contains("monitor=/monitor"));
    }

    #[test]
    fn workflow_connections_filter_matches_repair_and_draft_command_terms() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let context = ConnectorContext {
            connectors: vec![trigger_template()],
            connections: vec![ConnectionRecord {
                slug: "demo-main".to_string(),
                connector_slug: "demo-chat".to_string(),
                description: "Demo main channel".to_string(),
                state: ConnectionState::Authenticated,
                has_consumer: false,
                cursor: None,
                auth_failure_notified: false,
            }],
            error: None,
        };
        let mut out = String::new();

        write_connections(&mut out, &context, &roots, "repair demo-chat");

        assert!(out.contains("showing 1/1 connections for query=\"repair demo-chat\""));
        assert!(out.contains("repair=/connect demo-chat demo-main"));

        out.clear();
        write_connections(&mut out, &context, &roots, "draft demo-main");

        assert!(out.contains("showing 1/1 connections for query=\"draft demo-main\""));
        assert!(out.contains("draft=/workflows new demo-main-workflow demo-main"));
    }

    #[test]
    fn workflow_connections_filter_matches_monitor_and_trigger_terms() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut setup_only = trigger_template();
        setup_only.slug = "setup-chat".to_string();
        setup_only.can_subscribe = false;
        setup_only.command.clear();
        let context = ConnectorContext {
            connectors: vec![trigger_template(), setup_only],
            connections: vec![
                ConnectionRecord {
                    slug: "demo-main".to_string(),
                    connector_slug: "demo-chat".to_string(),
                    description: "Demo main channel".to_string(),
                    state: ConnectionState::Authenticated,
                    has_consumer: false,
                    cursor: None,
                    auth_failure_notified: false,
                },
                ConnectionRecord {
                    slug: "setup-main".to_string(),
                    connector_slug: "setup-chat".to_string(),
                    description: "Setup main channel".to_string(),
                    state: ConnectionState::Authenticated,
                    has_consumer: false,
                    cursor: None,
                    auth_failure_notified: false,
                },
            ],
            error: None,
        };
        let mut out = String::new();

        write_connections(&mut out, &context, &roots, "monitor");

        assert!(out.contains("showing 1/2 connections for query=\"monitor\""));
        assert!(out.contains("- demo-main"));
        assert!(out.contains("monitor=/monitor demo-main"));
        assert!(!out.contains("- setup-main"));

        out.clear();
        write_connections(&mut out, &context, &roots, "no-trigger");

        assert!(out.contains("showing 1/2 connections for query=\"no-trigger\""));
        assert!(out.contains("- setup-main"));
        assert!(!out.contains("- demo-main"));
    }

    #[test]
    fn workflow_connections_filter_matches_consumer_terms() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let context = ConnectorContext {
            connectors: vec![trigger_template()],
            connections: vec![
                ConnectionRecord {
                    slug: "demo-active".to_string(),
                    connector_slug: "demo-chat".to_string(),
                    description: "Demo active channel".to_string(),
                    state: ConnectionState::Active,
                    has_consumer: true,
                    cursor: None,
                    auth_failure_notified: false,
                },
                ConnectionRecord {
                    slug: "demo-idle".to_string(),
                    connector_slug: "demo-chat".to_string(),
                    description: "Demo idle channel".to_string(),
                    state: ConnectionState::Authenticated,
                    has_consumer: false,
                    cursor: None,
                    auth_failure_notified: false,
                },
            ],
            error: None,
        };
        let mut out = String::new();

        write_connections(&mut out, &context, &roots, "consumer active");

        assert!(out.contains("showing 1/2 connections for query=\"consumer active\""));
        assert!(out.contains("- demo-active"));
        assert!(!out.contains("- demo-idle"));

        out.clear();
        write_connections(&mut out, &context, &roots, "consumer idle");

        assert!(out.contains("showing 1/2 connections for query=\"consumer idle\""));
        assert!(out.contains("- demo-idle"));
        assert!(!out.contains("- demo-active"));
    }

    #[test]
    fn workflow_connections_report_filtered_result_counts() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let context = ConnectorContext {
            connectors: vec![trigger_template()],
            connections: vec![
                ConnectionRecord {
                    slug: "demo-main".to_string(),
                    connector_slug: "demo-chat".to_string(),
                    description: "Demo main channel".to_string(),
                    state: ConnectionState::Authenticated,
                    has_consumer: false,
                    cursor: None,
                    auth_failure_notified: false,
                },
                ConnectionRecord {
                    slug: "demo-archive".to_string(),
                    connector_slug: "demo-chat".to_string(),
                    description: "Demo archive channel".to_string(),
                    state: ConnectionState::Authenticated,
                    has_consumer: false,
                    cursor: None,
                    auth_failure_notified: false,
                },
            ],
            error: None,
        };
        let mut out = String::new();

        write_connections(&mut out, &context, &roots, "archive");

        assert!(out.contains("showing 1/2 connections for query=\"archive\""));
        assert!(out.contains("- demo-archive"));
        assert!(!out.contains("- demo-main"));
    }

    #[test]
    fn workflow_connectors_show_existing_connection_names() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let context = ConnectorContext {
            connectors: vec![trigger_template()],
            connections: vec![ConnectionRecord {
                slug: "team-demo".to_string(),
                connector_slug: "demo-chat".to_string(),
                description: "Team demo channel".to_string(),
                state: ConnectionState::Authenticated,
                has_consumer: false,
                cursor: None,
                auth_failure_notified: false,
            }],
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "");

        assert!(out.contains("showing 1/1 connectors"));
        assert!(out.contains("connections=team-demo"));
        assert!(out.contains("connect=/connect demo-chat demo-chat"));
        assert!(out.contains("draft=/workflows new team-demo-workflow team-demo"));
    }

    #[test]
    fn workflow_connectors_filter_matches_existing_connection_names() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut other = trigger_template();
        other.slug = "other-chat".to_string();
        other.description = "Other chat".to_string();
        let context = ConnectorContext {
            connectors: vec![trigger_template(), other],
            connections: vec![ConnectionRecord {
                slug: "team-demo".to_string(),
                connector_slug: "demo-chat".to_string(),
                description: "Team demo channel".to_string(),
                state: ConnectionState::Authenticated,
                has_consumer: false,
                cursor: None,
                auth_failure_notified: false,
            }],
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "team-demo");

        assert!(out.contains("showing 1/2 connectors for query=\"team-demo\""));
        assert!(out.contains("- demo-chat"));
        assert!(out.contains("connections=team-demo"));
        assert!(!out.contains("- other-chat"));
    }

    #[test]
    fn workflow_connectors_filter_matches_draft_command_terms() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut other = trigger_template();
        other.slug = "other-chat".to_string();
        other.description = "Other chat".to_string();
        let context = ConnectorContext {
            connectors: vec![trigger_template(), other],
            connections: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "draft /workflows new demo-chat");

        assert!(out.contains("- demo-chat"));
        assert!(out.contains("draft=/workflows new demo-chat-workflow demo-chat"));
        assert!(!out.contains("- other-chat"));
    }

    #[test]
    fn workflow_connectors_label_and_filter_setup_only_connectors() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut setup_only = trigger_template();
        setup_only.slug = "setup-chat".to_string();
        setup_only.description = "Setup only chat".to_string();
        setup_only.can_subscribe = false;
        setup_only.command.clear();
        let context = ConnectorContext {
            connectors: vec![trigger_template(), setup_only],
            connections: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "no trigger");

        assert!(out.contains("showing 1/2 connectors for query=\"no trigger\""));
        assert!(out.contains("filters: trigger-ready | no-trigger | draft | has-actions | serve"));
        assert!(out.contains("- setup-chat [no-trigger]"));
        assert!(!out.contains("draft=/workflows new setup-chat"));
        assert!(!out.contains("- demo-chat"));
    }

    #[test]
    fn workflow_connectors_filter_matches_trigger_ready_without_setup_only_rows() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut setup_only = trigger_template();
        setup_only.slug = "setup-chat".to_string();
        setup_only.description = "Setup only chat".to_string();
        setup_only.can_subscribe = false;
        setup_only.command.clear();
        let context = ConnectorContext {
            connectors: vec![trigger_template(), setup_only],
            connections: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "trigger-ready");

        assert!(out.contains("showing 1/2 connectors for query=\"trigger-ready\""));
        assert!(out.contains("- demo-chat [events,trigger]"));
        assert!(!out.contains("- setup-chat"));
    }

    #[test]
    fn workflow_connectors_filter_matches_runtime_hints() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut serve = trigger_template();
        serve.slug = "discord-bot".to_string();
        serve.description = "Discord bot".to_string();
        serve.can_subscribe = false;
        serve.command.clear();
        let context = ConnectorContext {
            connectors: vec![trigger_template(), serve],
            connections: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "serve");

        assert!(out.contains("showing 1/2 connectors for query=\"serve\""));
        assert!(out.contains("- discord-bot [no-trigger] Discord bot runtime=serve"));
        assert!(!out.contains("- demo-chat"));
    }
}
