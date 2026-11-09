//! UI for the Changes / Review panel (`SessionReviewV2` + `SessionReviewV2Sidebar`).

use std::borrow::Cow;

use rune::{
    Anim, Color, Cursor, Drag, El, GrowthSpec, Icon, Phase, ScrollSpec, TextEdit, TextStyle, Tx,
    col, el, icon, row, text,
};
use rune_kit::diff::{DiffMode, FileStatus};
use rune_theme_opencode::tree_icons;
use rune_theme_opencode::icons as ic;

use super::chrome;
use super::data::{ReviewFileMeta, filter_review_files};
use super::{SIDEBAR_DEFAULT_W, SIDEBAR_MAX_W, SIDEBAR_MIN_W, SessionReview};
use crate::App;
use crate::code_view::{DiffViewer, ExpandMode};
use crate::palette::Pal;
use crate::panels::PanelViewCtx;
use crate::view::{UI, UI_MED, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;
const TREE_ROW_H: f32 = chrome::ROW_H;
const LIST_ROW_H: f32 = 28.0;

const MONO_COUNT: TextStyle = TextStyle::new(11.0, 450.0).mono().lh(15.0).features("'tnum' 1");
/// The file's place in the list beside the collapsed title (`session-review-v2-file-position`).
const POSITION: TextStyle = TextStyle::new(11.0, 440.0).lh(13.0).features("'tnum' 1, 'lnum' 1");

// Desktop's `arrow-left`/`arrow-right`, `unified`, `split`, `expand` and `collapse` icons.
pub(crate) static ARROW_LEFT: Icon = Icon {
    name: "review-arrow-left",
    view: 20.0,
    body: r#"<path d="M8.33464 4.58398L2.91797 10.0007L8.33464 15.4173M3.33464 10.0007H17.0846" stroke="currentColor" stroke-linecap="square" fill="none"/>"#,
};
static ARROW_RIGHT: Icon = Icon {
    name: "review-arrow-right",
    view: 20.0,
    body: r#"<path d="M11.6654 4.58398L17.082 10.0007L11.6654 15.4173M16.6654 10.0007H2.91536" stroke="currentColor" stroke-linecap="square" fill="none"/>"#,
};
static UNIFIED_ICON: Icon = Icon {
    name: "review-unified",
    view: 16.0,
    body: r#"<path d="M3.00001 4.00045L12.9998 4L13 6.99955L3 7L3.00001 4.00045Z" fill="currentColor" fill-opacity="0.5"/><path d="M3.0001 9H13L12.9999 12H3L3.0001 9Z" fill="currentColor" fill-opacity="0.5"/><path d="M1 14H15L15 2H1V14Z" stroke="currentColor" fill="none"/>"#,
};
static SPLIT_ICON: Icon = Icon {
    name: "review-split",
    view: 16.0,
    body: r#"<path d="M1 14H15L15 2H1V14Z" stroke="currentColor" fill="none"/><rect x="3" y="4" width="4" height="8" fill="currentColor" fill-opacity="0.5"/><rect x="9" y="4" width="4" height="8" fill="currentColor" fill-opacity="0.5"/>"#,
};
static EXPAND_LINES: Icon = Icon {
    name: "review-expand-lines",
    view: 16.0,
    body: r#"<path d="M8.25 6.17773V1.17773M11.25 4.17773L8.25 1.17773L5.25 4.17773" stroke="currentColor" fill="none"/><path d="M8.25 9.17773V14.1777M11.25 11.1777L8.25 14.1777L5.25 11.1777" stroke="currentColor" fill="none"/><path d="M4.25 7.67773H12.25" stroke="currentColor"/>"#,
};
static COLLAPSE_LINES: Icon = Icon {
    name: "review-collapse-lines",
    view: 16.0,
    body: r#"<path d="M8 1V6M11 3L8 6L5 3" stroke="currentColor" fill="none"/><path d="M8 15V10M11 13L8 10L5 13" stroke="currentColor" fill="none"/><path d="M4 8H6M7 8H9M10 8H12" stroke="currentColor"/>"#,
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
    let max_sb_w = (ctx.pane_size.0 * 0.6).clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W);
    let sb_w = if s.review.sidebar_resized { s.review.sidebar_w } else { SIDEBAR_DEFAULT_W }.clamp(SIDEBAR_MIN_W, max_sb_w);

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
        .child(body_row)
        .child(chrome::sidebar_toggle(p, "review-tree-toggle", sidebar_open, false, |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.review.sidebar_open = !s.review.sidebar_open;
        }));

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
            .center_y()
            .child(text(label).font(UI_MED).color(p.text).truncate_fade());
    }
    row()
        .key("review-mode-btn")
        .h(24.0)
        .pr(4.0)
        .gap(4.0)
        .center_y()
        .radius(5.0)
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(opencode_i18n::t!("rune.review.scope"))
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.review.mode_menu_open = !s.review.mode_menu_open;
        })
        .child(text(label).font(UI_MED).color(p.text).truncate_fade())
        .child(icon(&ic::CHEVRON_DOWN, 11.0).color(p.icon_muted))
}

pub(super) fn mode_popover(p: &Pal, sr: &SessionReview) -> El {
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
        .box_shadow(Pal::ring_out(p.ring))
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
        .focusable(false)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.review.mode_menu_open = false;
        })
        .child(card)
}

fn review_sidebar(s: &App, p: &Pal, session_id: &str, sr: &SessionReview, width: f32) -> El {
    let (adds, dels) = sr.totals();
    let header = chrome::sidebar_header("review-sidebar-header", mode_button(p, sr), Some(diff_counts_badge(p, adds, dels)));
    let filter_box = chrome::filter_field(
        p,
        "review-filter",
        &s.review.filter,
        "review-filter-inner",
        |s: &mut App| &mut s.review.filter,
        |s: &mut App, tx: &mut Tx| s.submit_review_filter(tx),
        |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let f = s.review.filter.focused;
            s.review.filter = TextEdit::new("");
            s.review.filter.focused = f;
        },
        s.settings.spellcheck,
    );

    let list_area = if !sr.ready {
        col()
            .key("review-tree-loading")
            .w_full()
            .grow(1.0)
            .px(10.0)
            .pt(8.0)
            .child(text(opencode_i18n::t!("ui.sessionReview.image.loading")).font(UI_SMALL).color(p.muted))
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
        .child(list_area.pt(4.0))
}

fn change_tree_view(p: &Pal, session_id: &str, sr: &SessionReview) -> El {
    let count = sr.tree.visible_len();
    let sid_keys = session_id.to_owned();
    let sid_rows = session_id.to_owned();
    let pal = *p;
    let list = col()
        .key("review-tree-rows")
        .w_full()
        .px(8.0)
        .pb(12.0)
        .growth(GrowthSpec::default())
        .role("tree")
        .label(opencode_i18n::t!("settings.timeline.row.turnDiffs.title"))
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
    let glyph = (!is_dir).then(|| tree_icons::file(name));
    let r = chrome::tree_row(p, name, depth, expanded.filter(|_| is_dir), glyph, active)
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
    // Desktop's change tree marks only added and deleted files; a modified one is just its name.
    let r = r.children(meta.filter(|m| matches!(m.status, FileStatus::Added | FileStatus::Deleted)).map(|m| chrome::change_letter(p, m.status)));
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
            .child(text(opencode_i18n::t!("rune.files.noMatches")).font(UI_SMALL).color(p.muted));
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
                let (dir, name) = (Some(opencode_kit::path::parent(path)).filter(|d| !d.is_empty()), opencode_kit::path::basename(path));
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
                    .child({
                        let t = tree_icons::file(name);
                        icon(t.icon(), 16.0).color(if active || highlighted { t.color(pal.dark) } else { pal.icon_muted })
                    })
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
        .child(el().w(1.0).h_full().fill(if dragging { p.q.border_strong } else { p.border }))
}

/// A segmented control's icon item (`segmented-control-v2-item`): 40 × 28, the pressed one on
/// `bg-base` with a 0.5 px `border-strong` ring.
fn seg_icon_btn(
    p: &Pal,
    key: &'static str,
    glyph: &'static Icon,
    active: bool,
    help_text: &'static str,
    on_press: impl Fn(&mut App, &mut Tx) + 'static,
) -> El {
    el().key(key)
        .size(40.0, 28.0)
        .radius(6.0)
        .center()
        .cursor(Cursor::Pointer)
        .fill(if active { p.bg_base } else { Color::TRANSPARENT })
        .box_shadow(Pal::ring_out(if active { p.q.border_strong } else { Color::TRANSPARENT }))
        .role("button")
        .label(help_text)
        .help(help_text)
        .on_press_tx(on_press)
        .child(icon(glyph, 16.0).color(if active { p.icon } else { p.icon_muted }).group_color(p.icon))
        .hover_group()
}

/// A segmented control's track: `layer-01` under a 0.5 px `border-base` ring.
fn seg_track(p: &Pal, key: &'static str) -> El {
    row().key(key).h(28.0).center_y().radius(6.0).fill(p.layer1).box_shadow(Pal::ring_out(p.border))
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

    let mut start = row().key("review-toolbar-start").center_y().shrink().min_w(0.0);
    if !sidebar_open {
        // `session-review-v2-toolbar-collapsed-meta`: title, stats and the file's position. In a
        // narrow pane only the title gives way; the stats and the position keep their width, so
        // the file arrows after them never draw over them.
        let measure = |s: &str, st: TextStyle| rune::text::measure(s, &[], st, None).0;
        let stats_w = [(adds > 0).then(|| measure(&format!("+{adds}"), MONO_COUNT)), (dels > 0).then(|| measure(&format!("−{dels}"), MONO_COUNT))]
            .into_iter()
            .flatten()
            .fold((0.0, 0usize), |(w, n), x| (w + x, n + 1));
        let stats_w = stats_w.0 + 4.0 * stats_w.1.saturating_sub(1) as f32;
        let pos_w = if total > 0 { measure(&format!("{}/{total}", cur_idx + 1), POSITION) + 8.0 } else { 0.0 };
        start = start.child(
            row()
                .key("review-collapsed-meta")
                .gap(8.0)
                .pr(8.0)
                .center_y()
                .shrink()
                .min_w((stats_w + 8.0 + pos_w + 8.0).ceil())
                .child(mode_button(p, sr).shrink().min_w(0.0))
                .child(diff_counts_badge(p, adds, dels))
                .children((total > 0).then(|| text(format!("{}/{total}", cur_idx + 1)).font(POSITION).color(p.faint))),
        );
    }

    let nav_btn = |key: &'static str, glyph: &'static Icon, help_lbl: String, forward: bool| {
        let mut b = el()
            .key(key)
            .size(36.0, 28.0)
            .radius(6.0)
            .center()
            .role("button")
            .label(help_lbl.clone())
            .help(help_lbl)
            .child(icon(glyph, 16.0).color(p.icon));
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
        .child(nav_btn("review-prev-file", &ARROW_LEFT, format!("{} \u{2190}", opencode_i18n::t!("ui.sessionReviewV2.previousFile")), false))
        .child(nav_btn("review-next-file", &ARROW_RIGHT, format!("{} \u{2192}", opencode_i18n::t!("ui.sessionReviewV2.nextFile")), true));

    let exp_mode = s.review.expand_mode;
    let expand_seg = seg_track(p, "review-expand-seg")

        .child(seg_icon_btn(
            p,
            "review-expand-all",
            &EXPAND_LINES,
            exp_mode == ExpandMode::Expand,
            opencode_i18n::t!("ui.sessionReviewV2.showAllLines"),
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
            opencode_i18n::t!("ui.sessionReviewV2.hideNonDiffLines"),
            |s, tx| {
                tx.instant();
                s.set_review_expand_mode(ExpandMode::Collapse);
            },
        ));

    let diff_mode = s.review.diff_mode;
    let style_seg = seg_track(p, "review-style-seg")

        .child(seg_icon_btn(
            p,
            "review-style-unified",
            &UNIFIED_ICON,
            diff_mode == DiffMode::Unified,
            opencode_i18n::t!("ui.sessionReviewV2.unifiedDiff"),
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
            opencode_i18n::t!("ui.sessionReviewV2.splitDiff"),
            |s, tx| {
                tx.instant();
                s.set_review_diff_mode(DiffMode::Split);
            },
        ));

    // `session-review-v2-toolbar`: 48 px, past the tree toggle while the tree is closed; the
    // segments group ends it, "Open in" first, 12 px apart.
    let toolbar = row()
        .key("review-toolbar")
        .w_full()
        .h(chrome::BAR_H)
        .pl(if sidebar_open { 8.0 } else { chrome::TOGGLE_INSET })
        .pr(12.0)
        .gap(8.0)
        .center_y()
        .fill(p.bg_base)
        .child(start)
        .child(el().grow(1.0))
        .child(
            row()
                .key("review-toolbar-segments")
                .gap(12.0)
                .center_y()
                .children(sr.active_path.as_deref().and_then(|ap| crate::open_in::button(s, p, "review-open-in", ap)))
                .child(expand_seg)
                .child(style_seg),
        );

    let body = if !sr.ready {
        col()
            .key("review-loading")
            .w_full()
            .grow(1.0)
            .center()
            .child(text(opencode_i18n::t!("plugin.review.loadingChanges")).font(UI).color(p.muted))
    } else if !has_diffs {
        let msg = if sr.no_git {
            opencode_i18n::t!("rune.review.noGit")
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
        .on_fetch_text(|s: &mut App, f: usize| s.request_review_file_text(f))
        .build(s, p)
    };

    // The toolbar shows while there are changes, or the tree is closed (`SessionFilePanelV2`'s
    // `toolbar`), so an empty review with its tree open has none.
    let toolbar = (has_diffs || !sidebar_open).then(|| col().w_full().child(toolbar).child(el().w_full().h(1.0).fill(p.border)));
    col()
        .key("review-main")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .children(toolbar)
        .child(body)
}
