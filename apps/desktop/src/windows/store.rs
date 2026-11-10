//! What each window keeps across launches, in the files Desktop keeps it in
//! (`packages/desktop/src/main/windows`): the open windows' ids (`windowIds` in
//! `opencode.settings.dat`), each window's tabs, page, closed tabs, panes and drafts
//! (`opencode.window.<id>.dat`, one mixed-server document, as Desktop's) and its frame
//! (`window-state-<id>.json`).
//!
//! A file from before the app kept many servers live at once (one entry per server) is migrated
//! once, losslessly; the first window of a profile from before windows had files migrates
//! `tabs.json` and `ui.json`.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::Window;

/// The window ids saved at the last quit, in the order they opened.
pub fn ids() -> Vec<String> {
    read(&settings_file()).get("windowIds").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter(|id| !id.is_empty()).map(str::to_owned).collect()
}

/// Whether a window list was ever saved: without one, the first window takes the old files.
pub fn has_ids() -> bool {
    read(&settings_file()).get("windowIds").is_some()
}

pub fn save_ids(ids: Vec<String>) {
    let Some(path) = settings_file() else { return };
    crate::disk::later("window-ids", move || {
        let mut v = read(&Some(path.clone()));
        v["windowIds"] = json!(ids);
        crate::disk::replace(&path, v.to_string().as_bytes());
    });
}

/// A new window's id (Desktop's `randomUUID`).
pub fn new_id() -> String {
    let id = opencode_kit::client::session_id();
    id.strip_prefix("ses_").unwrap_or(&id).to_owned()
}

/// What window `w` saved, in Desktop's mixed-server shape (`opencode_kit::servers::tabs::WindowDoc`):
/// its own file, migrated once from the per-server shape this app kept while a window held one
/// server (`opencode_kit::servers::migration::window`), the profile's first window's from the
/// shared `tabs.json` and `ui.json` it read then. True when it was migrated: the caller writes
/// the new file, which keeps what the old shape held under `legacy`.
pub fn load(w: &Window, ctx: &opencode_kit::servers::migration::Context) -> (Value, bool) {
    use opencode_kit::servers::migration::window::{Legacy, is_legacy, migrate};
    let file = window_file(&w.id).and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| serde_json::from_str::<Value>(&s).ok());
    if !is_legacy(file.as_ref()) {
        return (file.unwrap_or_else(|| json!({})), false);
    }
    let (tabs_json, ui_json) = if w.legacy { crate::tabs::legacy_files() } else { (None, None) };
    let input = Legacy { file: file.as_ref(), first: w.legacy, tabs_json: tabs_json.as_ref(), ui_json: ui_json.as_ref() };
    (migrate(&input, ctx), true)
}

/// Sets `key` in window `w`'s file, off the UI thread. The window's writes share its file, so
/// each reads and writes it on the saves thread, in order.
pub fn save(w: &Window, key: &'static str, value: Value) {
    let Some(path) = window_file(&w.id) else { return };
    crate::disk::later(super::slot(&w.id, key), move || {
        let mut v = read(&Some(path.clone()));
        if !v.is_object() {
            v = json!({});
        }
        match value {
            Value::Null => {
                v.as_object_mut().map(|m| m.remove(key));
            }
            value => v[key] = value,
        }
        crate::disk::replace(&path, v.to_string().as_bytes());
    });
}

/// Writes window `w`'s whole file: a migration's result.
pub fn save_all(w: &Window, doc: Value) {
    let Some(path) = window_file(&w.id) else { return };
    crate::disk::later(super::slot(&w.id, "doc"), move || {
        crate::disk::replace(&path, doc.to_string().as_bytes());
    });
}

/// Window `id` closed while others stay open: its files go (Desktop's `registry.closed`).
pub fn forget(id: &str) {
    let files = [window_file(id), state_file(id)];
    crate::disk::later(super::slot(id, "forget"), move || {
        for f in files.into_iter().flatten() {
            let _ = std::fs::remove_file(f);
        }
    });
}

/// A window's frame, as Electron's window state keeps it: `x`, `y` the frame's top-left and
/// `width`, `height` its content, in desktop pixels; the display it was on; and whether it was
/// maximized or full screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
    pub full_screen: bool,
    pub display: Option<[i32; 4]>,
}

impl Frame {
    fn to_json(self) -> Value {
        let mut v = json!({ "x": self.x, "y": self.y, "width": self.width, "height": self.height, "isMaximized": self.maximized, "isFullScreen": self.full_screen });
        if let Some([x, y, width, height]) = self.display {
            v["displayBounds"] = json!({ "x": x, "y": y, "width": width, "height": height });
        }
        v
    }

    fn from_json(v: &Value) -> Option<Frame> {
        let int = |k: &str| v[k].as_i64();
        let display = v["displayBounds"].as_object().map(|_| {
            let d = &v["displayBounds"];
            [d["x"].as_i64().unwrap_or(0) as i32, d["y"].as_i64().unwrap_or(0) as i32, d["width"].as_i64().unwrap_or(0) as i32, d["height"].as_i64().unwrap_or(0) as i32]
        });
        Some(Frame {
            x: int("x")? as i32,
            y: int("y")? as i32,
            width: u32::try_from(int("width")?).ok()?,
            height: u32::try_from(int("height")?).ok()?,
            maximized: v["isMaximized"].as_bool().unwrap_or(false),
            full_screen: v["isFullScreen"].as_bool().unwrap_or(false),
            display,
        })
    }
}

pub fn load_frame(id: &str) -> Option<Frame> {
    Frame::from_json(&read(&state_file(id)))
}

/// Written off the UI thread, as the window moves and as it closes (disk::flush waits).
pub fn save_frame_later(id: &str, frame: Frame) {
    let Some(path) = state_file(id) else { return };
    crate::disk::later(super::slot(id, "frame"), move || {
        crate::disk::replace(&path, frame.to_json().to_string().as_bytes());
    });
}

fn read(path: &Option<PathBuf>) -> Value {
    path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

fn settings_file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("opencode.settings.dat"))
}

/// Desktop's `windowDataFile`.
fn window_file(id: &str) -> Option<PathBuf> {
    (!id.is_empty()).then(|| crate::dirs::data().map(|d| d.join(format!("opencode.window.{}.dat", safe(id))))).flatten()
}

/// Desktop's `windowStateFile`.
fn state_file(id: &str) -> Option<PathBuf> {
    (!id.is_empty()).then(|| crate::dirs::data().map(|d| d.join(format!("window-state-{}.json", safe(id))))).flatten()
}

fn safe(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect()
}
