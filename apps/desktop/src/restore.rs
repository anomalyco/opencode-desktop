//! What survives a relaunch beyond the tabs: the open session and every composer draft. The
//! auto-rebuild restarts the app on each merge, so live mode writes this on every change, to
//! `<data dir>/rune-opencode/ui.json`, one entry per server.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::{App, Route};

/// The draft key for a route: a session's id, or `new` for the new-session page.
pub fn draft_key(route: &Route) -> Option<String> {
    match route {
        Route::Session(id) => Some(id.clone()),
        Route::Draft => Some("new".into()),
        Route::Home => None,
    }
}

#[derive(Default)]
pub struct Restore {
    /// Unsent composer text per draft key; the open one lives in `App::composer`.
    pub drafts: HashMap<String, String>,
    /// Which draft `App::composer` holds.
    pub shown: Option<String>,
    saved: Option<Value>,
}

fn file() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from)
    };
    Some(dir.join("rune-opencode/ui.json"))
}

fn read() -> Value {
    file().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

fn route_json(route: &Route) -> Value {
    match route {
        Route::Session(id) => json!({ "session": id }),
        Route::Draft => json!("new"),
        Route::Home => json!("home"),
    }
}

fn route_from(v: &Value) -> Option<Route> {
    if let Some(id) = v["session"].as_str() {
        return Some(Route::Session(id.to_owned()));
    }
    match v.as_str()? {
        "new" => Some(Route::Draft),
        "home" => Some(Route::Home),
        _ => None,
    }
}

impl App {
    /// Puts back the saved route and drafts for `server` (live mode, before the first frame).
    pub fn restore_ui(&mut self, server: &str) {
        let v = &read()[server];
        self.restore.drafts =
            v["drafts"].as_object().into_iter().flatten().filter_map(|(k, t)| Some((k.clone(), t.as_str()?.to_owned()))).collect();
        if let Some(route) = route_from(&v["route"]) {
            let known = |id: &str| self.tabs.iter().any(|t| t.id == id) || self.sessions.iter().any(|s| s.id == id);
            if !matches!(&route, Route::Session(id) if !known(id)) {
                self.route = route;
            }
        }
        if let Some(sidebar) = v["sidebar"].as_bool() {
            self.sidebar_open = sidebar;
        }
        self.restore.saved = Some(self.ui_json());
        self.show_draft();
    }

    /// Swaps the composer's text when the route moves to another draft.
    fn show_draft(&mut self) {
        let key = draft_key(&self.route);
        if key == self.restore.shown {
            return;
        }
        if let Some(old) = self.restore.shown.take() {
            let text = std::mem::take(&mut self.composer.text);
            if text.trim().is_empty() {
                self.restore.drafts.remove(&old);
            } else {
                self.restore.drafts.insert(old, text);
            }
        }
        let text = key.as_ref().and_then(|k| self.restore.drafts.get(k).cloned()).unwrap_or_default();
        let focused = self.composer.focused;
        self.composer = rune::TextEdit::new(&text);
        self.composer.focused = focused;
        self.restore.shown = key;
    }

    fn ui_json(&self) -> Value {
        let mut drafts = self.restore.drafts.clone();
        if let Some(k) = &self.restore.shown {
            if self.composer.text.trim().is_empty() {
                drafts.remove(k);
            } else {
                drafts.insert(k.clone(), self.composer.text.clone());
            }
        }
        json!({ "route": route_json(&self.route), "drafts": drafts, "sidebar": self.sidebar_open })
    }

    /// Called every poll: keeps drafts per route and writes the file on any change (it's a few
    /// hundred bytes, renamed into place).
    pub fn poll_restore(&mut self, server: &str) {
        self.show_draft();
        let v = self.ui_json();
        if self.restore.saved.as_ref() == Some(&v) {
            return;
        }
        let Some(path) = file() else { return };
        let mut all = read();
        all[server] = v.clone();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, all.to_string()).is_ok() && std::fs::rename(tmp, path).is_ok() {
            self.restore.saved = Some(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_round_trip() {
        for r in [Route::Home, Route::Draft, Route::Session("ses_AbC".into())] {
            assert_eq!(route_from(&route_json(&r)), Some(r));
        }
    }

    #[test]
    fn each_session_keeps_its_own_draft() {
        let mut a = App::new();
        a.route = Route::Session("ses_a".into());
        a.show_draft();
        a.composer = rune::TextEdit::new("half a thought");
        a.route = Route::Session("ses_b".into());
        a.show_draft();
        assert_eq!(a.composer.text, "");
        a.composer = rune::TextEdit::new("other");
        a.route = Route::Session("ses_a".into());
        a.show_draft();
        assert_eq!(a.composer.text, "half a thought");
        assert_eq!(a.ui_json()["drafts"]["ses_b"], "other");
    }
}
