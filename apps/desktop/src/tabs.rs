//! Pinned session tabs. OpenCode Desktop keeps its tab list on the client (Electron's
//! `drafts.sqlite`), never on the server, so this example keeps its own in
//! `<data dir>/rune-opencode/tabs.json`, one list per server.
//!
//! Until you change a tab here, each launch reads Desktop Beta's list (read-only, through the
//! `sqlite3` CLI) so the two show the same tabs; without Beta it starts from recent sessions.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: String,
    /// The title and directory when the tab was saved, for a session outside the loaded list.
    pub title: String,
    pub directory: String,
}

#[cfg(test)]
thread_local! {
    /// The tab ids the last `App::save_tabs` would write, in order.
    pub static SAVED: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

/// Points the pointer travels before a press on a tab becomes a drag (Beta's
/// `PointerActivationConstraints.Distance`, 4 for a mouse).
const SLOP: f32 = 4.0;

/// A drag that moves a tab to a new place in the sidebar (Beta's `tab-strip.tsx` sortable).
/// Rows share one pitch: their height plus the list's gap.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reorder {
    from: Option<usize>,
    to: usize,
    dy: f32,
    /// The press that's ending was a drag, so its release isn't a click.
    dragged: bool,
}

impl Reorder {
    /// A press on a tab, which may become a drag.
    pub fn press(&mut self) {
        self.dragged = false;
    }

    /// Whether the release that's opening a tab ended a drag instead, which it then forgets.
    pub fn was_drag(&mut self) -> bool {
        std::mem::take(&mut self.dragged)
    }

    /// The tab being dragged, by its index before the drag.
    pub fn dragging(&self) -> Option<usize> {
        self.from
    }

    /// The pointer moved `dy` since the press on tab `index` of `count`.
    pub fn follow(&mut self, index: usize, count: usize, dy: f32, pitch: f32) {
        if self.from.is_none() {
            if dy.abs() < SLOP {
                return;
            }
            self.from = Some(index);
        }
        let from = self.from.unwrap_or(index);
        // Like Beta's sortable, it stays on the list's axis and inside the list.
        let lo = -(from as f32) * pitch;
        let hi = count.saturating_sub(1 + from) as f32 * pitch;
        self.dy = dy.clamp(lo, hi.max(lo));
        let to = from as f32 + (self.dy / pitch).round();
        self.to = to.clamp(0.0, count.saturating_sub(1) as f32) as usize;
    }

    /// The drag ended: `(from, to)` for a drop that moves a tab, `None` for a cancel, a drop in
    /// place or a press that never became a drag. `on_tab` is whether it was let go over the
    /// tab, whose release then also presses it.
    pub fn end(&mut self, cancelled: bool, on_tab: bool) -> Option<(usize, usize)> {
        let from = self.from.take()?;
        self.dy = 0.0;
        self.dragged = on_tab;
        (!cancelled && from != self.to).then_some((from, self.to))
    }

    /// How far tab `index` sits from its place: the dragged tab with the pointer, and the tabs
    /// between its place and its target one pitch toward the gap it left.
    pub fn shift(&self, index: usize, pitch: f32) -> f32 {
        let Some(from) = self.from else { return 0.0 };
        if index == from {
            self.dy
        } else if from < index && index <= self.to {
            -pitch
        } else if self.to <= index && index < from {
            pitch
        } else {
            0.0
        }
    }
}

fn data_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        return Some(home.join("Library/Application Support"));
    }
    Some(std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from))
}

fn file() -> Option<PathBuf> {
    Some(data_dir()?.join("rune-opencode/tabs.json"))
}

fn parse(list: &Value) -> Vec<Tab> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some(Tab {
                id: t["id"].as_str()?.to_owned(),
                title: t["title"].as_str().unwrap_or("").to_owned(),
                directory: t["directory"].as_str().unwrap_or("").to_owned(),
            })
        })
        .collect()
}

/// This example's saved list for `server`, if you've changed tabs here before.
pub fn load(server: &str) -> Option<Vec<Tab>> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file()?).ok()?).ok()?;
    saved(&v, server)
}

/// `server`'s list in the file's JSON; none when only other keys (such as `zoom`) are there.
fn saved(v: &Value, server: &str) -> Option<Vec<Tab>> {
    v.get(server)?.as_array()?;
    Some(parse(&v[server]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_only_zoom_has_no_saved_tabs() {
        let v = json!({ "zoom": 1.2 });
        assert_eq!(saved(&v, "http://x"), None);
        let v = json!({ "zoom": 1.2, "http://x": [{ "id": "a", "title": "A", "directory": "/d" }] });
        assert_eq!(saved(&v, "http://x").map(|t| t.len()), Some(1));
        assert_eq!(saved(&json!({ "http://x": [] }), "http://x"), Some(vec![]));
    }

    #[test]
    fn tabs_between_the_place_and_the_target_step_aside() {
        let mut r = Reorder::default();
        r.follow(1, 5, 3.0, 32.0);
        assert_eq!(r.dragging(), None, "inside the slop it's still a press");
        r.follow(1, 5, 70.0, 32.0);
        assert_eq!((r.dragging(), r.to), (Some(1), 3));
        let shifts: Vec<f32> = (0..5).map(|i| r.shift(i, 32.0)).collect();
        assert_eq!(shifts, [0.0, 70.0, -32.0, -32.0, 0.0]);
        r.follow(1, 5, -500.0, 32.0);
        assert_eq!((r.to, r.shift(1, 32.0)), (0, -32.0), "held inside the list");
        assert_eq!(r.shift(0, 32.0), 32.0);
        assert_eq!(r.end(false, true), Some((1, 0)));
        assert_eq!(r.shift(0, 32.0), 0.0);
        r.follow(2, 5, 60.0, 32.0);
        assert_eq!(r.end(true, true), None, "a cancel moves nothing");
        r.follow(2, 5, 10.0, 32.0);
        assert_eq!(r.end(false, true), None, "a drop in place moves nothing");
    }
}

pub fn save(server: &str, tabs: &[Tab]) {
    let Some(path) = file() else { return };
    let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    v[server] = tabs.iter().map(|t| json!({ "id": t.id, "title": t.title, "directory": t.directory })).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// The saved UI zoom (1.0 is 100%), kept in the same file under its own key.
pub fn load_zoom() -> Option<f32> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file()?).ok()?).ok()?;
    v["zoom"].as_f64().map(|z| z as f32)
}

pub fn save_zoom(zoom: f32) {
    let Some(path) = file() else { return };
    let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    v["zoom"] = json!(f64::from((zoom * 100.0).round()) / 100.0);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// Desktop Beta's tabs for its own sidecar server (the local `:4096` service), most recently
/// written window first. Read-only: `mode=ro` and `-readonly`.
pub fn from_beta() -> Option<Vec<Tab>> {
    let db = data_dir()?.join("ai.opencode.desktop.beta/drafts.sqlite");
    if !db.exists() {
        return None;
    }
    let uri = format!("file:{}?mode=ro", db.display());
    let sql = "select key, value from state where name = (select name from state where key = 'tabs' \
               and name like 'opencode.window.%' order by updated_at desc limit 1) and key in ('tabs', 'tabs.info')";
    let out = Command::new("sqlite3").args(["-readonly", "-json", &uri, sql]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let rows: Value = serde_json::from_slice(&out.stdout).ok()?;
    let get = |k: &str| -> Value {
        rows.as_array()
            .into_iter()
            .flatten()
            .find(|r| r["key"] == k)
            .and_then(|r| serde_json::from_str(r["value"].as_str()?).ok())
            .unwrap_or(Value::Null)
    };
    let (tabs, info) = (get("tabs"), get("tabs.info"));
    let list: Vec<Tab> = tabs
        .as_array()?
        .iter()
        .filter(|t| t["type"] == "session" && t["server"] == "sidecar")
        .filter_map(|t| {
            let id = t["sessionId"].as_str()?.to_owned();
            // tabs.info is keyed "<server>\n/server/<base64 server>/session/<id>".
            let meta = info.as_object().and_then(|m| m.iter().find(|(k, _)| k.ends_with(&format!("/session/{id}"))).map(|e| e.1));
            let field = |k: &str| meta.and_then(|m| m[k].as_str()).unwrap_or("").to_owned();
            Some(Tab { title: field("title"), directory: field("directory"), id })
        })
        .collect();
    Some(list)
}
