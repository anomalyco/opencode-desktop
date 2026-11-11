//! The browser entry: the desktop app's window, as the page. The page's query names the server
//! (`?server=http://127.0.0.1:4096`); any other `?flag` or `?flag=value` passes on as the desktop
//! app's `--flag [value]` (`?debug`, `?zoom=1.25`, `?theme=dark`). The page (index.html) takes a
//! password out of its address before it loads this, and hands it to [`start`].

use wasm_bindgen::prelude::wasm_bindgen;

use crate::{App, appearance, launch, rune_app, spec, storage};

/// The page calls this once the module is in, with the server's password if it has one. A JSPI
/// export, so the app's background work runs on fibers that wait on the network without holding
/// the page (`rune::task`). The password lives only in the kit's memory, which sends it as the
/// `Authorization` header: nothing stores or logs it.
#[allow(deprecated)]
#[wasm_bindgen(jspi)]
pub fn start(password: Option<String>) {
    rune::task::start_runners();
    // The page's arguments first: the storage and the app's spec read them.
    let args = page_args();
    rune::tooling::Args::parse_from(args.clone());
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        let server = crate::arg("--server").unwrap_or_else(|| opencode_kit::client::DEFAULT_SERVER.to_owned());
        opencode_kit::local_service::remember_page_password(&server, &password);
    }
    // The app's state: the browser's storage, as the database is natively.
    storage::start();
    // Reads wait for the first frame, as a window's do.
    launch::defer();
    let app = rune_app(rune::AppSpec { families: appearance::launch_families(), ..spec(App::new) }).args(args);
    let handle = app.handle();
    app.on_ready(move |_| {
        storage::presented();
        let wake = launch::inbox_waker(handle.clone());
        handle.update_if(move |s, tx| {
            s.launch.inbox_wake = Some(wake);
            launch::ready(s, tx)
        });
    })
    .run();
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
