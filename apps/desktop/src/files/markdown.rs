//! Markdown files read as prose in the file viewer: drawn with the replies' renderer and look,
//! a toolbar toggle to the source, ⌘F over the rendered text, and which files wrap by default.

use std::ops::Range;

use rune::{El, Icon, ScrollSpec, Tx, col, memo};
use rune_kit::Markdown;
use rune_kit::markdown::Case;

use super::SessionFiles;
use super::links::slug;
use crate::App;
use crate::code_view::CodeDocState;
use crate::palette::{Elevate as _, Pal};

/// The rendered column's width: a comfortable line for reading.
pub const READ_W: f32 = 720.0;

/// Bigger Markdown files open as source. Rune lays a Markdown document out whole, with no
/// virtualized rows: in a release build a 9,700-line, 835 KB file took 137 ms to open and
/// 1.5 ms a scrolled frame, so the cap sits just past what was measured.
pub const RENDER_LIMIT: usize = 1024 * 1024;

pub static READ_ICON: Icon = Icon {
    name: "files-read-rendered",
    view: 16.0,
    body: r#"<path d="M3 3.5H13M3 6.5H10M3 9.5H13M3 12.5H9" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" fill="none"/>"#,
};

/// A Markdown file's view: rendered or source, and its matches in the rendered text.
#[derive(Default)]
pub struct MdView {
    /// Kit's pick, rendered (`true`) or source; none is rendered up to [`RENDER_LIMIT`].
    pub rendered: Option<bool>,
    applied: String,
    pub matches: Vec<Vec<Range<usize>>>,
    pub active: usize,
}

fn ext(path: &str) -> Option<String> {
    let name = opencode_kit::path::basename(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => Some(ext.to_ascii_lowercase()),
        _ => None,
    }
}

pub fn is_markdown(path: &str) -> bool {
    ext(path).is_some_and(|e| matches!(e.as_str(), "md" | "markdown" | "mdx"))
}

/// Prose rather than code: Markdown, text, reStructuredText, AsciiDoc, and the extensionless
/// files a project keeps beside its code (`LICENSE`, `README`).
pub fn is_prose(path: &str) -> bool {
    const NAMES: &[&str] = &["LICENSE", "LICENCE", "README", "COPYING", "NOTICE", "AUTHORS", "CHANGELOG", "CONTRIBUTORS", "PATENTS"];
    let name = opencode_kit::path::basename(path);
    match ext(path) {
        Some(e) => matches!(e.as_str(), "md" | "markdown" | "mdx" | "txt" | "rst" | "adoc"),
        None => {
            let upper = name.to_ascii_uppercase();
            NAMES.iter().any(|n| upper.strip_prefix(n).is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '_'])))
        }
    }
}

/// Wrap per kind of file: every file wraps, as Desktop's file tab does (Pierre's `overflow:
/// "wrap"`, session-ui `pierre/index.ts:307`), until Kit flips one (kept across launches).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WrapPrefs {
    pub prose: Option<bool>,
    pub code: Option<bool>,
}

impl WrapPrefs {
    pub fn wraps(&self, path: &str) -> bool {
        if is_prose(path) { self.prose.unwrap_or(true) } else { self.code.unwrap_or(true) }
    }

    /// A document for `path`, wrapped as its kind is.
    pub fn doc(&self, path: &str, text: &str) -> CodeDocState {
        let mut d = CodeDocState::new(path, text);
        d.set_wrap(self.wraps(path));
        d
    }

    fn file() -> Option<std::path::PathBuf> {
        crate::dirs::data().map(|d| d.join("file-wrap.json"))
    }

    pub fn load() -> Self {
        let v = Self::file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .unwrap_or_default();
        Self { prose: v["prose"].as_bool(), code: v["code"].as_bool() }
    }

    fn save(&self) {
        let Some(f) = Self::file() else { return };
        let body = serde_json::json!({ "prose": self.prose, "code": self.code }).to_string();
        crate::disk::later("file-wrap", move || {
            crate::disk::replace(&f, body.as_bytes());
        });
    }
}

/// Whether `path` shows rendered now: Markdown as a page, an SVG as its picture (Desktop's
/// preview, which an SVG opens in).
pub fn shows_rendered(ws: &SessionFiles, path: &str) -> bool {
    let Some(d) = ws.docs.get(path) else { return false };
    let svg = super::image::is_svg(path);
    if !(is_markdown(path) || svg) || !d.ready || d.missing || d.binary {
        return false;
    }
    ws.md.get(path).and_then(|m| m.rendered).unwrap_or(svg || d.size_bytes <= RENDER_LIMIT)
}

pub fn scroll_key(sid: &str, path: &str) -> String {
    format!("file-md-{sid}-{path}-scroll")
}

fn heading_key(slug: &str) -> String {
    format!("mdh-{slug}")
}

/// Each top-level ATX heading (`## Usage`): where its line starts, and its slug.
fn headings(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    let mut at = 0;
    for line in src.split_inclusive('\n') {
        let t = line.trim_end_matches(['\n', '\r']);
        let lead = t.trim_start();
        if lead.starts_with("```") || lead.starts_with("~~~") {
            let mark = &lead[..3];
            match fence {
                Some(m) if m == mark => fence = None,
                None => fence = Some(mark),
                _ => {}
            }
        } else if fence.is_none() && t.starts_with('#') {
            let n = t.bytes().take_while(|b| *b == b'#').count();
            if n <= 6 && t[n..].starts_with(' ') {
                out.push((at, slug(t[n..].trim().trim_end_matches('#'))));
            }
        }
        at += line.len();
    }
    out
}

/// The rendered file: the replies' Markdown in a reading column that scrolls, its links
/// resolving against the file's folder. `page` lays it out as Desktop's artifact view does: on
/// `layer-01`, a raised `document-surface` sheet 20 in (12 in a view 600 wide or narrower) with its 0.5 pt
/// `border-base` ring and 8 pt corners, the text 64 from its top and 12.5% in, between 24 and
/// 134 (32 and 24 in a narrow view), at most 768 wide.
pub fn rendered(p: &Pal, sid: &str, path: &str, doc: &CodeDocState, view: Option<&MdView>, page: bool) -> El {
    let find = view.filter(|_| doc.find_open);
    let active = find.map(|v| v.active);
    let find_ver = find.map(|v| (v.applied.clone(), v.matches.len(), v.active));
    let (sid_link, path_link) = (sid.to_owned(), path.to_owned());
    let base = std::rc::Rc::new(opencode_kit::path::parent(path).to_owned());
    let body = memo(format!("md-body-{sid}-{path}"), (doc.keys_rev, doc.rev, p.dark, find_ver), || {
        let src = doc.text();
        let marks: Vec<(&[Range<usize>], rune_kit::Highlight)> = find
            .map(|v| v.matches.iter().enumerate().map(|(i, r)| (r.as_slice(), crate::find::markdown_mark(p, i, Some(i) == active))).collect())
            .unwrap_or_default();
        Markdown::new(src)
            .style(crate::view::markdown_style(p, p.text, READ_W))
            .highlights(marks)
            .block_keys(headings(src).into_iter().map(|(at, s)| (at, heading_key(&s))))
            .selectable(true)
            .on_link_tx(move |s: &mut App, url: &str, tx: &mut Tx| s.open_link(&sid_link, Some(&path_link), url, tx))
            .link_help({ let base = base.clone(); move |url| crate::files::link_tip::tip(url, base.as_str()) })
            .build(crate::palette::kit(p))
    });
    let key = scroll_key(sid, path);
    if page {
        let [_, _, w, h] = memo::layout(&key).unwrap_or([0.0, 0.0, 800.0, 0.0]);
        let narrow = w <= 600.0;
        let (gutter, pad_y, pad_x) = if narrow { (12.0, 32.0, 24.0) } else { (20.0, 64.0, (w - 40.0) * 0.125) };
        let foot = if narrow { 12.0 } else { 20.0 };
        let sheet = col()
            .key("artifact-markdown-page")
            .w_full()
            // `min-height: 100%`: the sheet runs to the view's foot however short the text.
            .min_h((h - 1.0 - foot).max(0.0))
            .items_center()
            .px(pad_x.clamp(24.0, 134.0))
            .py(pad_y)
            .radius(8.0)
            .fill(p.bg_base)
            .elevate(p.raised)
            .border(0.5, p.border)
            .child(body.w_full().max_w(768.0));
        return col()
            .key(key)
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(p.layer1)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full().px(gutter).pt(1.0).pb(foot).child(sheet));
    }
    col()
        .key(key)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(p.bg_base)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(col().w_full().items_center().px(28.0).pt(20.0).pb(48.0).child(body.w_full().max_w(READ_W)))
}

impl App {
    /// Switches `path` between rendered and source.
    pub fn toggle_rendered(&mut self, sid: &str, path: &str, tx: &mut Tx) {
        tx.instant();
        let Some(ws) = self.files.sessions.get_mut(sid) else { return };
        let next = !shows_rendered(ws, path);
        ws.md.entry(path.to_owned()).or_default().rendered = Some(next);
        if next && let Some(d) = ws.docs.get_mut(path) {
            d.set_selection(None);
        }
        self.sync_rendered_find(sid, path);
    }

    /// Turns wrap on or off for `path`, and for every file of its kind from now on.
    pub fn set_file_wrap(&mut self, sid: &str, path: &str, wrap: bool) {
        let prose = is_prose(path);
        if prose { self.files.wrap.prose = Some(wrap) } else { self.files.wrap.code = Some(wrap) }
        for ws in self.files.sessions.values_mut() {
            for (p, d) in ws.docs.iter_mut() {
                if is_prose(p) == prose {
                    d.set_wrap(wrap);
                }
            }
        }
        if let Some(d) = self.files.sessions.get_mut(sid).and_then(|w| w.docs.get_mut(path)) {
            d.set_wrap(wrap);
        }
        if self.persists() {
            self.files.wrap.save();
        }
    }

    /// Finds the find bar's query in the rendered text of `path` when it changed; whether it did.
    pub fn sync_rendered_find(&mut self, sid: &str, path: &str) -> bool {
        let Some(ws) = self.files.sessions.get_mut(sid) else { return false };
        if !shows_rendered(ws, path) {
            return false;
        }
        let Some(doc) = ws.docs.get(path).filter(|d| d.find_open) else { return false };
        let q = doc.find_input.text.clone();
        let v = ws.md.entry(path.to_owned()).or_default();
        if v.applied == q {
            return false;
        }
        v.matches = rune_kit::markdown::find(doc.text(), &q, Case::Insensitive);
        v.applied = q;
        v.active = 0;
        true
    }

    /// The find bar's next (or previous) match in `path`, in whichever view shows.
    pub fn file_find_step(&mut self, sid: &str, path: &str, forward: bool, tx: &mut Tx) {
        let Some(ws) = self.files.sessions.get_mut(sid) else { return };
        if let Some(d) = ws.docs.get_mut(path) {
            d.sync_find_query();
        }
        if !shows_rendered(ws, path) {
            if let Some(d) = ws.docs.get_mut(path) {
                d.step_match(&format!("file-code-{sid}-{path}-scroll"), forward, tx);
            }
            return;
        }
        self.sync_rendered_find(sid, path);
        let Some(v) = self.files.sessions.get_mut(sid).and_then(|w| w.md.get_mut(path)) else { return };
        let n = v.matches.len();
        if n == 0 {
            return;
        }
        v.active = if forward { (v.active + 1) % n } else { (v.active + n - 1) % n };
        tx.scroll_center_in(scroll_key(sid, path), "find-active");
    }

    /// Scrolls `path` to its heading `anchor`: in the rendered view, or to its line in the source.
    pub fn scroll_to_heading(&mut self, sid: &str, path: &str, anchor: &str, tx: &mut Tx) {
        let Some(ws) = self.files.sessions.get_mut(sid) else { return };
        let want = slug(anchor);
        let Some(doc) = ws.docs.get(path) else { return };
        let Some(at) = headings(doc.text()).into_iter().find(|(_, s)| *s == want).map(|(at, _)| at) else { return };
        if shows_rendered(ws, path) {
            tx.scroll_key_to(heading_key(&want), 12.0);
        } else if let Some(d) = ws.docs.get_mut(path) {
            let line = d.text()[..at].matches('\n').count() as u32 + 1;
            d.scroll_to_line(&format!("file-code-{sid}-{path}-scroll"), line, false, tx);
        }
    }
}
