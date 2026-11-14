//! JSON persistence (`ui.json`) and the zero-allocation change stamp for `SideState`.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde_json::{Value, json};

use super::state::{SessionSideState, SideState};
use super::tabs::{ClosedSideTab, SessionTabState};

impl SessionSideState {
    pub fn to_json(&self) -> Value {
        let closed: Vec<Value> = self.closed.iter().map(ClosedSideTab::to_json).collect();
        json!({
            "open": self.open,
            "bottomOpen": self.bottom_open,
            "tabState": self.tab_state.to_json(),
            "closed": closed,
            "sessionWidth": self.session_width,
            "splitRatio": self.split_ratio,
            "bottomHeight": self.bottom_height,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        let finite = |key: &str| v[key].as_f64().map(|n| n as f32).filter(|n| n.is_finite());
        Self {
            open: v["open"].as_bool().unwrap_or(false),
            bottom_open: v["bottomOpen"].as_bool().unwrap_or(true),
            tab_state: SessionTabState::from_json(&v["tabState"]),
            closed: v["closed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(ClosedSideTab::from_json)
                .collect(),
            session_width: finite("sessionWidth"),
            split_ratio: finite("splitRatio"),
            bottom_height: finite("bottomHeight"),
        }
    }
}

impl SideState {
    pub fn stamp_hash(&self, h: &mut impl Hasher) {
        self.sessions.len().hash(h);
        let mut combined = 0u64;
        for (k, st) in &self.sessions {
            let mut eh = DefaultHasher::new();
            k.hash(&mut eh);
            st.open.hash(&mut eh);
            st.bottom_open.hash(&mut eh);
            st.tab_state.tabs.active.hash(&mut eh);
            st.tab_state.tabs.all.hash(&mut eh);
            st.tab_state.preview.hash(&mut eh);
            st.closed.len().hash(&mut eh);
            st.session_width.map(f32::to_bits).hash(&mut eh);
            st.split_ratio.map(f32::to_bits).hash(&mut eh);
            st.bottom_height.map(f32::to_bits).hash(&mut eh);
            combined ^= eh.finish();
        }
        combined.hash(h);
    }

    pub fn to_json(&self) -> Value {
        self.to_json_where(|_| true)
    }

    /// The sessions `keep` names, as [`SideState::to_json`] keeps them.
    pub fn to_json_where(&self, keep: impl Fn(&str) -> bool) -> Value {
        let sessions: serde_json::Map<String, Value> = self
            .sessions
            .iter()
            .filter(|(k, _)| keep(k))
            .map(|(k, v)| (k.clone(), v.to_json()))
            .collect();
        json!({ "sessions": sessions })
    }

    /// Takes the sessions `v` holds that aren't here yet (the window's `side.rest`, read after
    /// the first frame): what the first frame drew stays as it is.
    pub fn restore_rest(&mut self, v: &Value) {
        for (k, val) in v["sessions"].as_object().into_iter().flatten() {
            self.sessions.entry(k.clone()).or_insert_with(|| SessionSideState::from_json(val));
        }
    }

    pub fn restore_json(&mut self, v: &Value) {
        if let Some(obj) = v["sessions"].as_object() {
            let map: HashMap<String, SessionSideState> = obj
                .iter()
                .map(|(k, val)| (k.clone(), SessionSideState::from_json(val)))
                .collect();
            if map.values().any(|s| s.open) {
                self.mounted = true;
            }
            self.sessions = map;
        }
    }
}
