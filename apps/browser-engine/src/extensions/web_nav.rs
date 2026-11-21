//! Per-tab frame tracking and full `chrome.webNavigation` event/query implementation (main frame
//! and subframes), mirroring official `packages/gui-plugins/src/browser/profile/extension-api/web-navigation.ts`
//! (`edacc8686a`).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use serde_json::{Value, json};

use super::manifest::{is_extension_scheme, is_private_url};

const MAX_SUBFRAMES_PER_TAB: usize = 256;

#[derive(Clone, Debug)]
pub enum WebNavKind {
    BeforeNavigate,
    Committed,
    Completed,
    ErrorOccurred { error: String },
    AddressChanged,
    FrameDestroyed,
}

#[derive(Clone, Debug)]
pub struct WebNavSignal {
    pub tab_id: u64,
    pub cef_frame_id: String,
    pub parent_cef_id: Option<String>,
    pub is_main: bool,
    pub url: String,
    pub kind: WebNavKind,
}

#[derive(Clone, Debug)]
struct FrameNavState {
    frame_id: i32,
    parent_frame_id: i32,
    url: String,
    loading: bool,
    error_occurred: bool,
}

impl Default for FrameNavState {
    fn default() -> Self {
        Self {
            frame_id: 0,
            parent_frame_id: -1,
            url: String::new(),
            loading: false,
            error_occurred: false,
        }
    }
}

#[derive(Clone, Debug)]
struct TabWebNav {
    main: FrameNavState,
    subframes: HashMap<String, FrameNavState>,
    next_frame_id: i32,
}

impl Default for TabWebNav {
    fn default() -> Self {
        Self { main: FrameNavState::default(), subframes: HashMap::new(), next_frame_id: 1 }
    }
}

impl TabWebNav {
    fn alloc_or_get_subframe(&mut self, cef_id: &str, parent_cef_id: Option<&str>) -> Option<&mut FrameNavState> {
        if !self.subframes.contains_key(cef_id) {
            if self.subframes.len() >= MAX_SUBFRAMES_PER_TAB {
                return None;
            }
            let parent_frame_id = parent_cef_id
                .and_then(|pid| self.subframes.get(pid).map(|f| f.frame_id))
                .unwrap_or(0);
            let fid = self.next_frame_id;
            self.next_frame_id = self.next_frame_id.saturating_add(1).max(1);
            self.subframes.insert(
                cef_id.to_owned(),
                FrameNavState {
                    frame_id: fid,
                    parent_frame_id,
                    url: String::new(),
                    loading: false,
                    error_occurred: false,
                },
            );
        }
        self.subframes.get_mut(cef_id)
    }
}

/// Tracks live frame trees & navigation state across non-isolated pane tabs for `chrome.webNavigation`.
#[derive(Default)]
pub struct WebNavTracker {
    tabs: HashMap<u64, TabWebNav>,
}

impl WebNavTracker {
    pub fn remove_tab(&mut self, engine_tab_id: u64) {
        self.tabs.remove(&engine_tab_id);
    }

    /// Processes a CEF navigation signal for a visible (non-agent-driven, non-isolated) tab and
    /// returns `(event_name, details_without_tab_id)` pairs to emit to `webNavigation` subscribers.
    pub fn ingest(&mut self, tab_url: &str, sig: WebNavSignal, now_ms: f64) -> Vec<(&'static str, Value)> {
        if matches!(sig.kind, WebNavKind::FrameDestroyed) {
            if !sig.is_main
                && let Some(tab) = self.tabs.get_mut(&sig.tab_id)
            {
                tab.subframes.remove(&sig.cef_frame_id);
            }
            return Vec::new();
        }
        let hidden = |u: &str| is_private_url(u) || is_extension_scheme(u);
        if hidden(&sig.url) || (!sig.is_main && hidden(tab_url)) {
            if sig.is_main && matches!(sig.kind, WebNavKind::Committed) {
                let tab = self.tabs.entry(sig.tab_id).or_default();
                tab.subframes.clear();
                tab.next_frame_id = 1;
                tab.main.url = sig.url;
                tab.main.loading = true;
                tab.main.error_occurred = false;
            }
            return Vec::new();
        }
        let tab = self.tabs.entry(sig.tab_id).or_default();
        if sig.is_main && matches!(sig.kind, WebNavKind::Committed) {
            tab.subframes.clear();
            tab.next_frame_id = 1;
        }
        let frame = if sig.is_main {
            &mut tab.main
        } else {
            let Some(sf) = tab.alloc_or_get_subframe(&sig.cef_frame_id, sig.parent_cef_id.as_deref()) else {
                return Vec::new();
            };
            sf
        };
        let (frame_id, parent_frame_id) = (frame.frame_id, frame.parent_frame_id);
        let url = sig.url;

        match sig.kind {
            WebNavKind::BeforeNavigate => {
                frame.loading = true;
                let frame_type = if sig.is_main { "outermost_frame" } else { "sub_frame" };
                vec![(
                    "webNavigation.onBeforeNavigate",
                    json!({
                        "url": url,
                        "processId": 0,
                        "frameId": frame_id,
                        "parentFrameId": parent_frame_id,
                        "frameType": frame_type,
                        "timeStamp": now_ms,
                    }),
                )]
            }
            WebNavKind::Committed => {
                frame.url = url.clone();
                frame.loading = true;
                frame.error_occurred = false;
                vec![(
                    "webNavigation.onCommitted",
                    json!({
                        "url": url,
                        "processId": 0,
                        "frameId": frame_id,
                        "parentFrameId": parent_frame_id,
                        "transitionType": "link",
                        "transitionQualifiers": [],
                        "timeStamp": now_ms,
                    }),
                )]
            }
            WebNavKind::Completed => {
                if !url.is_empty() {
                    frame.url = url.clone();
                }
                let final_url = frame.url.clone();
                frame.loading = false;
                if final_url.is_empty() || hidden(&final_url) {
                    return Vec::new();
                }
                vec![
                    (
                        "webNavigation.onDOMContentLoaded",
                        json!({
                            "url": final_url,
                            "processId": 0,
                            "frameId": frame_id,
                            "parentFrameId": parent_frame_id,
                            "timeStamp": now_ms,
                        }),
                    ),
                    (
                        "webNavigation.onCompleted",
                        json!({
                            "url": final_url,
                            "processId": 0,
                            "frameId": frame_id,
                            "parentFrameId": parent_frame_id,
                            "timeStamp": now_ms,
                        }),
                    ),
                ]
            }
            WebNavKind::ErrorOccurred { error } => {
                if !url.is_empty() {
                    frame.url = url.clone();
                }
                frame.loading = false;
                frame.error_occurred = true;
                let clean_err: String = error.chars().filter(|c| !c.is_control()).take(256).collect();
                vec![(
                    "webNavigation.onErrorOccurred",
                    json!({
                        "url": url,
                        "processId": 0,
                        "frameId": frame_id,
                        "parentFrameId": parent_frame_id,
                        "error": clean_err,
                        "timeStamp": now_ms,
                    }),
                )]
            }
            WebNavKind::AddressChanged => {
                if url.is_empty() {
                    return Vec::new();
                }
                if !frame.loading && !frame.url.is_empty() && frame.url != url {
                    let ev = if fragment_only(&frame.url, &url) {
                        "webNavigation.onReferenceFragmentUpdated"
                    } else {
                        "webNavigation.onHistoryStateUpdated"
                    };
                    frame.url = url.clone();
                    vec![(
                        ev,
                        json!({
                            "url": url,
                            "processId": 0,
                            "frameId": frame_id,
                            "parentFrameId": parent_frame_id,
                            "transitionType": "link",
                            "transitionQualifiers": [],
                            "timeStamp": now_ms,
                        }),
                    )]
                } else {
                    frame.url = url;
                    Vec::new()
                }
            }
            WebNavKind::FrameDestroyed => Vec::new(),
        }
    }

    /// Returns all non-private, non-extension frames for `engine_tab_id` (`web-navigation.ts:127-137`).
    pub fn all_frames(&self, engine_tab_id: u64, tab_url: &str) -> Vec<Value> {
        if is_private_url(tab_url) || is_extension_scheme(tab_url) {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Some(tab) = self.tabs.get(&engine_tab_id) {
            let main_url = if tab.main.url.is_empty() { tab_url } else { &tab.main.url };
            out.push(json!({
                "url": main_url,
                "errorOccurred": tab.main.error_occurred,
                "processId": 0,
                "frameId": 0,
                "parentFrameId": -1,
            }));
            let mut subs: Vec<&FrameNavState> = tab
                .subframes
                .values()
                .filter(|f| !f.url.is_empty() && !is_private_url(&f.url) && !is_extension_scheme(&f.url))
                .collect();
            subs.sort_by_key(|f| f.frame_id);
            for f in subs {
                out.push(json!({
                    "url": f.url,
                    "errorOccurred": f.error_occurred,
                    "processId": 0,
                    "frameId": f.frame_id,
                    "parentFrameId": f.parent_frame_id,
                }));
            }
        } else {
            out.push(json!({
                "url": tab_url,
                "errorOccurred": false,
                "processId": 0,
                "frameId": 0,
                "parentFrameId": -1,
            }));
        }
        out
    }

    pub fn get_frame(&self, engine_tab_id: u64, tab_url: &str, frame_id: i32) -> Option<Value> {
        self.all_frames(engine_tab_id, tab_url)
            .into_iter()
            .find(|f| f.get("frameId").and_then(Value::as_i64) == Some(i64::from(frame_id)))
    }
}

/// Official `web-navigation.ts:17-26`: whether an in-page navigation only changed the URL fragment.
fn fragment_only(previous: &str, next: &str) -> bool {
    if previous == next || !previous.contains("://") || !next.contains("://") {
        return false;
    }
    let before = previous.split('#').next().unwrap_or(previous);
    let after = next.split('#').next().unwrap_or(next);
    before == after
}
