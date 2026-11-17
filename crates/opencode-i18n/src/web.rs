//! A page's messages: its module carries English alone (`rune_i18n::codegen` leaves the other
//! locales for run time). At startup `index.html` fetches the chosen locale beside the module and
//! hands the raw JSON through `opencodeInitialLocale`, so the first frame comes up in that
//! language with no extra wait; a switch in Settings fetches `i18n/<id>.json` asynchronously via
//! `window.fetch` and flips `I18N` + wakes the window once the catalog is in memory, never
//! blocking the UI thread on a synchronous `XmlHttpRequest` or flashing English in between.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wasm_bindgen::prelude::wasm_bindgen;

use crate::{CATALOG, I18N};

static WANTED: AtomicUsize = AtomicUsize::new(0);
static WAKER: Mutex<Option<Box<dyn Fn() + Send + Sync>>> = Mutex::new(None);

#[wasm_bindgen(inline_js = "export function takeBootLocale(id) {\n  const boot = globalThis.opencodeInitialLocale;\n  if (Array.isArray(boot) && boot[0] === id && typeof boot[1] === 'string') {\n    globalThis.opencodeInitialLocale = null;\n    return boot[1];\n  }\n  return null;\n}\nexport async function fetchLocaleText(url) {\n  const r = await fetch(url);\n  if (!r.ok) throw new Error(`${url}: status ${r.status}`);\n  return await r.text();\n}\n")]
extern "C" {
    #[wasm_bindgen(js_name = takeBootLocale)]
    fn take_boot_locale(id: &str) -> Option<String>;
    #[wasm_bindgen(js_name = fetchLocaleText, catch)]
    async fn fetch_locale_text(url: &str) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
}

/// Registers the waker fired when an asynchronous locale catalog load completes.
pub fn on_loaded(wake: impl Fn() + Send + Sync + 'static) {
    *WAKER.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(wake));
}

/// Returns `true` when `index` is ready in `CATALOG` right now; otherwise starts an asynchronous
/// `fetch` that activates `index` and wakes the window when it lands (if still the newest pick).
pub(crate) fn load_or_spawn(index: usize, id: &str) -> bool {
    WANTED.store(index, Ordering::SeqCst);
    if CATALOG.present(index) {
        return true;
    }
    if let Some(json) = take_boot_locale(id) {
        if let Err(why) = CATALOG.load_json(index, &json) {
            web_sys::console::warn_1(&format!("opencode: the {id} messages didn't load ({why}); the page stays in English").into());
            return false;
        }
        return true;
    }
    let id = id.to_owned();
    let url = format!("i18n/{id}.json");
    wasm_bindgen_futures::spawn_local(async move {
        let loaded = fetch_locale_text(&url)
            .await
            .map_err(|e| format!("{e:?}"))
            .and_then(|v| v.as_string().ok_or_else(|| "an empty answer".to_owned()))
            .and_then(|json| CATALOG.load_json(index, &json));
        match loaded {
            Ok(()) => {
                if WANTED.load(Ordering::SeqCst) == index && I18N.set_locale(index) {
                    if let Some(wake) = WAKER.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
                        wake();
                    }
                }
            }
            Err(why) => {
                web_sys::console::warn_1(&format!("opencode: the {id} messages didn't load ({why}); the page stays in English").into());
            }
        }
    });
    false
}
