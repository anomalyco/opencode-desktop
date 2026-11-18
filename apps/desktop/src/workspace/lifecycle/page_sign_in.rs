//! A browser page's sign-in to its server: the page (apps/web's index.html) asks for the password,
//! may keep it on the device when the user says so, and signs out (`window.opencodeSignOut`).
//! A native window has none: its servers' credentials are the app's own.

/// Whether this window is a page that can sign out of its server.
#[cfg(target_arch = "wasm32")]
pub(crate) fn can_sign_out() -> bool {
    sign_out_fn().is_some()
}

/// Whether `url` is the server the page signed in to (its `?server=`).
pub(crate) fn is_page_server(url: &str) -> bool {
    can_sign_out() && crate::arg("--server").is_some_and(|server| server.trim_end_matches('/') == url.trim_end_matches('/'))
}

/// Forgets the password the page keeps for its server, and goes back to asking for it.
#[cfg(target_arch = "wasm32")]
pub(crate) fn sign_out() {
    if let Some(f) = sign_out_fn() {
        let _ = f.call0(&wasm_bindgen::JsValue::UNDEFINED);
    }
}

#[cfg(target_arch = "wasm32")]
fn sign_out_fn() -> Option<js_sys::Function> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window()?;
    js_sys::Reflect::get(&window, &"opencodeSignOut".into()).ok()?.dyn_into().ok()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn can_sign_out() -> bool {
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sign_out() {}
