use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Identifies one claim inside a Burbot belief graph.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct ClaimId(pub(crate) u64);

/// Identifies one evidence item inside a Burbot belief graph.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct EvidenceId(pub(crate) u64);

/// Describes the typed meaning of a claim tracked by the belief graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum ClaimKind {
    Observation {
        action_name: String,
    },
    SatisfiedPrecondition {
        precondition: String,
    },
    ChangedState {
        marker: String,
    },
    RepeatedActionFailure {
        contract_id: String,
        action_name: String,
        args_key: String,
    },
}

/// Records whether a claim is currently believed, rejected, or only tentative.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClaimStatus {
    Proposed,
    Accepted,
    Rejected,
}

/// Captures one source of support or contradiction for a claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Evidence {
    pub(crate) id: EvidenceId,
    pub(crate) summary: String,
    pub(crate) payload: Value,
}

/// Captures a claim and the evidence currently attached to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BeliefClaim {
    pub(crate) id: ClaimId,
    pub(crate) kind: ClaimKind,
    pub(crate) status: ClaimStatus,
    pub(crate) supporting_evidence: Vec<EvidenceId>,
    pub(crate) contradicting_evidence: Vec<EvidenceId>,
}

/// Stores typed Burbot beliefs with stable indexes for planner lookups.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BeliefGraph {
    claims: BTreeMap<ClaimId, BeliefClaim>,
    evidence: BTreeMap<EvidenceId, Evidence>,
    claim_index: BTreeMap<String, ClaimId>,
    next_claim_id: u64,
    next_evidence_id: u64,
}

/// Uniquely keys repeated failures for one action and argument payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct ActionArgsKey {
    pub(crate) contract_id: String,
    pub(crate) action_name: String,
    pub(crate) args_key: String,
}

impl BeliefGraph {
    /// Creates an empty belief graph.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns all claims in stable id order.
    pub(crate) fn claims(&self) -> impl Iterator<Item = &BeliefClaim> {
        self.claims.values()
    }

    /// Returns all evidence items in stable id order.
    pub(crate) fn evidence(&self) -> impl Iterator<Item = &Evidence> {
        self.evidence.values()
    }

    /// Returns a claim by id.
    pub(crate) fn claim(&self, id: ClaimId) -> Option<&BeliefClaim> {
        self.claims.get(&id)
    }

    /// Returns evidence by id.
    pub(crate) fn evidence_item(&self, id: EvidenceId) -> Option<&Evidence> {
        self.evidence.get(&id)
    }

    /// Records a successful observation produced by an action.
    pub(crate) fn record_observation(
        &mut self,
        action_name: impl Into<String>,
        summary: impl Into<String>,
        output: Value,
    ) -> ClaimId {
        let evidence = self.add_evidence(summary, output);
        self.accept_with_support(
            ClaimKind::Observation {
                action_name: action_name.into(),
            },
            evidence,
        )
    }

    /// Records that a named precondition has been satisfied.
    pub(crate) fn satisfy_precondition(
        &mut self,
        precondition: impl Into<String>,
        summary: impl Into<String>,
        payload: Value,
    ) -> ClaimId {
        let evidence = self.add_evidence(summary, payload);
        self.accept_with_support(
            ClaimKind::SatisfiedPrecondition {
                precondition: precondition.into(),
            },
            evidence,
        )
    }

    /// Records that an action changed a named piece of state.
    pub(crate) fn mark_changed_state(
        &mut self,
        marker: impl Into<String>,
        summary: impl Into<String>,
        payload: Value,
    ) -> ClaimId {
        let evidence = self.add_evidence(summary, payload);
        self.accept_with_support(
            ClaimKind::ChangedState {
                marker: marker.into(),
            },
            evidence,
        )
    }

    /// Records one failed attempt for an action and argument payload.
    pub(crate) fn record_action_failure(
        &mut self,
        contract_id: impl Into<String>,
        action_name: impl Into<String>,
        args: &Value,
        summary: impl Into<String>,
        payload: Value,
    ) -> ClaimId {
        let key = ActionArgsKey::new(contract_id, action_name, args);
        let evidence = self.add_evidence(summary, payload);
        self.accept_with_support(
            ClaimKind::RepeatedActionFailure {
                contract_id: key.contract_id,
                action_name: key.action_name,
                args_key: key.args_key,
            },
            evidence,
        )
    }

    /// Returns true when every named precondition has accepted support.
    pub(crate) fn preconditions_satisfied(&self, preconditions: &[String]) -> bool {
        preconditions.iter().all(|precondition| {
            self.has_accepted(&ClaimKind::SatisfiedPrecondition {
                precondition: precondition.clone(),
            })
        })
    }

    /// Returns true when the state marker is known to have changed.
    pub(crate) fn has_changed_state(&self, marker: &str) -> bool {
        self.has_accepted(&ClaimKind::ChangedState {
            marker: marker.to_string(),
        })
    }

    /// Returns how many failures have been recorded for the same action and args.
    pub(crate) fn repeated_failure_count(
        &self,
        contract_id: &str,
        action_name: &str,
        args: &Value,
    ) -> usize {
        let key = ActionArgsKey::new(contract_id, action_name, args);
        self.claim_index
            .get(&claim_key(&ClaimKind::RepeatedActionFailure {
                contract_id: key.contract_id,
                action_name: key.action_name,
                args_key: key.args_key,
            }))
            .and_then(|id| self.claims.get(id))
            .map(|claim| claim.supporting_evidence.len())
            .unwrap_or(0)
    }

    /// Returns whether an accepted claim of the same kind exists.
    pub(crate) fn has_accepted(&self, kind: &ClaimKind) -> bool {
        self.claim_index
            .get(&claim_key(kind))
            .and_then(|id| self.claims.get(id))
            .is_some_and(|claim| claim.status == ClaimStatus::Accepted)
    }

    fn add_evidence(&mut self, summary: impl Into<String>, payload: Value) -> EvidenceId {
        let id = EvidenceId(self.next_evidence_id);
        self.next_evidence_id += 1;
        self.evidence.insert(
            id,
            Evidence {
                id,
                summary: summary.into(),
                payload,
            },
        );
        id
    }

    fn accept_with_support(&mut self, kind: ClaimKind, evidence: EvidenceId) -> ClaimId {
        let key = claim_key(&kind);
        if let Some(id) = self.claim_index.get(&key).copied() {
            let claim = self.claims.get_mut(&id).expect("indexed claim must exist");
            claim.status = ClaimStatus::Accepted;
            claim.supporting_evidence.push(evidence);
            return id;
        }

        let id = ClaimId(self.next_claim_id);
        self.next_claim_id += 1;
        self.claim_index.insert(key, id);
        self.claims.insert(
            id,
            BeliefClaim {
                id,
                kind,
                status: ClaimStatus::Accepted,
                supporting_evidence: vec![evidence],
                contradicting_evidence: Vec::new(),
            },
        );
        id
    }
}

impl ActionArgsKey {
    /// Builds a stable key for an action and JSON argument payload.
    pub(crate) fn new(
        contract_id: impl Into<String>,
        action_name: impl Into<String>,
        args: &Value,
    ) -> Self {
        Self {
            contract_id: contract_id.into(),
            action_name: action_name.into(),
            args_key: canonical_json(args),
        }
    }
}

fn claim_key(kind: &ClaimKind) -> String {
    match kind {
        ClaimKind::Observation { action_name } => format!("observation:{action_name}"),
        ClaimKind::SatisfiedPrecondition { precondition } => {
            format!("satisfied_precondition:{precondition}")
        }
        ClaimKind::ChangedState { marker } => format!("changed_state:{marker}"),
        ClaimKind::RepeatedActionFailure {
            contract_id,
            action_name,
            args_key,
        } => format!("repeated_action_failure:{contract_id}:{action_name}:{args_key}"),
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
    use serde_json::json;

    #[test]
    fn records_preconditions_and_changed_state() {
        let mut graph = BeliefGraph::new();

        graph.satisfy_precondition("workspace indexed", "observed index", json!({"ok": true}));
        graph.mark_changed_state(
            "files touched",
            "write completed",
            json!({"paths": ["a.rs"]}),
        );

        assert!(graph.preconditions_satisfied(&["workspace indexed".to_string()]));
        assert!(graph.has_changed_state("files touched"));
        assert_eq!(graph.claims().count(), 2);
        assert_eq!(graph.evidence().count(), 2);
    }

    #[test]
    fn counts_repeated_failures_by_action_and_canonical_args() {
        let mut graph = BeliefGraph::new();
        let first_args = json!({"path": "src/lib.rs", "limit": 10});
        let reordered_args = json!({"limit": 10, "path": "src/lib.rs"});

        graph.record_action_failure(
            "puffer.tools",
            "Read",
            &first_args,
            "read failed",
            json!({"error": "missing"}),
        );
        graph.record_action_failure(
            "puffer.tools",
            "Read",
            &reordered_args,
            "read failed again",
            json!({"error": "still missing"}),
        );

        assert_eq!(
            graph.repeated_failure_count("puffer.tools", "Read", &first_args),
            2
        );
        assert_eq!(
            graph.repeated_failure_count("puffer.tools", "Write", &first_args),
            0
        );
        assert_eq!(graph.claims().count(), 1);
    }
}
