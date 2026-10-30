//! JSON persistence (`ui.json`), zero-allocation change stamp, and effective width clamping for `SideState`.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde_json::{Value, json};

use super::state::{
    BOTTOM_MAX_H, BOTTOM_MIN_H, DOCK_MAX_H, DOCK_MIN_H, SESSION_MIN_W, SIDE_MAX_W, SIDE_MIN_W,
    SIDE_WIDE_W, SessionSideState, SideState,
};
use super::tabs::{ClosedSideTab, SessionTabState};

impl SessionSideState {
    pub fn to_json(&self) -> Value {
        let closed: Vec<Value> = self.closed.iter().map(ClosedSideTab::to_json).collect();
        json!({
            "open": self.open,
            "dockOpen": self.dock_open,
            "bottomOpen": self.bottom_open,
            "tabState": self.tab_state.to_json(),
            "closed": closed,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        Self {
            open: v["open"].as_bool().unwrap_or(false),
            dock_open: v["dockOpen"].as_bool().unwrap_or(false),
            bottom_open: v["bottomOpen"].as_bool().unwrap_or(true),
            tab_state: SessionTabState::from_json(&v["tabState"]),
            closed: v["closed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(ClosedSideTab::from_json)
                .collect(),
        }
    }
}

impl SideState {
    pub fn effective_width(&self, zoomed_w: f32, left_sidebar_w: f32, wide: bool) -> f32 {
        let desired = if self.resized {
            self.width
        } else if wide {
            self.width.max(SIDE_WIDE_W)
        } else {
            self.width
        };
        if zoomed_w <= 0.0 {
            return desired.clamp(SIDE_MIN_W, SIDE_MAX_W);
        }
        let avail = (zoomed_w - left_sidebar_w - 24.0).max(240.0);
        let min_chat = if self.resized { 280.0 } else { SESSION_MIN_W };
        let max_side = (avail - min_chat).max(SIDE_MIN_W.min(avail * 0.52));
        desired.clamp(SIDE_MIN_W.min(max_side), max_side.min(SIDE_MAX_W))
    }

    pub fn stamp_hash(&self, h: &mut impl Hasher) {
        self.width.to_bits().hash(h);
        self.resized.hash(h);
        self.split_ratio.to_bits().hash(h);
        self.dock_height.to_bits().hash(h);
        self.bottom_height.to_bits().hash(h);
        self.sessions.len().hash(h);
        let mut combined = 0u64;
        for (k, st) in &self.sessions {
            let mut eh = DefaultHasher::new();
            k.hash(&mut eh);
            st.open.hash(&mut eh);
            st.dock_open.hash(&mut eh);
            st.bottom_open.hash(&mut eh);
            st.tab_state.tabs.active.hash(&mut eh);
            st.tab_state.tabs.all.hash(&mut eh);
            st.tab_state.preview.hash(&mut eh);
            st.closed.len().hash(&mut eh);
            combined ^= eh.finish();
        }
        combined.hash(h);
    }

    pub fn to_json(&self) -> Value {
        let sessions: serde_json::Map<String, Value> = self
            .sessions
            .iter()
            .map(|(k, v)| (k.clone(), v.to_json()))
            .collect();
        json!({
            "width": self.width,
            "resized": self.resized,
            "splitRatio": self.split_ratio,
            "dockHeight": self.dock_height,
            "bottomHeight": self.bottom_height,
            "sessions": sessions,
        })
    }

    pub fn restore_json(&mut self, v: &Value) {
        if let Some(w) = v["width"].as_f64() {
            self.width = (w as f32).clamp(SIDE_MIN_W, SIDE_MAX_W);
        }
        if let Some(r) = v["resized"].as_bool() {
            self.resized = r;
        }
        if let Some(sr) = v["splitRatio"].as_f64() {
            self.split_ratio = (sr as f32).clamp(0.2, 0.8);
        }
        if let Some(dh) = v["dockHeight"].as_f64() {
            self.dock_height = (dh as f32).clamp(DOCK_MIN_H, DOCK_MAX_H);
        }
        if let Some(bh) = v["bottomHeight"].as_f64() {
            self.bottom_height = (bh as f32).clamp(BOTTOM_MIN_H, BOTTOM_MAX_H);
        }
        if let Some(obj) = v["sessions"].as_object() {
            let map: HashMap<String, SessionSideState> = obj
                .iter()
                .map(|(k, val)| (k.clone(), SessionSideState::from_json(val)))
                .collect();
            if map.values().any(|s| s.open || s.dock_open) {
                self.mounted = true;
            }
            if map.values().any(|s| s.dock_open) {
                self.dock_mounted = true;
            }
            self.sessions = map;
        }
    }
}
