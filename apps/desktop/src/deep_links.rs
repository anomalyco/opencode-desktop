//! Links opened with the app (`App::on_open_url`), on OpenCode Desktop's scheme (`opencode://`)
//! or the app's own (`opencode-rune://`, see `launch::product`):
//!
//! - `open?project=<folder>` opens the folder as a project with a new session, as Open project
//!   does: OpenCode Desktop's link (`packages/desktop/src/main/deep-links`).
//! - `notification/<id>?…` is a clicked notification (`rune_notify::click_from_url`): its
//!   session opens, the app coming forward, also when the click started it.
//! - `console/authorized?window=<id>` is the OpenCode Console's return link after a sign-in:
//!   the window that started it comes forward (Desktop's `consoleReturnWindow`).
//!
//! The window comes forward for each, as Desktop's `focusWindow` does. A link that arrives
//! before the server connects (one that started the app) waits for it.
//!
//! Windows registers both schemes at install (`win_install`) and hands each link to the running
//! app through `rune::single_instance`; macOS gets them from the bundle's `CFBundleURLTypes`.

use rune::Tx;

use crate::{App, Source};

/// `App::on_open_url`.
pub fn open(s: &mut App, url: &str, tx: &mut Tx) {
    if let Some(click) = rune_notify::click_from_url(url) {
        crate::notifications::clicked(s, &click);
        return;
    }
    if let Some(id) = console_return(url, crate::launch::product().scheme()) {
        match crate::windows::handle(&id) {
            Some(w) => w.focus(),
            None => rune_notify::bring_to_front(),
        }
        return;
    }
    match project(url, crate::launch::product().scheme()) {
        Some(dir) => {
            s.links.push(dir);
            drain(s, tx);
        }
        None => eprintln!("opencode-rune: ignored a link it doesn't open: {url}"),
    }
}

/// Opens the projects links asked for, once the app can (from `poll`).
pub fn drain(s: &mut App, tx: &mut Tx) -> bool {
    if s.links.is_empty() || matches!(s.source, Source::Live { writes: None, .. }) {
        return false;
    }
    for dir in std::mem::take(&mut s.links) {
        if s.live() {
            rune_notify::bring_to_front();
        }
        s.close_settings();
        s.open_project_dir(&dir, true, tx);
    }
    true
}

/// The folder an `open?project=<folder>` link on `opencode` or `own` names.
fn project(url: &str, own: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case(win_install::OPENCODE_SCHEME) && !scheme.eq_ignore_ascii_case(own) {
        return None;
    }
    let (host, query) = rest.split_once('?')?;
    if host.trim_end_matches('/') != "open" {
        return None;
    }
    let dir = query.split('&').find_map(|pair| pair.strip_prefix("project=")).map(unpercent)?;
    (!dir.trim().is_empty() && !dir.chars().any(char::is_control)).then_some(dir)
}

/// The window an `opencode://console/authorized?window=<id>` link names.
fn console_return(url: &str, own: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case(win_install::OPENCODE_SCHEME) && !scheme.eq_ignore_ascii_case(own) {
        return None;
    }
    let (path, query) = rest.split_once('?')?;
    if path != "console/authorized" {
        return None;
    }
    let id = query.split('&').find_map(|pair| pair.strip_prefix("window=")).map(unpercent)?;
    (!id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)).then_some(id)
}

/// A query value: `%XX` escapes and `+` for a space, as `URLSearchParams` reads it.
fn unpercent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match (b[i], hex) {
            (_, Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b'+', None) => {
                out.push(b' ');
                i += 1;
            }
            (c, None) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
