use clap::{Args, Subcommand};
use std::path::PathBuf;

/// Top-level CLI arguments for `puffer browser`.
#[derive(Debug, Args)]
pub(crate) struct BrowserArgs {
    /// Output machine-readable JSON instead of text.
    #[arg(long = "json", global = true, default_value_t = false)]
    pub(crate) json: bool,
    /// Explicit root browser session id. Use this to share tabs with another session.
    #[arg(long = "session-id", global = true)]
    pub(crate) session_id: Option<String>,
    #[command(subcommand)]
    pub(crate) command: BrowserCommand,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct BrowserTargetArgs {
    /// Stable browser tab id such as `t1`.
    #[arg(long = "tab-id")]
    pub(crate) tab_id: Option<String>,
    /// Optional viewport width.
    #[arg(long = "width")]
    pub(crate) width: Option<u32>,
    /// Optional viewport height.
    #[arg(long = "height")]
    pub(crate) height: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum BrowserTabCommand {
    /// List managed browser tabs for the selected root session.
    List,
    /// Open a fresh managed browser tab.
    New {
        /// Optional URL to open.
        url: Option<String>,
        /// Stable browser tab id such as `t3`.
        #[arg(long = "tab-id")]
        tab_id: Option<String>,
        /// Optional stable label for the tab.
        #[arg(long = "label")]
        label: Option<String>,
        /// Optional viewport width.
        #[arg(long = "width")]
        width: Option<u32>,
        /// Optional viewport height.
        #[arg(long = "height")]
        height: Option<u32>,
    },
    /// Close one managed browser tab, defaulting to the active tab.
    Close {
        /// Stable browser tab id such as `t1`.
        tab_id: Option<String>,
    },
    /// Focus an existing managed browser tab by id.
    #[command(visible_alias = "select")]
    Focus {
        /// Stable browser tab id such as `t4`.
        tab_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum BrowserKeyboardCommand {
    /// Type text into the active or selected tab.
    Type {
        /// Text to insert.
        text: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Insert raw text without synthesizing per-key events.
    InsertText {
        /// Text to insert.
        text: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
}

/// Browser CLI subcommands backed by the managed daemon browser surface.
#[derive(Debug, Subcommand)]
pub(crate) enum BrowserCommand {
    /// List managed browser tabs for the selected root session.
    List,
    /// Open or reuse a managed browser tab, optionally at a URL.
    Open {
        /// Optional URL to open.
        url: Option<String>,
        /// Stable browser tab id such as `t1`.
        #[arg(long = "tab-id")]
        tab_id: Option<String>,
        /// Optional stable label for the tab.
        #[arg(long = "label")]
        label: Option<String>,
        /// Optional viewport width.
        #[arg(long = "width")]
        width: Option<u32>,
        /// Optional viewport height.
        #[arg(long = "height")]
        height: Option<u32>,
    },
    /// Navigate the active or selected tab to a URL.
    #[command(visible_alias = "goto")]
    Navigate {
        /// Destination URL.
        url: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Move back in browser history.
    Back {
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Move forward in browser history.
    Forward {
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Reload the active or selected tab.
    Reload {
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Close one managed browser tab, defaulting to the active tab.
    Close {
        #[arg(long = "group", default_value_t = false)]
        group: bool,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Close every managed tab in the selected browser session.
    #[command(visible_alias = "exit")]
    Quit,
    /// Manage tabs within the selected browser session.
    Tab {
        #[command(subcommand)]
        command: BrowserTabCommand,
    },
    /// Capture a DOM snapshot with fresh element refs.
    Snapshot {
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Capture one still screenshot of the active or selected tab.
    Screenshot {
        /// Optional output path. When omitted, Puffer writes under `.puffer/screenshots/`.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
        /// Overlay fresh `@eN` ref labels onto the captured screenshot.
        #[arg(long = "annotate", default_value_t = false)]
        annotate: bool,
        /// Directory for auto-generated screenshot files when no `PATH` is provided.
        #[arg(long = "screenshot-dir", value_name = "DIR")]
        screenshot_dir: Option<PathBuf>,
        /// Image format for the captured screenshot. Defaults to `png`.
        #[arg(long = "screenshot-format", value_parser = ["png", "jpeg"])]
        screenshot_format: Option<String>,
        /// JPEG quality from 0 to 100.
        #[arg(long = "screenshot-quality", value_parser = clap::value_parser!(u8).range(0..=100))]
        screenshot_quality: Option<u8>,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Click an element ref from the latest snapshot.
    Click {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Double-click an element ref from the latest snapshot.
    Dblclick {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Hover one element ref from the latest snapshot.
    Hover {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Focus one element ref from the latest snapshot.
    #[command(visible_alias = "focus-ref")]
    Focus {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Replace text in an editable element ref from the latest snapshot.
    Fill {
        /// Element ref such as `@e5`.
        #[arg(value_name = "REF")]
        ref_id: String,
        /// Replacement text.
        text: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Select one option in a native `<select>` ref from the latest snapshot.
    ///
    /// The current ref model matches one option by exact option value or label text.
    Select {
        /// Element ref such as `@e6`.
        #[arg(value_name = "REF")]
        ref_id: String,
        /// Exact option value or label text.
        value: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Upload one or more files into a native `<input type="file">` ref.
    Upload {
        /// Element ref such as `@e9`.
        #[arg(value_name = "REF")]
        ref_id: String,
        /// One or more file paths to attach.
        #[arg(value_name = "FILE", required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Check one checkbox-like ref from the latest snapshot.
    Check {
        /// Element ref such as `@e7`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Uncheck one checkbox-like ref from the latest snapshot.
    ///
    /// The current ref model does not support unchecking radio buttons directly.
    Uncheck {
        /// Element ref such as `@e7`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Type text into the page or into one target ref.
    Type {
        /// Text to insert.
        text: String,
        /// Optional element ref to focus before typing.
        #[arg(long = "ref", value_name = "REF")]
        ref_id: Option<String>,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Press one key in the active or selected tab.
    #[command(visible_alias = "key")]
    Press {
        /// Key name such as `Enter`.
        key: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Hold one key down in the active or selected tab.
    Keydown {
        /// Key name such as `Shift`.
        key: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Release one key in the active or selected tab.
    Keyup {
        /// Key name such as `Shift`.
        key: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Send page-level keyboard actions.
    Keyboard {
        #[command(subcommand)]
        command: BrowserKeyboardCommand,
    },
    /// Scroll the page in one direction.
    Scroll {
        /// Direction: up, down, left, or right.
        direction: String,
        /// Optional pixel distance. Defaults to 600.
        px: Option<u32>,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Scroll one ref into view from the latest snapshot.
    #[command(visible_alias = "scrollinto")]
    ScrollIntoView {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Evaluate one JavaScript expression in the active or selected tab.
    #[command(visible_alias = "evaluate")]
    Eval {
        /// JavaScript expression.
        script: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
}
