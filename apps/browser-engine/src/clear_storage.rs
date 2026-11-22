//! "Clear browsing data"'s storage half (official `profile/index.ts:523-540` `clear`): every site's
//! stored data in one request context, not only the open tabs' sites.
//!
//! CEF has no context-wide clear, so this asks DevTools `Storage.clearDataForOrigin` with an empty
//! origin: Chromium makes an opaque storage key of it, takes that as "no filter", and clears the
//! storage types for every storage key in the partition (`StoragePartitionImpl::ClearData`). That is
//! Chromium's behaviour, not a documented contract, so the clear is never trusted on its word:
//! - When the call errors (a Chromium that refuses an empty origin), each known site is cleared on
//!   its own instead.
//! - Then each known site is checked with `Storage.getUsageAndQuota`: a cleared storage type still
//!   holding bytes fails the clear. The known sites are what the app passes (every site the profile
//!   had cookies for) plus the open tabs'. The caller counts the cookies left.
//!
//! Storage types: `all` without installed extensions, as official's `clearStorageData()`. With
//! extensions every type but service workers, then every service worker registration that is not an
//! extension's (`chrome-extension://`) is unregistered: official's `clearData({ excludeOrigins })`
//! (`profile/index.ts:524-528`) keeps only the extensions' workers.
//!
//! Every call goes through a hidden page made for it, never a tab: an `about:blank` windowless
//! browser in the context being cleared (`None`: the global context, the shared profile's), marked
//! hidden, never in the engine's tab list (so neither the app nor an agent reaches it), opening no
//! popup. It is used once it has loaded and is observed, whichever comes last (DevTools drops a
//! message sent before), and closed when the clear ends, fails, its DevTools agent goes away, or
//! after `GIVE_UP_MS`. The teardown runs as its own UI task, never inside a CEF callback.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cef::rc::Rc;
use cef::{
    Browser, BrowserHost, BrowserSettings, CefString, Client, DevToolsMessageObserver, DictionaryValue, Frame,
    ImplBrowser, ImplBrowserHost, ImplClient, ImplDevToolsMessageObserver, ImplLifeSpanHandler, ImplLoadHandler,
    ImplRenderHandler, ImplTask, LifeSpanHandler, LoadHandler, PopupFeatures, Rect, Registration, RenderHandler,
    RequestContext, Task, ThreadId, WindowInfo, WindowOpenDisposition, WrapClient, WrapDevToolsMessageObserver,
    WrapLifeSpanHandler, WrapLoadHandler, WrapRenderHandler, WrapTask, wrap_client,
    wrap_dev_tools_message_observer, wrap_life_span_handler, wrap_load_handler, wrap_render_handler, wrap_task,
};
use serde_json::{Value, json};

/// Every `Storage.clearDataForOrigin` type but `service_workers`.
const WITHOUT_WORKERS: &str =
    "cookies,file_systems,indexeddb,local_storage,shader_cache,websql,cache_storage,interest_groups,shared_storage,storage_buckets";

/// `Storage.getUsageAndQuota` `usageBreakdown` types a clear empties. The known sites are web
/// origins, so their service workers go in either mode.
const CHECKED: &[&str] =
    &["file_systems", "indexeddb", "websql", "cache_storage", "shared_storage", "storage_buckets", "service_workers"];

/// A clear that has not ended by then has failed.
const GIVE_UP_MS: i64 = 30_000;

/// How long registrations may take to arrive after `ServiceWorker.enable` answers.
const WORKERS_SETTLE_MS: i64 = 1_000;

/// At most this many sites are cleared one by one or checked, and workers unregistered.
const MAX_SITES: usize = 400;
const MAX_WORKERS: usize = 400;

/// How many DevTools ids a clear over `sites` sites takes from `base`.
pub fn ids_for(sites: usize) -> i32 {
    (4 + 2 * sites.min(MAX_SITES) + 2 * MAX_WORKERS) as i32
}

type Done = Box<dyn FnOnce(bool) + Send>;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    /// The context-wide clear (`base`).
    Clearing,
    /// Each site on its own, after the context-wide clear errored.
    OneByOne,
    /// `ServiceWorker.enable` (`base + 1`), then the registrations it lists.
    Listing,
    /// Each listed site worker's `ServiceWorker.unregister`.
    Unregistering,
    /// Each site's remaining bytes.
    Checking,
}

struct State {
    phase: Phase,
    waiting: HashSet<i32>,
    leftover: bool,
    loaded: bool,
    observed: bool,
    started: bool,
    finished: bool,
    /// Site (not extension) worker registrations alive, by scope, as the events tell.
    workers: std::collections::BTreeSet<String>,
    /// Extension worker registrations seen and kept.
    kept: usize,
}

/// One clear in one context. The observer, the hidden page's handlers and the give-up task hold it;
/// it holds the page and the observer's registration until the teardown takes both, which breaks
/// every cycle.
struct Pending {
    base: i32,
    sites: Vec<String>,
    keep_workers: bool,
    state: Mutex<State>,
    host: Mutex<Option<BrowserHost>>,
    registration: Mutex<Option<Registration>>,
    done: Mutex<Option<Done>>,
}

impl Pending {
    fn send(&self, id: i32, method: &str, params: Value) -> bool {
        let raw = json!({ "id": id, "method": method, "params": params }).to_string();
        let host = self.host.lock().unwrap().clone();
        host.is_some_and(|h| h.send_dev_tools_message(Some(raw.as_bytes())) != 0)
    }

    /// Starts once the page has loaded and is observed.
    fn ready(self: &Arc<Self>, loaded: bool, observed: bool) {
        {
            let mut s = self.state.lock().unwrap();
            s.loaded |= loaded;
            s.observed |= observed;
            if !(s.loaded && s.observed) || s.started || s.finished {
                return;
            }
            s.started = true;
            s.phase = Phase::Clearing;
            s.waiting = HashSet::from([self.base]);
        }
        let types = if self.keep_workers { WITHOUT_WORKERS } else { "all" };
        if !self.send(self.base, "Storage.clearDataForOrigin", json!({ "origin": "", "storageTypes": types })) {
            self.answered(self.base, false, None);
        }
    }

    /// Sends one call per item from `first`, waiting for all of them in `phase`.
    fn each(self: &Arc<Self>, phase: Phase, first: i32, items: &[String], method: &str, params: impl Fn(&str) -> Value) {
        let ids: Vec<i32> = (0..items.len() as i32).map(|i| first + i).collect();
        {
            let mut s = self.state.lock().unwrap();
            s.phase = phase;
            s.waiting = ids.iter().copied().collect();
        }
        for (item, id) in items.iter().zip(&ids) {
            if !self.send(*id, method, params(item)) {
                let mut s = self.state.lock().unwrap();
                s.waiting.remove(id);
                s.leftover = true;
            }
        }
        self.settle();
    }

    fn sites_first(&self) -> i32 {
        self.base + 4
    }

    fn check_first(&self) -> i32 {
        self.base + 4 + self.sites.len() as i32
    }

    fn workers_first(&self) -> i32 {
        self.base + 4 + 2 * self.sites.len() as i32
    }

    /// After the storage clear: the site workers when extensions keep theirs, then the check.
    fn after_clear(self: &Arc<Self>) {
        if !self.keep_workers {
            return self.check();
        }
        {
            let mut s = self.state.lock().unwrap();
            s.phase = Phase::Listing;
            s.waiting = HashSet::from([self.base + 1]);
        }
        if !self.send(self.base + 1, "ServiceWorker.enable", json!({})) {
            self.state.lock().unwrap().leftover = true;
            self.check();
        }
    }

    /// Each live site worker: unregistered, and its origin's workers cleared from storage (an
    /// unregistered worker that never finished installing would otherwise stay listed).
    fn unregister(self: &Arc<Self>) {
        let (scopes, origins): (Vec<String>, Vec<String>) = {
            let s = self.state.lock().unwrap();
            let scopes: Vec<String> = s.workers.iter().take(MAX_WORKERS).cloned().collect();
            let mut origins: Vec<String> = scopes.iter().filter_map(|scope| url::Url::parse(scope).ok()).map(|u| u.origin().ascii_serialization()).collect();
            origins.sort();
            origins.dedup();
            eprintln!("clear-storage: {} site worker(s) to remove, {} extension worker registration(s) kept", scopes.len(), s.kept);
            (scopes, origins)
        };
        let ids: Vec<i32> = (0..(scopes.len() + origins.len()) as i32).map(|i| self.workers_first() + i).collect();
        {
            let mut s = self.state.lock().unwrap();
            s.phase = Phase::Unregistering;
            s.waiting = ids.iter().copied().collect();
        }
        let calls = scopes
            .iter()
            .map(|scope| ("ServiceWorker.unregister", json!({ "scopeURL": scope })))
            .chain(origins.iter().map(|o| ("Storage.clearDataForOrigin", json!({ "origin": o, "storageTypes": "service_workers" }))));
        for ((method, params), id) in calls.zip(&ids) {
            if !self.send(*id, method, params) {
                let mut s = self.state.lock().unwrap();
                s.waiting.remove(id);
                s.leftover = true;
            }
        }
        self.settle();
    }

    /// Lists the site workers again: the domain off and on sends every registration there is.
    fn relist(self: &Arc<Self>) {
        self.state.lock().unwrap().workers.clear();
        let _ = self.send(self.base + 2, "ServiceWorker.disable", json!({}));
        let _ = self.send(self.base + 3, "ServiceWorker.enable", json!({}));
        let mut later = Step::new(self.clone(), 4);
        cef::post_delayed_task(ThreadId::UI, Some(&mut later), WORKERS_SETTLE_MS);
    }

    fn check(self: &Arc<Self>) {
        self.each(Phase::Checking, self.check_first(), &self.sites.clone(), "Storage.getUsageAndQuota", |site| {
            json!({ "origin": site })
        });
    }

    /// Moves on once nothing of the current phase is waiting.
    fn settle(self: &Arc<Self>) {
        let (phase, empty, leftover) = {
            let s = self.state.lock().unwrap();
            (s.phase, s.waiting.is_empty(), s.leftover)
        };
        if !empty {
            return;
        }
        match phase {
            Phase::Clearing | Phase::Listing => {}
            Phase::OneByOne => self.after_clear(),
            Phase::Unregistering => {
                // The list is read again from scratch (removals are not always told as events).
                self.relist();
            }
            Phase::Checking => {
                // With extensions, the site workers the events still list count as left.
                let workers_left = self.keep_workers && !self.state.lock().unwrap().workers.is_empty();
                if leftover || workers_left {
                    eprintln!("clear-storage: left after the clear: storage {leftover}, site workers {:?}", self.state.lock().unwrap().workers);
                }
                if self.keep_workers {
                    let _ = self.send(self.base + 2, "ServiceWorker.disable", json!({}));
                }
                self.finish(!leftover && !workers_left)
            }
        }
    }

    fn answered(self: &Arc<Self>, id: i32, success: bool, result: Option<&[u8]>) {
        let phase = {
            let mut s = self.state.lock().unwrap();
            if s.finished || !s.waiting.remove(&id) {
                return;
            }
            s.phase
        };
        match phase {
            Phase::Clearing if success => self.after_clear(),
            Phase::Clearing => {
                // This Chromium refused the empty origin: each known site on its own instead. With
                // none known, nothing says the clear happened.
                if self.sites.is_empty() {
                    return self.finish(false);
                }
                let types = if self.keep_workers { WITHOUT_WORKERS } else { "all" };
                self.each(Phase::OneByOne, self.sites_first(), &self.sites.clone(), "Storage.clearDataForOrigin", |site| {
                    json!({ "origin": site, "storageTypes": types })
                });
            }
            Phase::Listing => {
                if !success {
                    self.state.lock().unwrap().leftover = true;
                    return self.check();
                }
                // Registrations come as events after the answer: give them a moment.
                let mut later = Step::new(self.clone(), 1);
                cef::post_delayed_task(ThreadId::UI, Some(&mut later), WORKERS_SETTLE_MS);
            }
            Phase::OneByOne | Phase::Unregistering => {
                if !success {
                    self.state.lock().unwrap().leftover = true;
                }
                self.settle();
            }
            Phase::Checking => {
                let body = result.and_then(|b| serde_json::from_slice::<Value>(b).ok());
                let left = !success
                    || body.is_some_and(|body| {
                        body["usageBreakdown"].as_array().into_iter().flatten().any(|u| {
                            CHECKED.contains(&u["storageType"].as_str().unwrap_or_default())
                                && u["usage"].as_f64().unwrap_or(0.0) > 0.0
                        })
                    });
                if left {
                    self.state.lock().unwrap().leftover = true;
                }
                self.settle();
            }
        }
    }

    /// A site's service worker registration, from `ServiceWorker.workerRegistrationUpdated`.
    fn event(&self, method: &str, params: Option<&[u8]>) {
        if method != "ServiceWorker.workerRegistrationUpdated" {
            return;
        }
        let Some(body) = params.and_then(|b| serde_json::from_slice::<Value>(b).ok()) else { return };
        let mut s = self.state.lock().unwrap();
        for r in body["registrations"].as_array().into_iter().flatten() {
            let scope = r["scopeURL"].as_str().unwrap_or_default();
            let deleted = r["isDeleted"].as_bool().unwrap_or(false);
            if scope.is_empty() {
                continue;
            }
            if scope.starts_with("chrome-extension://") {
                s.kept += usize::from(!deleted);
            } else if deleted {
                s.workers.remove(scope);
            } else if s.workers.len() < MAX_WORKERS * 2 {
                s.workers.insert(scope.to_owned());
            }
        }
    }

    /// Ends the clear once: the teardown (page closed, observer gone) and `done` run as a UI task.
    fn finish(self: &Arc<Self>, ok: bool) {
        {
            let mut s = self.state.lock().unwrap();
            if s.finished {
                return;
            }
            s.finished = true;
            s.leftover |= !ok;
        }
        let mut teardown = Step::new(self.clone(), 2);
        if cef::post_task(ThreadId::UI, Some(&mut teardown)) == 0 {
            self.teardown();
        }
    }

    fn teardown(&self) {
        let ok = !self.state.lock().unwrap().leftover;
        self.registration.lock().unwrap().take();
        if let Some(host) = self.host.lock().unwrap().take() {
            host.close_browser(1);
        }
        if let Some(done) = self.done.lock().unwrap().take() {
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

// `what`: `1` removes the listed site workers, `4` checks, `2` tears down, `3` gives up.
wrap_task! {
    struct Step {
        pending: Arc<Pending>,
        what: u8,
    }

    impl Task {
        fn execute(&self) {
            match self.what {
                1 => self.pending.unregister(),
                4 => self.pending.check(),
                2 => self.pending.teardown(),
                _ => self.pending.finish(false),
            }
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
            result: Option<&[u8]>,
        ) {
            self.pending.answered(message_id, success != 0, result);
        }

        fn on_dev_tools_event(&self, _browser: Option<&mut Browser>, method: Option<&CefString>, params: Option<&[u8]>) {
            if let Some(method) = method {
                self.pending.event(&method.to_string(), params);
            }
        }

        fn on_dev_tools_agent_detached(&self, _browser: Option<&mut Browser>) {
            self.pending.finish(false);
        }
    }
}

/// Clears every site's stored data in `context` (`None`: the global context, the shared profile's)
/// through a hidden page, then checks `sites` (origins such as `https://example.com`); `done` gets
/// whether it all went. Uses DevTools ids `base .. base + ids_for(sites.len())`.
pub fn clear_context(context: Option<RequestContext>, keep_workers: bool, mut sites: Vec<String>, base: i32, done: Done) {
    sites.truncate(MAX_SITES);
    let pending = Arc::new(Pending {
        base,
        sites,
        keep_workers,
        state: Mutex::new(State {
            phase: Phase::Clearing,
            waiting: HashSet::new(),
            leftover: false,
            loaded: false,
            observed: false,
            started: false,
            finished: false,
            workers: Default::default(),
            kept: 0,
        }),
        host: Mutex::new(None),
        registration: Mutex::new(None),
        done: Mutex::new(Some(done)),
    });
    let mut give_up = Step::new(pending.clone(), 3);
    cef::post_delayed_task(ThreadId::UI, Some(&mut give_up), GIVE_UP_MS);

    let mut client = HiddenClient::new(HiddenRender::new(), HiddenLoad::new(pending.clone()), HiddenLife::new(pending.clone()));
    let mut window = WindowInfo::default().set_as_windowless(Default::default());
    window.shared_texture_enabled = 0;
    let settings = BrowserSettings { windowless_frame_rate: 1, ..Default::default() };
    let mut context = context;
    let host = cef::browser_host_create_browser_sync(
        Some(&window),
        Some(&mut client),
        Some(&CefString::from("about:blank")),
        Some(&settings),
        None,
        context.as_mut(),
    )
    .and_then(|b| b.host());
    let Some(host) = host else { return pending.finish(false) };
    host.was_hidden(1);
    *pending.host.lock().unwrap() = Some(host.clone());
    let mut observer = ClearObserver::new(pending.clone());
    *pending.registration.lock().unwrap() = host.add_dev_tools_message_observer(Some(&mut observer));
    pending.ready(false, true);
}

wrap_load_handler! {
    struct HiddenLoad {
        pending: Arc<Pending>,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            _can_go_back: ::std::os::raw::c_int,
            _can_go_forward: ::std::os::raw::c_int,
        ) {
            if is_loading == 0 {
                self.pending.ready(true, false);
            }
        }
    }
}

wrap_life_span_handler! {
    struct HiddenLife {
        pending: Arc<Pending>,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            _target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            // Closed by anyone before it answered: the clear did not happen here.
            self.pending.finish(false);
        }
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
        load: LoadHandler,
        life: LifeSpanHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life.clone())
        }
    }
}
