//! The narrow window's Changes view (`gui-plugins/src/review/mobile.tsx`, `SessionReview` with
//! `changeSummary`, `session-review.css`, `accordion.css`): a 40 pt header with the change
//! scope, the totals in its middle and Expand all / Collapse all at its end, over every changed
//! file as one accordion. A file's row opens to its `+N -M` and Open file over its unified diff.

use rune::{Color, El, Icon, ScrollSpec, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::ScrollBar;
use rune_kit::diff::FileDiff;
use rune_theme_opencode::icons as ic;

use super::SessionReview;
use crate::App;
use crate::code_view::{DiffDocState, DiffViewer};
use crate::palette::{Elevate as _, Pal};
use opencode_i18n::t;

/// The open files of a session's narrow review, in the order they opened, each with its own
/// unified diff (`Accordion multiple`).
#[derive(Default)]
pub struct NarrowReview {
    open: Vec<(String, DiffDocState)>,
}

impl NarrowReview {
    /// The open files' paths, in the order they opened.
    pub fn open_paths(&self) -> Vec<String> {
        self.open.iter().map(|(p, _)| p.clone()).collect()
    }

    pub fn is_open(&self, path: &str) -> bool {
        self.open.iter().any(|(p, _)| p == path)
    }

    /// Opens `path` shut, or shuts it open.
    pub fn toggle(&mut self, files: &[FileDiff], path: &str) {
        if self.is_open(path) {
            self.open.retain(|(p, _)| p != path);
            return;
        }
        if let Some(f) = files.iter().find(|f| f.path() == path) {
            self.open.push((path.to_owned(), DiffDocState::inline(f.clone())));
        }
    }

    /// Expand all opens every file that has lines; Collapse all, shown while any is open, shuts
    /// them all (`handleExpandOrCollapseAll`).
    pub fn toggle_all(&mut self, files: &[FileDiff], meta: &[super::data::ReviewFileMeta]) {
        if !self.open.is_empty() {
            self.open.clear();
            return;
        }
        for m in meta.iter().filter(|m| m.additions + m.deletions > 0) {
            self.toggle(files, &m.path);
        }
    }

    /// Takes the session's newly loaded diffs: open files keep open with their new lines, gone
    /// ones shut.
    pub fn refresh(&mut self, files: &[FileDiff]) {
        let paths = self.open_paths();
        *self = Self::from_paths(&paths, files);
    }

    pub fn from_paths(paths: &[String], files: &[FileDiff]) -> Self {
        let open = paths
            .iter()
            .filter_map(|path| {
                files
                    .iter()
                    .find(|f| f.path() == path)
                    .map(|f| (path.clone(), DiffDocState::inline(f.clone())))
            })
            .collect();
        Self { open }
    }

    pub fn prepare_paths(paths: &[String], files: &[FileDiff]) -> Vec<(String, crate::code_view::diff_state::PreparedDiffDoc)> {
        paths
            .iter()
            .filter_map(|path| {
                files
                    .iter()
                    .find(|f| f.path() == path)
                    .map(|f| (path.clone(), DiffDocState::inline_prepared(f.clone())))
            })
            .collect()
    }

    pub fn from_prepared(open: Vec<(String, crate::code_view::diff_state::PreparedDiffDoc)>) -> Self {
        Self {
            open: open.into_iter().map(|(p, d)| (p, d.into_state())).collect(),
        }
    }

    pub fn into_prepared(self) -> Vec<(String, crate::code_view::diff_state::PreparedDiffDoc)> {
        self.open.into_iter().map(|(p, d)| (p, d.into_prepared())).collect()
    }

    /// Settings › Wrap lines: the open diffs wrap long lines or scroll them.
    /// Whether `set_wrap(wrap)` would change an open diff.
    pub fn wraps_otherwise(&self, wrap: bool) -> bool {
        self.open.iter().any(|(_, d)| d.wrap() != wrap)
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        for (_, d) in &mut self.open {
            d.set_wrap(wrap);
        }
    }

    pub(crate) fn diff(&self, path: &str) -> Option<&DiffDocState> {
        self.open.iter().find(|(p, _)| p == path).map(|(_, d)| d)
    }

    pub(crate) fn diff_mut(&mut self, path: &str) -> Option<&mut DiffDocState> {
        self.open.iter_mut().find(|(p, _)| p == path).map(|(_, d)| d)
    }
}

/// `DiffChanges` standard: the mono face at 14 / 21, regular.
const COUNT: TextStyle = TextStyle::new(14.0, 440.0).mono().lh(21.0);
/// `button-v2` small: 13 / 20 at 530.
const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0);
/// An accordion trigger: `font-size-small` at 440 on `line-height-large`.
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(19.5);

/// `chevron-grabber-vertical`.
static GRABBER: Icon = Icon {
    name: "chevron-grabber-vertical",
    view: 20.0,
    body: r#"<path d="M6.66675 12.4998L10.0001 15.8332L13.3334 12.4998M6.66675 7.49984L10.0001 4.1665L13.3334 7.49984" stroke="currentColor" stroke-linecap="square" fill="none"/>"#,
};

/// The narrow Changes view of session `id`, `title` (the plugin's source picker) at its header's start.
pub fn view(s: &App, p: &Pal, id: &str, title: El) -> El {
    let empty = SessionReview::default();
    let sr = s.review.sessions.get(id).unwrap_or(&empty);
    let (adds, dels) = sr.totals();
    let has = !sr.meta.is_empty();
    col()
        .key("narrow-review")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(p.bg_base)
        .child(header(p, id, sr, has, adds, dels, title))
        .child(if has { list(s, p, id, sr) } else { empty_state(p, sr) })
}

/// `session-review-header` (`!px-2 !h-10 !pb-0`, a 1 px `border-base` rule along its foot):
/// three columns, the middle one the totals.
fn header(p: &Pal, id: &str, sr: &SessionReview, has: bool, adds: u32, dels: u32, scope: El) -> El {
    let side = |e: El| row().grow(1.0).w(0.0).min_w(0.0).center_y().child(e);
    let toggle = has.then(|| {
        let sid = id.to_owned();
        let label = if sr.narrow.open.is_empty() { t!("ui.sessionReview.expandAll") } else { t!("ui.sessionReview.collapseAll") };
        row()
            .key("narrow-review-expand")
            .role("button")
            .label(label)
            .h(24.0)
            .px(9.0)
            .gap(6.0)
            .center_y()
            .radius(4.0)
            .fill(p.q.neutral)
            .elevate(neutral_ring(p))
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                if let Some(sr) = s.review.sessions.get_mut(&sid) {
                    sr.narrow.toggle_all(sr.diffs.files(), &sr.meta);
                }
            })
            .child(icon(&GRABBER, 16.0).color(p.icon))
            .child(text(label).font(BUTTON).color(p.text))
    });
    row()
        .key("narrow-review-header")
        .w_full()
        .h(40.0)
        .min_h(40.0)
        .px(8.0)
        .center_y()
        .child(el().key("rule").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(p.border))
        .child(side(row().key("title").child(scope)))
        .children(has.then(|| counts(p, adds, dels)))
        .child(side(row().w_full().justify_end().children(toggle)))
}

/// `button-v2` neutral's ring and drop (`--v2-elevation-button`'s two shadows).
fn neutral_ring(p: &Pal) -> [rune::Shadow; 4] {
    let ring = rune::Shadow { spread: 0.5, color: p.q.neutral_ring, ..rune::Shadow::default() };
    let drop = rune::Shadow { y: 1.0, blur: 2.0, color: p.q.neutral_shadow, ..rune::Shadow::default() };
    [ring, drop, rune::Shadow::default(), rune::Shadow::default()]
}

/// `DiffChanges` standard: `+N` and `-M`, 8 apart, in the diff's colours.
fn counts(p: &Pal, adds: u32, dels: u32) -> El {
    row()
        .key("counts")
        .gap(8.0)
        .center_y()
        // `--text-diff-add-base` and `-delete-base`, which Desktop's theme points at `state-fg-*`.
        .child(text(format!("+{adds}")).dir(rune::Direction::Ltr).font(COUNT).color(p.success))
        .child(text(format!("-{dels}")).dir(rune::Direction::Ltr).font(COUNT).color(p.danger))
}

fn empty_state(p: &Pal, sr: &SessionReview) -> El {
    let msg = match () {
        _ if !sr.load.ready() => t!("plugin.review.loadingChanges"),
        _ if sr.load.no_git() => t!("rune.review.noGit"),
        _ => sr.mode.empty_label(),
    };
    col().key("narrow-review-empty").w_full().px(8.0).py(8.0).child(text(msg).font(crate::view::UI).color(p.faint))
}

/// The accordion, edge to edge: 1 px `border-base` rules between rows, none at its ends. Files
/// keep the server's order, as Desktop's narrow list does.
fn list(s: &App, p: &Pal, id: &str, sr: &SessionReview) -> El {
    let n = sr.meta.len();
    let mut listed: Vec<_> = sr.meta.iter().collect();
    listed.sort_by_key(|m| m.listed);
    let items = listed.into_iter().enumerate().fold(col().key("narrow-review-list").w_full(), |list, (i, m)| {
        let open = sr.narrow.is_open(&m.path);
        let item = col()
            .key(format!("narrow-review-item-{}", m.path))
            .w_full()
            .child(trigger(p, id, m, open, i + 1 == n && !open));
        let item = if open { item.child(summary(p, m)).child(diff(s, p, id, &m.path, i + 1 == n)) } else { item };
        list.child(item)
    });
    let key = format!("narrow-review-scroll-{id}");
    let bar = crate::code_view::scroll_bar(p);
    let items = items.pr(crate::code_view::scroll_gutter(&bar, &key, &format!("{key}/narrow-review-list")));
    col().key(key).w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(items).scroll_bar_styled(bar)
}

/// An accordion trigger: 32 tall, `8 12` in, the file's icon, its folder in `text-base` (v1)
/// and its name in `text-strong`, 12 apart; the chevron at its end turns down while open.
fn trigger(p: &Pal, id: &str, m: &super::data::ReviewFileMeta, open: bool, last: bool) -> El {
    let (dir, name) = opencode_kit::path::split(&m.path);
    let can = m.additions + m.deletions > 0;
    let (sid, path) = (id.to_owned(), m.path.clone());
    let mut r = row()
        .key("trigger")
        .role("button")
        .label(m.path.clone())
        .expanded(open)
        .w_full()
        .h(32.0)
        .px(12.0)
        .gap(20.0)
        .center_y()
        .fill(p.bg_base)
        .border_sides([0.0, 0.0, if last { 0.0 } else { 1.0 }, 0.0], p.border)
        // `simple-overlay-hover` under the pointer, eased in and out over 0.15 s
        // (`accordion.css`'s `transition: background-color 0.15s ease`). Desktop's selected file
        // (`session-review.css:53-57`) never fills: Kobalte's item overwrites the
        // `session-review-accordion-item` slot that rule needs.
        .hover(p.hover)
        .hover_motion(session_view::style::HOVER_FADE, session_view::style::HOVER_FADE)
        .child(
            row()
                .key("file")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(12.0)
                .center_y()
                .child(icon(rune_theme_opencode::file_icons::file(&m.path), 16.0))
                .child(
                    row()
                        .key("name")
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .center_y()
                        .children((!dir.is_empty()).then(|| text(dir.to_owned()).font(ROW).color(p.v1_text_base).shrink().min_w(0.0).truncate_head()))
                        .child(text(name.to_owned()).font(ROW).color(p.standard.text).truncate()),
                ),
        );
    if can {
        r = r
            // `session-review-diff-chevron`: chevron-down at small (14) in icon-weaker, turned
            // -90deg while shut (`session-review.css:163-172`); the press lands at once.
            .child(
                icon(&ic::CHEVRON_DOWN, 14.0)
                    .key("chevron")
                    .color(p.icon_faint)
                    .icon_rotation(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 }),
            )
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                if let Some(sr) = s.review.sessions.get_mut(&sid) {
                    sr.narrow.toggle(sr.diffs.files(), &path);
                }
            });
    }
    r
}

/// `session-review-change-summary`: `8 12` in, the file's `+N -M` (Added or Removed for a new or
/// deleted file), and Open file at its end; a 0.5 pt `border-base` rule under it.
fn summary(p: &Pal, m: &super::data::ReviewFileMeta) -> El {
    use rune_kit::diff::FileStatus;
    const CHANGE: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
    let lead = match m.status {
        FileStatus::Added => row().gap(12.0).center_y().child(text(t!("ui.sessionReview.change.added")).font(CHANGE).color(p.success)).child(counts(p, m.additions, m.deletions)),
        FileStatus::Deleted => row().gap(12.0).center_y().child(text(t!("ui.sessionReview.change.removed")).font(CHANGE).color(p.danger)).child(counts(p, m.additions, m.deletions)),
        _ => counts(p, m.additions, m.deletions),
    };
    let path = m.path.clone();
    let open = row()
        .key("open-file")
        .role("button")
        .label_static(t!("ui.sessionReview.openFile"))
        .h(24.0)
        .px(9.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
            s.open_link(&sid, None, &path, tx);
        })
        .child(icon(&ic::OPEN_FILE, 16.0).color(p.icon))
        .child(text(t!("ui.sessionReview.openFile")).font(BUTTON).color(p.text));
    row()
        .key("summary")
        .w_full()
        .min_h(32.0)
        .py(8.0)
        .px(12.0)
        .gap(12.0)
        .center_y()
        .fill(p.bg_base)
        .border_sides([0.0, 0.0, 0.5, 0.0], p.border)
        .border_layout()
        .child(lead)
        .child(el().grow(1.0))
        .child(open)
}

/// The file's unified diff, on `layer-01` (`accordion-content`).
fn diff(s: &App, p: &Pal, id: &str, path: &str, last: bool) -> El {
    let (get_sid, get_path, mut_sid, mut_path) = (id.to_owned(), path.to_owned(), id.to_owned(), path.to_owned());
    // Wrapped (Settings › Wrap lines), the diff is as tall as its rows come out; unwrapped, it
    // scrolls sideways at the height of its rows one under another.
    let doc = s.review.sessions.get(id).and_then(|sr| sr.narrow.diff(path));
    let body = doc.map(|d| {
        let view = DiffViewer::new(
            format!("narrow-review-diff-{}", path.replace(['/', '\\', ':'], "_")),
            move |s: &App| s.review.sessions.get(&get_sid).and_then(|sr| sr.narrow.diff(&get_path)).unwrap_or(&s.review.empty_diffs),
            move |s: &mut App| {
                let review = &mut *s.review;
                match review.sessions.get_mut(&mut_sid).and_then(|sr| sr.narrow.diff_mut(&mut_path)) {
                    Some(d) => d,
                    None => &mut review.empty_diffs,
                }
            },
        )
        .sticky_headers(false)
        .whole(true)
        .notes(crate::line_comments::host::Notes::new(id, crate::line_comments::Surface::Narrow))
        .build(s, p);
        if d.wraps() { view } else { view.h(d.rows_height()) }
    });
    col()
        .key("content")
        .w_full()
        .fill(p.layer1)
        .border_sides([0.0, 0.0, if last { 0.0 } else { 1.0 }, 0.0], if last { Color::TRANSPARENT } else { p.border })
        .children(body)
}
