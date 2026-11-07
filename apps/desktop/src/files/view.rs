//! UI for the Files panel (`SessionFileBrowserTab`): the file tree beside the open file
//! ([`super::viewer`]).

use std::borrow::Cow;

use rune::{
    Anim, Color, Cursor, Drag, El, GrowthSpec, Phase, ScrollSpec, TextEdit, Tx, col,
    el, icon, row, text,
};
use rune_theme_opencode::tree_icons;

use super::data::{OPEN_TAB_ID, file_tab_path};
use super::{
    FILES_SIDEBAR_DEFAULT_W, FILES_SIDEBAR_MAX_W, FILES_SIDEBAR_MIN_W, SessionFiles,
};
use crate::App;
use crate::palette::Pal;
use crate::panels::PanelViewCtx;
use crate::review::chrome;
use crate::view::{UI_MED, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;
const TREE_ROW_H: f32 = chrome::ROW_H;
const LIST_ROW_H: f32 = 28.0;

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
    let max_sb_w = (ctx.pane_size.0 * 0.6).clamp(FILES_SIDEBAR_MIN_W, FILES_SIDEBAR_MAX_W);
    let sb_w = if s.files.sidebar_resized { s.files.sidebar_w } else { FILES_SIDEBAR_DEFAULT_W }.clamp(FILES_SIDEBAR_MIN_W, max_sb_w);

    let proj_title = s
        .project(ctx.project_id)
        .map(|pr| Some(opencode_kit::path::basename(&pr.directory)).filter(|n| !n.is_empty()).unwrap_or("Workspace").to_owned())
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
    body_row = body_row.child(super::viewer::files_main(
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
        // While browsing ("Open file"), the tree stays open and its toggle is disabled.
        .child(chrome::sidebar_toggle(p, "files-tree-toggle", sidebar_open, placeholder, |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.files.sidebar_open = !s.files.sidebar_open;
        }))
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
    // `SessionReviewV2Sidebar`'s header: the project's name past the tree toggle. Desktop has no
    // quick-open button here; Ctrl+P (Cmd+P) opens the palette.
    let header = chrome::sidebar_header(
        "files-sidebar-header",
        text(proj_title.to_owned()).font(UI_MED).color(p.text).truncate_fade(),
        None,
    );
    let filter_box = chrome::filter_field(
        p,
        "files-filter",
        &s.files.filter,
        "files-filter-inner",
        |s: &mut App| {
            s.request_sidebar_file_search();
            &mut s.files.filter
        },
        |s: &mut App, tx: &mut Tx| s.submit_files_filter(tx),
        |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let f = s.files.filter.focused;
            s.files.filter = TextEdit::new("");
            s.files.filter.focused = f;
        },
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
        .child(list_area.pt(4.0))
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
        .px(8.0)
        .pb(12.0)
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
                let glyph = (!is_dir).then(|| tree_icons::file(&node.name));
                let expanded = if is_dir { Some(tr.expanded.unwrap_or(false)) } else { None };
                let mut r = chrome::tree_row(&pal, &node.name, tr.depth, expanded, glyph, active)
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        if is_dir {
                            s.toggle_workspace_dir(&id_click);
                        } else {
                            s.open_workspace_file(&id_click, true, None, tx);
                        }
                    });
                if !is_dir {
                    r = r.on_double_press_tx(move |s: &mut App, tx: &mut Tx| {
                        s.open_workspace_file(&id_dbl, false, None, tx);
                    });
                }
                let r = r.children(status.map(|st| chrome::change_letter(&pal, st)));
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
                let (dir, name) = (Some(opencode_kit::path::parent(path)).filter(|d| !d.is_empty()), opencode_kit::path::basename(path));
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
