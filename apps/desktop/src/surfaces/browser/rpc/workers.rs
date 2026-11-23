//! The attachment's calls that run off the UI thread and answer through `WorkerMsg`: the held
//! `attach`, and the plugin list a Location the server has only just started is asked for
//! before the browser attaches (`packages/gui-plugins/src/browser/pane.ts` `loaded`,
//! opencode#65720).

use std::sync::mpsc::Sender;

use opencode_kit::browser_rpc::AbortHandle;
use opencode_kit::client::PluginEndpoint;
use rune::Waker;

use super::WorkerMsg;
use super::session::SessionAttachment;

/// Starts the held `attach` call for `attempt`, ending any earlier one.
pub(super) fn attach(att: &mut SessionAttachment, attempt: u64, connection_id: String, tx: Sender<WorkerMsg>, wake: Option<Waker>) {
    if let Some((_, prev_abort)) = att.attach_abort.take() {
        prev_abort.abort();
    }
    let abort = AbortHandle::new();
    att.attach_abort = Some((attempt, abort.clone()));
    let server = att.server.clone();
    let location = att.location.clone();
    let session_id = att.session_id.clone();
    let server_key = att.server_key.clone();
    std::thread::Builder::new()
        .name("browser-rpc-attach".into())
        .spawn(move || {
            let outcome = server.browser_rpc_attach(&location, &session_id, &connection_id, &abort);
            let _ = tx.send(WorkerMsg::AttachEnded { server_key, session_id, attempt, outcome });
            if let Some(w) = wake {
                w();
            }
        })
        .ok();
}

/// Asks the session's Location for its plugins and answers the attach machine with
/// `WorkerMsg::PluginsListed`: a failed request lists none.
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
