use anyhow::{anyhow, Error};

/// Shared maximum number of model-to-tool iterations allowed in one turn.
pub(super) const MAX_TOOL_ITERATIONS_PER_TURN: usize = 8;

/// Identifies which provider family is using the shared tool-loop policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolLoopProvider {
    Anthropic,
    OpenAi,
}

impl ToolLoopProvider {
    fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }
}

/// Returns the shared range used to bound repeated tool-call iterations.
pub(super) fn tool_loop_iterations() -> std::ops::Range<usize> {
    0..MAX_TOOL_ITERATIONS_PER_TURN
}

/// Builds the shared iteration-limit error for a provider turn loop.
pub(super) fn tool_loop_limit_error(provider: ToolLoopProvider, streaming: bool) -> Error {
    let provider = provider.label();
    let message = if streaming {
        format!("{provider} streaming tool loop exceeded iteration limit")
    } else {
        format!("{provider} tool loop exceeded iteration limit")
    };
    anyhow!(message)
}
