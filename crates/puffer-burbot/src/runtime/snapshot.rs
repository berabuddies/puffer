use crate::belief::BeliefGraph;
use crate::graph::PlanGraph;
use crate::graph_store::UnifiedGraphSnapshot;
use crate::ids::RunId;
use crate::trace::JsonlTraceStore;
use anyhow::Result;

pub(super) fn persist_run_snapshot(
    trace_store: &JsonlTraceStore,
    run_id: RunId,
    graph: &PlanGraph,
    beliefs: &BeliefGraph,
) -> Result<()> {
    let events = trace_store.events_for_run(run_id)?;
    UnifiedGraphSnapshot::from_parts(graph, beliefs, &events)
        .save_json(&trace_store.graph_snapshot_path_for_run(run_id))
}
