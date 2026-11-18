//! What hovering a link in Markdown shows: where it goes, shortened. Web links show their host
//! and path, files their path in the session's folder, and a bare word that isn't a file there
//! says it points nowhere, as a click would (`links.rs`).

use std::cell::RefCell;

use super::links::{Link, classify, is_bare_word, link_directory};
use crate::FilesHost;

/// The open session's folder, and its bare-word files once its file list has loaded.
#[derive(Default)]
struct Scope {
    dir: String,
    home: String,
    known_len: Option<usize>,
    bare: Option<Vec<String>>,
}

thread_local! {
    static SCOPE: RefCell<Scope> = RefCell::default();
}

/// Before the view builds: the folder links in the open session resolve against.
pub fn prepare(host: &dyn FilesHost) {
    let sid = host.active_session_id();
    let dir = sid.map_or_else(|| host.draft_directory().to_owned(), |sid| link_directory(host, sid));
    let home = host.home_directory();
    let files = host.files_untracked();
    let ws = sid.and_then(|sid| files.sessions.get(sid)).filter(|w| w.ready);
    let known_len = ws.map(|w| w.known_files.len());
    SCOPE.with_borrow_mut(|sc| {
        if sc.dir == dir && sc.home == home && sc.known_len == known_len {
            return;
        }
        let bare = ws.map(|w| w.known_files.iter().filter(|f| is_bare_word(f)).cloned().collect());
        *sc = Scope { dir, home: home.to_owned(), known_len, bare };
    });
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
        Link::Inside { path, .. } if is_bare_word(&path) && bare.is_some_and(|b| !b.contains(&path)) => {
            opencode_i18n::t!("rune.files.link.nowhere").to_owned()
        }
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
    let Some(rest) = ["https://", "http://"]
        .iter()
        .find(|p| lower.starts_with(**p))
        .map(|p| &url[p.len()..])
    else {
        return url.to_owned();
    };
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let rest = rest
        .rsplit_once('@')
        .filter(|(user, _)| !user.contains('/'))
        .map_or(rest, |(_, r)| r);
    rest.trim_end_matches('/').to_owned()
}
