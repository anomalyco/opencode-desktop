//! Consolidated navigation's switch between Tabs and List
//! (`packages/app/src/shell/tabs/navigation-switch.ts`, #64668).
//!
//! List still opens sessions as tabs, so switching to List snapshots the tabs that were open.
//! Switching back to Tabs restores any snapshotted session tabs that closed and closes tabs opened
//! during List, while always keeping the currently viewed session.

use std::collections::HashSet;

use opencode_kernel::Slot;
use opencode_kit::servers::ServerKey;
use rune::Tx;
use serde_json::{Value, json};

use crate::App;
use crate::settings::Navigation;
use crate::tabs::Tab;

pub static NAV_SWITCH: Slot<NavSwitchState> = Slot::new();

const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "navigation.list-snapshot";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListSnapshot {
    pub keys: Vec<String>,
    pub sessions: Vec<(ServerKey, String)>,
}

impl ListSnapshot {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.sessions.is_empty()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "keys": self.keys,
            "sessions": self.sessions.iter().map(|(server, id)| json!({
                "server": server.as_str(),
                "sessionId": id,
            })).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Self {
        if !v.is_object() {
            return Self::default();
        }
        let keys = v["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect();
        let sessions = v["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| Some((ServerKey::new(entry["server"].as_str()?), entry["sessionId"].as_str()?.to_owned())))
            .collect();
        Self { keys, sessions }
    }
}

#[derive(Default)]
pub struct NavSwitchState {
    pub snapshot: ListSnapshot,
    pub last: Option<Navigation>,
}

fn save(s: &App, snap: &ListSnapshot) {
    if s.persists() {
        crate::storage::set_json(GLOBAL, KEY, &snap.to_json());
    }
}

/// Restores any saved `navigation.list-snapshot` at window launch. Restarting while still in List
/// preserves the snapshot taken when List was first enabled.
pub fn restore(s: &mut App) {
    let snapshot = crate::storage::json(GLOBAL, KEY).map(|v| ListSnapshot::from_json(&v)).unwrap_or_default();
    let mode = s.settings.navigation();
    let st = s.kernel.slots.get_mut(&NAV_SWITCH);
    st.snapshot = snapshot;
    st.last = Some(mode);
}

/// Reconciles `navigation.list-snapshot` whenever `Settings::navigation()` transitions.
pub fn sync(s: &mut App, tx: &mut Tx) {
    let next = s.settings.navigation();
    let prev = s.kernel.slots.untracked(&NAV_SWITCH).last;
    if prev == Some(next) {
        return;
    }
    s.kernel.slots.get_mut(&NAV_SWITCH).last = Some(next);
    if next == Navigation::List {
        // Already holding a saved snapshot across restart: keep it intact.
        if prev.is_none() && !s.kernel.slots.untracked(&NAV_SWITCH).snapshot.keys.is_empty() {
            return;
        }
        let snap = ListSnapshot {
            keys: s.tabs.iter().map(|t| t.key().encode()).collect(),
            sessions: s
                .tabs
                .iter()
                .filter(|t| !t.is_draft())
                .map(|t| (t.server.clone(), t.id.clone()))
                .collect(),
        };
        s.kernel.slots.get_mut(&NAV_SWITCH).snapshot = snap.clone();
        save(s, &snap);
        return;
    }
    if prev != Some(Navigation::List) {
        return;
    }
    let snap = std::mem::take(&mut s.kernel.slots.get_mut(&NAV_SWITCH).snapshot);
    if snap.keys.is_empty() {
        return;
    }
    restore_tabs(s, &snap, tx);
    save(s, &ListSnapshot::default());
}

fn restore_tabs(s: &mut App, snap: &ListSnapshot, tx: &mut Tx) {
    let kept: HashSet<&str> = snap.keys.iter().map(String::as_str).collect();
    let current_uid = s.current_tab();
    let current_key = current_uid.as_deref().and_then(|u| s.tab(u)).map(|t| t.key().encode());
    let mut open: HashSet<String> = s.tabs.iter().map(|t| t.key().encode()).collect();

    for (server, session_id) in &snap.sessions {
        let key = opencode_kit::servers::TabKey::session(server.clone(), session_id.clone()).encode();
        if open.contains(&key) {
            continue;
        }
        let title = if server == crate::servers::runtime::active(s) {
            s.session(session_id).map(|i| i.title.clone()).unwrap_or_default()
        } else {
            String::new()
        };
        let dir = if server == crate::servers::runtime::active(s) {
            s.session(session_id).map(|i| i.directory.clone()).unwrap_or_default()
        } else {
            String::new()
        };
        s.tabs.push(Tab {
            id: session_id.clone(),
            kind: crate::tabs::TabKind::Session,
            server: server.clone(),
            title,
            directory: dir,
        });
        open.insert(key);
    }

    let to_close: Vec<String> = s
        .tabs
        .iter()
        .filter(|t| {
            let key = t.key().encode();
            !kept.contains(key.as_str()) && current_key.as_deref() != Some(key.as_str())
        })
        .map(Tab::uid)
        .collect();
    let has_keep = current_uid.is_some() || s.tabs.iter().any(|t| kept.contains(t.key().encode().as_str()));
    if !to_close.is_empty() && has_keep {
        for uid in to_close {
            s.close_tab(&uid, tx);
        }
    } else {
        s.save_tabs();
    }
}
