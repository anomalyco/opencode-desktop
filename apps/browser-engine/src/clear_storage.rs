//! "Clear browsing data"'s storage half (official `profile/index.ts:523-540` `clear`): every site's
//! stored data in a request context, not only the open tabs' sites. CEF has no context-wide clear,
//! so this asks the context's DevTools `Storage.clearDataForOrigin` with an empty origin: Chromium
//! takes the opaque storage key it makes from that as "no filter" and clears the storage types for
//! every storage key in the partition (`StoragePartitionImpl::ClearData`), first-party and partitioned.
//!
//! - Without installed extensions every type goes (`all`), as official's `clearStorageData()`.
//! - With extensions, service workers stay (`WITHOUT_WORKERS`): official clears site workers only
//!   (`clearData` with `excludeOrigins`), and an extension's background worker is a registration in
//!   this same partition. The open tabs' sites still lose theirs through the per-tab clear beside it.
//!
//! A context with no page to speak DevTools through (the shared profile with no tab open) gets a
//! hidden `about:blank` browser for the call, closed once it answers.

use std::sync::{Arc, Mutex};

use cef::rc::Rc;
use cef::{
    Browser, BrowserHost, BrowserSettings, CefString, Client, DevToolsMessageObserver, ImplBrowser,
    ImplBrowserHost, ImplClient, ImplDevToolsMessageObserver, ImplRenderHandler, Rect, Registration,
    RenderHandler, WindowInfo, WrapClient, WrapDevToolsMessageObserver, WrapRenderHandler, wrap_client,
    wrap_dev_tools_message_observer, wrap_render_handler,
};
use serde_json::json;

/// Every `Storage.clearDataForOrigin` type but `service_workers`.
const WITHOUT_WORKERS: &str =
    "cookies,file_systems,indexeddb,local_storage,shader_cache,websql,cache_storage,interest_groups,shared_storage,storage_buckets";

type Done = Box<dyn FnOnce(bool) + Send>;

/// The pending call: its id, what to run with the answer, and what to drop then (the observer's
/// registration and, for a hidden browser, the browser).
struct Pending {
    id: i32,
    done: Mutex<Option<Done>>,
    registration: Mutex<Option<Registration>>,
    hidden: Mutex<Option<BrowserHost>>,
}

impl Pending {
    fn finish(&self, ok: bool) {
        let done = self.done.lock().unwrap().take();
        self.registration.lock().unwrap().take();
        if let Some(host) = self.hidden.lock().unwrap().take() {
            host.close_browser(1);
        }
        if let Some(done) = done {
            done(ok);
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(done) = self.done.get_mut().unwrap().take() {
            done(false);
        }
    }
}

wrap_dev_tools_message_observer! {
    struct ClearObserver {
        pending: Arc<Pending>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: i32,
            success: i32,
            _result: Option<&[u8]>,
        ) {
            if message_id == self.pending.id {
                self.pending.finish(success != 0);
            }
        }
    }
}

/// Clears every site's stored data in the context `host` belongs to; `done` gets whether DevTools
/// answered success.
pub fn clear_all_sites(host: &BrowserHost, keep_workers: bool, id: i32, done: Done) {
    run(host, None, keep_workers, id, done);
}

/// As `clear_all_sites`, for the shared profile when no tab of it is open: through a hidden
/// `about:blank` browser in the global context.
pub fn clear_all_sites_hidden(keep_workers: bool, id: i32, done: Done) {
    let mut client = HiddenClient::new(HiddenRender::new());
    let mut window = WindowInfo::default().set_as_windowless(Default::default());
    window.shared_texture_enabled = 0;
    let settings = BrowserSettings { windowless_frame_rate: 1, ..Default::default() };
    let host = cef::browser_host_create_browser_sync(
        Some(&window),
        Some(&mut client),
        Some(&CefString::from("about:blank")),
        Some(&settings),
        None,
        None,
    )
    .and_then(|b| b.host());
    let Some(host) = host else { return done(false) };
    host.was_hidden(1);
    run(&host.clone(), Some(host), keep_workers, id, done);
}

fn run(host: &BrowserHost, hidden: Option<BrowserHost>, keep_workers: bool, id: i32, done: Done) {
    let pending = Arc::new(Pending {
        id,
        done: Mutex::new(Some(done)),
        registration: Mutex::new(None),
        hidden: Mutex::new(hidden),
    });
    let mut observer = ClearObserver::new(pending.clone());
    let registration = host.add_dev_tools_message_observer(Some(&mut observer));
    *pending.registration.lock().unwrap() = registration;
    let types = if keep_workers { WITHOUT_WORKERS } else { "all" };
    let raw = json!({ "id": id, "method": "Storage.clearDataForOrigin", "params": { "origin": "", "storageTypes": types } }).to_string();
    if host.send_dev_tools_message(Some(raw.as_bytes())) == 0 {
        pending.finish(false);
    }
}

wrap_render_handler! {
    struct HiddenRender;

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                *rect = Rect { x: 0, y: 0, width: 1, height: 1 };
            }
        }
    }
}

wrap_client! {
    struct HiddenClient {
        render: RenderHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }
    }
}
