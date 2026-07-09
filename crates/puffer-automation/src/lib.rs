//! User-facing Automation specs and storage contracts.
//!
//! Automation is the product concept. AgentEnv workflow definitions and
//! workflow ids are internal runtime artifacts represented only after
//! compilation.

mod compiler;
mod hash;
mod spec;
mod store;
mod validation;

pub use compiler::{
    compile_automation, AgentEnvWorkflowDefinition, AgentEnvWorkflowEdge, AgentEnvWorkflowNode,
    AutomationCompileError, AutomationCompilePlan, CompiledPufferBindingPlan,
    CompiledWorkflowDefinition,
};
pub use hash::automation_spec_hash;
pub use spec::{
    AgentEnvNodeRef, AutomationAgentMode, AutomationAgentToolSpec, AutomationFlowSpec,
    AutomationLoopInput, AutomationLoopSpec, AutomationRecord, AutomationReviewSpec,
    AutomationRunLocation, AutomationRuntimeState, AutomationRuntimeStatus, AutomationSource,
    AutomationSpec, AutomationStatus, AutomationStepSpec, AutomationTriggerSpec,
    CompiledAgentEnvWorkflow, CompiledPufferBinding, CompiledWorkflowRole, AUTOMATION_SPEC_VERSION,
};
pub use store::{AutomationStore, AutomationStoreError};
pub use validation::{
    validate_automation_record, validate_automation_runtime_state, validate_automation_spec,
};
