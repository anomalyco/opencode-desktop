//! An agent's browser command, started the turn the server announces it. The window's stream
//! events wait for its next frame (`launch::inbox_waker`), 5 ms median and 12 ms at p90 as
//! measured, per command; the command frames also come to their own subscription here, woken by
//! the browser's quiet waker (`launch::browser_waker`). The frame's poll sees each again
//! (`on_control_event`), where the attachment ignores a request it already took, as it ignores
//! the server's replays. A cancel comes this way too, so it never overtakes its command, nor a
//! command the cleanup of one cancelled before it.

use std::sync::mpsc::Receiver;

use opencode_kernel::Slot;
use opencode_kit::browser_rpc::CONTROL_EVENT;
use opencode_kit::client::Incoming;

use crate::App;
use crate::windows::peer;

static EXPRESS: Slot<Express> = Slot::new();

/// The subscription, on the stream of the server it was made on.
#[derive(Default)]
struct Express {
    sub: Option<(String, Receiver<Incoming>)>,
}

/// The browser's quiet poll: starts (or cancels) the commands the window's server announced since
/// the last, in order, subscribing on the first poll after the window joins a server (or moves to another).
pub fn poll(s: &mut App) {
    let current = peer::base(s).map(str::to_owned);
    let st = s.kernel.slots.get_or_default(&EXPRESS);
    if st.sub.as_ref().map(|(base, _)| base) != current.as_ref() {
        st.sub = None;
    }
    let Some((_, rx)) = &st.sub else {
        let sub = s.workspace_panes.browser.host.message_waker().and_then(|wake| peer::subscribe_to(s, CONTROL_EVENT, wake));
        s.kernel.slots.get_or_default(&EXPRESS).sub = sub;
        return;
    };
    let frames: Vec<serde_json::Value> = rx
        .try_iter()
        .filter_map(|m| match m {
            Incoming::Event { data, .. } if matches!(data["type"].as_str(), Some("command" | "cancel")) => Some(data),
            _ => None,
        })
        .collect();
    let key = crate::servers::runtime::active(s).clone();
    for data in &frames {
        super::on_control_event(s, &key, data);
    }
}
