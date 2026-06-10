use super::super::command::UploadReply;
use super::super::screenshot::{BrowserCapturedScreenshot, BrowserScreenshotFormat};
use super::super::{BrowserCopySelection, BrowserCursor, BrowserEvaluation};
use std::sync::mpsc::Sender;

pub(super) enum PendingKind {
    StateEval,
    CopySelection {
        reply: Sender<std::result::Result<BrowserCopySelection, String>>,
    },
    Cursor {
        reply: Sender<std::result::Result<BrowserCursor, String>>,
    },
    Evaluate {
        reply: Sender<std::result::Result<BrowserEvaluation, String>>,
    },
    CaptureScreenshot {
        format: BrowserScreenshotFormat,
        reply: Sender<std::result::Result<BrowserCapturedScreenshot, String>>,
    },
    UploadResolve {
        files: Vec<String>,
        reply: UploadReply,
    },
    UploadPrepare {
        object_id: String,
        files: Vec<String>,
        reply: UploadReply,
    },
    UploadSetFiles {
        object_id: String,
        reply: UploadReply,
    },
    UploadFinalize {
        object_id: String,
        reply: UploadReply,
    },
}
