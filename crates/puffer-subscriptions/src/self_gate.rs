//! Generic seam for gating self/outgoing messages into monitor actions.
//!
//! The router is deliberately generic and holds no knowledge of monitor tasks.
//! When a user's own outgoing message flows through the router it must reach a
//! monitor action only when some monitor-specific predicate says "this chat has
//! an open task". That predicate is injected as a [`SelfMessageGate`] so the
//! router stays generic. The concrete monitor implementation lives elsewhere.

use puffer_subscriber_runtime::Event;

/// Event kind a connector may use to mark a self-authored message. The telegram
/// path uses the `payload.is_outgoing` bool, which is what matters today; this
/// constant lets a future connector tag self messages by kind instead.
pub const SELF_MESSAGE_KIND: &str = "message_self";

/// Decides whether a self/outgoing message should reach a monitor action.
/// Returns true to dispatch (e.g. the chat has an open monitor task), false to
/// drop. Kept generic so the router holds no task knowledge.
pub trait SelfMessageGate: Send + Sync {
    fn should_dispatch_self_message(&self, event: &Event) -> bool;
}

/// Default: drop ALL self messages — exactly the master behaviour (outgoing
/// never acted on). If the daemon forgets to install a real gate, this can
/// never regress into the #569 credit-burn.
pub struct DropAllSelfGate;

impl SelfMessageGate for DropAllSelfGate {
    fn should_dispatch_self_message(&self, _event: &Event) -> bool {
        false
    }
}
