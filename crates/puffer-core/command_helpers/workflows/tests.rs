#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriptions::{
        ConnectionState, ConnectorActionDefinition, ConnectorPermissionDefinition,
    };
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

    fn action_definition(slug: &str) -> ConnectorActionDefinition {
        ConnectorActionDefinition {
            slug: slug.to_string(),
            description: format!("{slug} action"),
            input_schema: json!({}),
            output_schema: json!({}),
            permission: ConnectorPermissionDefinition {
                category: "external_message_send".to_string(),
                summary: format!("{slug} action"),
                external_side_effect: true,
            },
        }
    }

    fn sample_workflow(slug: &str, name: &str, trigger: TriggerSpec) -> WorkflowDefinition {
        WorkflowDefinition {
            schema: "puffer.workflow.v1".to_string(),
            slug: slug.to_string(),
            enabled: true,
            trigger,
            pipeline: puffer_workflow::AgentFlowPipeline {
                name: name.to_string(),
                working_dir: None,
                concurrency: Some(1),
                nodes: vec![puffer_workflow::PipelineNode {
                    id: "codex".to_string(),
                    node_type: Some("agent".to_string()),
                    agent: Some("Codex".to_string()),
                    prompt: format!("Run {name}."),
                    model: None,
                    tools: Vec::new(),
                    env: BTreeMap::new(),
                    depends_on: Vec::new(),
                    extra: BTreeMap::new(),
                }],
                extra: BTreeMap::new(),
            },
        }
    }

    #[test]
    fn workflow_arg_split_preserves_quoted_command_tail() {
        let (mode, query) = split_workflows_args(r#"append demo-main /tmp/hi "hello world""#);

        assert_eq!(mode, "append");
        assert_eq!(query, r#"demo-main /tmp/hi "hello world""#);

        let (mode, query) = split_workflows_args("  connectors   append file  ");

        assert_eq!(mode, "connectors");
        assert_eq!(query, "append file");
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
            bindings: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connections(&mut out, &context, &roots, "");

        assert!(out.contains("filters: trigger-ready | no-trigger | draft | append | monitor | repair"));
        assert!(out.contains(
            "trigger=ready repair=/connect demo-chat demo-main draft=/workflows new demo-main-workflow demo-main append=/workflows append demo-main /tmp/demo-main.log monitor=/monitor demo-main"
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

        write_workflows(&mut out, &[workflow], &[], "");

        assert!(out.contains("trigger=subscription:workspace.task.created"));
    }

    #[test]
    fn workflow_list_filters_and_reports_counts() {
        let review = sample_workflow(
            "agent-review",
            "Agent review",
            TriggerSpec::Subscription {
                source_topic: "workspace.task.created".to_string(),
                pattern: Some("review".to_string()),
                classify_prompt: None,
            },
        );
        let digest = sample_workflow(
            "daily-digest",
            "Daily digest",
            TriggerSpec::Cron {
                cron: "0 9 * * *".to_string(),
            },
        );
        let mut out = String::new();

        write_workflows(&mut out, &[review, digest], &[], "review subscription");

        assert!(out.contains("showing 1/2 workflows for query=\"review subscription\""));
        assert!(out.contains("- agent-review [enabled]"));
        assert!(!out.contains("- daily-digest ["));

        out.clear();
        write_workflows(&mut out, &[], &[], "missing");

        assert!(out.contains("showing 0/0 workflows for query=\"missing\""));
        assert!(out.contains("- none registered"));
    }

    #[test]
    fn workflow_runs_report_filtered_counts() {
        let runs = vec![
            WorkflowRun {
                idx: 1,
                workflow_slug: "agent-review".to_string(),
                run_id: "run-review".to_string(),
                trigger: json!({"text": "review this"}),
                status: puffer_workflow::WorkflowRunStatus::Completed,
                started_at_ms: 1,
                ended_at_ms: Some(2),
                nodes: vec![puffer_workflow::WorkflowRunNode {
                    id: "review".to_string(),
                    status: puffer_workflow::WorkflowRunStatus::Completed,
                    started_at_ms: Some(1),
                    ended_at_ms: Some(2),
                    output: Some("approved".to_string()),
                    error: None,
                }],
                error: None,
                trigger_key: Some("review-key".to_string()),
            },
            WorkflowRun {
                idx: 2,
                workflow_slug: "daily-digest".to_string(),
                run_id: "run-digest".to_string(),
                trigger: json!({"text": "digest"}),
                status: puffer_workflow::WorkflowRunStatus::Failed,
                started_at_ms: 3,
                ended_at_ms: Some(4),
                nodes: Vec::new(),
                error: Some("delivery failed".to_string()),
                trigger_key: None,
            },
        ];
        let mut out = String::new();

        write_runs(&mut out, &runs, "failed delivery");

        assert!(out.contains("showing 1/2 runs for query=\"failed delivery\""));
        assert!(out.contains("- #2 daily-digest Failed"));
        assert!(!out.contains("- #1 agent-review"));

        out.clear();
        write_runs(&mut out, &runs, "does-not-exist");

        assert!(out.contains("showing 0/2 runs for query=\"does-not-exist\""));
        assert!(out.contains("- no matching runs"));
    }

    #[test]
    fn workflow_actions_show_file_append_bindings() {
        let binding = WorkflowBindingSpec {
            slug: "append-demo-main-hi".to_string(),
            description: "Append demo-main messages to /tmp/hi".to_string(),
            connection_slug: "demo-main".to_string(),
            connector_slug: Some("demo-chat".to_string()),
            status: puffer_subscriptions::WorkflowBindingStatus::Enabled,
            filter: Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
                pattern: "hi".to_string(),
                case_insensitive: true,
            })),
            classify_prompt: None,
            classify_model: None,
            action: serde_json::from_value(json!({
                "type": "file_append",
                "path": "/tmp/hi",
                "format": "text"
            }))
            .unwrap(),
            created_at_ms: 42,
        };
        let context = ConnectorContext {
            connectors: vec![trigger_template()],
            connections: Vec::new(),
            bindings: vec![binding],
            error: None,
        };
        let mut out = String::new();

        write_workflow_bindings(&mut out, &context, "append hi");

        assert!(out.contains("filters: append | file | connection | connector | pattern | enabled | paused | delete"));
        assert!(out.contains("showing 1/1 workflow actions for query=\"append hi\""));
        assert!(out.contains("- append-demo-main-hi [enabled]"));
        assert!(out.contains("connection=demo-main"));
        assert!(out.contains("connector=demo-chat"));
        assert!(out.contains("action=file_append path=/tmp/hi filter=hi"));
        assert!(out.contains("delete=/workflows delete append-demo-main-hi"));

        out.clear();
        write_workflow_bindings(&mut out, &context, "delete append-demo-main-hi");

        assert!(out.contains("showing 1/1 workflow actions for query=\"delete append-demo-main-hi\""));
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
            bindings: Vec::new(),
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
            bindings: Vec::new(),
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

        out.clear();
        write_connections(&mut out, &context, &roots, "append /tmp/demo-main.log");

        assert!(out.contains("showing 1/1 connections for query=\"append /tmp/demo-main.log\""));
        assert!(out.contains("append=/workflows append demo-main /tmp/demo-main.log"));
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
            bindings: Vec::new(),
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
            bindings: Vec::new(),
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
            bindings: Vec::new(),
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
            bindings: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "");

        assert!(out.contains("showing 1/1 connectors"));
        assert!(out.contains("connections=team-demo"));
        assert!(out.contains("connect=/connect demo-chat demo-chat"));
        assert!(out.contains("draft=/workflows new team-demo-workflow team-demo"));
        assert!(out.contains("append=/workflows append team-demo /tmp/team-demo.log"));
    }

    #[test]
    fn workflow_connectors_unique_query_shows_all_action_slugs() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut template = trigger_template();
        for slug in [
            "send_message",
            "edit_message",
            "delete_messages",
            "vote_poll",
        ] {
            template
                .actions
                .insert(slug.to_string(), action_definition(slug));
        }
        let mut other = trigger_template();
        other.slug = "other-chat".to_string();
        other.description = "Other chat".to_string();
        let context = ConnectorContext {
            connectors: vec![template, other],
            connections: Vec::new(),
            bindings: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "demo-chat");

        assert!(out.contains("showing 1/2 connectors for query=\"demo-chat\""));
        assert!(out.contains("actions=send_message,delete_messages,edit_message,+1"));
        assert!(out.contains("actions_all=send_message,delete_messages,edit_message,vote_poll"));
        assert!(!out.contains("- other-chat"));

        out.clear();
        write_connectors(&mut out, &context, &roots, true, "");

        assert!(out.contains("showing 2/2 connectors"));
        assert!(!out.contains("actions_all="));
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
            bindings: Vec::new(),
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
            bindings: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "draft /workflows new demo-chat");

        assert!(out.contains("- demo-chat"));
        assert!(out.contains("draft=/workflows new demo-chat-workflow demo-chat"));
        assert!(!out.contains("- other-chat"));
    }

    #[test]
    fn workflow_connectors_filter_matches_append_command_terms() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut other = trigger_template();
        other.slug = "other-chat".to_string();
        other.description = "Other chat".to_string();
        let context = ConnectorContext {
            connectors: vec![trigger_template(), other],
            connections: Vec::new(),
            bindings: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "append /tmp/demo-chat.log");

        assert!(out.contains("- demo-chat"));
        assert!(out.contains(
            "append=/workflows append demo-chat /tmp/demo-chat.log --connector demo-chat"
        ));
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
            bindings: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "no trigger");

        assert!(out.contains("showing 1/2 connectors for query=\"no trigger\""));
        assert!(out.contains("filters: trigger-ready | no-trigger | draft | append | has-actions | serve"));
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
            bindings: Vec::new(),
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
            bindings: Vec::new(),
            error: None,
        };
        let mut out = String::new();

        write_connectors(&mut out, &context, &roots, true, "serve");

        assert!(out.contains("showing 1/2 connectors for query=\"serve\""));
        assert!(out.contains("- discord-bot [no-trigger] Discord bot runtime=serve"));
        assert!(!out.contains("- demo-chat"));
    }
}
