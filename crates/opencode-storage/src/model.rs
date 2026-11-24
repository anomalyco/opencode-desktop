//! Plain data the stores hand out: a window's frame, an outbox row, plugin enable and trust.

use serde_json::{Value, json};

/// A window's frame as Electron's window state keeps it (`window-state-<id>.json`): `x`, `y` the
/// frame's top-left and `width`, `height` its content, in desktop pixels; the display it was on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowFrame {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
    pub full_screen: bool,
    /// `[x, y, width, height]`.
    pub display: Option<[i32; 4]>,
}

impl WindowFrame {
    /// The `window-state-<id>.json` shape.
    pub fn to_json(&self) -> Value {
        let mut v = json!({ "width": self.width, "height": self.height, "isMaximized": self.maximized, "isFullScreen": self.full_screen });
        if let Some(x) = self.x {
            v["x"] = json!(x);
        }
        if let Some(y) = self.y {
            v["y"] = json!(y);
        }
        if let Some([x, y, width, height]) = self.display {
            v["displayBounds"] = json!({ "x": x, "y": y, "width": width, "height": height });
        }
        v
    }

    /// Reads the `window-state-<id>.json` shape; none without a size.
    pub fn from_json(v: &Value) -> Option<WindowFrame> {
        let int = |v: &Value| v.as_i64().and_then(|n| i32::try_from(n).ok());
        let d = &v["displayBounds"];
        let display = d.as_object().map(|_| [int(&d["x"]).unwrap_or(0), int(&d["y"]).unwrap_or(0), int(&d["width"]).unwrap_or(0), int(&d["height"]).unwrap_or(0)]);
        Some(WindowFrame {
            x: int(&v["x"]),
            y: int(&v["y"]),
            width: u32::try_from(v["width"].as_i64()?).ok()?,
            height: u32::try_from(v["height"].as_i64()?).ok()?,
            maximized: v["isMaximized"].as_bool().unwrap_or(false),
            full_screen: v["isFullScreen"].as_bool().unwrap_or(false),
            display,
        })
    }
}

/// One file on an outbox prompt: its bytes are a `blob` (by SHA-256), or a server `media` ref.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OutboxAttachment {
    pub name: String,
    pub mime: String,
    /// The `blob` holding the bytes; none when the attachment had no bytes (a media ref).
    pub blob: Option<String>,
    pub media: Option<String>,
}

/// A prompt in a server's outbox (Tier 0: committed with `synchronous = FULL` before the caller
/// proceeds). The JSON columns keep the app's own shapes; binary bytes live in `blob`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutboxRow {
    pub server: String,
    pub id: String,
    /// Order within the server's outbox.
    pub seq: i64,
    pub session: String,
    /// `Waiting` | `Sending` | `Unconfirmed` | `Failed` | `Accepted`.
    pub state: String,
    pub operation_json: String,
    pub draft_json: String,
    /// The prompt, with each inline `data:` file replaced by `blob:<sha256>`.
    pub prompt_json: Option<String>,
    pub attachments: Vec<OutboxAttachment>,
    pub paths_json: String,
    pub inbox: bool,
    pub why: String,
    pub after_check: Option<String>,
    pub restored: bool,
    pub durable: bool,
    /// The journal row verbatim, when the columns couldn't hold it exactly.
    pub legacy_json: Option<String>,
}

/// What a flush is for: the hook that asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushReason {
    /// A window was hidden or lost focus.
    Hide,
    /// A window closed while others stay.
    Close,
    /// The app quits: pending rows land and the WAL is checkpointed, so the next open finds it empty.
    Quit,
    /// Windows logoff or shutdown (`WM_ENDSESSION`).
    SessionEnd,
    /// The updater restarts the app: pending rows land in the WAL and nothing is checkpointed or
    /// synced. The app the update starts opens a moment later, reads the WAL as it would after a
    /// kill, and checkpoints it at its first quiet spell: the person waits for that app's window,
    /// and the quit's checkpoint (two disk syncs) stood between the click and it.
    UpdateRestart,
}

/// What a flush did, in µs from the request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlushReport {
    /// Nothing was dirty: no write, checkpoint, sync or file touch.
    pub clean: bool,
    pub rows: usize,
    pub micros: u64,
    pub checkpointed: bool,
    /// The deadline passed before the writer answered.
    pub timed_out: bool,
}

/// The legacy files, in the groups the app's packages cut over one at a time. A group's files
/// are imported, verified and retired only once the running build names the group
/// (`ImportConfig::groups`), and never looked at again once its flag is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportGroup {
    /// The window list, windows, frames, the shared app files, settings, history and small prefs.
    App,
    /// `plugins-state.json` and `plugins-state\<id>.json`.
    Plugins,
    /// `outbox-<sha>.json`.
    Outbox,
    /// `terminals.json`.
    Terminal,
    /// The updater's `ready` and `rollout-bucket`.
    Updater,
    /// `browser-zoom.json`.
    Browser,
}

impl ImportGroup {
    pub const ALL: [ImportGroup; 6] = [ImportGroup::App, ImportGroup::Plugins, ImportGroup::Outbox, ImportGroup::Terminal, ImportGroup::Updater, ImportGroup::Browser];
}