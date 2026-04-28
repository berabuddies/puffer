//! Persistent unified graph snapshots for Burbot planning state.

use crate::belief::{BeliefClaim, BeliefGraph, ClaimId, Evidence, EvidenceId};
use crate::graph::{PlanEdge, PlanEdgeKind, PlanGraph, PlanNode, PlanNodeKind, PlanStatus};
use crate::ids::{NodeId, RunId, TraceId};
use crate::trace::{TraceEvent, TraceEventType};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Captures PlanGraph, BeliefGraph, and TraceEvent state in deterministic form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UnifiedGraphSnapshot {
    /// Identifies the on-disk JSON format used by this snapshot.
    pub(crate) schema_version: u32,
    /// Preserves the next plan node id from the source PlanGraph.
    pub(crate) plan_next_id: u64,
    /// Stores plan nodes sorted by NodeId.
    pub(crate) plan_nodes: Vec<PlanNode>,
    /// Stores plan edges sorted by endpoint and edge kind.
    pub(crate) plan_edges: Vec<PlanEdge>,
    /// Stores belief claims sorted by ClaimId.
    pub(crate) belief_claims: Vec<BeliefClaim>,
    /// Stores belief evidence sorted by EvidenceId.
    pub(crate) evidence_items: Vec<Evidence>,
    /// Stores trace events sorted by timestamp, run id, node id, and trace id.
    pub(crate) trace_events: Vec<TraceEvent>,
}

/// Identifies any node-like record exposed by the unified graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "id")]
pub(crate) enum UnifiedNodeRef {
    /// Refers to a node from the plan graph.
    Plan(NodeId),
    /// Refers to a claim from the belief graph.
    Claim(ClaimId),
    /// Refers to an evidence item from the belief graph.
    Evidence(EvidenceId),
    /// Refers to one trace event.
    Trace(TraceId),
}

/// Describes the typed relationship represented by a unified graph edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "plan_kind")]
pub(crate) enum UnifiedEdgeKind {
    /// Wraps an existing PlanGraph edge kind.
    Plan(PlanEdgeKind),
    /// Links a trace event to the plan node it records.
    TraceForPlanNode,
    /// Links an accepted or proposed claim to supporting evidence.
    ClaimSupportedByEvidence,
    /// Links a rejected or disputed claim to contradicting evidence.
    ClaimContradictedByEvidence,
}

/// Captures a typed edge between unified graph records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct UnifiedEdge {
    /// Source node for this relationship.
    pub(crate) source: UnifiedNodeRef,
    /// Target node for this relationship.
    pub(crate) target: UnifiedNodeRef,
    /// Typed relationship kind.
    pub(crate) kind: UnifiedEdgeKind,
    /// Relationship metadata preserved as JSON.
    pub(crate) payload: Value,
}

/// Describes whether evidence supports or contradicts a claim.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceRelation {
    /// Evidence supports the linked claim.
    Supports,
    /// Evidence contradicts the linked claim.
    Contradicts,
}

/// Links one belief claim to one evidence item.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EvidenceLink {
    /// Claim receiving support or contradiction.
    pub(crate) claim_id: ClaimId,
    /// Evidence attached to the claim.
    pub(crate) evidence_id: EvidenceId,
    /// Directional meaning of the evidence attachment.
    pub(crate) relation: EvidenceRelation,
}

/// Returns the trace events associated with a plan node and its plan ancestors.
#[derive(Debug, Clone)]
pub(crate) struct TraceLineage<'a> {
    /// Node used as the lineage root.
    pub(crate) node_id: NodeId,
    /// Root and ancestor plan nodes included in the lineage query.
    pub(crate) related_node_ids: Vec<NodeId>,
    /// Trace events for the related plan nodes in deterministic order.
    pub(crate) events: Vec<&'a TraceEvent>,
}

impl UnifiedGraphSnapshot {
    /// Builds a normalized snapshot from the current plan, beliefs, and trace events.
    pub(crate) fn from_parts(
        plan: &PlanGraph,
        beliefs: &BeliefGraph,
        trace_events: &[TraceEvent],
    ) -> Self {
        let mut snapshot = Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            plan_next_id: plan.next_id,
            plan_nodes: plan.nodes.values().cloned().collect(),
            plan_edges: plan.edges.clone(),
            belief_claims: beliefs.claims().cloned().collect(),
            evidence_items: beliefs.evidence().cloned().collect(),
            trace_events: trace_events.to_vec(),
        };
        snapshot.sort_records();
        snapshot
    }

    /// Saves this snapshot as deterministic, pretty-printed JSON.
    pub(crate) fn save_json(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let mut file = File::create(path)
            .with_context(|| format!("failed to create graph snapshot {}", path.display()))?;
        let mut snapshot = self.clone();
        snapshot.sort_records();
        serde_json::to_writer_pretty(&mut file, &snapshot)
            .with_context(|| format!("failed to write graph snapshot {}", path.display()))?;
        writeln!(file)?;
        Ok(())
    }

    /// Loads a snapshot from JSON and normalizes query ordering.
    pub(crate) fn load_json(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("failed to open graph snapshot {}", path.display()))?;
        let mut snapshot: Self = serde_json::from_reader(file)
            .with_context(|| format!("failed to parse graph snapshot {}", path.display()))?;
        if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
            bail!(
                "unsupported graph snapshot schema version {}",
                snapshot.schema_version
            );
        }
        snapshot.sort_records();
        Ok(snapshot)
    }

    /// Returns all plan nodes sorted by NodeId.
    pub(crate) fn plan_nodes(&self) -> Vec<&PlanNode> {
        let mut nodes = self.plan_nodes.iter().collect::<Vec<_>>();
        nodes.sort_by_key(|node| node.id.0);
        nodes
    }

    /// Returns a plan node by NodeId.
    pub(crate) fn plan_node(&self, id: NodeId) -> Option<&PlanNode> {
        self.plan_nodes.iter().find(|node| node.id == id)
    }

    /// Returns plan nodes matching the requested kind in NodeId order.
    pub(crate) fn nodes_by_kind(&self, kind: PlanNodeKind) -> Vec<&PlanNode> {
        let mut nodes = self
            .plan_nodes
            .iter()
            .filter(|node| node.kind == kind)
            .collect::<Vec<_>>();
        nodes.sort_by_key(|node| node.id.0);
        nodes
    }

    /// Returns plan nodes matching the requested status in NodeId order.
    pub(crate) fn nodes_by_status(&self, status: PlanStatus) -> Vec<&PlanNode> {
        let mut nodes = self
            .plan_nodes
            .iter()
            .filter(|node| node.status == status)
            .collect::<Vec<_>>();
        nodes.sort_by_key(|node| node.id.0);
        nodes
    }

    /// Returns plan nodes matching both kind and status in NodeId order.
    pub(crate) fn nodes_by_kind_and_status(
        &self,
        kind: PlanNodeKind,
        status: PlanStatus,
    ) -> Vec<&PlanNode> {
        let mut nodes = self
            .plan_nodes
            .iter()
            .filter(|node| node.kind == kind && node.status == status)
            .collect::<Vec<_>>();
        nodes.sort_by_key(|node| node.id.0);
        nodes
    }

    /// Returns all plan edges in deterministic order.
    pub(crate) fn plan_edges(&self) -> Vec<&PlanEdge> {
        let mut edges = self.plan_edges.iter().collect::<Vec<_>>();
        edges.sort_by_key(|edge| plan_edge_key(edge));
        edges
    }

    /// Returns plan edges with the requested edge kind in deterministic order.
    pub(crate) fn plan_edges_by_kind(&self, kind: PlanEdgeKind) -> Vec<&PlanEdge> {
        let mut edges = self
            .plan_edges
            .iter()
            .filter(|edge| edge.kind == kind)
            .collect::<Vec<_>>();
        edges.sort_by_key(|edge| plan_edge_key(edge));
        edges
    }

    /// Returns plan edges leaving the requested plan node in deterministic order.
    pub(crate) fn plan_edges_from(&self, source: NodeId) -> Vec<&PlanEdge> {
        let mut edges = self
            .plan_edges
            .iter()
            .filter(|edge| edge.source == source)
            .collect::<Vec<_>>();
        edges.sort_by_key(|edge| plan_edge_key(edge));
        edges
    }

    /// Returns plan edges entering the requested plan node in deterministic order.
    pub(crate) fn plan_edges_to(&self, target: NodeId) -> Vec<&PlanEdge> {
        let mut edges = self
            .plan_edges
            .iter()
            .filter(|edge| edge.target == target)
            .collect::<Vec<_>>();
        edges.sort_by_key(|edge| plan_edge_key(edge));
        edges
    }

    /// Returns plan, trace, and evidence edges in one deterministic list.
    pub(crate) fn unified_edges(&self) -> Vec<UnifiedEdge> {
        let mut edges = Vec::new();
        for edge in self.plan_edges() {
            edges.push(UnifiedEdge {
                source: UnifiedNodeRef::Plan(edge.source),
                target: UnifiedNodeRef::Plan(edge.target),
                kind: UnifiedEdgeKind::Plan(edge.kind.clone()),
                payload: edge.payload.clone(),
            });
        }
        for event in self.trace_events() {
            let Some(node_id) = event.plan_node_id else {
                continue;
            };
            edges.push(UnifiedEdge {
                source: UnifiedNodeRef::Trace(event.trace_id),
                target: UnifiedNodeRef::Plan(node_id),
                kind: UnifiedEdgeKind::TraceForPlanNode,
                payload: json!({
                    "run_id": event.run_id,
                    "event_type": event.event_type,
                    "timestamp_ms": event.timestamp_ms,
                }),
            });
        }
        for link in self.evidence_links() {
            edges.push(UnifiedEdge {
                source: UnifiedNodeRef::Claim(link.claim_id),
                target: UnifiedNodeRef::Evidence(link.evidence_id),
                kind: match link.relation {
                    EvidenceRelation::Supports => UnifiedEdgeKind::ClaimSupportedByEvidence,
                    EvidenceRelation::Contradicts => UnifiedEdgeKind::ClaimContradictedByEvidence,
                },
                payload: json!({}),
            });
        }
        edges.sort_by_key(unified_edge_key);
        edges
    }

    /// Returns all trace events in deterministic order.
    pub(crate) fn trace_events(&self) -> Vec<&TraceEvent> {
        let mut events = self.trace_events.iter().collect::<Vec<_>>();
        events.sort_by_key(|event| trace_event_key(event));
        events
    }

    /// Returns trace events for a run in deterministic order.
    pub(crate) fn trace_events_for_run(&self, run_id: RunId) -> Vec<&TraceEvent> {
        let mut events = self
            .trace_events
            .iter()
            .filter(|event| event.run_id == run_id)
            .collect::<Vec<_>>();
        events.sort_by_key(|event| trace_event_key(event));
        events
    }

    /// Returns trace events directly attached to a plan node.
    pub(crate) fn trace_events_for_node(&self, node_id: NodeId) -> Vec<&TraceEvent> {
        let mut events = self
            .trace_events
            .iter()
            .filter(|event| event.plan_node_id == Some(node_id))
            .collect::<Vec<_>>();
        events.sort_by_key(|event| trace_event_key(event));
        events
    }

    /// Returns trace events for a plan node plus ancestors reached through incoming plan edges.
    pub(crate) fn trace_lineage(&self, node_id: NodeId) -> TraceLineage<'_> {
        let related = self.related_plan_node_ids(node_id);
        let mut events = self
            .trace_events
            .iter()
            .filter(|event| {
                event
                    .plan_node_id
                    .is_some_and(|event_node_id| related.contains(&event_node_id))
            })
            .collect::<Vec<_>>();
        events.sort_by_key(|event| trace_event_key(event));
        TraceLineage {
            node_id,
            related_node_ids: related.into_iter().collect(),
            events,
        }
    }

    /// Returns all claim-to-evidence links in deterministic order.
    pub(crate) fn evidence_links(&self) -> Vec<EvidenceLink> {
        let mut links = Vec::new();
        for claim in &self.belief_claims {
            links.extend(
                claim
                    .supporting_evidence
                    .iter()
                    .copied()
                    .map(|evidence_id| EvidenceLink {
                        claim_id: claim.id,
                        evidence_id,
                        relation: EvidenceRelation::Supports,
                    }),
            );
            links.extend(
                claim
                    .contradicting_evidence
                    .iter()
                    .copied()
                    .map(|evidence_id| EvidenceLink {
                        claim_id: claim.id,
                        evidence_id,
                        relation: EvidenceRelation::Contradicts,
                    }),
            );
        }
        links.sort_by_key(evidence_link_key);
        links
    }

    /// Returns evidence links for a claim in deterministic order.
    pub(crate) fn evidence_links_for_claim(&self, claim_id: ClaimId) -> Vec<EvidenceLink> {
        self.evidence_links()
            .into_iter()
            .filter(|link| link.claim_id == claim_id)
            .collect()
    }

    /// Returns evidence items attached to a claim in deterministic order.
    pub(crate) fn evidence_for_claim(&self, claim_id: ClaimId) -> Vec<&Evidence> {
        self.evidence_links_for_claim(claim_id)
            .into_iter()
            .filter_map(|link| self.evidence_item(link.evidence_id))
            .collect()
    }

    /// Returns belief claims linked to an evidence item in deterministic order.
    pub(crate) fn claims_for_evidence(&self, evidence_id: EvidenceId) -> Vec<&BeliefClaim> {
        self.evidence_links()
            .into_iter()
            .filter(|link| link.evidence_id == evidence_id)
            .filter_map(|link| self.claim(link.claim_id))
            .collect()
    }

    /// Returns a belief claim by ClaimId.
    pub(crate) fn claim(&self, id: ClaimId) -> Option<&BeliefClaim> {
        self.belief_claims.iter().find(|claim| claim.id == id)
    }

    /// Returns evidence by EvidenceId.
    pub(crate) fn evidence_item(&self, id: EvidenceId) -> Option<&Evidence> {
        self.evidence_items
            .iter()
            .find(|evidence| evidence.id == id)
    }

    fn related_plan_node_ids(&self, node_id: NodeId) -> BTreeSet<NodeId> {
        let mut related = BTreeSet::from([node_id]);
        let mut stack = vec![node_id];
        while let Some(current) = stack.pop() {
            for edge in self.plan_edges.iter().filter(|edge| edge.target == current) {
                if related.insert(edge.source) {
                    stack.push(edge.source);
                }
            }
        }
        related
    }

    fn sort_records(&mut self) {
        self.plan_nodes.sort_by_key(|node| node.id.0);
        self.plan_edges.sort_by_key(|edge| plan_edge_key(edge));
        self.belief_claims.sort_by_key(|claim| claim.id.0);
        self.evidence_items.sort_by_key(|evidence| evidence.id.0);
        self.trace_events
            .sort_by_key(|event| trace_event_key(event));
    }
}

fn plan_edge_key(edge: &PlanEdge) -> (u64, u64, String, String) {
    (
        edge.source.0,
        edge.target.0,
        plan_edge_kind_key(&edge.kind),
        canonical_json(&edge.payload),
    )
}

fn plan_edge_kind_key(kind: &PlanEdgeKind) -> String {
    serde_json::to_string(kind).expect("serializing plan edge kind cannot fail")
}

fn trace_event_key(event: &TraceEvent) -> (u64, String, u64, String, String, String, String) {
    (
        event.timestamp_ms,
        event.run_id.0.to_string(),
        event.plan_node_id.map(|id| id.0).unwrap_or(u64::MAX),
        event.trace_id.0.to_string(),
        trace_event_type_key(&event.event_type),
        canonical_json(&event.input),
        canonical_json(&event.output),
    )
}

fn trace_event_type_key(event_type: &TraceEventType) -> String {
    serde_json::to_string(event_type).expect("serializing trace event type cannot fail")
}

fn evidence_link_key(link: &EvidenceLink) -> (u64, u8, u64) {
    (
        link.claim_id.0,
        match link.relation {
            EvidenceRelation::Supports => 0,
            EvidenceRelation::Contradicts => 1,
        },
        link.evidence_id.0,
    )
}

fn unified_edge_key(edge: &UnifiedEdge) -> (String, String, String, String) {
    (
        unified_node_key(&edge.source),
        unified_node_key(&edge.target),
        unified_edge_kind_key(&edge.kind),
        canonical_json(&edge.payload),
    )
}

fn unified_node_key(node: &UnifiedNodeRef) -> String {
    match node {
        UnifiedNodeRef::Plan(id) => format!("plan:{:020}", id.0),
        UnifiedNodeRef::Claim(id) => format!("claim:{:020}", id.0),
        UnifiedNodeRef::Evidence(id) => format!("evidence:{:020}", id.0),
        UnifiedNodeRef::Trace(id) => format!("trace:{}", id.0),
    }
}

fn unified_edge_kind_key(kind: &UnifiedEdgeKind) -> String {
    match kind {
        UnifiedEdgeKind::Plan(kind) => format!("plan:{}", plan_edge_kind_key(kind)),
        UnifiedEdgeKind::TraceForPlanNode => "trace_for_plan_node".to_string(),
        UnifiedEdgeKind::ClaimSupportedByEvidence => "claim_supported_by_evidence".to_string(),
        UnifiedEdgeKind::ClaimContradictedByEvidence => {
            "claim_contradicted_by_evidence".to_string()
        }
    }
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            let body = items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{body}]")
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let body = entries
                .into_iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        canonical_json(&Value::String(key.clone())),
                        canonical_json(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        _ => serde_json::to_string(value).expect("serializing JSON value cannot fail"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::belief::BeliefGraph;
    use crate::graph::{ActionScores, PlanNode};
    use crate::ids::{RunId, TraceId};
    use serde_json::json;

    fn action_node(label: &str, status: PlanStatus) -> PlanNode {
        PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status,
            label: label.to_string(),
            payload: json!({}),
            action_ref: None,
            scores: ActionScores::default(),
        }
    }

    fn trace_event(
        run_id: RunId,
        timestamp_ms: u64,
        event_type: TraceEventType,
        plan_node_id: Option<NodeId>,
    ) -> TraceEvent {
        TraceEvent {
            trace_id: TraceId::new(),
            run_id,
            timestamp_ms,
            event_type,
            plan_node_id,
            contract_id: None,
            action_name: None,
            input: json!({}),
            output: json!({}),
            success: None,
            failure_mode: None,
            cost: None,
            latency_ms: None,
            metadata: Value::Null,
        }
    }

    #[test]
    fn queries_plan_nodes_edges_and_trace_lineage_deterministically() {
        let mut plan = PlanGraph::from_goal("ship".to_string());
        let action_id = plan.add_node(action_node("Read", PlanStatus::Open));
        let executed_id = plan.add_node(action_node("Write", PlanStatus::Executed));
        plan.add_edge(
            executed_id,
            action_id,
            PlanEdgeKind::DependsOn,
            json!({"b": 2, "a": 1}),
        );
        plan.add_edge(NodeId(0), action_id, PlanEdgeKind::Supports, json!({}));

        let beliefs = BeliefGraph::new();
        let run_id = RunId::new();
        let events = vec![
            trace_event(run_id, 30, TraceEventType::ActionExecuted, Some(action_id)),
            trace_event(run_id, 10, TraceEventType::GoalParsed, Some(NodeId(0))),
            trace_event(
                run_id,
                20,
                TraceEventType::ActionSelected,
                Some(executed_id),
            ),
        ];
        let snapshot = UnifiedGraphSnapshot::from_parts(&plan, &beliefs, &events);

        let open_actions =
            snapshot.nodes_by_kind_and_status(PlanNodeKind::Action, PlanStatus::Open);
        assert_eq!(
            open_actions.iter().map(|node| node.id).collect::<Vec<_>>(),
            vec![action_id]
        );
        assert_eq!(
            snapshot
                .nodes_by_status(PlanStatus::Executed)
                .iter()
                .map(|node| node.id)
                .collect::<Vec<_>>(),
            vec![executed_id]
        );
        assert_eq!(
            snapshot
                .plan_edges()
                .iter()
                .map(|edge| (edge.source, edge.target, edge.kind.clone()))
                .collect::<Vec<_>>(),
            vec![
                (NodeId(0), action_id, PlanEdgeKind::Supports),
                (executed_id, action_id, PlanEdgeKind::DependsOn),
            ]
        );

        let lineage = snapshot.trace_lineage(action_id);
        assert_eq!(lineage.node_id, action_id);
        assert_eq!(
            lineage.related_node_ids,
            vec![NodeId(0), action_id, executed_id]
        );
        assert_eq!(
            lineage
                .events
                .iter()
                .map(|event| event.event_type.clone())
                .collect::<Vec<_>>(),
            vec![
                TraceEventType::GoalParsed,
                TraceEventType::ActionSelected,
                TraceEventType::ActionExecuted,
            ]
        );
    }

    #[test]
    fn links_claims_to_evidence_deterministically() {
        let plan = PlanGraph::new();
        let mut beliefs = BeliefGraph::new();
        let observation_claim =
            beliefs.record_observation("Read", "read succeeded", json!({"ok": true}));
        let failure_claim = beliefs.record_action_failure(
            "puffer.tools",
            "Write",
            &json!({"path": "src/lib.rs"}),
            "write failed",
            json!({"error": "denied"}),
        );
        beliefs.record_action_failure(
            "puffer.tools",
            "Write",
            &json!({"path": "src/lib.rs"}),
            "write failed again",
            json!({"error": "still denied"}),
        );

        let snapshot = UnifiedGraphSnapshot::from_parts(&plan, &beliefs, &[]);
        assert_eq!(
            snapshot.evidence_links_for_claim(observation_claim),
            vec![EvidenceLink {
                claim_id: observation_claim,
                evidence_id: EvidenceId(0),
                relation: EvidenceRelation::Supports,
            }]
        );
        assert_eq!(
            snapshot
                .evidence_for_claim(failure_claim)
                .iter()
                .map(|evidence| evidence.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["write failed", "write failed again"]
        );
        assert_eq!(
            snapshot
                .claims_for_evidence(EvidenceId(2))
                .iter()
                .map(|claim| claim.id)
                .collect::<Vec<_>>(),
            vec![failure_claim]
        );
    }

    #[test]
    fn unified_edges_include_plan_trace_and_evidence_links() {
        let mut plan = PlanGraph::from_goal("inspect".to_string());
        let action_id = plan.add_node(action_node("Read", PlanStatus::Open));
        plan.add_edge(NodeId(0), action_id, PlanEdgeKind::Supports, json!({}));

        let mut beliefs = BeliefGraph::new();
        beliefs.record_observation("Read", "read succeeded", json!({"ok": true}));
        let run_id = RunId::new();
        let event = trace_event(run_id, 1, TraceEventType::ActionSelected, Some(action_id));
        let snapshot = UnifiedGraphSnapshot::from_parts(&plan, &beliefs, &[event]);
        let edges = snapshot.unified_edges();

        assert!(edges
            .iter()
            .any(|edge| edge.kind == UnifiedEdgeKind::Plan(PlanEdgeKind::Supports)));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == UnifiedEdgeKind::TraceForPlanNode));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == UnifiedEdgeKind::ClaimSupportedByEvidence));
    }

    #[test]
    fn saves_and_loads_snapshot_json() {
        let mut plan = PlanGraph::from_goal("persist".to_string());
        let action_id = plan.add_node(action_node("Read", PlanStatus::Open));
        let mut beliefs = BeliefGraph::new();
        beliefs.record_observation("Read", "read succeeded", json!({"ok": true}));
        let run_id = RunId::new();
        let event = trace_event(run_id, 1, TraceEventType::ActionSelected, Some(action_id));
        let snapshot = UnifiedGraphSnapshot::from_parts(&plan, &beliefs, &[event]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.json");
        snapshot.save_json(&path).unwrap();
        let loaded = UnifiedGraphSnapshot::load_json(&path).unwrap();

        assert_eq!(loaded.schema_version, 1);
        assert_eq!(loaded.plan_nodes().len(), 2);
        assert_eq!(loaded.trace_events_for_run(run_id).len(), 1);
        assert_eq!(loaded.evidence_links().len(), 1);
    }
}
