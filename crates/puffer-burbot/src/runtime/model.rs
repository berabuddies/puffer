use crate::graph::PlanGraph;
use crate::ids::RunId;
use crate::planner::CandidateSource;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub(crate) struct RunOptions {
    pub(crate) puffer_tool: Option<String>,
    pub(crate) puffer_args: Option<Value>,
    pub(crate) puffer_tool_source: CandidateSource,
    pub(crate) allow_failed_terminal_completion: bool,
    pub(crate) enable_symbolic_workers: bool,
    pub(crate) enable_parallel_read_only: bool,
    pub(crate) enable_observe_act_llm: bool,
    pub(crate) model: Option<String>,
    pub(crate) goal_verification_min_confidence: f64,
    pub(crate) yolo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum RunResult {
    Completed {
        run_id: RunId,
        graph: PlanGraph,
        artifact: Value,
    },
    Stalled {
        run_id: RunId,
        graph: PlanGraph,
    },
}

#[derive(Debug, Clone)]
pub(super) struct VerificationOutcome {
    pub(super) required: bool,
    pub(super) passed: bool,
}
