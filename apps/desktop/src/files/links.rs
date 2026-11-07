//! Links in Markdown (replies, and files rendered in the viewer): web and app links open as
//! before; a file link opens the file in the side file viewer, as Desktop's file `LinkHandler`
//! does (`gui-plugins/src/file/renderer.tsx`, `resolve-link.ts`).
//!
//! | Link | Opens |
//! | --- | --- |
//! | `https:`, `mailto:`, `seer:`, any scheme but `file:` | The browser or the app that owns it (live runs only) |
//! | `docs/plan.md`, `./x.md`, `../x.md`, `/abs/in/folder`, `~/…`, `file://…` | The file's tab, at `#L12`, `#L12-L20`, `:12` or `:12-20`; a `#heading` scrolls a rendered file to it |
//! | A file outside the session's folder | Its default Mac app (the server reads only inside the folder) |
//! | A file that isn't there | A toast naming it |
//! | A bare word (`link`, `Makefile`) that isn't a file there | A toast saying it points nowhere (a bare word that is a file opens it) |
//!
//! Relative links resolve against the session's folder in a reply, and against the file's own
//! folder in a rendered file.

use opencode_kit::path::is_absolute;
use rune::Tx;

use crate::App;

/// Where a link goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    /// A web or app address, opened as it is.
    Web(String),
    /// A heading in the document the link is in (`#usage`).
    Heading(String),
    /// A file in the session's folder, relative to it.
    Inside { path: String, lines: Option<(u32, u32)>, heading: Option<String> },
    /// A file outside the session's folder, absolute (or as written, when there's no folder).
    Outside { path: String },
}

/// Classifies `href` from a document in folder `base` (relative to `directory`, the session's
/// folder; `""` for a reply). `home` expands `~`.
pub fn classify(href: &str, base: &str, directory: &str, home: &str) -> Link {
    let href = href.trim();
    if let Some(frag) = href.strip_prefix('#') {
        return Link::Heading(decode(frag));
    }
    let file_url = href.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("file://"));
    if !file_url && scheme(href) {
        return Link::Web(href.to_owned());
    }
    let raw = if file_url {
        let rest = &href[7..];
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        rest.split('?').next().unwrap_or(rest)
    } else {
        href
    };
    let (raw, frag) = match raw.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (raw, None),
    };
    let (raw, mut lines) = split_line_suffix(raw);
    let mut heading = None;
    if let Some(f) = frag.filter(|f| !f.is_empty()) {
        match parse_l_fragment(f) {
            Some(l) => lines = Some(l),
            None => heading = Some(decode(f)),
        }
    }
    let path = decode(raw).replace('\\', "/");
    // `file:///C:/notes.md` names `C:/notes.md`.
    let path = match path.strip_prefix('/') {
        Some(p) if p.as_bytes().get(1) == Some(&b':') && is_absolute(p) => p.to_owned(),
        _ => path,
    };
    let (directory, home) = (directory.replace('\\', "/"), home.replace('\\', "/"));
    let dir = directory.trim_end_matches('/');
    let joined = if let Some(rest) = path.strip_prefix("~/").or((path == "~").then_some("")) {
        format!("{}/{rest}", home.trim_end_matches('/'))
    } else if is_absolute(&path) {
        path
    } else {
        let base = base.trim_matches('/');
        let rel = if base.is_empty() { path } else { format!("{base}/{path}") };
        if dir.is_empty() { rel } else { format!("{dir}/{rel}") }
    };
    let abs = is_absolute(&joined);
    let norm = collapse(&joined);
    if !dir.is_empty() {
        return match crate::tool_open::relative(&norm, dir) {
            Some(path) => Link::Inside { path, lines, heading },
            None => Link::Outside { path: norm },
        };
    }
    if abs || norm.starts_with("../") || norm == ".." {
        return Link::Outside { path: norm };
    }
    Link::Inside { path: norm, lines, heading }
}

/// Whether `path` is one bare word (`link`, `Makefile`): no folder, no extension.
pub(super) fn is_bare_word(path: &str) -> bool {
    !path.is_empty() && !path.contains(['/', '.', ':', '\\', '%', '#', '~']) && !path.contains(char::is_whitespace)
}

/// Whether `href` starts with a URL scheme (`https:`, `mailto:`, `seer:`) rather than a path
/// with a line (`README.md:12`).
fn scheme(href: &str) -> bool {
    let Some((head, rest)) = href.split_once(':') else { return false };
    let mut chars = head.chars();
    let named = chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    named && head.len() > 1 && !rest.starts_with(|c: char| c.is_ascii_digit())
}

/// `path:12`, `path:12-20` and `path:12:5` (a column, dropped): the path and its lines.
fn split_line_suffix(raw: &str) -> (&str, Option<(u32, u32)>) {
    let mut parts = raw.rsplitn(3, ':');
    let (Some(last), Some(mid)) = (parts.next(), parts.next()) else { return (raw, None) };
    // `path:12:5`: line 12, column 5.
    if let Some(first) = parts.next()
        && !first.is_empty()
        && let (Ok(line), true) = (mid.parse::<u32>(), last.chars().all(|c| c.is_ascii_digit()) && !last.is_empty())
    {
        return (first, Some((line.max(1), line.max(1))));
    }
    let base = &raw[..raw.len() - last.len() - 1];
    match parse_range(last) {
        Some(r) if !base.is_empty() => (base, Some(r)),
        _ => (raw, None),
    }
}

/// `L12`, `L12-L20`, `L12-20`.
fn parse_l_fragment(f: &str) -> Option<(u32, u32)> {
    let rest = f.strip_prefix('L')?;
    let (a, b) = match rest.split_once('-') {
        Some((a, b)) => (a, Some(b.strip_prefix('L').unwrap_or(b))),
        None => (rest, None),
    };
    let a = a.parse::<u32>().ok()?.max(1);
    let b = match b {
        Some(b) => b.parse::<u32>().ok()?.max(a),
        None => a,
    };
    Some((a, b))
}

/// `12` or `12-20`.
fn parse_range(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('-').unwrap_or((s, s));
    let a = a.parse::<u32>().ok()?.max(1);
    Some((a, b.parse::<u32>().ok()?.max(a)))
}

/// `a/./b/../c` → `a/c`; leading `..` stay on a relative path and drop at `/`.
fn collapse(path: &str) -> String {
    let abs = is_absolute(path);
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                // A drive (`C:`) is a root, like `/`.
                if out.last().is_some_and(|l| *l != ".." && !(abs && out.len() == 1 && l.ends_with(':'))) {
                    out.pop();
                } else if !abs {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if path.starts_with('/') { format!("/{joined}") } else { joined }
}

/// Percent-decodes `s`; a malformed escape stays as written.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// A heading's GitHub-style anchor: lowercase, spaces to `-`, punctuation dropped.
pub fn slug(text: &str) -> String {
    text.trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

impl App {
    /// The folder links in session `sid` resolve against.
    pub fn link_directory(&self, sid: &str) -> String {
        self.session(sid).map(|i| i.directory.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| self.draft_dir().to_owned())
    }

    /// Follows `href` clicked in the open session's timeline (a reply, a note, a card).
    pub fn open_reply_link(&mut self, href: &str, tx: &mut Tx) {
        if let Some(sid) = self.active_session_id().map(str::to_owned) {
            return self.open_link(&sid, None, href, tx);
        }
        // A new session's page: no side panel to open a file in.
        match classify(href, "", self.draft_dir(), &self.home) {
            Link::Web(url) => self.open_web(&url, tx),
            Link::Heading(_) => {}
            Link::Inside { path, .. } => {
                let abs = format!("{}/{path}", self.draft_dir().trim_end_matches('/'));
                self.open_outside_file(&abs);
            }
            Link::Outside { path } => self.open_outside_file(&path),
        }
    }

    fn open_web(&self, url: &str, tx: &mut Tx) {
        if self.live() || cfg!(test) {
            let _ = tx.open_url(url);
        }
    }

    /// Follows `href` clicked in session `sid`'s timeline, or in its file `from` (`Some`) rendered
    /// in the viewer, whose folder relative links resolve against.
    pub fn open_link(&mut self, sid: &str, from: Option<&str>, href: &str, tx: &mut Tx) {
        let base = from.map_or("", opencode_kit::path::parent);
        match classify(href, base, &self.link_directory(sid), &self.home) {
            Link::Web(url) => self.open_web(&url, tx),
            Link::Heading(h) => {
                if let Some(f) = from {
                    self.scroll_to_heading(sid, f, &h, tx);
                }
            }
            Link::Inside { path, lines, heading } if from == Some(path.as_str()) && lines.is_none() => {
                if let Some(h) = heading {
                    self.scroll_to_heading(sid, &path, &h, tx);
                }
            }
            Link::Inside { path, lines, heading } => self.open_file_link(sid, &path, lines, heading, tx),
            Link::Outside { path } => self.open_outside_file(&path),
        }
    }

    /// Opens `path` (in session `sid`'s folder) in the viewer once it's known to exist, at
    /// `lines` or `heading`; says so when it doesn't.
    fn open_file_link(&mut self, sid: &str, path: &str, lines: Option<(u32, u32)>, heading: Option<String>, tx: &mut Tx) {
        tx.instant();
        self.ensure_session_review(sid);
        let ws = self.files.sessions.entry(sid.to_owned()).or_default();
        // Ask again for a file that wasn't there, or whose answer was lost.
        if ws.docs.get(path).is_some_and(|d| d.missing || !d.ready) {
            ws.docs.remove(path);
        }
        self.ensure_file_loaded(sid, path);
        let Some(ws) = self.files.sessions.get_mut(sid) else { return };
        let link = super::LinkOpen { lines, heading };
        match ws.docs.get(path) {
            Some(d) if !d.ready => {
                ws.link_opens.insert(path.to_owned(), link);
            }
            _ => self.finish_file_link(sid, path, link, tx),
        }
    }

    /// The file a link asked for has loaded: opens its tab, or says it isn't there.
    pub(super) fn finish_file_link(&mut self, sid: &str, path: &str, link: super::LinkOpen, tx: &mut Tx) {
        let Some(ws) = self.files.sessions.get_mut(sid) else { return };
        if ws.docs.get(path).is_none_or(|d| d.missing) {
            ws.docs.remove(path);
            ws.known_files.remove(path);
            ws.rebuild_tree();
            let title = if is_bare_word(path) { "This link doesn't point anywhere" } else { "File not found" };
            crate::status_toast::say(self, title, path);
            return;
        }
        // A line is in the source: a rendered file shows it there.
        if link.lines.is_some() && super::markdown::is_markdown(path) {
            ws.md.entry(path.to_owned()).or_default().rendered = Some(false);
        }
        self.open_workspace_file_for_session(sid, path, false, link.lines, false, tx);
        if let Some(h) = link.heading {
            self.scroll_to_heading(sid, path, &h, tx);
        }
    }

    /// A file outside the session's folder: its default Mac app, where this Mac has it.
    fn open_outside_file(&mut self, path: &str) {
        if !self.can_open_in() || !is_absolute(path) {
            crate::status_toast::say(self, "Can't open files outside this session's folder", path);
            return;
        }
        if self.live() && !std::path::Path::new(path).exists() {
            crate::status_toast::say(self, "File not found", path);
            return;
        }
        self.open_in.open_default(path.to_owned());
    }
}
