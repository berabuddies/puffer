//! Shared data types for managed browser sessions.

use serde_json::Value;

/// Snapshot of the browser state reported to the renderer.
#[derive(Clone, Debug, Default)]
pub(crate) struct BrowserState {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) loading: bool,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Text copied from the active managed Chrome selection.
#[derive(Clone, Debug, Default)]
pub(crate) struct BrowserCopySelection {
    pub(crate) text: String,
    pub(crate) copied_from: String,
}

/// Cursor shape Chrome reports for a page coordinate.
#[derive(Clone, Debug, Default)]
pub(crate) struct BrowserCursor {
    pub(crate) cursor: String,
}

/// JavaScript evaluation result returned from a managed Chrome page.
#[derive(Clone, Debug, Default)]
pub(crate) struct BrowserEvaluation {
    pub(crate) value: Value,
}

/// Direction for a browser history movement request.
pub(crate) enum BrowserHistoryDirection {
    Back,
    Forward,
}

/// User input events accepted by the Browser tab.
pub(crate) enum BrowserInputEvent {
    Mouse {
        event_type: String,
        x: f64,
        y: f64,
        button: String,
        buttons: Option<u32>,
        click_count: u32,
    },
    Wheel {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
    },
    Key {
        event_type: String,
        key: String,
        code: String,
        text: Option<String>,
        modifiers: u32,
        /// Browser editing commands (e.g. `selectAll`) attached to the key
        /// event. Routed renderer-side, so they work in focused cross-origin
        /// frames where macOS-style shortcut translation never happens.
        commands: Vec<String>,
    },
    Text {
        text: String,
    },
}
