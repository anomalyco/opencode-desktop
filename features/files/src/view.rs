//! The Files panel's tree (`SessionFileBrowserTab`'s sidebar): the workspace's folders, or the
//! files a filter finds. The Luau file plugin places it (`ui.fileTree`); the narrow Files view
//! draws it under its own filter.

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

use opencode_ui::keymap::Bind;
use opencode_ui::panel_chrome as chrome;
use opencode_ui::popup::{Item, Menu};
use rune::{Anim, Color, Cursor, El, GrowthSpec, KeyEvent, PointerPress, ScrollPos, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::diff::FileStatus;
use rune_theme_opencode::icons as ic;
use rune_theme_opencode::tree_icons;
use opencode_ui::palette::Pal;
use opencode_ui::style::{UI_MED, UI_SMALL};

use crate::{FilesHost, SessionFiles, ops};

const INSTANT: Anim = Anim::INSTANT;
const TREE_ROW_H: f32 = chrome::ROW_H;
const LIST_ROW_H: f32 = 28.0;
// `.text-12-medium` (`utilities.css:30-37` + `theme.css:8,16`): `--font-size-small: 13px`,
// `--font-weight-medium: 500`, `--line-height-large: 150%` (19.5px).
const LIST_LABEL: rune::TextStyle = rune::TextStyle::new(13.0, 500.0).lh(19.5);

fn on_filter_edit(host: &mut dyn FilesHost) -> &mut TextEdit {
    ops::request_sidebar_file_search(host);
    &mut host.files_mut().filter
}

fn filter_edit<H: FilesHost>(s: &mut H) -> &mut TextEdit {
    on_filter_edit(s)
}

fn on_filter_clear(host: &mut dyn FilesHost, tx: &mut Tx) {
    tx.instant();
    let pane = host.files_mut();
    let f = pane.filter.focused;
    pane.filter = TextEdit::new("");
    pane.filter.focused = f;
}

fn on_filter_key(host: &mut dyn FilesHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    host.side_quiet_clear();
    [Bind::TabPrev, Bind::TabNext, Bind::PaneTabPrev, Bind::PaneTabNext]
        .iter()
        .any(|b| b.matches_tx(ev, tx))
        && host.handle_panels_key(ev, tx)
}

fn on_tree_row_press(host: &mut dyn FilesHost, id: &str, is_dir: bool, tx: &mut Tx) {
    tx.instant();
    if is_dir {
        ops::toggle_workspace_dir(host, id);
    } else {
        ops::open_workspace_file_or_page(host, id, true, tx);
    }
}

/// Desktop's `OpenInAppMenuItemsV2`: "Open in" over each app with its mark (the current one
/// checked, from the toolbar), then Copy path.
pub(crate) fn open_in_menu(host: &dyn FilesHost, with_current: bool) -> Menu {
    let apps = host
        .open_in_menu_options(with_current)
        .into_iter()
        .map(|opt| Item::new(opt.item_id, opt.label).app_icon(opt.icon).checked(opt.checked));
    // menu-v2 content is at least 160 pt wide (`menu.css`).
    Menu::new(opencode_i18n::t!("plugin.file.open.in"))
        .min_width(160.0)
        .group(opencode_i18n::t!("plugin.file.open.in"))
        .items(apps)
        .separator()
        .item(Item::new("copy", opencode_i18n::t!("plugin.file.open.copyPath")).weak_icon(&ic::COPY))
}

fn open_tree_context_menu(host: &mut dyn FilesHost, rel: &str, press: PointerPress, tx: &mut Tx) {
    if !host.can_open_in() {
        return;
    }
    let menu = open_in_menu(host, false);
    let rel = rel.to_owned();
    tx.instant();
    host.open_context_menu(
        menu,
        press.into(),
        tx,
        Box::new(move |h: &mut dyn FilesHost, picked: Option<&str>, tx: &mut Tx| {
            tx.instant();
            if let Some(id) = picked {
                h.open_in_pick(&rel, id, true, tx);
            }
        }),
    );
}

fn attach_open_in_context<H: FilesHost + 'static>(row: El, rel: String) -> El {
    let dragged = rel.clone();
    row.drag_out(move |s: &H| s.open_in_drag_item(&dragged))
        .on_secondary_press(move |s: &mut H, press: PointerPress, tx: &mut Tx| {
            open_tree_context_menu(s, &rel, press, tx);
        })
}

pub fn files_sidebar<H: FilesHost + 'static>(
    s: &H,
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
        &s.files().filter,
        "files-filter-inner",
        opencode_i18n::t!("ui.sessionReviewV2.filterFiles"),
        filter_edit::<H>,
        |s: &mut H, tx: &mut Tx| ops::submit_files_filter(s, tx),
        |s: &mut H, tx: &mut Tx| on_filter_clear(s, tx),
        // Desktop's filter (`session-review-v2.tsx`, a `type="search"` input) marks no spelling.
        false,
        // The panel's tab keys from its filter (`file.tab.prev`/`next` act while the side region
        // has focus), not the field's Tab traversal.
        |s: &mut H, ev: &KeyEvent, tx: &mut Tx| on_filter_key(s, ev, tx),
    );

    let list_area = if !ws.tree_load.is_ready() {
        col()
            .key("files-tree-loading")
            .w_full()
            .grow(1.0)
            .px(10.0)
            .pt(8.0)
            .child(text(opencode_i18n::t!("ui.sessionReview.image.loading")).font(UI_SMALL).color(p.muted))
    } else if s.files().filter.text.trim().is_empty() {
        workspace_tree_view::<H>(p, session_id, ws, active_path, None)
    } else {
        filtered_files_view(s, p, session_id, active_path, &s.files().filter.text, None)
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
pub fn workspace_tree_view<H: FilesHost + 'static>(
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
        .pb(8.0)
        .growth(GrowthSpec::default())
        .role("tree")
        .label_static(opencode_i18n::t!("rune.files.tree"))
        // One Tab stop; Up and Down move through the rows, Right and Left open and close a folder
        // or step into and out of it, as a native tree does.
        .focus_group()
        .on_key({
            let sid = session_id.to_owned();
            move |s: &mut H, ev: &KeyEvent, tx: &mut Tx| tree_key(s, &sid, ev, tx)
        })
        .virtual_rows(
            count,
            TREE_ROW_H,
            move |s: &H, i| {
                let raw = idx_keys[i];
                s.files()
                    .sessions
                    .get(&sid_keys)
                    .and_then(|w| w.tree.row(raw).map(|(_, tr)| Cow::Owned(format!("ft:{}", tr.id))))
                    .unwrap_or(Cow::Borrowed("ft:empty"))
            },
            move |s: &H, i| {
                let raw = visible_indices[i];
                let Some(w) = s.files().sessions.get(&sid_rows) else {
                    return el().h(TREE_ROW_H);
                };
                let Some((node, tr)) = w.tree.row(raw) else {
                    return el().h(TREE_ROW_H);
                };
                let status = match &kinds {
                    // A folder holding changes shows no letter (Pierre hides its git lane).
                    Some(k) => k.get(tr.id).filter(|_| !node.dir).and_then(|k| kind_status(k)),
                    None => s.review_file_status(&sid_rows, tr.id),
                };
                let active = act_owned.as_deref() == Some(tr.id);
                let is_dir = node.dir;
                let id_click = tr.id.to_owned();
                let id_dbl = tr.id.to_owned();
                let id_ctx = tr.id.to_owned();
                let glyph = (!is_dir).then(|| tree_icons::file(&node.name));
                let expanded = if is_dir { Some(tr.expanded.unwrap_or(false)) } else { None };
                let mut r = chrome::tree_row_in(&pal, &node.name, tr.depth, expanded, glyph, active, chrome::PIERRE)
                    .position_in_set(i + 1, count)
                    .on_press_tx(move |s: &mut H, tx: &mut Tx| on_tree_row_press(s, &id_click, is_dir, tx));
                if !is_dir {
                    r = r.on_double_press_tx(move |s: &mut H, tx: &mut Tx| {
                        ops::open_workspace_file_or_page(s, &id_dbl, false, tx);
                    });
                }
                let r = r.children(status.map(|st| chrome::change_letter(&pal, st)));
                attach_open_in_context::<H>(r, id_ctx)
            },
        );
    // Desktop's Pierre tree scrolls itself, with its own bar (`--trees-fg` muted on `bg-base`,
    // `gui-plugins/src/file/pierre-tree.tsx:36-38`).
    tree_scroll::<H>(p, "files-tree", session_id, list)
}

enum TreeAction {
    Toggle,
    Focus(String),
}

/// Right on a closed folder opens it and on an open one steps into its first child; Left on an
/// open folder closes it and elsewhere steps out to the parent folder; PageUp and PageDown scroll
/// the tree viewport without leaking to the session timeline (`pierre-tree.tsx` overflow scroll).
fn tree_key(host: &mut dyn FilesHost, sid: &str, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.mods != rune::Mods::default() {
        return false;
    }
    if matches!(ev.key, rune::Key::PageUp | rune::Key::PageDown)
        && tx.focused().is_some_and(|k| k.starts_with("ft:"))
    {
        page_tree(tx, "files-tree", sid, if ev.key == rune::Key::PageDown { 1 } else { -1 });
        return true;
    }
    let right = matches!(ev.key, rune::Key::Right);
    if !(right || matches!(ev.key, rune::Key::Left)) {
        return false;
    }
    let Some(id) = tx.focused().and_then(|k| k.strip_prefix("ft:")).map(str::to_owned) else {
        return false;
    };
    let action = {
        let Some(ws) = host.files().sessions.get(sid) else { return false };
        let dir = ws.tree.get(&id).is_some_and(|n| n.dir);
        let open = ws.expanded_dirs.contains(&id);
        match (right, dir, open) {
            (true, true, false) | (false, true, true) => TreeAction::Toggle,
            (true, true, true) => match ws.tree.children(Some(&id)).next() {
                Some(child) => TreeAction::Focus(format!("ft:{child}")),
                None => return false,
            },
            (false, _, _) => match ws.tree.parent(&id) {
                Some(parent) => TreeAction::Focus(format!("ft:{parent}")),
                None => return false,
            },
            (true, false, _) => return false,
        }
    };
    match action {
        TreeAction::Toggle => ops::toggle_workspace_dir(host, &id),
        TreeAction::Focus(key) => tx.focus(key, true),
    }
    true
}

/// The files whose path holds `query`, flat, best first; the highlighted one (else the first)
/// is what Enter opens. `kinds` end the rows with their change letters.
pub fn filtered_files_view<H: FilesHost + 'static>(
    s: &H,
    p: &Pal,
    session_id: &str,
    active_path: Option<&str>,
    query: &str,
    kinds: Option<Rc<HashMap<String, String>>>,
) -> El {
    let filtered = ops::filtered_files(s, session_id, query);
    if filtered.is_empty() {
        // `file/browser.tsx:313-315`: "No results found" as a status, inside the list's padding.
        return col()
            .key("files-filter-empty")
            .role("status")
            .w_full()
            .grow(1.0)
            .px(16.0)
            .pt(18.0)
            .child(text(opencode_i18n::t!("palette.empty")).font(UI_SMALL).color(p.muted));
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
        .pb(12.0)
        .growth(GrowthSpec::default())
        .role("listbox")
        .virtual_rows(
            count,
            LIST_ROW_H,
            move |_s: &H, i| Cow::Owned(format!("ff:{}", keys_vec[i])),
            move |s: &H, i| {
                let path = &filtered[i];
                let highlighted = s
                    .files()
                    .sessions
                    .get(&sid_rows)
                    .and_then(|w| w.highlighted_filter_path.as_deref())
                    .filter(|hp| filtered.iter().any(|p| p == hp))
                    .map_or(i == 0, |hp| hp == path);
                let active = act_owned.as_deref() == Some(path.as_str());
                let on = active || highlighted;
                let name = opencode_kit::path::basename(path);
                let dir = &path[..path.len() - name.len()];
                let (click_p, dbl_p) = (path.clone(), path.clone());
                let t = tree_icons::file(name);
                // `file-tree-v2-row` (`packages/gui-plugins/src/file/list.tsx:201-206`): 28 tall,
                // 8 in, 6 apart, 6 round; folder muted, filename base ink, both `.text-12-medium`.
                let label = row()
                    .grow(1.0)
                    .min_w(0.0)
                    .mr(12.0)
                    .children((!dir.is_empty()).then(|| text(dir.to_owned()).font(LIST_LABEL).color(pal.muted)))
                    .child(
                        text(name.to_owned())
                            .font(LIST_LABEL)
                            .color(pal.text)
                            .shrink()
                            .min_w(0.0)
                            .truncate_fade(),
                    );
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
                    .checked(highlighted)
                    .on_press_tx(move |s: &mut H, tx: &mut Tx| ops::open_workspace_file_or_page(s, &click_p, true, tx))
                    .on_double_press_tx(move |s: &mut H, tx: &mut Tx| ops::open_workspace_file_or_page(s, &dbl_p, false, tx))
                    .child(icon(t.icon(), 16.0).color(if on { t.color(pal.dark) } else { pal.icon_muted }))
                    .child(label);
                let status = kinds.as_ref().and_then(|k| k.get(path.as_str())).and_then(|k| kind_status(k));
                let r = r.children(status.map(|st| chrome::change_letter(&pal, st)));
                attach_open_in_context::<H>(r, path.clone())
            },
        );
    // The filter's flat list isn't Pierre's tree: it scrolls in the sidebar's own viewport
    // (`padding: 4px 8px 12px; overflow: auto`, session-ui `session-review-v2.css`), with
    // `scrollbar.css`'s bar, so its rows reach the pane's 8 px inset.
    let scroll = opencode_ui::scroller::scroller("files-filter-scroll").build::<H>(p, col().w_full().grow(1.0).min_h(0.0), list);
    col().w_full().grow(1.0).min_h(0.0).child(scroll)
}

/// A Pierre tree's scroller keyed `key` with the tree's bar, in a box that takes the padding a
/// caller gives it: Desktop pads above the scroller (`[data-file-tree-virtualized-root]`'s
/// `padding-block-start: 4px`), so the bar's track starts with the rows. `sid` is the session
/// whose tree it is: [`page_tree`] reads where that tree's scroller is.
pub fn tree_scroll<H: 'static>(p: &Pal, key: &str, sid: &str, rows: El) -> El {
    let watched = scroll_slot(key, sid);
    let viewport = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .on_scroll_offset(move |at: ScrollPos| TREE_SCROLLS.with_borrow_mut(|m| drop(m.insert(watched.clone(), [at.offset[1], at.viewport[1], at.max[1]]))));
    let scroll = opencode_ui::scroller::scroller(key)
        .bar(opencode_ui::scroller::Bar::Tree { fg: p.muted, bg: p.bg_base })
        .build::<H>(p, viewport, rows);
    col().w_full().grow(1.0).min_h(0.0).child(scroll)
}

thread_local! {
    /// Each tree scroller's last offset, viewport height and end, by key and session, for
    /// [`page_tree`].
    static TREE_SCROLLS: std::cell::RefCell<HashMap<String, [f32; 3]>> = std::cell::RefCell::default();
}

fn scroll_slot(key: &str, sid: &str) -> String {
    format!("{key}\0{sid}")
}

/// Pages session `sid`'s tree scroller `key` by `pages` as Chromium pages an `overflow: auto` box
/// (Desktop's Pierre tree): 0.875 of its height per page, cut to whole points (Blink
/// `ScrollableArea::PageStep` with `MinFractionToStepWhenPaging`; measured on Desktop: 534 of a
/// 611 pt tree), not Rune's 0.9 `Tx::scroll_page`. The stored offset moves with the page at once,
/// so a second page before the scroller reports again starts from the first one's end.
pub fn page_tree(tx: &mut Tx, key: &str, sid: &str, pages: i32) {
    let slot = scroll_slot(key, sid);
    let target = TREE_SCROLLS.with_borrow_mut(|m| {
        let at = m.get_mut(&slot)?;
        let [offset, height, end] = *at;
        let to = (offset + pages as f32 * (height * 0.875).trunc().max(1.0)).clamp(0.0, end.max(0.0));
        at[0] = to;
        Some(to)
    });
    match target {
        Some(y) => tx.scroll_to_y(key, y),
        None => tx.scroll_page(key, pages),
    }
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
