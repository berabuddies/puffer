use serde_json::Value;

/// One admitted formal host call awaiting its concrete Puffer tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingLambdaHostCall {
    host_tool: String,
    host_args: Value,
    metadata_host_args: Value,
    concrete_tool: String,
    concrete_input: Value,
    require_approval: bool,
}

impl PendingLambdaHostCall {
    /// Creates a pending bridge with separately redacted metadata payloads.
    pub(crate) fn new(
        host_tool: impl Into<String>,
        host_args: Value,
        metadata_host_args: Value,
        concrete_tool: impl Into<String>,
        concrete_input: Value,
        require_approval: bool,
    ) -> Self {
        Self {
            host_tool: host_tool.into(),
            host_args,
            metadata_host_args,
            concrete_tool: concrete_tool.into(),
            concrete_input,
            require_approval,
        }
    }

    /// Returns the formal host tool name admitted by the Lambda gate.
    pub(crate) fn host_tool(&self) -> &str {
        &self.host_tool
    }

    /// Returns the formal host arguments admitted by the Lambda gate.
    pub(crate) fn host_args(&self) -> &Value {
        &self.host_args
    }

    /// Returns the redacted formal host arguments safe for trace metadata.
    pub(crate) fn metadata_host_args(&self) -> &Value {
        &self.metadata_host_args
    }

    /// Returns the concrete Puffer tool name this bridge permits next.
    pub(crate) fn concrete_tool(&self) -> &str {
        &self.concrete_tool
    }

    /// Returns true when the pending bridge permits this concrete call.
    pub(crate) fn permits_concrete_call(&self, tool_id: &str, input: &Value) -> bool {
        self.concrete_tool == tool_id && self.concrete_input == *input
    }

    /// Returns true when this verified bridge should still run normal approval.
    pub(crate) fn requires_approval(&self) -> bool {
        self.require_approval
    }
}
