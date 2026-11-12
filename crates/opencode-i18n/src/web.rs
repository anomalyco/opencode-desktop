//! A page's messages: its module carries English alone (`rune_i18n::codegen` leaves the other
//! locales for run time), and the locale a page shows is fetched when it's first made current,
//! from `i18n/<id>.json` beside the page (`scripts/web.ps1` merges each locale's layers into
//! one, a later layer winning, as the build does). The request is synchronous, so the first
//! frame and a switch in Settings come up in their language at once, never English first; a
//! locale that doesn't load stays English, key by key as a missing message would.

use crate::CATALOG;

pub(crate) fn load(index: usize, id: &str) {
    if CATALOG.present(index) { return; }
    let loaded = fetch(&format!("i18n/{id}.json")).and_then(|json| CATALOG.load_json(index, &json));
    if let Err(why) = loaded {
        web_sys::console::warn_1(&format!("opencode: the {id} messages didn't load ({why}); the page stays in English").into());
    }
}

fn fetch(url: &str) -> Result<String, String> {
    let fail = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let xhr = web_sys::XmlHttpRequest::new().map_err(fail)?;
    xhr.open_with_async("GET", url, false).map_err(fail)?;
    xhr.send().map_err(fail)?;
    match xhr.status().map_err(fail)? {
        200 => xhr.response_text().map_err(fail)?.ok_or_else(|| "an empty answer".into()),
        status => Err(format!("{url}: status {status}")),
    }
}