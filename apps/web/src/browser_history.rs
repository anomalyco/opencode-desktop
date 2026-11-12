//! The browser's history follows the app's (`history.rs`, Desktop's titlebar history): every
//! place the app goes is a browser entry with its own address (`?session=<id>`), so Back and
//! Forward, a reload and a pasted address go where a web app's would.

use std::cell::RefCell;

use wasm_bindgen::{JsCast, prelude::*};

use crate::{App, Route};

thread_local! {
    /// The app's history index, the place it showed (session, Settings open) and the address of
    /// the browser entry that stands for it.
    static SEEN: RefCell<Option<Seen>> = const { RefCell::new(None) };
}

struct Seen {
    index: usize,
    session: Option<String>,
    settings: bool,
    url: String,
}

/// After each poll: a move the app made since becomes the browser's too. A new place is a new
/// entry; Back and Forward from the app's own keys move the browser's entry with them. A poll
/// that finds the app where it was reads nothing of the page.
pub(crate) fn sync(s: &App) {
    let index = s.nav.index();
    let session = match &s.route {
        Route::Session { id, .. } => Some(id.as_str()),
        _ => None,
    };
    SEEN.with_borrow_mut(|seen| {
        if seen.as_ref().is_some_and(|v| v.index == index && v.session.as_deref() == session && v.settings == s.settings.open) {
            return;
        }
        let url = address(s);
        let Some(history) = web_sys::window().and_then(|w| w.history().ok()) else { return };
        let state = JsValue::from(index as u32);
        match seen.as_ref() {
            Some(v) if v.url == url => {}
            Some(v) if index == v.index + 1 => drop(history.push_state_with_url(&state, "", Some(&url))),
            Some(v) if v.index != index => drop(history.go_with_delta(index as i32 - v.index as i32)),
            _ => drop(history.replace_state_with_url(&state, "", Some(&url))),
        }
        *seen = Some(Seen { index, session: session.map(str::to_owned), settings: s.settings.open, url });
    });
}

/// The browser's Back and Forward step the app through its own history to the entry's place.
pub(crate) fn listen(handle: rune::AppHandle<App>) {
    let Some(window) = web_sys::window() else { return };
    let on_pop = Closure::<dyn FnMut(web_sys::PopStateEvent)>::new(move |event: web_sys::PopStateEvent| {
        let Some(target) = event.state().as_f64().map(|i| i as usize) else { return };
        handle.update_if(move |s, tx| {
            let start = s.nav.index();
            while s.nav.index() != target && s.navigate(target > s.nav.index(), tx) {}
            // The browser is on that entry already.
            let session = match &s.route {
                Route::Session { id, .. } => Some(id.clone()),
                _ => None,
            };
            let seen = Seen { index: s.nav.index(), session, settings: s.settings.open, url: address(s) };
            SEEN.with_borrow_mut(|v| *v = Some(seen));
            s.nav.index() != start
        });
    });
    let _ = window.add_event_listener_with_callback("popstate", on_pop.as_ref().unchecked_ref());
    on_pop.forget();
}

/// The page's address for where the app is: its own query (the server and flags) with the
/// session open, if any.
fn address(s: &App) -> String {
    let Some(location) = web_sys::window().map(|w| w.location()) else { return String::new() };
    let Ok(params) = web_sys::UrlSearchParams::new_with_str(&location.search().unwrap_or_default()) else { return String::new() };
    params.delete("session");
    params.delete("settings");
    if let Route::Session { id, .. } = &s.route {
        params.set("session", id);
    }
    if s.settings.open {
        params.set("settings", "");
    }
    format!("{}?{}", location.pathname().unwrap_or_default(), String::from(params.to_string()))
}
