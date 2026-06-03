//! Browser worker command messages.

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
