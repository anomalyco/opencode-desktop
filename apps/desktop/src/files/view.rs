//! UI for the Files panel (`SessionFileBrowserTab` + shared [`CodeViewer`]).

use std::borrow::Cow;

use rune::{
    Anim, Color, Cursor, Drag, El, GrowthSpec, Icon, Phase, ScrollSpec, TextEdit, TextStyle, Tx,
    col, el, icon, row, text, text_input,
};
use rune_kit::diff::FileStatus;
use rune_kit::file_icon;
use rune_theme_opencode::icons as ic;

use super::data::{OPEN_TAB_ID, file_tab_path};
use super::{
    FILES_SIDEBAR_DEFAULT_W, FILES_SIDEBAR_MAX_W, FILES_SIDEBAR_MIN_W, SessionFiles,
};
use crate::App;
use crate::code_view::CodeViewer;
use crate::palette::Pal;
use crate::panels::PanelViewCtx;
use crate::view::{UI_MED, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;
const TREE_ROW_H: f32 = 26.0;
const LIST_ROW_H: f32 = 28.0;
const MONO_TINY: TextStyle = TextStyle::new(11.0, 450.0).mono().lh(15.0).features("'tnum' 1");

static WRAP_ICON: Icon = Icon {
    name: "files-wrap-lines",
    view: 16.0,
    body: r#"<path d="M2.5 4.5H13.5M2.5 8.5H11C12.38 8.5 13.5 9.62 13.5 11C13.5 12.38 12.38 13.5 11 13.5H8.5M10 11.75L8.25 13.5L10 15.25M2.5 12.5H6" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

pub fn render(s: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
    if !ctx.visible && !s.side.mounted && s.stage.is_none() {
        return col().key("files-idle").w_full().grow(1.0);
    }

    let sid = ctx.session_id.to_owned();
    let placeholder = ctx.tab.id == OPEN_TAB_ID;
    let active_path = if placeholder {
        None
    } else {
        file_tab_path(&ctx.tab.id)
    };
    let sidebar_open = placeholder || s.files.sidebar_open;
    let max_sb_w = (ctx.pane_size.0 * 0.52).clamp(FILES_SIDEBAR_MIN_W, FILES_SIDEBAR_MAX_W);
    let sb_w = if s.files.sidebar_resized {
        s.files.sidebar_w.clamp(FILES_SIDEBAR_MIN_W, max_sb_w)
    } else {
        (ctx.pane_size.0 * 0.40).clamp(FILES_SIDEBAR_MIN_W, FILES_SIDEBAR_DEFAULT_W)
    };

    let proj_title = s
        .project(ctx.project_id)
        .map(|pr| {
            pr.directory
                .rsplit('/')
                .find(|seg| !seg.is_empty())
                .unwrap_or("Workspace")
                .to_owned()
        })
        .unwrap_or_else(|| "Workspace".into());

    let empty = SessionFiles::default();
    let ws = s.files.sessions.get(&sid).unwrap_or(&empty);

    let mut body_row = row().key("files-body").w_full().grow(1.0).min_h(0.0);
    if sidebar_open {
        body_row = body_row
            .child(files_sidebar(
                s,
                p,
                &sid,
                &proj_title,
                ws,
                active_path.as_deref(),
                sb_w,
            ))
            .child(sidebar_resize_handle(s, p, sb_w, max_sb_w));
    }
    body_row = body_row.child(files_main(
        s,
        p,
        &sid,
        &proj_title,
        ws,
        active_path.as_deref(),
        placeholder,
        sidebar_open,
    ));

    col()
        .key("files-panel")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(p.bg_base)
        .child(body_row)
}

fn files_sidebar(
    s: &App,
    p: &Pal,
    session_id: &str,
    proj_title: &str,
    ws: &SessionFiles,
    active_path: Option<&str>,
    width: f32,
) -> El {
    let header = row()
        .key("files-sidebar-header")
        .w_full()
        .h(38.0)
        .pl(10.0)
        .pr(6.0)
        .gap(6.0)
        .center_y()
        .child(
            text(proj_title.to_owned())
                .font(UI_MED)
                .color(p.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate_fade(),
        )
        .child(
            el().key("files-open-palette-btn")
                .size(24.0, 24.0)
                .radius(5.0)
                .center()
                .cursor(Cursor::Pointer)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .role("button")
                .label("Quick open file ⌘P")
                .help("Quick open file ⌘P")
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.open_file_palette(tx))
                .child(icon(&ic::MAGNIFYING_GLASS, 13.0).color(p.icon_muted)),
        );

    let has_query = !s.files.filter.text.is_empty();
    let filter_box = row()
        .key("files-filter-box")
        .w_full()
        .px(8.0)
        .pb(6.0)
        .child(
            row()
                .key("files-filter-inner")
                .w_full()
                .h(26.0)
                .px(7.0)
                .gap(5.0)
                .center_y()
                .radius(6.0)
                .clip_rounded(6.0)
                .fill(p.bg_deep)
                .border(0.5, if s.files.filter.focused { p.border } else { p.border_muted })
                .motion(rune::Spring::NONE)
                .child(icon(&ic::MAGNIFYING_GLASS, 12.0).color(p.faint))
                .child(
                    text_input(&s.files.filter, UI_SMALL)
                        .key("files-filter")
                        .placeholder("Filter files…", p.faint)
                        .color(p.text)
                        .grow(1.0)
                        .shrink()
                        .on_edit(|s: &mut App| {
                            s.request_sidebar_file_search();
                            &mut s.files.filter
                        })
                        .on_submit(|s: &mut App, tx: &mut Tx| s.submit_files_filter(tx)),
                )
                .children(has_query.then(|| {
                    el().key("files-filter-clear")
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
                            let f = s.files.filter.focused;
                            s.files.filter = TextEdit::new("");
                            s.files.filter.focused = f;
                        })
                        .child(icon(&ic::XMARK_SMALL, 11.0).color(p.icon_muted))
                })),
        );

    let list_area = if !ws.ready {
        col()
            .key("files-tree-loading")
            .w_full()
            .grow(1.0)
            .px(10.0)
            .pt(8.0)
            .child(text("Loading…").font(UI_SMALL).color(p.muted))
    } else if s.files.filter.text.trim().is_empty() {
        workspace_tree_view(p, session_id, ws, active_path)
    } else {
        filtered_files_view(s, p, session_id, active_path)
    };

    col()
        .key("files-sidebar")
        .w(width)
        .h_full()
        .fill(p.bg_base)
        .child(header)
        .child(filter_box)
        .child(list_area)
}

fn workspace_tree_view(
    p: &Pal,
    session_id: &str,
    ws: &SessionFiles,
    active_path: Option<&str>,
) -> El {
    // Filter out internal `__placeholder__` sentinel leaves kept for empty collapsed directories.
    let visible_indices: Vec<usize> = (0..ws.tree.visible_len())
        .filter(|&i| ws.tree.row(i).is_some_and(|(n, _)| n.data != Some(true)))
        .collect();
    let count = visible_indices.len();
    let idx_keys = visible_indices.clone();
    let sid_keys = session_id.to_owned();
    let sid_rows = session_id.to_owned();
    let act_owned = active_path.map(str::to_owned);
    let pal = *p;

    let list = col()
        .key("files-tree-rows")
        .w_full()
        .px(4.0)
        .pb(8.0)
        .growth(GrowthSpec::default())
        .role("tree")
        .label("Workspace files")
        .virtual_rows(
            count,
            TREE_ROW_H,
            move |s: &App, i| {
                let raw = idx_keys[i];
                s.files
                    .sessions
                    .get(&sid_keys)
                    .and_then(|w| w.tree.row(raw).map(|(_, tr)| Cow::Owned(format!("ft:{}", tr.id))))
                    .unwrap_or(Cow::Borrowed("ft:empty"))
            },
            move |s: &App, i| {
                let raw = visible_indices[i];
                let Some(w) = s.files.sessions.get(&sid_rows) else {
                    return el().h(TREE_ROW_H);
                };
                let Some((node, tr)) = w.tree.row(raw) else {
                    return el().h(TREE_ROW_H);
                };
                let status = s
                    .review
                    .sessions
                    .get(&sid_rows)
                    .and_then(|sr| sr.meta.iter().find(|m| m.path == tr.id))
                    .map(|m| m.status);
                let active = act_owned.as_deref() == Some(tr.id);
                let is_dir = node.dir;
                let id_click = tr.id.to_owned();
                let id_dbl = tr.id.to_owned();
                let id_ctx = tr.id.to_owned();
                let indent = 6.0 + tr.depth as f32 * 12.0;

                let chev = match tr.expanded {
                    Some(true) => el()
                        .size(14.0, 14.0)
                        .center()
                        .child(icon(&ic::CHEVRON_DOWN, 11.0).color(pal.icon_muted)),
                    Some(false) => el()
                        .size(14.0, 14.0)
                        .center()
                        .child(icon(&ic::CHEVRON_RIGHT, 11.0).color(pal.icon_muted)),
                    None => el().size(14.0, 14.0),
                };
                let ic_glyph = if is_dir { &ic::FOLDER } else { file_icon(&node.name) };
                let mut r = row()
                    .w_full()
                    .h(TREE_ROW_H)
                    .pl(indent)
                    .pr(6.0)
                    .gap(4.0)
                    .center_y()
                    .radius(5.0)
                    .fill(if active { pal.row_active } else { Color::TRANSPARENT })
                    .hover(pal.hover)
                    .hover_motion(INSTANT, INSTANT)
                    .cursor(Cursor::Pointer)
                    .role("treeitem")
                    .label(node.name.clone())
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        if is_dir {
                            s.toggle_workspace_dir(&id_click);
                        } else {
                            s.open_workspace_file(&id_click, true, None, tx);
                        }
                    })
                    .child(chev)
                    .child(icon(ic_glyph, 14.0).color(if active { pal.icon } else { pal.icon_muted }))
                    .child(
                        text(node.name.clone())
                            .font(if active { UI_MED } else { UI_SMALL })
                            .color(if active { pal.text } else { pal.muted })
                            .grow(1.0)
                            .shrink()
                            .min_w(0.0)
                            .truncate_fade(),
                    );
                if !is_dir {
                    r = r.on_double_press_tx(move |s: &mut App, tx: &mut Tx| {
                        s.open_workspace_file(&id_dbl, false, None, tx);
                    });
                }
                if let Some(st) = status {
                    let (lbl, col_fg) = match st {
                        FileStatus::Added => ("A", pal.success),
                        FileStatus::Deleted => ("D", pal.danger),
                        _ => ("M", pal.warning),
                    };
                    r = r.child(text(lbl).font(MONO_TINY).color(col_fg));
                }
                crate::open_in::context(r, id_ctx)
            },
        );
    col()
        .key("files-tree")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(list)
}

fn filtered_files_view(
    s: &App,
    p: &Pal,
    session_id: &str,
    active_path: Option<&str>,
) -> El {
    let filtered = s.filtered_sidebar_files(session_id);
    if filtered.is_empty() {
        return col()
            .key("files-filter-empty")
            .w_full()
            .grow(1.0)
            .px(10.0)
            .pt(8.0)
            .child(text("No matching files").font(UI_SMALL).color(p.muted));
    }
    let count = filtered.len();
    let keys_vec = filtered.clone();
    let sid_rows = session_id.to_owned();
    let act_owned = active_path.map(str::to_owned);
    let pal = *p;
    let list = col()
        .key("files-filter-rows")
        .w_full()
        .px(4.0)
        .pb(8.0)
        .growth(GrowthSpec::default())
        .role("listbox")
        .virtual_rows(
            count,
            LIST_ROW_H,
            move |_s: &App, i| Cow::Owned(format!("ff:{}", keys_vec[i])),
            move |s: &App, i| {
                let path = &filtered[i];
                let highlighted = s
                    .files
                    .sessions
                    .get(&sid_rows)
                    .and_then(|w| w.highlighted_filter_path.as_deref())
                    .map_or(i == 0, |hp| hp == path);
                let active = act_owned.as_deref() == Some(path.as_str());
                let (dir, name) = match path.rsplit_once('/') {
                    Some((d, b)) => (Some(d), b),
                    None => (None, path.as_str()),
                };
                let click_p = path.clone();
                let dbl_p = path.clone();
                let mut r = row()
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
                        s.open_workspace_file(&click_p, true, None, tx);
                    })
                    .on_double_press_tx(move |s: &mut App, tx: &mut Tx| {
                        s.open_workspace_file(&dbl_p, false, None, tx);
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
                    r = r.child(
                        text(d.to_owned())
                            .font(UI_SMALL)
                            .color(pal.faint)
                            .grow(1.0)
                            .shrink()
                            .min_w(0.0)
                            .truncate_fade(),
                    );
                } else {
                    r = r.child(el().grow(1.0));
                }
                crate::open_in::context(r, path.clone())
            },
        );
    col()
        .key("files-filter-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(list)
}

fn sidebar_resize_handle(s: &App, p: &Pal, cur_w: f32, max_w: f32) -> El {
    let dragging = s.files.sidebar_drag.is_some();
    el().key("files-sidebar-resize")
        .w(5.0)
        .h_full()
        .center()
        .cursor(Cursor::ResizeColumn)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_double_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.files.sidebar_resized = false;
            s.files.sidebar_w = FILES_SIDEBAR_DEFAULT_W;
        })
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            match d.phase {
                Phase::Down => s.files.sidebar_drag = Some(cur_w),
                Phase::Move => {
                    if let Some(orig) = s.files.sidebar_drag {
                        s.files.sidebar_resized = true;
                        s.files.sidebar_w = (orig + d.dx / s.zoom).clamp(FILES_SIDEBAR_MIN_W, max_w);
                    }
                }
                Phase::Up => {
                    if d.cancelled()
                        && let Some(orig) = s.files.sidebar_drag
                    {
                        s.files.sidebar_w = orig;
                    }
                    s.files.sidebar_drag = None;
                }
            }
        })
        .child(el().w(0.5).h_full().fill(if dragging { p.border } else { p.border_muted }))
}

#[allow(clippy::too_many_arguments)]
fn files_main(
    s: &App,
    p: &Pal,
    session_id: &str,
    proj_title: &str,
    ws: &SessionFiles,
    active_path: Option<&str>,
    placeholder: bool,
    sidebar_open: bool,
) -> El {
    if placeholder || active_path.is_none() {
        return col()
            .key("files-placeholder")
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .center()
            .gap(8.0)
            .padding(24.0)
            .child(icon(&ic::FILE_TREE, 24.0).color(p.icon_faint))
            .child(text("Open file").font(UI_MED).color(p.text))
            .child(text("Select a file to open or press ⌘P").font(UI_SMALL).color(p.muted));
    }

    let path = active_path.unwrap();
    let doc = ws.docs.get(path);
    let wrap_on = doc.is_some_and(|d| d.wrap());
    let find_on = doc.is_some_and(|d| d.find_open);
    let sel_badge = doc.and_then(|d| d.selection()).map(|sel| {
        let (a, b) = sel.ordered();
        if a == b { format!("L{a}") } else { format!("L{a}–{b}") }
    });

    let (dir_part, base_name) = match path.rsplit_once('/') {
        Some((d, b)) => (Some(format!("{d}/")), b),
        None => (None, path),
    };
    let mut crumb = row().key("files-crumb").grow(1.0).shrink().min_w(0.0).gap(3.0).center_y();
    if !sidebar_open && dir_part.is_none() {
        crumb = crumb.child(text(format!("{proj_title}/")).font(UI_SMALL).color(p.faint).shrink().min_w(0.0).truncate_fade());
    }
    if let Some(d) = dir_part {
        crumb = crumb.child(text(d).font(UI_SMALL).color(p.muted).shrink().min_w(0.0).truncate_fade());
    }
    crumb = crumb.child(
        text(base_name.to_owned())
            .font(UI_MED)
            .color(p.text)
            .shrink()
            .min_w(0.0)
            .truncate_fade(),
    );
    if let Some(lbl) = sel_badge {
        crumb = crumb.child(
            el().px(5.0)
                .py(1.0)
                .radius(4.0)
                .fill(p.layer1)
                .child(text(lbl).font(MONO_TINY).color(p.muted)),
        );
    }

    let sid_find = session_id.to_owned();
    let path_find = path.to_owned();
    let sid_wrap = session_id.to_owned();
    let path_wrap = path.to_owned();
    let path_copy = path.to_owned();

    let toolbar = row()
        .key("files-toolbar")
        .w_full()
        .h(38.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .fill(p.bg_base)
        .child(
            el().key("files-tree-toggle")
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
                    s.files.sidebar_open = !s.files.sidebar_open;
                })
                .child(icon(&ic::FILE_TREE, 14.0).color(if sidebar_open { p.icon } else { p.icon_muted })),
        )
        .child(crumb)
        .child(
            el().key("files-find-btn")
                .size(25.0, 25.0)
                .radius(5.0)
                .center()
                .cursor(Cursor::Pointer)
                .fill(if find_on { p.row_active } else { Color::TRANSPARENT })
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .role("button")
                .label("Find in file")
                .help("Find in file ⌘F")
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    if let Some(w) = s.files.sessions.get_mut(&sid_find)
                        && let Some(d) = w.docs.get_mut(&path_find)
                    {
                        d.find_open = !d.find_open;
                        d.find_input.focused = d.find_open;
                    }
                })
                .child(icon(&ic::MAGNIFYING_GLASS, 13.0).color(if find_on { p.icon } else { p.icon_muted })),
        )
        .child(
            el().key("files-wrap-btn")
                .size(25.0, 25.0)
                .radius(5.0)
                .center()
                .cursor(Cursor::Pointer)
                .fill(if wrap_on { p.row_active } else { Color::TRANSPARENT })
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .role("button")
                .label("Wrap lines")
                .help("Wrap lines")
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    if let Some(w) = s.files.sessions.get_mut(&sid_wrap)
                        && let Some(d) = w.docs.get_mut(&path_wrap)
                    {
                        d.set_wrap(!d.wrap());
                    }
                })
                .child(icon(&WRAP_ICON, 13.0).color(if wrap_on { p.icon } else { p.icon_muted })),
        )
        .child(
            el().key("files-copy-path")
                .size(25.0, 25.0)
                .radius(5.0)
                .center()
                .cursor(Cursor::Pointer)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .role("button")
                .label("Copy path")
                .help("Copy path")
                .on_press_tx(move |_s: &mut App, tx: &mut Tx| {
                    tx.copy(path_copy.clone());
                })
                .child(icon(&ic::COPY, 13.0).color(p.icon_muted)),
        )
        .children(crate::open_in::button(s, p, "files-open-in", path));

    let mut out = col()
        .key("files-main")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(toolbar)
        .child(el().w_full().h(0.5).fill(p.border_muted));

    if let Some(d) = doc
        && d.find_open
    {
        out = out
            .child(file_find_bar(p, session_id, path, d))
            .child(el().w_full().h(0.5).fill(p.border_muted));
    }

    let sid_get = session_id.to_owned();
    let path_get = path.to_owned();
    let sid_mut = session_id.to_owned();
    let path_mut = path.to_owned();
    let viewer = CodeViewer::new(
        format!("file-code-{session_id}-{path}"),
        move |s: &App| {
            s.files
                .sessions
                .get(&sid_get)
                .and_then(|w| w.docs.get(&path_get))
                .unwrap_or(&s.files.empty_doc)
        },
        move |s: &mut App| {
            let has = s
                .files
                .sessions
                .get(&sid_mut)
                .is_some_and(|w| w.docs.contains_key(&path_mut));
            if has {
                s.files
                    .sessions
                    .get_mut(&sid_mut)
                    .unwrap()
                    .docs
                    .get_mut(&path_mut)
                    .unwrap()
            } else {
                &mut s.files.empty_doc
            }
        },
    )
    .build(s, p);

    out.child(viewer)
}

fn file_find_bar(p: &Pal, session_id: &str, path: &str, doc: &crate::code_view::CodeDocState) -> El {
    let total = doc.match_count();
    let cur = if total == 0 { 0 } else { doc.active_match_index() + 1 };
    let sid_edit = session_id.to_owned();
    let path_edit = path.to_owned();
    let sid_sub = session_id.to_owned();
    let path_sub = path.to_owned();
    let sid_close = session_id.to_owned();
    let path_close = path.to_owned();

    row()
        .key("files-find-bar")
        .w_full()
        .h(32.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .fill(p.layer1)
        .child(icon(&ic::MAGNIFYING_GLASS, 12.0).color(p.faint))
        .child(
            text_input(&doc.find_input, UI_SMALL)
                .key("files-find-input")
                .placeholder("Find in file…", p.faint)
                .color(p.text)
                .grow(1.0)
                .shrink()
                .on_edit(move |s: &mut App| {
                    let has = s
                        .files
                        .sessions
                        .get(&sid_edit)
                        .is_some_and(|w| w.docs.contains_key(&path_edit));
                    if has {
                        &mut s
                            .files
                            .sessions
                            .get_mut(&sid_edit)
                            .unwrap()
                            .docs
                            .get_mut(&path_edit)
                            .unwrap()
                            .find_input
                    } else {
                        &mut s.files.empty_doc.find_input
                    }
                })
                .on_submit(move |s: &mut App, tx: &mut Tx| {
                    let scroll_key = format!("file-code-{sid_sub}-{path_sub}-scroll");
                    if let Some(w) = s.files.sessions.get_mut(&sid_sub)
                        && let Some(d) = w.docs.get_mut(&path_sub)
                    {
                        d.sync_find_query();
                        d.step_match(&scroll_key, true, tx);
                    }
                }),
        )
        .children((!doc.find_input.text.is_empty()).then(|| {
            text(format!("{cur}/{total}")).font(MONO_TINY).color(p.muted)
        }))
        .child(
            el().key("files-find-close")
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .cursor(Cursor::Pointer)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .role("button")
                .label("Close find")
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    if let Some(w) = s.files.sessions.get_mut(&sid_close)
                        && let Some(d) = w.docs.get_mut(&path_close)
                    {
                        d.find_open = false;
                        d.find_input = TextEdit::new("");
                        d.sync_find_query();
                    }
                })
                .child(icon(&ic::XMARK_SMALL, 11.0).color(p.icon_muted)),
        )
}
