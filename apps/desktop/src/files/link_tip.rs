//! What hovering a link in Markdown shows: where it goes, shortened. Web links show their host
//! and path, files their path in the session's folder, and a bare word that isn't a file there
//! says it points nowhere, as a click would (`links.rs`).

use std::cell::RefCell;

use super::links::{Link, classify, is_bare_word};
use crate::App;

/// The open session's folder, and its bare-word files once its file list has loaded.
#[derive(Default)]
struct Scope {
    dir: String,
    home: String,
    bare: Option<Vec<String>>,
}

thread_local! {
    static SCOPE: RefCell<Scope> = RefCell::default();
}

/// Before the view builds: the folder links in the open session resolve against.
pub fn prepare(s: &App) {
    let sid = s.active_session_id();
    let dir = sid.map_or_else(|| s.draft_dir().to_owned(), |sid| s.link_directory(sid));
    let ws = sid.and_then(|sid| s.files.sessions.get(sid)).filter(|w| w.ready);
    let bare = ws.map(|w| w.known_files.iter().filter(|f| is_bare_word(f)).cloned().collect());
    SCOPE.with_borrow_mut(|sc| *sc = Scope { dir, home: s.home.clone(), bare });
}

/// The tip for `href` in a reply, or in a file at `base` in the open session's folder.
pub fn tip(href: &str, base: &str) -> Option<String> {
    SCOPE.with_borrow(|sc| label(href, base, &sc.dir, &sc.home, sc.bare.as_deref()))
}

/// Where `href` goes, shortened; `bare` lists the folder's bare-word files when known.
pub fn label(href: &str, base: &str, dir: &str, home: &str, bare: Option<&[String]>) -> Option<String> {
    let out = match classify(href, base, dir, home) {
        Link::Web(url) => web(&url),
        Link::Heading(h) => format!("#{h}"),
        Link::Inside { path, .. } if is_bare_word(&path) && bare.is_some_and(|b| !b.contains(&path)) => opencode_i18n::t!("rune.files.link.nowhere").to_owned(),
        Link::Inside { path, lines, heading } => {
            let at = match (lines, heading) {
                (Some((a, b)), _) if a == b => format!(":{a}"),
                (Some((a, b)), _) => format!(":{a}-{b}"),
                (None, Some(h)) => format!("#{h}"),
                (None, None) => String::new(),
            };
            format!("{path}{at}")
        }
        Link::Outside { path } if path != home => opencode_kit::path::tilde(&path, home),
        Link::Outside { path } => path,
    };
    (!out.is_empty()).then_some(out)
}

/// `https://www.example.com/a/b?q#x` → `www.example.com/a/b`; other schemes as written.
fn web(url: &str) -> String {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let Some(rest) = ["https://", "http://"].iter().find(|p| lower.starts_with(**p)).map(|p| &url[p.len()..]) else {
        return url.to_owned();
    };
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let rest = rest.rsplit_once('@').filter(|(user, _)| !user.contains('/')).map_or(rest, |(_, r)| r);
    rest.trim_end_matches('/').to_owned()
}
