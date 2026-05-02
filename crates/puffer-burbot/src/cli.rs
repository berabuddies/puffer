use crate::contract::{ContractRegistry, InMemoryContractRegistry};
use crate::eval::{load_eval_suite, run_eval_suite};
use crate::evolve::{propose_mutations, EvalComparisonReport};
use crate::ids::RunId;
use crate::llm::{probe_openai, validate_puffer_tool_call, LlmProbeOptions};
use crate::planner::CandidateSource;
use crate::promotion::{
    advance_promotion, candidate_from_json, evaluate_candidate, write_promotion_record,
    PromotionRecord,
};
use crate::puffer_tools::{lint_puffer_tool_contracts, report_as_json};
use crate::runtime::{default_trace_dir, BurbotRuntime, RunOptions, RunResult};
use crate::symbolic::symbolic_contract;
use crate::trace::JsonlTraceStore;
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    bin_name = "burbot",
    about = "Experimental contract-driven Burbot agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect and validate capability contracts.
    Contract {
        #[command(subcommand)]
        command: ContractCommand,
    },
    /// Run one Burbot goal through contracts, rewrites, scheduler, safety, and executor.
    Run {
        /// User goal to run.
        #[arg(long)]
        goal: String,
        /// Use the configured OpenAI/Codex credential to propose Puffer tool calls.
        #[arg(long, default_value_t = false)]
        llm_tool_call: bool,
        /// Disable the default LLM proposal source for free-text goals.
        #[arg(long, default_value_t = false)]
        no_llm_tool_call: bool,
        /// Model for the Puffer-tool proposal operator.
        #[arg(long, default_value = "gpt-5.5")]
        model: String,
        /// Puffer tools resource directory to import as contracts.
        #[arg(long, default_value = "resources/tools")]
        tools: PathBuf,
        /// Optional Puffer tool id to execute through the puffer.tools contract.
        #[arg(long)]
        puffer_tool: Option<String>,
        /// JSON arguments for --puffer-tool.
        #[arg(long)]
        puffer_args: Option<String>,
        /// Treat a failed terminal action as the expected task result.
        #[arg(long, default_value_t = false)]
        expect_failure: bool,
        /// Enable Burbot's internal symbolic proposal/critic/verifier workers.
        #[arg(long, default_value_t = false)]
        symbolic_workers: bool,
        /// Execute contract-proven read-only frontier actions in a safe parallel batch.
        #[arg(long, default_value_t = false)]
        parallel_read_only: bool,
        /// YOLO mode: bypass the safety gate entirely. Risky; use only in sandboxed benchmarks.
        #[arg(long, default_value_t = false)]
        yolo: bool,
    },
    /// Inspect JSONL traces written by Burbot.
    Trace {
        #[command(subcommand)]
        command: TraceCommand,
    },
    /// Run Burbot eval suites.
    Eval {
        #[command(subcommand)]
        command: EvalCommand,
    },
    /// Propose candidate contract mutations from traces.
    Evolve {
        #[command(subcommand)]
        command: EvolveCommand,
    },
    /// Probe Puffer/OpenAI credentials through the Burbot LLM adapter.
    Llm {
        #[command(subcommand)]
        command: LlmCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ContractCommand {
    /// Lint every contract.md under a path.
    Lint {
        /// Contract root or a single contract.md.
        path: PathBuf,
    },
    /// Print a compact capability graph summary.
    Graph {
        /// Contract root or a single contract.md.
        path: PathBuf,
    },
    /// Lint Puffer resource tool contracts and report excluded tools.
    LintPufferTools {
        /// Tool resource root or a single tool YAML.
        path: PathBuf,
    },
    /// Print the supported machine-contract schema shape.
    Schema,
}

#[derive(Debug, Subcommand)]
enum TraceCommand {
    /// List recorded run IDs.
    List,
    /// Show a run trace.
    Show {
        /// Run UUID.
        run_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum EvalCommand {
    /// Run an eval suite YAML.
    Run {
        /// Eval suite YAML path.
        suite: PathBuf,
        /// Puffer tools resource directory to import as contracts.
        #[arg(long, default_value = "resources/tools")]
        tools: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum EvolveCommand {
    /// Propose candidate mutations from one run trace.
    Propose {
        /// Run UUID.
        run_id: String,
    },
    /// Evaluate a candidate mutation against a JSON comparison report.
    Evaluate {
        /// Mutation candidate JSON, or an array whose first item is a candidate.
        candidate: PathBuf,
        /// EvalComparisonReport JSON.
        report: PathBuf,
    },
    /// Advance and persist a Burbot-local promotion record.
    Promote {
        /// Promotion record JSON, or mutation candidate JSON when --report is set.
        input: PathBuf,
        /// Optional EvalComparisonReport JSON when input is a mutation candidate.
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum LlmCommand {
    /// Send a minimal Responses API probe with local Puffer/Codex credentials.
    Probe {
        /// Model to test.
        #[arg(long, default_value = "gpt-5.5")]
        model: String,
        /// Prompt sent to the model.
        #[arg(long, default_value = "Return {\"burbot\":\"ok\"} and no extra prose.")]
        prompt: String,
        /// Build the request without sending it.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

struct CliRunOptions {
    goal: String,
    llm_tool_call: bool,
    no_llm_tool_call: bool,
    model: String,
    tools: PathBuf,
    puffer_tool: Option<String>,
    puffer_args: Option<String>,
    expect_failure: bool,
    symbolic_workers: bool,
    parallel_read_only: bool,
    yolo: bool,
}

pub(crate) fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Contract { command } => run_contract(command),
        Command::Run {
            goal,
            llm_tool_call,
            no_llm_tool_call,
            model,
            tools,
            puffer_tool,
            puffer_args,
            expect_failure,
            symbolic_workers,
            parallel_read_only,
            yolo,
        } => run_goal(CliRunOptions {
            goal,
            llm_tool_call,
            no_llm_tool_call,
            model,
            tools,
            puffer_tool,
            puffer_args,
            expect_failure,
            symbolic_workers,
            parallel_read_only,
            yolo,
        }),
        Command::Trace { command } => run_trace(command),
        Command::Eval { command } => run_eval(command),
        Command::Evolve { command } => run_evolve(command),
        Command::Llm { command } => run_llm(command),
    }
}

fn run_contract(command: ContractCommand) -> Result<()> {
    match command {
        ContractCommand::Lint { path } => {
            let errors = InMemoryContractRegistry::lint_dir(&path)?;
            if errors.is_empty() {
                println!("ok: contracts linted successfully");
                Ok(())
            } else {
                for error in &errors {
                    eprintln!("{}: {}", error.path, error.message);
                }
                Err(anyhow!("{} contract lint error(s)", errors.len()))
            }
        }
        ContractCommand::Graph { path } => {
            let registry = InMemoryContractRegistry::load_dir(&path)?;
            let summary = registry
                .active_contracts()
                .into_iter()
                .map(|contract| {
                    json!({
                        "contract_id": contract.contract_id,
                        "version": contract.version,
                        "actions": contract.actions.into_iter().map(|action| {
                            json!({
                                "name": action.name,
                                "risk_level": action.risk_level,
                                "side_effect_class": action.side_effect_class,
                                "failure_modes": action.failure_modes.len(),
                                "verification_methods": action.verification.methods.len(),
                            })
                        }).collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>();
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(())
        }
        ContractCommand::LintPufferTools { path } => {
            let report = lint_puffer_tool_contracts(&path)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report_as_json(&report))?
            );
            if report.missing_contracts.is_empty() && report.lint_errors.is_empty() {
                Ok(())
            } else {
                Err(anyhow!(
                    "{} missing contract(s), {} lint error(s)",
                    report.missing_contracts.len(),
                    report.lint_errors.len()
                ))
            }
        }
        ContractCommand::Schema => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "required": ["contract_id", "version", "status", "trust_level", "description", "actions"],
                    "action_required": [
                        "name",
                        "description",
                        "input_schema",
                        "output_schema",
                        "side_effect_class",
                        "reversibility",
                        "idempotency",
                        "risk_level",
                        "verification",
                        "approval"
                    ],
                    "format": "Markdown file with a ## Machine Contract section containing a fenced yaml block"
                }))?
            );
            Ok(())
        }
    }
}

fn run_goal(options: CliRunOptions) -> Result<()> {
    let workspace_root = std::env::current_dir()?;
    let report = lint_puffer_tool_contracts(&options.tools)?;
    if !report.missing_contracts.is_empty() || !report.lint_errors.is_empty() {
        return Err(anyhow!(
            "Puffer tool contracts are not usable: {} missing contract(s), {} lint error(s)",
            report.missing_contracts.len(),
            report.lint_errors.len()
        ));
    }
    let should_propose_tool =
        options.puffer_tool.is_none() && (options.llm_tool_call || !options.no_llm_tool_call);
    let (puffer_tool, puffer_args, puffer_tool_source, enable_observe_act_llm) =
        if should_propose_tool {
            (None, None, CandidateSource::ModelProposal, true)
        } else if let Some(tool_id) = options.puffer_tool {
            let args = options
                .puffer_args
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or_else(|| json!({}));
            let proposal = validate_puffer_tool_call(tool_id.trim(), args, &report.contract)?;
            (
                Some(proposal.tool_id),
                Some(proposal.args),
                CandidateSource::ExplicitUserTool,
                false,
            )
        } else {
            (None, None, CandidateSource::ExplicitUserTool, false)
        };
    if puffer_tool.is_none() && !enable_observe_act_llm {
        return Err(anyhow!(
            "free-text Burbot goals require a proposal source; pass --puffer-tool/--puffer-args, allow the default LLM proposal, or omit --no-llm-tool-call"
        ));
    }
    let mut registry = InMemoryContractRegistry::default();
    registry.register(report.contract)?;
    if options.symbolic_workers {
        registry.register(symbolic_contract())?;
    }
    let trace_store = JsonlTraceStore::new(default_trace_dir(workspace_root.clone()));
    let mut runtime = BurbotRuntime::new(registry, workspace_root, trace_store)?;
    let result = runtime.run_goal(
        options.goal,
        RunOptions {
            puffer_tool,
            puffer_args,
            puffer_tool_source,
            allow_failed_terminal_completion: options.expect_failure,
            enable_symbolic_workers: options.symbolic_workers,
            enable_parallel_read_only: options.parallel_read_only,
            enable_observe_act_llm,
            model: enable_observe_act_llm.then_some(options.model),
            goal_verification_min_confidence: 0.4,
            yolo: options.yolo,
        },
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&run_result_summary(&result))?
    );
    ensure_run_result_completed(&result)
}

fn run_trace(command: TraceCommand) -> Result<()> {
    let store = JsonlTraceStore::new(default_trace_dir(std::env::current_dir()?));
    match command {
        TraceCommand::List => {
            for run_id in store.list_runs()? {
                println!("{}", run_id.0);
            }
            Ok(())
        }
        TraceCommand::Show { run_id } => {
            let run_id = parse_run_id(&run_id)?;
            let events = store.events_for_run(run_id)?;
            for event in events {
                let action = match (&event.contract_id, &event.action_name) {
                    (Some(contract), Some(action)) => format!(" {contract}.{action}"),
                    _ => String::new(),
                };
                let success = event
                    .success
                    .map(|value| format!(" success={value}"))
                    .unwrap_or_default();
                println!(
                    "{} {:?}{}{} {}",
                    event.timestamp_ms,
                    event.event_type,
                    action,
                    success,
                    compact_json(&event.output)
                );
            }
            Ok(())
        }
    }
}

fn run_eval(command: EvalCommand) -> Result<()> {
    match command {
        EvalCommand::Run { suite, tools } => {
            let suite = load_eval_suite(&suite)?;
            let report = run_eval_suite(suite, &tools, std::env::current_dir()?)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

fn run_evolve(command: EvolveCommand) -> Result<()> {
    let workspace_root = std::env::current_dir()?;
    let store = JsonlTraceStore::new(default_trace_dir(workspace_root.clone()));
    match command {
        EvolveCommand::Propose { run_id } => {
            let events = store.events_for_run(parse_run_id(&run_id)?)?;
            let mutations = propose_mutations(&events);
            println!("{}", serde_json::to_string_pretty(&mutations)?);
            Ok(())
        }
        EvolveCommand::Evaluate { candidate, report } => {
            let candidate = candidate_from_json(read_json_file(&candidate)?)?;
            let report: EvalComparisonReport = serde_json::from_value(read_json_file(&report)?)?;
            let record = evaluate_candidate(&candidate, report);
            println!("{}", serde_json::to_string_pretty(&record)?);
            Ok(())
        }
        EvolveCommand::Promote { input, report } => {
            let record = if let Some(report_path) = report {
                let candidate = candidate_from_json(read_json_file(&input)?)?;
                let report: EvalComparisonReport =
                    serde_json::from_value(read_json_file(&report_path)?)?;
                evaluate_candidate(&candidate, report)
            } else {
                let record: PromotionRecord = serde_json::from_value(read_json_file(&input)?)?;
                advance_promotion(&record)
            };
            let path = write_promotion_record(&workspace_root, &record)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "promotion_record": path,
                    "status": record.status,
                    "decision": record.decision,
                }))?
            );
            Ok(())
        }
    }
}

fn run_llm(command: LlmCommand) -> Result<()> {
    match command {
        LlmCommand::Probe {
            model,
            prompt,
            dry_run,
        } => {
            let result = probe_openai(
                &std::env::current_dir()?,
                LlmProbeOptions {
                    model,
                    prompt,
                    dry_run,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
    }
}

fn run_result_summary(result: &RunResult) -> serde_json::Value {
    match result {
        RunResult::Completed {
            run_id, artifact, ..
        } => json!({
            "status": "completed",
            "run_id": run_id.0.to_string(),
            "artifact": artifact,
        }),
        RunResult::Stalled { run_id, graph } => json!({
            "status": "stalled",
            "run_id": run_id.0.to_string(),
            "open_actions": graph.frontier_actions().len(),
        }),
    }
}

fn ensure_run_result_completed(result: &RunResult) -> Result<()> {
    match result {
        RunResult::Completed { .. } => Ok(()),
        RunResult::Stalled { run_id, graph } => Err(anyhow!(
            "Burbot run stalled before verified completion: run_id={}, open_actions={}",
            run_id.0,
            graph.frontier_actions().len()
        )),
    }
}

fn parse_run_id(value: &str) -> Result<RunId> {
    Ok(RunId(uuid::Uuid::parse_str(value)?))
}

fn compact_json(value: &serde_json::Value) -> String {
    let mut text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    if text.len() > 240 {
        text.truncate(237);
        text.push_str("...");
    }
    text
}

fn read_json_file(path: &PathBuf) -> Result<serde_json::Value> {
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}
