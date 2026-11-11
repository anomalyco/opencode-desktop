//! The Files panel's tree (`SessionFileBrowserTab`'s sidebar): the workspace's folders, or the
//! files a filter finds. The Luau file plugin places it (`ui.fileTree`); the narrow Files view
//! draws it under its own filter.

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

use rune::{Anim, Color, Cursor, El, GrowthSpec, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::diff::FileStatus;
use rune_theme_opencode::tree_icons;

use super::SessionFiles;
use crate::App;
use crate::palette::Pal;
use crate::review::chrome;
use crate::view::{UI_MED, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;
const TREE_ROW_H: f32 = chrome::ROW_H;
const LIST_ROW_H: f32 = 28.0;
const LIST_LABEL: rune::TextStyle = rune::TextStyle::new(13.0, 530.0).lh(16.0);

pub(super) fn files_sidebar(
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
        opencode_i18n::t!("ui.sessionReviewV2.filterFiles"),
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
        // Desktop's filter (`session-review-v2.tsx`, a `type="search"` input) marks no spelling.
        false,
        |_: &mut App, _: &rune::KeyEvent, _: &mut Tx| false,
    );

    let list_area = if !ws.ready {
        col()
            .key("files-tree-loading")
            .w_full()
            .grow(1.0)
            .px(10.0)
            .pt(8.0)
            .child(text(opencode_i18n::t!("ui.sessionReview.image.loading")).font(UI_SMALL).color(p.muted))
    } else if s.files.filter.text.trim().is_empty() {
        workspace_tree_view(p, session_id, ws, active_path, None)
    } else {
        filtered_files_view(s, p, session_id, active_path, &s.files.filter.text, None)
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

/// The workspace's folders and files, its folders opening as they're pressed. A file's row opens
/// it in a preview tab, a double press keeps it. `kinds` are the change letters (`add`, `del`,
/// `mix`) to end rows with; without them, the session's review says.
pub(crate) fn workspace_tree_view(
    p: &Pal,
    session_id: &str,
    ws: &SessionFiles,
    active_path: Option<&str>,
    kinds: Option<Rc<HashMap<String, String>>>,
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
        .label(opencode_i18n::t!("rune.files.tree"))
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
                let status = match &kinds {
                    // A folder holding changes shows no letter (Pierre hides its git lane).
                    Some(k) => k.get(tr.id).filter(|_| !node.dir).and_then(|k| kind_status(k)),
                    None => s.review.sessions.get(&sid_rows).and_then(|sr| sr.meta.iter().find(|m| m.path == tr.id)).map(|m| m.status),
                };
                let active = act_owned.as_deref() == Some(tr.id);
                let is_dir = node.dir;
                let id_click = tr.id.to_owned();
                let id_dbl = tr.id.to_owned();
                let id_ctx = tr.id.to_owned();
                let glyph = (!is_dir).then(|| tree_icons::file(&node.name));
                let expanded = if is_dir { Some(tr.expanded.unwrap_or(false)) } else { None };
                let mut r = chrome::tree_row_in(&pal, &node.name, tr.depth, expanded, glyph, active, chrome::PIERRE)
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
    // Desktop's Pierre tree scrolls itself in its shadow root, where `scrollbar.css` doesn't
    // reach (`gui-plugins/src/file/pierre-tree.tsx:32-50`): not matched yet, no bar.
    session_view::scroller::scroller("files-tree").hidden(col().w_full().grow(1.0).min_h(0.0), list)
}

/// The files whose path holds `query`, flat, best first; the highlighted one (else the first)
/// is what Enter opens. `kinds` end the rows with their change letters.
pub(crate) fn filtered_files_view(
    s: &App,
    p: &Pal,
    session_id: &str,
    active_path: Option<&str>,
    query: &str,
    kinds: Option<Rc<HashMap<String, String>>>,
) -> El {
    let filtered = s.filtered_files(session_id, query);
    if filtered.is_empty() {
        return col()
            .key("files-filter-empty")
            .w_full()
            .grow(1.0)
            .px(10.0)
            .pt(8.0)
            .child(text(opencode_i18n::t!("rune.files.noMatches")).font(UI_SMALL).color(p.muted));
    }
    let count = filtered.len();
    let keys_vec = filtered.clone();
    let sid_rows = session_id.to_owned();
    let act_owned = active_path.map(str::to_owned);
    let pal = *p;
    let list = col()
        .key("files-filter-rows")
        .w_full()
        .px(8.0)
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
                let on = active || highlighted;
                let name = opencode_kit::path::basename(path);
                let dir = &path[..path.len() - name.len()];
                let (click_p, dbl_p) = (path.clone(), path.clone());
                let t = tree_icons::file(name);
                // `file-tree-v2-row`: 28 tall, 8 in, 6 apart, 6 round; its folder (with its slash) then its
                // name, both 13 at 530, the folder darker as Desktop draws them.
                let label = row()
                    .grow(1.0)
                    .min_w(0.0)
                    .mr(12.0)
                    .children((!dir.is_empty()).then(|| text(dir.to_owned()).font(LIST_LABEL).color(pal.text)))
                    .child(text(name.to_owned()).font(LIST_LABEL).color(pal.muted).shrink().min_w(0.0).truncate_fade());
                let r = row()
                    .w_full()
                    .h(LIST_ROW_H)
                    .px(8.0)
                    .gap(6.0)
                    .center_y()
                    .radius(6.0)
                    .fill(if on { pal.pressed } else { Color::TRANSPARENT })
                    .hover(if on { pal.pressed } else { pal.hover })
                    .hover_motion(INSTANT, INSTANT)
                    .cursor(Cursor::Pointer)
                    .role("option")
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_workspace_file(&click_p, true, None, tx))
                    .on_double_press_tx(move |s: &mut App, tx: &mut Tx| s.open_workspace_file(&dbl_p, false, None, tx))
                    .child(icon(t.icon(), 16.0).color(if on { t.color(pal.dark) } else { pal.icon_muted }))
                    .child(label);
                let status = kinds.as_ref().and_then(|k| k.get(path.as_str())).and_then(|k| kind_status(k));
                let r = r.children(status.map(|st| chrome::change_letter(&pal, st)));
                crate::open_in::context(r, path.clone())
            },
        );
    // The filter's rows in the same Pierre tree as the folders (see `tree`): no bar yet.
    session_view::scroller::scroller("files-filter-scroll").hidden(col().w_full().grow(1.0).min_h(0.0), list)
}

/// The change letter a `review.changes` kind (`add`, `del`, `mix`) ends a row with.
fn kind_status(kind: &str) -> Option<FileStatus> {
    match kind {
        "add" => Some(FileStatus::Added),
        "del" => Some(FileStatus::Deleted),
        "mix" => Some(FileStatus::Modified),
        _ => None,
    }
}
