//! A same-document back/forward while extensions run. The engine then keeps `Page.*` to itself
//! (`cdp_ledger`, #274), so the main frame's `Page.navigatedWithinDocument` never reaches the app
//! and the wait would end only at the grace. The page's main execution context answers instead: a
//! same-document entry keeps it, a new document destroys it. Once the tab's address moved, that
//! context is asked for `location.href`; the new address from it is the entry's commit.

use serde_json::Value;

use super::read::send_eval_cdp;
use crate::App;

/// At most this many asks per back/forward: a page whose `location.href` never equals the tab's
/// address (a different spelling) ends at the grace, as before.
const MAX_ASKS: u8 = 6;

pub(super) struct SameDocProbe {
    context_id: i64,
    req: Option<u64>,
    asks: u8,
    done: bool,
    pub confirmed: bool,
}

impl SameDocProbe {
    /// For a back/forward from a page whose top-level main context is known, while extensions run.
    pub fn start(context_id: Option<i64>, extensions_active: bool) -> Option<SameDocProbe> {
        let context_id = context_id.filter(|_| extensions_active)?;
        Some(SameDocProbe { context_id, req: None, asks: 0, done: false, confirmed: false })
    }

    /// Reads the answer to the last ask, and asks again once the tab's address left `from_url`.
    pub fn step(&mut self, app: &mut App, host_id: u64, url: &str, from_url: &str) {
        if self.done {
            return;
        }
        if let Some(req) = self.req {
            let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(req) else { return };
            self.req = None;
            match resp.ok().and_then(|v| v.pointer("/result/value").and_then(Value::as_str).map(str::to_owned)) {
                Some(href) if href == url => {
                    self.confirmed = true;
                    self.done = true;
                    return;
                }
                Some(_) => {}
                // The context is gone (a new document) or the read threw: not a same-document entry.
                None => {
                    self.done = true;
                    return;
                }
            }
        }
        if url == from_url {
            return;
        }
        if self.asks >= MAX_ASKS {
            self.done = true;
            return;
        }
        self.asks += 1;
        self.req = send_eval_cdp(app, host_id, "location.href", false, Some(self.context_id), None).ok();
        self.done = self.req.is_none();
    }
}
