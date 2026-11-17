//! The browser entry: the desktop app's window, as the page. The page's query names the server
//! (`?server=http://127.0.0.1:4096`); any other `?flag` or `?flag=value` passes on as the desktop
//! app's `--flag [value]` (`?debug`, `?zoom=1.25`, `?theme=dark`). The page (index.html) takes a
//! password out of its address before it loads this, and hands it to [`start`].

use wasm_bindgen::{JsCast, closure::Closure, prelude::wasm_bindgen};

use crate::{App, appearance, launch, rune_app, spec, storage};

#[path = "browser_history.rs"]
mod browser_history;

/// The page calls this once the module is in, with the server's password if it has one. A JSPI
/// export, so the app's background work runs on fibers that wait on the network without holding
/// the page (`rune::task`). The password lives only in the kit's memory, which sends it as the
/// `Authorization` header: nothing stores or logs it.
#[allow(deprecated)]
#[wasm_bindgen(jspi)]
pub fn start(password: Option<String>) {
    rune::task::start_runners();
    // The dictionaries beside the page (`spell/`, scripts/web.ps1).
    rune::spell::set_web_dictionaries(&["en-US"]);
    // The page's arguments first: the storage and the app's spec read them.
    let args = page_args();
    rune::tooling::Args::parse_from(args.clone());
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        let server = crate::arg("--server").unwrap_or_else(|| opencode_kit::client::DEFAULT_SERVER.to_owned());
        opencode_kit::local_service::remember_page_password(&server, &password);
    }
    // The app's state: the browser's storage, as the database is natively.
    storage::start();
    // Start the reads before the first view. If they are already in, setup can connect here.
    launch::defer();
    launch::start_first_reads();
    let app_spec = rune::AppSpec { families: appearance::launch_families(), poll: Some(poll), ..spec(App::new) };
    let app = crate::LATE_FONTS.iter().fold(rune_app(app_spec).args(args), |app, url| app.font_url_later(url));
    let handle = app.handle();
    let wake = launch::inbox_waker(handle.clone());
    browser_history::listen(handle.clone());
    flush_on_pagehide();
    app.init(move || {
        let mut s = App::new();
        s.launch.inbox_wake = Some(wake);
        s
    }).on_ready(move |_| {
        storage::presented();
        handle.update_if(launch::ready);
    })
    .run();
}

/// The app's poll, then the browser's history catching up with where it went.
fn poll(s: &mut App, now: f64, tx: &mut rune::Tx) -> bool {
    let changed = crate::poll(s, now, tx);
    browser_history::sync(s);
    changed
}

/// Closing or reloading a browser tab never runs winit's `on_close`: land any queued terminal
/// snapshot and write-behind state rows synchronously on `pagehide` so nothing in the 1 s / 250 ms
/// buffers is lost.
fn flush_on_pagehide() {
    let Some(window) = web_sys::window() else { return };
    let on_hide = Closure::<dyn FnMut()>::new(|| {
        crate::terminal::persist::flush();
        storage::flush(opencode_storage::FlushReason::Close, false);
    });
    let _ = window.add_event_listener_with_callback("pagehide", on_hide.as_ref().unchecked_ref());
    on_hide.forget();
}

/// The query as the app's arguments: `?a=1&b` is `--a 1 --b`.
fn page_args() -> Vec<String> {
    let Some(search) = web_sys::window().and_then(|w| w.location().search().ok()) else { return Vec::new() };
    let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in params.entries().into_iter().flatten() {
        let pair = js_sys::Array::from(&entry);
        let (Some(key), value) = (pair.get(0).as_string(), pair.get(1).as_string().unwrap_or_default()) else { continue };
        out.push(format!("--{key}"));
        if !value.is_empty() {
            out.push(value);
        }
    }
    out
}
