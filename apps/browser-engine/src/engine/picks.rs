//! Per-tick engine work outside `handle`: deferred isolated-context loads, inspector picks and
//! `ClearSite` cookie deletions (moved verbatim from `engine.rs` to stay under its 1,500-line cap).

use std::sync::atomic::Ordering;

use cef::{CefString, ImplBrowser, ImplFrame};
use serde_json::json;

use super::EngineManager;
use crate::wire::{EngineMsg, PickedElement};

impl EngineManager {
    pub fn poll_picks(&mut self) {
        for outcome in self.contexts.drain_ready_loads() {
            match outcome {
                crate::context::DeferredOutcome::Load { tab_id, generation, url } => {
                    if let Some(tab) = self.tabs.get(&tab_id)
                        && tab.generation == generation
                        && self.contexts.is_tab_ready(tab_id)
                    {
                        tab.deferred_initial.store(false, Ordering::Release);
                        if !url.trim().eq_ignore_ascii_case("about:blank")
                            && let Some(frame) = tab.browser.main_frame()
                        {
                            frame.load_url(Some(&CefString::from(url.as_str())));
                        }
                    }
                }
                crate::context::DeferredOutcome::Refuse { tab_id, generation, reason } => {
                    if self.tabs.get(&tab_id).is_some_and(|t| t.generation == generation) {
                        self.close_and_refuse_tab(tab_id, reason);
                    }
                }
            }
        }
        let mut ready = Vec::new();
        let mut clears = Vec::new();
        for (&tab_id, tab) in &self.tabs {
            for backend_id in tab.internal_picked.lock().unwrap().drain(..) {
                ready.push((tab_id, backend_id));
            }
            for clear in tab.pending_clears.lock().unwrap().drain(..) {
                clears.push(clear);
            }
        }
        for clear in clears {
            if let Some(tab) = self.tabs.get(&clear.tab_id) {
                if !self.contexts.is_tab_ready(clear.tab_id) {
                    let reason = self.contexts.not_ready_reason(clear.tab_id);
                    let _ = self.out_tx.send(EngineMsg::CdpResponse {
                        tab_id: clear.tab_id,
                        request_id: clear.request_id,
                        ok: false,
                        result: json!({ "message": reason }),
                    });
                    continue;
                }
                for (name, domain, path) in clear.cookies {
                    self.send_internal_cdp(
                        &tab.host,
                        "Network.deleteCookies",
                        json!({ "name": name, "domain": domain, "path": path }),
                    );
                }
                self.send_internal_cdp(
                    &tab.host,
                    "Storage.clearDataForOrigin",
                    json!({
                        "origin": clear.origin,
                        "storageTypes": "local_storage,indexeddb,service_workers,cache_storage,file_systems,shader_cache",
                    }),
                );
                tab.browser.reload();
                let _ = self.out_tx.send(EngineMsg::CdpResponse {
                    tab_id: clear.tab_id,
                    request_id: clear.request_id,
                    ok: true,
                    result: json!({}),
                });
            }
        }
        for (tab_id, backend_id) in ready {
            let eref = format!("@e{}", self.next_eref);
            self.next_eref += 1;
            if let Some(tab) = self.tabs.get(&tab_id) {
                // Turn off inspect mode and highlight the chosen node cleanly.
                self.send_internal_cdp(
                    &tab.host,
                    "Overlay.setInspectMode",
                    json!({ "mode": "none", "highlightConfig": {} }),
                );
                self.send_internal_cdp(
                    &tab.host,
                    "Overlay.highlightNode",
                    json!({
                        "backendNodeId": backend_id,
                        "highlightConfig": {
                            "showInfo": true,
                            "contentColor": { "r": 56, "g": 189, "b": 248, "a": 0.28 },
                            "borderColor": { "r": 56, "g": 189, "b": 248, "a": 0.90 }
                        }
                    }),
                );
            }
            let _ = self.out_tx.send(EngineMsg::InspectState {
                tab_id,
                active: false,
                picked: Some(PickedElement {
                    tab_id,
                    eref: eref.clone(),
                    backend_node_id: backend_id,
                    selector: format!("node#{backend_id}"),
                    label: format!("{eref} (node #{backend_id})"),
                    role: None,
                    name: None,
                    text: None,
                    rect: [0.0; 4],
                }),
            });
        }
    }
}
