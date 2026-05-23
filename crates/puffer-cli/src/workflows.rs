use crate::cli_args::{WorkflowCommand, WorkflowRunsCommand};
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    ActionSpec, FilterSpec, SubscriptionSpec, SubscriptionStatus, SubscriptionStore,
    TaggedFilterSpec,
};
use puffer_workflow::{
    AgentExecution, AgentExecutor, DagRunner, ExecutionContext, RegisterOptions, TriggerSpec,
    WorkflowDefinition, WorkflowRun, WorkflowStore,
};
use serde::Serialize;
use std::fs;
use time::OffsetDateTime;

/// Executes a `puffer workflow ...` CLI command.
pub(crate) fn run_workflow_command(command: WorkflowCommand, paths: &ConfigPaths) -> Result<()> {
    match command {
        WorkflowCommand::Register {
            json_path,
            slug,
            cron,
            connection_slug,
            connection_pattern,
            classify_prompt,
        } => {
            let trigger =
                trigger_from_flags(cron, connection_slug, connection_pattern, classify_prompt);
            let value = read_json_file(&json_path)?;
            let store = WorkflowStore::new(&paths.workspace_config_dir);
            let definition = store.register_json(value, RegisterOptions { slug, trigger })?;
            sync_subscription_trigger(paths, &definition)?;
            println!("{}", serde_json::to_string_pretty(&definition)?);
            Ok(())
        }
        WorkflowCommand::Ls { json } => {
            let store = WorkflowStore::new(&paths.workspace_config_dir);
            let workflows = store.list()?;
            let runs = store.list_runs()?;
            if json {
                print_json(&workflow_summaries(&workflows, &runs))
            } else {
                print_workflow_table(&workflows, &runs);
                Ok(())
            }
        }
        WorkflowCommand::Runs { command } => run_runs_command(command, paths),
        WorkflowCommand::Run {
            workflow_slug,
            trigger_json,
            dry_run,
            json,
        } => run_once(
            paths,
            &workflow_slug,
            trigger_json.as_deref(),
            dry_run,
            json,
        ),
    }
}

fn run_once(
    paths: &ConfigPaths,
    workflow_slug: &str,
    trigger_json: Option<&str>,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    if !dry_run {
        anyhow::bail!(
            "manual workflow runs currently require --dry-run; cron and subscription triggers use the real agent runtime"
        );
    }
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    let definition = store
        .get(workflow_slug)?
        .ok_or_else(|| anyhow::anyhow!("workflow `{workflow_slug}` not found"))?;
    let trigger = match trigger_json {
        Some(raw) => serde_json::from_str(raw).context("parse --trigger-json")?,
        None => serde_json::json!({"type":"manual","dry_run":true}),
    };
    let run = DagRunner::new(&store, DryRunExecutor).run(&definition, trigger, None)?;
    if json {
        print_json(&run)
    } else {
        print_run_detail(&run);
        Ok(())
    }
}

struct DryRunExecutor;

impl AgentExecutor for DryRunExecutor {
    fn execute(&mut self, context: ExecutionContext) -> Result<AgentExecution> {
        Ok(AgentExecution {
            output: format!(
                "dry-run node `{}` agent={} model={} prompt={}",
                context.node_id,
                context.agent.as_deref().unwrap_or("default"),
                context.model.as_deref().unwrap_or("default"),
                context.prompt
            ),
        })
    }
}

fn run_runs_command(command: WorkflowRunsCommand, paths: &ConfigPaths) -> Result<()> {
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    match command {
        WorkflowRunsCommand::Ls {
            workflow_slug,
            json,
        } => {
            let runs = store.list_runs_for(&workflow_slug)?;
            if json {
                print_json(&runs)
            } else {
                print_runs_table(&runs);
                Ok(())
            }
        }
        WorkflowRunsCommand::Show { idx, json } => {
            let run = store
                .get_run(idx)?
                .ok_or_else(|| anyhow::anyhow!("workflow run index {idx} not found"))?;
            if json {
                print_json(&run)
            } else {
                print_run_detail(&run);
                Ok(())
            }
        }
    }
}

fn read_json_file(path: &str) -> Result<serde_json::Value> {
    let text = fs::read_to_string(path).with_context(|| format!("read workflow JSON {path}"))?;
    serde_json::from_str(&text).with_context(|| format!("parse workflow JSON {path}"))
}

fn trigger_from_flags(
    cron: Option<String>,
    connection_slug: Option<String>,
    connection_pattern: Option<String>,
    classify_prompt: Option<String>,
) -> Option<TriggerSpec> {
    if let Some(cron) = cron {
        return Some(TriggerSpec::Cron { cron });
    }
    connection_slug.map(|connection_slug| TriggerSpec::Connection {
        connection_slug,
        filter: None,
        pattern: connection_pattern,
        classify_prompt,
    })
}

fn sync_subscription_trigger(paths: &ConfigPaths, definition: &WorkflowDefinition) -> Result<()> {
    let (connection_slug, filter, classify_prompt) = match &definition.trigger {
        TriggerSpec::Subscription {
            source_topic,
            pattern,
            classify_prompt,
        } => (
            source_topic.clone(),
            pattern.as_ref().map(|pattern| {
                FilterSpec::Tagged(TaggedFilterSpec::Regex {
                    pattern: pattern.clone(),
                    case_insensitive: true,
                })
            }),
            classify_prompt.clone(),
        ),
        TriggerSpec::Connection {
            connection_slug,
            filter,
            pattern,
            classify_prompt,
        } => {
            let filter = match (filter, pattern) {
                (Some(filter), _) => Some(
                    serde_json::from_value::<FilterSpec>(filter.clone())
                        .context("invalid connection trigger filter")?,
                ),
                (None, Some(pattern)) => Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
                    pattern: pattern.clone(),
                    case_insensitive: true,
                })),
                (None, None) => None,
            };
            (connection_slug.clone(), filter, classify_prompt.clone())
        }
        TriggerSpec::Cron { .. } => return Ok(()),
    };
    let store = SubscriptionStore::load(paths.user_config_dir.join("subscriptions.json"))?;
    let id = format!("workflow-{}", definition.slug);
    if store.get(&id).is_some() {
        store.delete(&id)?;
    }
    let spec = SubscriptionSpec {
        slug: id,
        description: format!("Run workflow `{}`", definition.slug),
        connection_slug,
        connector_slug: None,
        status: if definition.enabled {
            SubscriptionStatus::Enabled
        } else {
            SubscriptionStatus::Paused
        },
        filter,
        classify_prompt,
        classify_model: None,
        action: ActionSpec::RunWorkflow {
            slug: definition.slug.clone(),
        },
        created_at_ms: OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000,
    };
    store.create(spec)?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct WorkflowSummary<'a> {
    slug: &'a str,
    enabled: bool,
    trigger: String,
    node_count: usize,
    latest_run: Option<&'a WorkflowRun>,
}

fn workflow_summaries<'a>(
    workflows: &'a [WorkflowDefinition],
    runs: &'a [WorkflowRun],
) -> Vec<WorkflowSummary<'a>> {
    workflows
        .iter()
        .map(|workflow| WorkflowSummary {
            slug: &workflow.slug,
            enabled: workflow.enabled,
            trigger: trigger_label(&workflow.trigger),
            node_count: workflow.pipeline.nodes.len(),
            latest_run: runs
                .iter()
                .filter(|run| run.workflow_slug == workflow.slug)
                .max_by_key(|run| run.idx),
        })
        .collect()
}

fn trigger_label(trigger: &TriggerSpec) -> String {
    match trigger {
        TriggerSpec::Cron { cron } => format!("cron {cron}"),
        TriggerSpec::Subscription { source_topic, .. } => format!("connection {source_topic}"),
        TriggerSpec::Connection {
            connection_slug, ..
        } => format!("connection {connection_slug}"),
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_workflow_table(workflows: &[WorkflowDefinition], runs: &[WorkflowRun]) {
    println!("SLUG                 ENABLED  TRIGGER                         NODES  LATEST");
    for summary in workflow_summaries(workflows, runs) {
        let latest = summary
            .latest_run
            .map(|run| format!("#{} {:?}", run.idx, run.status).to_lowercase())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<20} {:<7} {:<31} {:<5} {}",
            summary.slug, summary.enabled, summary.trigger, summary.node_count, latest
        );
    }
}

fn print_runs_table(runs: &[WorkflowRun]) {
    println!("IDX    WORKFLOW             STATUS      STARTED_MS       NODES");
    for run in runs {
        println!(
            "{:<6} {:<20} {:<11} {:<16} {}",
            run.idx,
            run.workflow_slug,
            format!("{:?}", run.status).to_lowercase(),
            run.started_at_ms,
            run.nodes.len()
        );
    }
}

fn print_run_detail(run: &WorkflowRun) {
    println!("run #{} {} {:?}", run.idx, run.workflow_slug, run.status);
    if let Some(error) = &run.error {
        println!("error: {error}");
    }
    for node in &run.nodes {
        println!("- {} {:?}", node.id, node.status);
        if let Some(output) = &node.output {
            println!("  output: {output}");
        }
        if let Some(error) = &node.error {
            println!("  error: {error}");
        }
    }
}
