use crate::belief::BeliefGraph;
use crate::contract::{ArgumentSafetySpec, ContractRegistry, SideEffectClass};
use crate::graph::PlanNode;
use crate::ids::NodeId;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockReason {
    ApprovalRequired,
    ExternalWriteApprovalRequired,
    CommunicationApprovalRequired,
    UnknownSideEffectsNeedPreconditions,
    ArgumentBlocked,
    ArgumentApprovalRequired,
    DestructiveActionBlocked,
    FinancialOrLegalActionBlocked,
}

#[derive(Debug, Clone)]
pub(crate) struct SafetyGate;

#[derive(Debug, Clone, Default)]
pub(crate) struct ApprovalStore {
    approved_nodes: HashSet<NodeId>,
}

pub(crate) struct RuntimeContext<'a> {
    pub(crate) contracts: &'a dyn ContractRegistry,
    pub(crate) approvals: ApprovalStore,
    pub(crate) beliefs: BeliefGraph,
}

impl ApprovalStore {
    pub(crate) fn has_approval_for(&self, id: NodeId) -> bool {
        self.approved_nodes.contains(&id)
    }

    pub(crate) fn approve(&mut self, id: NodeId) {
        self.approved_nodes.insert(id);
    }
}

impl SafetyGate {
    pub(crate) fn blocks(
        &self,
        node: &PlanNode,
        ctx: &RuntimeContext<'_>,
    ) -> Result<Option<BlockReason>> {
        let Some(action_ref) = &node.action_ref else {
            return Ok(None);
        };
        let action = ctx
            .contracts
            .get_action(&action_ref.contract_id, &action_ref.action_name)
            .ok_or_else(|| anyhow!("missing action contract"))?;
        if matches!(action.side_effect_class, SideEffectClass::ExternalWrite)
            && !ctx.approvals.has_approval_for(node.id)
        {
            return Ok(Some(BlockReason::ExternalWriteApprovalRequired));
        }
        if matches!(action.side_effect_class, SideEffectClass::Communication)
            && !ctx.approvals.has_approval_for(node.id)
        {
            return Ok(Some(BlockReason::CommunicationApprovalRequired));
        }
        if action.side_effect_class == SideEffectClass::DestructiveWrite {
            return Ok(Some(BlockReason::DestructiveActionBlocked));
        }
        if action.side_effect_class == SideEffectClass::FinancialOrLegalEffect {
            return Ok(Some(BlockReason::FinancialOrLegalActionBlocked));
        }
        if argument_safety_matches(&node.payload, &action.argument_safety, true) {
            return Ok(Some(BlockReason::ArgumentBlocked));
        }
        if argument_safety_matches(&node.payload, &action.argument_safety, false)
            && !ctx.approvals.has_approval_for(node.id)
        {
            return Ok(Some(BlockReason::ArgumentApprovalRequired));
        }
        if action.approval.required && !ctx.approvals.has_approval_for(node.id) {
            return Ok(Some(BlockReason::ApprovalRequired));
        }
        if action.side_effect_class == SideEffectClass::Unknown
            && !ctx.beliefs.preconditions_satisfied(&action.preconditions)
        {
            return Ok(Some(BlockReason::UnknownSideEffectsNeedPreconditions));
        }
        Ok(None)
    }
}

fn argument_safety_matches(payload: &Value, specs: &[ArgumentSafetySpec], blocked: bool) -> bool {
    specs.iter().any(|spec| {
        let Some(text) = payload.get(&spec.source_arg).and_then(Value::as_str) else {
            return false;
        };
        let patterns = if blocked {
            &spec.blocked_patterns
        } else {
            &spec.approval_patterns
        };
        patterns.iter().any(|pattern| {
            regex::Regex::new(&pattern.pattern)
                .map(|regex| regex.is_match(text))
                .unwrap_or(false)
        })
    })
}
