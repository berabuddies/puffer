use crate::ids::{NodeId, RunId};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use anyhow::{anyhow, Result};
use puffer_config::{load_config, ConfigPaths};
use puffer_core::StandaloneToolExecutor;
use puffer_resources::load_resources;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ActionInvocation {
    pub(crate) run_id: RunId,
    pub(crate) plan_node_id: NodeId,
    pub(crate) contract_id: String,
    pub(crate) action_name: String,
    pub(crate) args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Observation {
    pub(crate) invocation: ActionInvocation,
    pub(crate) success: bool,
    pub(crate) output: Value,
    pub(crate) error: Option<String>,
    pub(crate) raw: Option<String>,
    pub(crate) latency_ms: Option<u64>,
    pub(crate) cost: Option<f64>,
}

pub(crate) trait Executor: Send + Sync {
    fn contract_id(&self) -> &'static str;
    fn execute(&self, invocation: ActionInvocation) -> Result<Observation>;
}

#[derive(Default)]
pub(crate) struct ExecutorDispatcher {
    executors: HashMap<String, Arc<dyn Executor>>,
}

pub(crate) struct PufferToolExecutor {
    inner: Mutex<StandaloneToolExecutor>,
}

impl ExecutorDispatcher {
    pub(crate) fn new() -> Self {
        Self {
            executors: HashMap::new(),
        }
    }

    pub(crate) fn register(&mut self, executor: Arc<dyn Executor>) {
        self.executors
            .insert(executor.contract_id().to_string(), executor);
    }

    pub(crate) fn execute(&self, invocation: ActionInvocation) -> Result<Observation> {
        let executor = self
            .executors
            .get(&invocation.contract_id)
            .ok_or_else(|| anyhow!("missing executor for {}", invocation.contract_id))?;
        executor.execute(invocation)
    }

    pub(crate) fn execute_many_parallel(
        &self,
        invocations: Vec<ActionInvocation>,
    ) -> Vec<Result<Observation>> {
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for invocation in invocations {
                let executor = self.executors.get(&invocation.contract_id).cloned();
                handles.push(scope.spawn(move || {
                    let Some(executor) = executor else {
                        return Err(anyhow!("missing executor for {}", invocation.contract_id));
                    };
                    executor.execute(invocation)
                }));
            }
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(anyhow!("executor panicked")))
                })
                .collect()
        })
    }
}

impl PufferToolExecutor {
    pub(crate) fn new(workspace_root: PathBuf) -> Result<Self> {
        let paths = ConfigPaths::discover(&workspace_root);
        let config = load_config(&paths)?;
        let resources = load_resources(&paths)?;
        let inner = StandaloneToolExecutor::new(config, workspace_root.clone(), resources)
            .with_working_dirs(vec![workspace_root])
            .with_sandbox_mode("workspace-write");
        Ok(Self {
            inner: Mutex::new(inner),
        })
    }
}

impl Executor for PufferToolExecutor {
    fn contract_id(&self) -> &'static str {
        PUFFER_TOOLS_CONTRACT_ID
    }

    fn execute(&self, invocation: ActionInvocation) -> Result<Observation> {
        let started = Instant::now();
        let result = self
            .inner
            .lock()
            .map_err(|_| anyhow!("Puffer tool executor lock is poisoned"))?
            .execute_tool(&invocation.action_name, invocation.args.clone());
        match result {
            Ok(result) => {
                let success = result.success;
                let structured_output = structured_stdout(&result.stdout);
                Ok(observation(
                    invocation,
                    success,
                    json!({
                        "success": success,
                        "tool_id": result.tool_id,
                        "stdout": result.stdout,
                        "stderr": result.stderr,
                        "metadata": result.metadata,
                        "structured_output": structured_output,
                    }),
                    (!success).then(|| "Puffer tool returned unsuccessful result".to_string()),
                    started,
                ))
            }
            Err(error) => {
                let message = error.to_string();
                Ok(observation(
                    invocation,
                    false,
                    json!({"success": false, "error": message}),
                    Some(message),
                    started,
                ))
            }
        }
    }
}

fn structured_stdout(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or(Value::Null)
}

fn observation(
    invocation: ActionInvocation,
    success: bool,
    output: Value,
    error: Option<String>,
    started: Instant,
) -> Observation {
    Observation {
        invocation,
        success,
        output,
        error,
        raw: None,
        latency_ms: Some(started.elapsed().as_millis() as u64),
        cost: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestExecutor;

    impl Executor for TestExecutor {
        fn contract_id(&self) -> &'static str {
            "test"
        }

        fn execute(&self, invocation: ActionInvocation) -> Result<Observation> {
            Ok(observation(
                invocation,
                true,
                json!({"test": true}),
                None,
                Instant::now(),
            ))
        }
    }

    #[test]
    fn dispatcher_calls_registered_executor() {
        let mut dispatcher = ExecutorDispatcher::new();
        dispatcher.register(Arc::new(TestExecutor));
        let observation = dispatcher
            .execute(ActionInvocation {
                run_id: RunId::new(),
                plan_node_id: NodeId(1),
                contract_id: "test".to_string(),
                action_name: "Anything".to_string(),
                args: json!({}),
            })
            .unwrap();
        assert!(observation.success);
        assert_eq!(observation.output["test"], true);
    }

    #[test]
    fn dispatcher_returns_parallel_results_in_input_order() {
        let mut dispatcher = ExecutorDispatcher::new();
        dispatcher.register(Arc::new(TestExecutor));
        let observations = dispatcher.execute_many_parallel(vec![
            ActionInvocation {
                run_id: RunId::new(),
                plan_node_id: NodeId(1),
                contract_id: "test".to_string(),
                action_name: "First".to_string(),
                args: json!({"index": 1}),
            },
            ActionInvocation {
                run_id: RunId::new(),
                plan_node_id: NodeId(2),
                contract_id: "test".to_string(),
                action_name: "Second".to_string(),
                args: json!({"index": 2}),
            },
        ]);

        assert_eq!(observations.len(), 2);
        assert_eq!(
            observations[0].as_ref().unwrap().invocation.plan_node_id,
            NodeId(1)
        );
        assert_eq!(
            observations[1].as_ref().unwrap().invocation.plan_node_id,
            NodeId(2)
        );
    }

    #[test]
    fn puffer_tool_executor_runs_sleep() {
        let executor = PufferToolExecutor::new(std::env::current_dir().unwrap()).unwrap();
        let observation = executor
            .execute(ActionInvocation {
                run_id: RunId::new(),
                plan_node_id: NodeId(1),
                contract_id: PUFFER_TOOLS_CONTRACT_ID.to_string(),
                action_name: "Sleep".to_string(),
                args: json!({"duration_ms": 1, "reason": "test"}),
            })
            .unwrap();
        assert!(observation.success);
        assert!(observation.output["stdout"]
            .as_str()
            .is_some_and(|stdout| stdout.contains("\"completed\": true")));
    }
}
