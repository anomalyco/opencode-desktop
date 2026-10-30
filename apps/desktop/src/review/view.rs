//! UI for the Changes / Review panel (`SessionReviewV2` + `SessionReviewV2Sidebar`).

use std::borrow::Cow;

use rune::{
    Anim, Color, Cursor, Drag, El, GrowthSpec, Icon, Phase, ScrollSpec, TextEdit, TextStyle, Tx,
    col, el, icon, row, text, text_input,
};
use rune_kit::diff::{DiffMode, Expand, FileStatus};
use rune_kit::file_icon;
use rune_theme_opencode::icons as ic;

use super::data::{ReviewFileMeta, filter_review_files};
use super::{
    SIDEBAR_DEFAULT_W, SIDEBAR_MAX_W, SIDEBAR_MIN_W, SessionReview,
};
use crate::App;
use crate::code_view::{DiffViewer, ExpandMode};
use crate::palette::Pal;
use crate::panels::PanelViewCtx;
use crate::view::{UI, UI_MED, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;
const TREE_ROW_H: f32 = 26.0;
const LIST_ROW_H: f32 = 28.0;

const MONO_COUNT: TextStyle = TextStyle::new(11.0, 450.0).mono().lh(15.0).features("'tnum' 1");

static ARROW_LEFT: Icon = Icon {
    name: "review-arrow-left",
    view: 16.0,
    body: r#"<path d="M9.75 3.75L5.5 8L9.75 12.25" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static ARROW_RIGHT: Icon = Icon {
    name: "review-arrow-right",
    view: 16.0,
    body: r#"<path d="M6.25 3.75L10.5 8L6.25 12.25" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static UNIFIED_ICON: Icon = Icon {
    name: "review-unified",
    view: 16.0,
    body: r#"<rect x="2.5" y="2.5" width="11" height="11" rx="2" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M5 6H11M5 8.5H11M5 11H9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};
static SPLIT_ICON: Icon = Icon {
    name: "review-split",
    view: 16.0,
    body: r#"<rect x="2.25" y="2.5" width="11.5" height="11" rx="2" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M8 2.5V13.5" stroke="currentColor" stroke-width="1.2"/>"#,
};
static EXPAND_LINES: Icon = Icon {
    name: "review-expand-lines",
    view: 16.0,
    body: r#"<path d="M5.25 5.5L8 2.75L10.75 5.5M5.25 10.5L8 13.25L10.75 10.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static COLLAPSE_LINES: Icon = Icon {
    name: "review-collapse-lines",
    view: 16.0,
    body: r#"<path d="M5.25 3.25L8 6L10.75 3.25M5.25 12.75L8 10L10.75 12.75" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

pub fn render(s: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
    // Stay idle until the side region has been opened at least once (`review/renderer.tsx:175`).
    if !ctx.visible && !s.side.mounted && s.stage.is_none() {
        return col().key("review-idle").w_full().grow(1.0);
    }

    let sid = ctx.session_id.to_owned();
    let empty = SessionReview::default();
    let sr = s.review.sessions.get(&sid).unwrap_or(&empty);
    let has_diffs = !sr.meta.is_empty();
    let sidebar_open = s.review.sidebar_open;
    let max_sb_w = (ctx.pane_size.0 * 0.52).clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W);
    let sb_w = if s.review.sidebar_resized {
        s.review.sidebar_w.clamp(SIDEBAR_MIN_W, max_sb_w)
    } else {
        (ctx.pane_size.0 * 0.42).clamp(SIDEBAR_MIN_W, SIDEBAR_DEFAULT_W)
    };

    let mut body_row = row().key("review-body").w_full().grow(1.0).min_h(0.0);
    if sidebar_open {
        body_row = body_row
            .child(review_sidebar(s, p, &sid, sr, sb_w))
            .child(sidebar_resize_handle(s, p, sb_w, max_sb_w));
    }
    body_row = body_row.child(review_main(s, p, &sid, sr, has_diffs, sidebar_open));

    let mut root = col()
        .key("review-panel")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(p.bg_base)
        .child(body_row);

    if s.review.mode_menu_open {
        root = root.child(mode_popover(p, sr));
    }
    root
}

fn diff_counts_badge(p: &Pal, adds: u32, dels: u32) -> El {
    let mut r = row().key("stats").gap(4.0).center_y();
    if adds > 0 {
        r = r.child(text(format!("+{adds}")).font(MONO_COUNT).color(p.success));
    }
    if dels > 0 {
        r = r.child(text(format!("−{dels}")).font(MONO_COUNT).color(p.danger));
    }
    r
}

fn mode_button(p: &Pal, sr: &SessionReview) -> El {
    let modes = sr.available_modes();
    let label = sr.mode.label();
    if modes.len() <= 1 {
        return row()
            .key("review-mode-static")
            .h(24.0)
            .px(6.0)
            .center_y()
            .child(text(label).font(UI_MED).color(p.text).truncate_fade());
    }
    row()
        .key("review-mode-btn")
        .h(24.0)
        .px(6.0)
        .gap(4.0)
        .center_y()
        .radius(5.0)
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .cursor(Cursor::Pointer)
        .role("button")
        .label("Change review scope")
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.review.mode_menu_open = !s.review.mode_menu_open;
        })
        .child(text(label).font(UI_MED).color(p.text).truncate_fade())
        .child(icon(&ic::CHEVRON_DOWN, 11.0).color(p.icon_muted))
}

fn mode_popover(p: &Pal, sr: &SessionReview) -> El {
    let modes = sr.available_modes();
    let cur = sr.mode;
    let mut card = col()
        .key("review-mode-menu")
        .pin(Some(10.0), Some(36.0), None, None)
        .w(164.0)
        .padding(4.0)
        .gap(1.0)
        .radius(8.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(2.0, 8.0, p.shadow_far)
        .shadow(1.0, 2.0, p.shadow);
    for m in modes {
        let on = m == cur;
        card = card.child(
            row()
                .key(format!("review-mode-opt-{m:?}"))
                .w_full()
                .h(28.0)
                .px(8.0)
                .gap(6.0)
                .center_y()
                .radius(5.0)
                .fill(if on { p.row_active } else { Color::TRANSPARENT })
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .cursor(Cursor::Pointer)
                .role("menuitem")
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.set_review_change_mode(m);
                })
                .child(text(m.label()).font(UI_SMALL).color(p.text).grow(1.0).truncate_fade())
                .children(on.then(|| icon(&ic::CHECK_SMALL, 12.0).color(p.icon))),
        );
    }
    el().key("review-mode-scrim")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.review.mode_menu_open = false;
        })
        .child(card)
}

fn review_sidebar(s: &App, p: &Pal, session_id: &str, sr: &SessionReview, width: f32) -> El {
    let (adds, dels) = sr.totals();
    let header = row()
        .key("review-sidebar-header")
        .w_full()
        .h(38.0)
        .pl(6.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .child(mode_button(p, sr).shrink().min_w(0.0))
        .child(el().grow(1.0))
        .child(diff_counts_badge(p, adds, dels));

    let has_query = !s.review.filter.text.is_empty();
    let filter_box = row()
        .key("review-filter-box")
        .w_full()
        .px(8.0)
        .pb(6.0)
        .child(
            row()
                .key("review-filter-inner")
                .w_full()
                .h(26.0)
                .px(7.0)
                .gap(5.0)
                .center_y()
                .radius(6.0)
                .clip_rounded(6.0)
                .fill(p.bg_deep)
                .border(0.5, if s.review.filter.focused { p.border } else { p.border_muted })
                .motion(rune::Spring::NONE)
                .child(icon(&ic::MAGNIFYING_GLASS, 12.0).color(p.faint))
                .child(
                    text_input(&s.review.filter, UI_SMALL)
                        .key("review-filter")
                        .placeholder("Filter files…", p.faint)
                        .color(p.text)
                        .grow(1.0)
                        .shrink()
                        .on_edit(|s: &mut App| &mut s.review.filter)
                        .on_submit(|s: &mut App, tx: &mut Tx| s.submit_review_filter(tx)),
                )
                .children(has_query.then(|| {
                    el().key("review-filter-clear")
                        .size(16.0, 16.0)
                        .radius(4.0)
                        .center()
                        .cursor(Cursor::Pointer)
                        .hover(p.hover)
                        .hover_motion(INSTANT, INSTANT)
                        .role("button")
                        .label("Clear filter")
                        .on_press_tx(|s: &mut App, tx: &mut Tx| {
                            tx.instant();
                            let f = s.review.filter.focused;
                            s.review.filter = TextEdit::new("");
                            s.review.filter.focused = f;
                        })
                        .child(icon(&ic::XMARK_SMALL, 11.0).color(p.icon_muted))
                })),
        );

    let list_area = if !sr.ready {
        col()
            .key("review-tree-loading")
            .w_full()
            .grow(1.0)
            .px(10.0)
            .pt(8.0)
            .child(text("Loading…").font(UI_SMALL).color(p.muted))
    } else if s.review.filter.text.trim().is_empty() {
        change_tree_view(p, session_id, sr)
    } else {
        filtered_list_view(s, p, session_id, sr)
    };

    col()
        .key("review-sidebar")
        .w(width)
        .h_full()
        .fill(p.bg_base)
        .child(header)
        .child(filter_box)
        .child(list_area)
}

fn change_tree_view(p: &Pal, session_id: &str, sr: &SessionReview) -> El {
    let count = sr.tree.visible_len();
    let sid_keys = session_id.to_owned();
    let sid_rows = session_id.to_owned();
    let pal = *p;
    let list = col()
        .key("review-tree-rows")
        .w_full()
        .px(4.0)
        .pb(8.0)
        .growth(GrowthSpec::default())
        .role("tree")
        .label("Changed files")
        .virtual_rows(
            count,
            TREE_ROW_H,
            move |s: &App, i| {
                s.review
                    .sessions
                    .get(&sid_keys)
                    .and_then(|r| r.tree.row(i).map(|(_, tr)| Cow::Owned(format!("rt:{}", tr.id))))
                    .unwrap_or(Cow::Borrowed("rt:empty"))
            },
            move |s: &App, i| {
                let Some(r) = s.review.sessions.get(&sid_rows) else {
                    return el().h(TREE_ROW_H);
                };
                let Some((node, tr)) = r.tree.row(i) else {
                    return el().h(TREE_ROW_H);
                };
                tree_row_el(&pal, node.name.as_str(), node.dir, node.data.as_ref(), tr.id, tr.depth, tr.expanded, r.active_path.as_deref() == Some(tr.id))
            },
        );
    col()
        .key("review-tree")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(list)
}

#[allow(clippy::too_many_arguments)]
fn tree_row_el(
    p: &Pal,
    name: &str,
    is_dir: bool,
    meta: Option<&ReviewFileMeta>,
    node_id: &str,
    depth: u32,
    expanded: Option<bool>,
    active: bool,
) -> El {
    let id_owned = node_id.to_owned();
    let indent = 6.0 + depth as f32 * 12.0;
    let mut r = row()
        .w_full()
        .h(TREE_ROW_H)
        .pl(indent)
        .pr(6.0)
        .gap(4.0)
        .center_y()
        .radius(5.0)
        .fill(if active { p.row_active } else { Color::TRANSPARENT })
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .cursor(Cursor::Pointer)
        .role("treeitem")
        .label(name.to_owned())
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if is_dir {
                if let Some(sid) = s.active_session_id().map(str::to_owned)
                    && let Some(sr) = s.review.sessions.get_mut(&sid)
                {
                    sr.tree.toggle(&id_owned);
                }
            } else {
                s.focus_review_file(&id_owned, tx);
            }
        });

    let chev = match expanded {
        Some(true) => el().size(14.0, 14.0).center().child(icon(&ic::CHEVRON_DOWN, 11.0).color(p.icon_muted)),
        Some(false) => el().size(14.0, 14.0).center().child(icon(&ic::CHEVRON_RIGHT, 11.0).color(p.icon_muted)),
        None => el().size(14.0, 14.0),
    };
    let file_ic = if is_dir { &ic::FOLDER } else { file_icon(name) };
    r = r
        .child(chev)
        .child(icon(file_ic, 14.0).color(if active { p.icon } else { p.icon_muted }))
        .child(
            text(name.to_owned())
                .font(if active { UI_MED } else { UI_SMALL })
                .color(if active { p.text } else { p.muted })
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate_fade(),
        );

    if let Some(m) = meta {
        match m.status {
            FileStatus::Added => {
                r = r.child(text("A").font(MONO_COUNT).color(p.success));
            }
            FileStatus::Deleted => {
                r = r.child(text("D").font(MONO_COUNT).color(p.danger));
            }
            _ => {}
        }
        if m.additions > 0 || m.deletions > 0 {
            r = r.child(diff_counts_badge(p, m.additions, m.deletions));
        }
    }
    crate::open_in::context(r, node_id.to_owned())
}

fn filtered_list_view(s: &App, p: &Pal, session_id: &str, sr: &SessionReview) -> El {
    let all_paths: Vec<String> = sr.meta.iter().map(|m| m.path.clone()).collect();
    let filtered = filter_review_files(&all_paths, &s.review.filter.text);
    if filtered.is_empty() {
        return col()
            .key("review-filter-empty")
            .w_full()
            .grow(1.0)
            .px(10.0)
            .pt(8.0)
            .child(text("No matching files").font(UI_SMALL).color(p.muted));
    }
    let count = filtered.len();
    let paths_keys = filtered.clone();
    let sid_rows = session_id.to_owned();
    let pal = *p;
    let list = col()
        .key("review-filter-rows")
        .w_full()
        .px(4.0)
        .pb(8.0)
        .growth(GrowthSpec::default())
        .role("listbox")
        .virtual_rows(
            count,
            LIST_ROW_H,
            move |_s: &App, i| Cow::Owned(format!("rf:{}", paths_keys[i])),
            move |s: &App, i| {
                let path = &filtered[i];
                let r = s.review.sessions.get(&sid_rows);
                let meta = r.and_then(|sr| sr.meta.iter().find(|m| &m.path == path));
                let active = r.and_then(|sr| sr.active_path.as_deref()) == Some(path.as_str());
                let highlighted = r
                    .and_then(|sr| sr.highlighted_filter_path.as_deref())
                    .map_or(i == 0, |hp| hp == path);
                let (dir, name) = match path.rsplit_once('/') {
                    Some((d, b)) => (Some(d), b),
                    None => (None, path.as_str()),
                };
                let path_click = path.clone();
                let mut row_el = row()
                    .w_full()
                    .h(LIST_ROW_H)
                    .px(6.0)
                    .gap(5.0)
                    .center_y()
                    .radius(5.0)
                    .fill(if active || highlighted { pal.row_active } else { Color::TRANSPARENT })
                    .hover(pal.hover)
                    .hover_motion(INSTANT, INSTANT)
                    .cursor(Cursor::Pointer)
                    .role("option")
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        s.focus_review_file(&path_click, tx);
                    })
                    .child(icon(file_icon(name), 14.0).color(pal.icon_muted))
                    .child(
                        text(name.to_owned())
                            .font(UI_SMALL)
                            .color(pal.text)
                            .shrink()
                            .min_w(0.0)
                            .truncate_fade(),
                    );
                if let Some(d) = dir {
                    row_el = row_el.child(
                        text(d.to_owned())
                            .font(UI_SMALL)
                            .color(pal.faint)
                            .grow(1.0)
                            .shrink()
                            .min_w(0.0)
                            .truncate_fade(),
                    );
                } else {
                    row_el = row_el.child(el().grow(1.0));
                }
                if let Some(m) = meta {
                    row_el = row_el.child(diff_counts_badge(&pal, m.additions, m.deletions));
                }
                crate::open_in::context(row_el, path.clone())
            },
        );
    col()
        .key("review-filter-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(list)
}

fn sidebar_resize_handle(s: &App, p: &Pal, cur_w: f32, max_w: f32) -> El {
    let dragging = s.review.sidebar_drag.is_some();
    el().key("review-sidebar-resize")
        .w(5.0)
        .h_full()
        .center()
        .cursor(Cursor::ResizeColumn)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_double_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.review.sidebar_resized = false;
            s.review.sidebar_w = SIDEBAR_DEFAULT_W;
        })
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            match d.phase {
                Phase::Down => s.review.sidebar_drag = Some(cur_w),
                Phase::Move => {
                    if let Some(orig) = s.review.sidebar_drag {
                        s.review.sidebar_resized = true;
                        s.review.sidebar_w = (orig + d.dx / s.zoom).clamp(SIDEBAR_MIN_W, max_w);
                    }
                }
                Phase::Up => {
                    if d.cancelled()
                        && let Some(orig) = s.review.sidebar_drag
                    {
                        s.review.sidebar_w = orig;
                    }
                    s.review.sidebar_drag = None;
                }
            }
        })
        .child(el().w(0.5).h_full().fill(if dragging { p.border } else { p.border_muted }))
}

fn seg_icon_btn(
    p: &Pal,
    key: &'static str,
    glyph: &'static Icon,
    active: bool,
    help_text: &'static str,
    on_press: impl Fn(&mut App, &mut Tx) + 'static,
) -> El {
    el().key(key)
        .size(24.0, 22.0)
        .radius(4.0)
        .center()
        .cursor(Cursor::Pointer)
        .fill(if active { p.bg_base } else { Color::TRANSPARENT })
        .hover(if active { p.bg_base } else { p.hover })
        .hover_motion(INSTANT, INSTANT)
        .role("button")
        .label(help_text)
        .help(help_text)
        .on_press_tx(on_press)
        .child(icon(glyph, 13.0).color(if active { p.icon } else { p.icon_muted }))
}

fn review_main(
    s: &App,
    p: &Pal,
    session_id: &str,
    sr: &SessionReview,
    has_diffs: bool,
    sidebar_open: bool,
) -> El {
    let total = sr.meta.len();
    let cur_idx = sr
        .active_path
        .as_deref()
        .and_then(|ap| sr.meta.iter().position(|m| m.path == ap))
        .unwrap_or(0);
    let (adds, dels) = sr.totals();

    let mut start = row().key("review-toolbar-start").gap(4.0).center_y().shrink().min_w(0.0);
    start = start.child(
        el().key("review-tree-toggle")
            .size(26.0, 26.0)
            .radius(6.0)
            .center()
            .cursor(Cursor::Pointer)
            .fill(if sidebar_open { p.row_active } else { Color::TRANSPARENT })
            .hover(p.hover)
            .hover_motion(INSTANT, INSTANT)
            .role("button")
            .label("Toggle file tree")
            .help("Toggle file tree ⌘\\")
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.review.sidebar_open = !s.review.sidebar_open;
            })
            .child(icon(&ic::FILE_TREE, 14.0).color(if sidebar_open { p.icon } else { p.icon_muted })),
    );

    if !sidebar_open {
        start = start
            .child(mode_button(p, sr).shrink().min_w(0.0))
            .child(diff_counts_badge(p, adds, dels));
        if total > 0 {
            start = start.child(
                text(format!("{}/{total}", cur_idx + 1))
                    .font(MONO_COUNT)
                    .color(p.faint),
            );
        }
    }

    let nav_btn = |key: &'static str, glyph: &'static Icon, help_lbl: &'static str, forward: bool| {
        let mut b = el()
            .key(key)
            .size(24.0, 24.0)
            .radius(5.0)
            .center()
            .role("button")
            .label(help_lbl)
            .help(help_lbl)
            .child(icon(glyph, 13.0).color(p.icon_muted));
        if has_diffs {
            let sid = session_id.to_owned();
            b = b
                .cursor(Cursor::Pointer)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    if let Some(r) = s.review.sessions.get_mut(&sid) {
                        let _ = r.diffs.next_file(forward, tx);
                    }
                    s.sync_review_tree_to_diff(&sid, tx);
                });
        } else {
            b = b.dim(0.35).inert();
        }
        b
    };

    start = start
        .child(nav_btn("review-prev-file", &ARROW_LEFT, "Previous file ←", false))
        .child(nav_btn("review-next-file", &ARROW_RIGHT, "Next file →", true));

    let exp_mode = s.review.expand_mode;
    let expand_seg = row()
        .key("review-expand-seg")
        .h(26.0)
        .padding(2.0)
        .gap(1.0)
        .center_y()
        .radius(6.0)
        .fill(p.layer1)
        .border(0.5, p.border_muted)
        .child(seg_icon_btn(
            p,
            "review-expand-all",
            &EXPAND_LINES,
            exp_mode == ExpandMode::Expand,
            "Show all lines",
            |s, tx| {
                tx.instant();
                s.set_review_expand_mode(ExpandMode::Expand);
            },
        ))
        .child(seg_icon_btn(
            p,
            "review-collapse-unchanged",
            &COLLAPSE_LINES,
            exp_mode == ExpandMode::Collapse,
            "Hide unchanged lines",
            |s, tx| {
                tx.instant();
                s.set_review_expand_mode(ExpandMode::Collapse);
            },
        ));

    let diff_mode = s.review.diff_mode;
    let style_seg = row()
        .key("review-style-seg")
        .h(26.0)
        .padding(2.0)
        .gap(1.0)
        .center_y()
        .radius(6.0)
        .fill(p.layer1)
        .border(0.5, p.border_muted)
        .child(seg_icon_btn(
            p,
            "review-style-unified",
            &UNIFIED_ICON,
            diff_mode == DiffMode::Unified,
            "Unified diff",
            |s, tx| {
                tx.instant();
                s.set_review_diff_mode(DiffMode::Unified);
            },
        ))
        .child(seg_icon_btn(
            p,
            "review-style-split",
            &SPLIT_ICON,
            diff_mode == DiffMode::Split,
            "Split diff",
            |s, tx| {
                tx.instant();
                s.set_review_diff_mode(DiffMode::Split);
            },
        ));

    let toolbar = row()
        .key("review-toolbar")
        .w_full()
        .h(38.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .fill(p.bg_base)
        .child(start)
        .child(el().grow(1.0))
        .child(expand_seg)
        .child(style_seg)
        .children(sr.active_path.as_deref().and_then(|ap| crate::open_in::button(s, p, "review-open-in", ap)));

    let body = if !sr.ready {
        col()
            .key("review-loading")
            .w_full()
            .grow(1.0)
            .center()
            .child(text("Loading changes…").font(UI).color(p.muted))
    } else if !has_diffs {
        let msg = if sr.no_git {
            "Not a Git repository"
        } else {
            sr.mode.empty_label()
        };
        col()
            .key("review-empty")
            .w_full()
            .grow(1.0)
            .center()
            .padding(24.0)
            .child(text(msg).font(UI).color(p.muted))
    } else {
        let sid_get = session_id.to_owned();
        let sid_mut = session_id.to_owned();
        let sid_vis = session_id.to_owned();
        DiffViewer::new(
            format!("review-diff-{session_id}"),
            move |s: &App| {
                s.review
                    .sessions
                    .get(&sid_get)
                    .map_or(&s.review.empty_diffs, |r| &r.diffs)
            },
            move |s: &mut App| {
                if s.review.sessions.contains_key(&sid_mut) {
                    &mut s.review.sessions.get_mut(&sid_mut).unwrap().diffs
                } else {
                    &mut s.review.empty_diffs
                }
            },
        )
        .on_visible_file(move |s: &mut App, f| {
            if let Some(r) = s.review.sessions.get_mut(&sid_vis)
                && let Some(fd) = r.diffs.files().get(f)
            {
                let path = fd.path().to_owned();
                r.active_path = Some(path.clone());
                r.tree.select(&path);
            }
        })
        .on_request_expand(|s: &mut App, f: usize, gap: usize, how: Expand| {
            s.request_review_gap_expand(f, gap, how);
        })
        .build(s, p)
    };

    col()
        .key("review-main")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(toolbar)
        .child(el().w_full().h(0.5).fill(p.border_muted))
        .child(body)
}
