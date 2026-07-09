use puffer_subscriptions::FilterSpec;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Current user-facing Automation spec schema version.
pub const AUTOMATION_SPEC_VERSION: u32 = 1;

/// Stored Automation record.
///
/// This separates user-authored semantics (`spec`) from Puffer record metadata
/// and internal runtime compilation state. Product APIs should expose
/// Automation, while AgentEnv workflow ids remain internal artifacts.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AutomationRecord {
    /// Stable Puffer Automation id. Not an AgentEnv workflow id.
    pub id: String,
    /// User-visible lifecycle state.
    pub status: AutomationStatus,
    /// Monotonic user-spec revision.
    pub revision: u64,
    /// User-authored Automation semantics.
    pub spec: AutomationSpec,
    /// Internal compiled runtime state.
    #[serde(default)]
    pub runtime: AutomationRuntimeState,
    /// Created-at, milliseconds since UNIX epoch.
    pub created_at_ms: i128,
    /// Updated-at, milliseconds since UNIX epoch.
    pub updated_at_ms: i128,
}

/// User-authored Automation semantics.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AutomationSpec {
    /// Version of this Puffer-side Automation schema.
    pub spec_version: u32,
    /// User-facing Automation name.
    pub name: String,
    /// Optional user-facing description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How the Automation was created.
    pub source: AutomationSource,
    /// Top-level user instructions for the Automation.
    pub instructions: String,
    /// Where this Automation should run. Local is the product default and is
    /// bootstrapped by Puffer rather than configured through user credentials.
    #[serde(default)]
    pub run_location: AutomationRunLocation,
    /// Events that start the Automation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<AutomationTriggerSpec>,
    /// Linear flow shown in the UI. Loops are Puffer syntax, not AgentEnv graph
    /// backedges.
    pub flow: AutomationFlowSpec,
    /// Review policy before outward effects are executed.
    #[serde(default)]
    pub review: AutomationReviewSpec,
}

/// Lifecycle state for a persisted Automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationStatus {
    /// Automation is active.
    Enabled,
    /// Automation is saved but inactive.
    Paused,
    /// Automation is hidden from normal execution.
    Archived,
}

/// User-selected Automation execution location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunLocation {
    /// Run through the Puffer-managed local AgentEnv runtime.
    Local,
    /// Run through an AgentEnv Cloud runtime.
    AgentEnvCloud,
}

impl Default for AutomationRunLocation {
    fn default() -> Self {
        Self::Local
    }
}

/// Creation source for an Automation.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationSource {
    /// User started from an empty builder.
    Blank,
    /// User started from a natural-language prompt.
    NaturalLanguage {
        /// Original user prompt used to seed the builder.
        prompt: String,
    },
    /// User started from a template.
    Template {
        /// Stable template id.
        template_id: String,
        /// Optional template version used to create the Automation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template_version: Option<String>,
    },
}

/// Trigger options for an Automation.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationTriggerSpec {
    /// AgentEnv-owned webhook, polling, gateway, or schedule trigger node.
    ///
    /// Loop Automations do not support this trigger in the MVP because Puffer
    /// needs to run the loop and AgentEnv-triggered ingress does not yet bridge
    /// back into the Puffer Automation runner.
    AgentEnvNode {
        /// Stable trigger id inside the Automation.
        id: String,
        /// AgentEnv node reference selected from runtime node definitions.
        node: AgentEnvNodeRef,
        /// Optional UI summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// Puffer-owned connector event trigger. This compiles to internal
    /// workflow binding records.
    PufferConnection {
        /// Stable trigger id inside the Automation.
        id: String,
        /// Authorized connection slug.
        connection_slug: String,
        /// Optional connector template slug.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connector_slug: Option<String>,
        /// Optional include filter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterSpec>,
        /// Optional suppressing filters.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ignore_filters: Vec<FilterSpec>,
        /// Optional normalized contact ids.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        contact_ids: Vec<String>,
        /// Optional UI summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
}

/// A reference to one AgentEnv workflow node.
///
/// Puffer stores the selected node type and opaque config, but does not copy
/// the AgentEnv node catalog or schema. Runtime validation remains owned by
/// AgentEnv node definitions.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct AgentEnvNodeRef {
    /// Runtime node type resolved through AgentEnv node definitions.
    pub node_type: String,
    /// Optional node display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional AgentEnv trusted flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted: Option<bool>,
    /// Opaque node config validated by AgentEnv.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Value>,
}

/// Linear Automation flow.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct AutomationFlowSpec {
    /// Ordered Automation steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<AutomationStepSpec>,
}

/// One user-visible Automation step.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationStepSpec {
    /// One AgentEnv node compiled into a workflow definition.
    AgentEnvNode {
        /// Stable step id inside the Automation.
        id: String,
        /// AgentEnv node reference selected from runtime node definitions.
        node: AgentEnvNodeRef,
        /// Optional UI summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// First-class iterative agent step. The loop is the agent's run strategy,
    /// not a hand-authored control-flow node: the Puffer runner observes state,
    /// runs one agent turn, dispatches any tools the agent requests, feeds the
    /// results back, and repeats until the agent reports done or the iteration
    /// cap is reached. The agent is executed by the Puffer daemon and is never
    /// emitted as an AgentEnv node.
    Agent {
        /// Stable step id inside the Automation.
        id: String,
        /// Agent instructions for this step.
        instructions: String,
        /// Whether the agent runs a single turn or loops until done.
        #[serde(default)]
        mode: AutomationAgentMode,
        /// Hard iteration cap for `until_done` mode so a stored agent step can
        /// never run forever. Ignored for `once` mode.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_iterations: Option<u32>,
        /// Tools the agent may invoke between turns. Each tool is a Puffer-owned
        /// runtime action dispatched by the daemon.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools: Vec<AutomationAgentToolSpec>,
        /// Optional UI summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// Puffer-owned loop syntax for deterministic iteration over a finite
    /// collection. Iterative agent behavior belongs on [`AutomationStepSpec::Agent`];
    /// this step exists only for bounded `for_each` fan-out.
    Loop {
        /// Stable step id inside the Automation.
        id: String,
        /// Loop configuration.
        #[serde(rename = "loop")]
        loop_spec: AutomationLoopSpec,
        /// Body executed for each loop iteration.
        body: AutomationFlowSpec,
        /// Optional UI summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
}

/// Run strategy for an [`AutomationStepSpec::Agent`] step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationAgentMode {
    /// Run exactly one agent turn.
    #[default]
    Once,
    /// Loop the agent until it reports done or the iteration cap is reached.
    UntilDone,
}

/// One tool available to an [`AutomationStepSpec::Agent`] step.
///
/// The tool references a Puffer-owned runtime action (currently a
/// `puffer_connector_action` node). The agent decides when to call it; the
/// daemon executes it and feeds the result back into the next turn.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AutomationAgentToolSpec {
    /// Stable tool id the agent references when requesting a call. Unique within
    /// the owning agent step.
    pub id: String,
    /// Puffer-owned node backing this tool.
    pub node: AgentEnvNodeRef,
    /// Optional UI summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Loop shape. The only supported loop is deterministic `for_each` iteration
/// over a finite collection; open-ended "repeat until" behavior is owned by the
/// iterative agent step ([`AutomationStepSpec::Agent`]), not by loop syntax.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AutomationLoopSpec {
    /// Run the body once for each item in the input collection.
    ForEach {
        /// Input collection.
        input: AutomationLoopInput,
        /// Per-item variable name visible to the loop body compiler.
        item_alias: String,
        /// Optional defensive cap. The collection itself is the finite bound.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_iterations: Option<u32>,
    },
}

/// Input reference for a loop.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationLoopInput {
    /// Use the current trigger payload.
    Trigger,
    /// Use output from a previous Automation step.
    StepOutput {
        /// Source step id.
        step_id: String,
        /// Optional JSON path within the step output.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// Static JSON value.
    Static {
        /// Literal loop input.
        value: Value,
    },
}

/// Review policy for an Automation.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AutomationReviewSpec {
    /// Whether outward effects require human approval.
    #[serde(default = "default_true")]
    pub human_approval_required: bool,
}

impl Default for AutomationReviewSpec {
    fn default() -> Self {
        Self {
            human_approval_required: true,
        }
    }
}

/// Internal runtime compilation state for an Automation.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct AutomationRuntimeState {
    /// Hash of canonical [`AutomationSpec`] JSON only. Does not include record
    /// metadata, runtime artifacts, revision, or timestamps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_hash: Option<String>,
    /// Record revision used for the latest compile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiled_revision: Option<u64>,
    /// Runtime status.
    #[serde(default)]
    pub status: AutomationRuntimeStatus,
    /// AgentEnv workflow artifacts produced by compilation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agentenv_workflows: Vec<CompiledAgentEnvWorkflow>,
    /// Puffer connector bindings produced by compilation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub puffer_bindings: Vec<CompiledPufferBinding>,
    /// Last compile/deploy error, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Internal runtime status for an Automation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRuntimeStatus {
    /// No runtime artifacts have been compiled.
    #[default]
    NotCompiled,
    /// Runtime draft artifacts are synced.
    DraftSynced,
    /// Runtime artifacts are deployed.
    Deployed,
    /// Runtime artifacts are older than the user spec.
    Stale,
    /// Runtime compile or deploy failed.
    Error,
}

/// One compiled AgentEnv workflow artifact.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CompiledAgentEnvWorkflow {
    /// Role of this workflow in the compiled Automation.
    pub role: CompiledWorkflowRole,
    /// AgentEnv runtime workflow id, once persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    /// Hash of the generated AgentEnv workflow definition for this artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_hash: Option<String>,
    /// Whether the artifact is deployed in AgentEnv.
    #[serde(default)]
    pub deployed: bool,
}

/// Type-safe role for compiled AgentEnv workflow artifacts.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompiledWorkflowRole {
    /// Main workflow artifact.
    Root,
    /// Loop body workflow for the referenced loop step.
    LoopBody {
        /// Source loop step id.
        step_id: String,
    },
    /// Continuation workflow for the referenced step.
    Continuation {
        /// Source step id.
        step_id: String,
    },
    /// Helper workflow for the referenced step.
    Helper {
        /// Source step id.
        step_id: String,
    },
}

/// One compiled Puffer-side workflow binding artifact.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CompiledPufferBinding {
    /// Source trigger id inside the Automation.
    pub trigger_id: String,
    /// Internal Puffer workflow binding slug.
    pub binding_slug: String,
}

fn default_true() -> bool {
    true
}
