//! Browser worker command messages.

use serde_json::Value;
use std::sync::mpsc::Sender;

use super::screenshot::{BrowserCaptureScreenshotOptions, BrowserCapturedScreenshot};
use super::{
    BrowserCopySelection, BrowserCursor, BrowserEvaluation, BrowserHistoryDirection,
    BrowserInputEvent,
};

/// Reply channel used by file upload commands.
pub(super) type UploadReply = Sender<std::result::Result<(), String>>;

/// Command sent from the registry handle to a page worker.
pub(super) enum BrowserCommand {
    Navigate(String),
    Reload,
    History(BrowserHistoryDirection),
    Resize {
        width: u32,
        height: u32,
    },
    Input(BrowserInputEvent),
    CopySelection {
        reply: Sender<std::result::Result<BrowserCopySelection, String>>,
    },
    Cursor {
        x: f64,
        y: f64,
        reply: Sender<std::result::Result<BrowserCursor, String>>,
    },
    Evaluate {
        expression: String,
        reply: Sender<std::result::Result<BrowserEvaluation, String>>,
    },
    /// A raw CDP method call on the page target, returning the `result` object.
    /// Used by the cross-origin payment-iframe deep pass (`DOM.getDocument`,
    /// `DOM.getContentQuads`, `DOM.resolveNode`, `Runtime.callFunctionOn`, ...).
    CdpCall {
        method: String,
        params: Value,
        reply: Sender<std::result::Result<Value, String>>,
    },
    CaptureScreenshot {
        options: BrowserCaptureScreenshotOptions,
        reply: Sender<std::result::Result<BrowserCapturedScreenshot, String>>,
    },
    Upload {
        expression: String,
        files: Vec<String>,
        reply: UploadReply,
    },
    Close {
        reply: Sender<()>,
    },
}

impl BrowserCommand {
    /// Short action name + bounded detail for the browser action log. Detail is
    /// capped so an oversized URL/expression can never bloat a log line.
    pub(super) fn log_summary(&self) -> (&'static str, String) {
        fn clip(value: &str) -> String {
            const MAX: usize = 160;
            if value.chars().count() <= MAX {
                value.to_string()
            } else {
                let head: String = value.chars().take(MAX).collect();
                format!("{head}…")
            }
        }
        match self {
            BrowserCommand::Navigate(url) => ("navigate", clip(url)),
            BrowserCommand::Reload => ("reload", String::new()),
            BrowserCommand::History(direction) => (
                "history",
                match direction {
                    BrowserHistoryDirection::Back => "back".to_string(),
                    BrowserHistoryDirection::Forward => "forward".to_string(),
                },
            ),
            BrowserCommand::Resize { width, height } => ("resize", format!("{width}x{height}")),
            // Input events may carry typed text (passwords); log the variant only.
            BrowserCommand::Input(event) => (
                "input",
                match event {
                    BrowserInputEvent::Mouse { event_type, .. } => format!("mouse:{event_type}"),
                    BrowserInputEvent::Wheel { .. } => "wheel".to_string(),
                    BrowserInputEvent::Key { event_type, .. } => format!("key:{event_type}"),
                    BrowserInputEvent::Text { .. } => "text".to_string(),
                },
            ),
            BrowserCommand::CopySelection { .. } => ("copy_selection", String::new()),
            BrowserCommand::Cursor { x, y, .. } => ("cursor", format!("{x:.0},{y:.0}")),
            // Evaluate JS can embed credentials (form fills); log size only.
            BrowserCommand::Evaluate { expression, .. } => {
                ("evaluate", format!("{} chars", expression.chars().count()))
            }
            // callFunctionOn params can embed a card value; log the method only.
            BrowserCommand::CdpCall { method, .. } => ("cdp_call", method.clone()),
            BrowserCommand::CaptureScreenshot { .. } => ("screenshot", String::new()),
            BrowserCommand::Upload { files, .. } => ("upload", format!("{} file(s)", files.len())),
            BrowserCommand::Close { .. } => ("close", String::new()),
        }
    }
}
