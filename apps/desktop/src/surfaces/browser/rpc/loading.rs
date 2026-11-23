//! A Location the server has only just started: the wait for its plugins before the browser
//! attaches (`packages/gui-plugins/src/browser/pane.ts` `loaded`, opencode#65720).

use std::sync::mpsc::Sender;

use opencode_kit::client::PluginEndpoint;
use rune::Waker;

use super::WorkerMsg;
use super::session::SessionAttachment;

/// Asks the session's Location for its plugins off the UI thread and answers the attach
/// machine with `WorkerMsg::PluginsListed`: a failed request lists none.
pub(super) fn list_plugins(att: &SessionAttachment, attempt: u64, tx: Sender<WorkerMsg>, wake: Option<Waker>) {
    let server = att.server.clone();
    let directory = att.location.directory.clone();
    let session_id = att.session_id.clone();
    let server_key = att.server_key.clone();
    std::thread::Builder::new()
        .name("browser-rpc-plugins".into())
        .spawn(move || {
            let any = matches!(server.list_plugins(Some(&directory)), Ok(PluginEndpoint::Supported(plugins)) if !plugins.is_empty());
            let _ = tx.send(WorkerMsg::PluginsListed { server_key, session_id, attempt, any });
            if let Some(w) = wake {
                w();
            }
        })
        .ok();
}
