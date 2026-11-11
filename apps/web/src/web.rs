//! The browser entry: the desktop app's window, as the page. The page's query names the server
//! (`?server=http://127.0.0.1:4096`, and `&password=` for one that asks for it, which the page
//! takes out of its address at once, as Desktop's web build does with `auth_token`). Any other
//! `?flag` or `?flag=value` passes on as the desktop app's `--flag [value]` (`?debug`,
//! `?zoom=1.25`, `?theme=dark`).

use wasm_bindgen::prelude::wasm_bindgen;

use crate::{App, appearance, launch, rune_app, spec, storage};

/// The page calls this once the module is in (index.html). A JSPI export, so the app's background
/// work runs on fibers that wait on the network without holding the page (`rune::task`).
#[allow(deprecated)]
#[wasm_bindgen(jspi)]
pub fn start() {
    rune::task::start_runners();
    // The app's state: the browser's storage, as the database is natively.
    storage::start();
    // Reads wait for the first frame, as a window's do.
    launch::defer();
    let args = page_args();
    // Before anything reads them: building the app's spec does.
    rune::tooling::Args::parse_from(args.clone());
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

/// The query as the app's arguments: `?a=1&b` is `--a 1 --b`. A password goes to the kit for its
/// server and leaves the address.
fn page_args() -> Vec<String> {
    let Some(window) = web_sys::window() else { return Vec::new() };
    let Ok(search) = window.location().search() else { return Vec::new() };
    let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) else { return Vec::new() };
    if let Some(password) = params.get("password").or_else(|| params.get("auth_token")) {
        let server = params.get("server").unwrap_or_else(|| opencode_kit::client::DEFAULT_SERVER.to_owned());
        opencode_kit::local_service::remember_page_password(&server, &password);
        params.delete("password");
        params.delete("auth_token");
        let rest = params.to_string().as_string().unwrap_or_default();
        let path = window.location().pathname().unwrap_or_default();
        let url = if rest.is_empty() { path } else { format!("{path}?{rest}") };
        if let Ok(history) = window.history() {
            let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
        }
    }
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
