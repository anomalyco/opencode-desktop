//! `FileTreeModel` and `ui.fileTree`: the panels' file tree (`file-tree-v2`, Desktop's Pierre
//! tree), drawn from the engine's tree model. A `diff` model is a session's changed files, every
//! folder open until the user shuts it; its `filter` shows the files that match as a flat list
//! (`FileTree.List`). Rows are 28 tall, a level 22 in; a file's row runs the plugin's `onSelect`,
//! a folder's opens or shuts, and a secondary click offers the apps that open the path.

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{Lua, Table, Value};
use rune::{Color, Cursor, El, GrowthSpec, Tx, col, el, icon, row, text};
use rune_kit::diff::FileStatus;
use rune_luau::{Data, NodeArgs, Press};
use rune_theme_opencode::tree_icons;

use super::super::cx::{self, Dep};
use super::super::services::{app_now, method};
use super::{handle_field, p, string_map};
use crate::App;
use crate::palette::Pal;
use crate::review::chrome;
use crate::review::data::filter_review_files;
use crate::view::UI_SMALL;

const ROW_H: f32 = chrome::ROW_H;
const LIST_ROW_H: f32 = 28.0;
const MONO_COUNT: rune::TextStyle = rune::TextStyle::new(11.0, 450.0).mono().lh(15.0).features("'tnum' 1");

/// The methods of a `FileTreeModel`.
pub fn methods(lua: &Lua) -> mlua::Result<Table> {
    let m = lua.create_table()?;
    for (name, open) in [("Expand", true), ("Collapse", false)] {
        m.raw_set(
            name,
            lua.create_function(move |_, (this, path): (Table, String)| {
                let sid: String = this.raw_get("sessionId")?;
                let workspace = is_workspace(&this);
                app_now("FileTreeModel")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    if workspace {
                        if s.files.sessions.get(&sid).is_some_and(|w| w.expanded_dirs.contains(&path)) != open {
                            s.toggle_workspace_dir(&path);
                        }
                    } else if let Some(sr) = s.review.sessions.get_mut(&sid) {
                        sr.tree.set_expanded(&path, open);
                        sr.stamp += 1;
                    }
                }));
                Ok(())
            })?,
        )?;
    }
    m.raw_set(
        "Reveal",
        lua.create_function(|_, (this, path): (Table, String)| {
            let sid: String = this.raw_get("sessionId")?;
            let workspace = is_workspace(&this);
            app_now("FileTreeModel:Reveal")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                if workspace {
                    if let Some(w) = s.files.sessions.get_mut(&sid) {
                        let parts: Vec<&str> = path.split('/').collect();
                        (1..parts.len()).for_each(|i| {
                            w.expanded_dirs.insert(parts[..i].join("/"));
                        });
                        w.rebuild_tree();
                        w.tree.select(&path);
                    }
                } else if let Some(sr) = s.review.sessions.get_mut(&sid) {
                    sr.tree.reveal(&path, tx);
                    sr.stamp += 1;
                }
            }));
            Ok(())
        })?,
    )?;
    m.raw_set(
        "Version",
        lua.create_function(|_, this: Table| {
            let sid: String = this.raw_get("sessionId")?;
            let app = app_now("FileTreeModel:Version")?;
            if is_workspace(&this) {
                cx::read(Dep::Files(sid.clone()));
                return Ok((super::files::version(app, &sid) >> 11) as f64);
            }
            cx::read(Dep::Diff(sid.clone()));
            Ok(app.review.sessions.get(&sid).map_or(0, |sr| sr.stamp) as f64)
        })?,
    )?;
    m.raw_set(
        "Find",
        lua.create_function(|lua, (this, query): (Table, String)| {
            let sid: String = this.raw_get("sessionId")?;
            let app = app_now("FileTreeModel:Find")?;
            let found = if is_workspace(&this) {
                cx::read(Dep::Files(sid.clone()));
                app.filtered_files(&sid, &query)
            } else {
                cx::read(Dep::Diff(sid.clone()));
                let all: Vec<String> = app.review.sessions.get(&sid).map(|r| r.meta.iter().map(|m| m.path.clone()).collect()).unwrap_or_default();
                filter_review_files(&all, &query)
            };
            lua.create_sequence_from(found)
        })?,
    )?;
    for name in ["SetFilter", "Refresh", "Close"] {
        method(lua, &m, name, |_, _: mlua::MultiValue| Ok(()))?;
    }
    let _ = Value::Nil;
    Ok(m)
}

/// Whether `this` is a workspace's tree (`Workspace:OpenTree`) rather than a diff's.
fn is_workspace(this: &Table) -> bool {
    this.raw_get::<Option<String>>("kind").ok().flatten().as_deref() == Some("workspace")
}

/// `ui.fileTree`.
pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    let app = cx::app().ok_or("ui.fileTree draws only in a render")?;
    let pal = p(a);
    let sid = handle_field(a, "tree", "sessionId").ok_or("ui.fileTree needs a tree from DiffDocument:Tree or Workspace:OpenTree")?;
    let kinds = Rc::new(string_map(a.data("kinds")));
    let selected = a.string("selectedPath").map(str::to_owned);
    let highlighted = a.string("highlightedPath").map(str::to_owned);
    let filter = a.string("filter").unwrap_or_default().trim().to_owned();
    if handle_field(a, "tree", "kind").as_deref() == Some("workspace") {
        return Ok(workspace(app, &pal, a, &sid, kinds, selected, &filter));
    }
    let pick = a.press_later("onSelect");
    Ok(if filter.is_empty() { change_tree(app, &pal, &sid, kinds, selected, pick) } else { filtered(app, &pal, &sid, &filter, kinds, selected, highlighted, pick) })
}

/// A workspace's tree, or with `filter` the files the server and the tree find; with `narrow`,
/// the narrow Files view (its open files over the tree, or the open file).
fn workspace(app: &App, p: &Pal, a: &NodeArgs, sid: &str, kinds: Rc<HashMap<String, String>>, selected: Option<String>, filter: &str) -> El {
    cx::read(Dep::Files(sid.to_owned()));
    if a.flag("narrow") {
        let project = app.session(sid).map(|s| s.project.clone()).unwrap_or_default();
        return crate::files::narrow::view(app, p, sid, &project, a.number("width").unwrap_or(0.0) as f32);
    }
    let empty = crate::files::SessionFiles::default();
    let ws = app.files.sessions.get(sid).unwrap_or(&empty);
    if !ws.ready {
        return col().key("files-tree-loading").w_full().grow(1.0).px(10.0).pt(8.0).child(text(opencode_i18n::t!("ui.sessionReview.image.loading")).font(UI_SMALL).color(p.muted));
    }
    if filter.is_empty() {
        return crate::files::view::workspace_tree_view(p, sid, ws, selected.as_deref(), Some(kinds));
    }
    if ws.search.asked() != Some(filter) {
        let (id, q) = (sid.to_owned(), filter.to_owned());
        app.later(Box::new(move |s: &mut App, _: &mut Tx| s.request_file_search(&id, &q)));
    }
    crate::files::view::filtered_files_view(app, p, sid, selected.as_deref(), filter, Some(kinds))
}

/// The change letter of `kind` (`add`, `del`, `mix`) at a row's end.
fn letter(p: &Pal, kind: &str) -> Option<El> {
    let status = match kind {
        "add" => FileStatus::Added,
        "del" => FileStatus::Deleted,
        "mix" => FileStatus::Modified,
        _ => return None,
    };
    Some(chrome::change_letter(p, status))
}

fn picked(row: El, pick: &Option<Press>, path: &str) -> El {
    match pick {
        Some(press) => press.attach(row, vec![("path", Data::String(path.to_owned()))]),
        None => row,
    }
}

fn change_tree(app: &App, p: &Pal, sid: &str, kinds: Rc<HashMap<String, String>>, selected: Option<String>, pick: Option<Press>) -> El {
    let count = app.review.sessions.get(sid).map_or(0, |r| r.tree.visible_len());
    let (sid_keys, sid_rows) = (sid.to_owned(), sid.to_owned());
    let pal = *p;
    let selected = Rc::new(selected);
    let list = col()
        .key("review-tree-rows")
        .w_full()
        .px(8.0)
        .pb(8.0)
        .growth(GrowthSpec::default())
        .role("tree")
        .label(opencode_i18n::t!("settings.timeline.row.turnDiffs.title"))
        // One Tab stop; Up and Down move through the rows, Right and Left open, close, step in and out.
        .focus_group()
        .on_key({
            let sid = sid.to_owned();
            move |s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx| change_tree_key(s, &sid, ev, tx)
        })
        .virtual_rows(
            count,
            ROW_H,
            move |s: &App, i| s.review.sessions.get(&sid_keys).and_then(|r| r.tree.row(i).map(|(_, tr)| Cow::Owned(format!("rt:{}", tr.id)))).unwrap_or(Cow::Borrowed("rt:empty")),
            move |s: &App, i| {
                let Some((node, tr)) = s.review.sessions.get(&sid_rows).and_then(|r| r.tree.row(i)) else { return el().h(ROW_H) };
                let (id, dir) = (tr.id.to_owned(), node.dir);
                let glyph = (!dir).then(|| tree_icons::file(node.name.as_str()));
                let on = selected.as_deref() == Some(tr.id);
                let row = chrome::tree_row(&pal, node.name.as_str(), tr.depth, tr.expanded.filter(|_| dir), glyph, on).children(kinds.get(tr.id).and_then(|k| letter(&pal, k)));
                let row = if dir {
                    let sid = sid_rows.clone();
                    row.on_press_tx(move |s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        if let Some(sr) = s.review.sessions.get_mut(&sid) {
                            sr.tree.toggle(&id);
                            sr.stamp += 1;
                        }
                    })
                } else {
                    picked(row, &pick, tr.id)
                };
                crate::open_in::context(row, tr.id.to_owned())
            },
        );
    // Desktop's Pierre tree scrolls itself, with its own bar (`session-review-v2.css:213-220`;
    // `--trees-fg` muted on `bg-base`, `gui-plugins/src/file/pierre-tree.tsx:36-38`).
    crate::files::view::tree_scroll(p, "review-tree", list)
}

/// The change tree's Right and Left, as the Files tree's (`files::view::tree_key`).
fn change_tree_key(s: &mut App, sid: &str, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    let right = matches!(ev.key, rune::Key::Right);
    if !(right || matches!(ev.key, rune::Key::Left)) || ev.mods != rune::Mods::default() { return false; }
    let Some(id) = tx.focused().and_then(|k| k.strip_prefix("rt:")).map(str::to_owned) else { return false };
    let Some(sr) = s.review.sessions.get_mut(sid) else { return false };
    let dir = sr.tree.has_children(&id);
    let open = sr.tree.is_expanded(&id);
    match (right, dir, open) {
        (true, true, false) | (false, true, true) => {
            sr.tree.toggle(&id);
            sr.stamp += 1;
        }
        (true, true, true) => match sr.tree.children(Some(&id)).next() {
            Some(child) => tx.focus(format!("rt:{child}"), true),
            None => return false,
        },
        (false, _, _) => match sr.tree.parent(&id) {
            Some(parent) => tx.focus(format!("rt:{parent}"), true),
            None => return false,
        },
        (true, false, _) => return false,
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn filtered(app: &App, p: &Pal, sid: &str, query: &str, _kinds: Rc<HashMap<String, String>>, selected: Option<String>, highlighted: Option<String>, pick: Option<Press>) -> El {
    let all: Vec<String> = app.review.sessions.get(sid).map(|r| r.meta.iter().map(|m| m.path.clone()).collect()).unwrap_or_default();
    let found = filter_review_files(&all, query);
    if found.is_empty() {
        return col().key("review-filter-empty").w_full().grow(1.0).px(10.0).pt(8.0).child(text(opencode_i18n::t!("rune.files.noMatches")).font(UI_SMALL).color(p.muted));
    }
    let keys = found.clone();
    let sid_rows = sid.to_owned();
    let pal = *p;
    let list = col().key("review-filter-rows").w_full().px(4.0).pb(8.0).growth(GrowthSpec::default()).role("listbox").virtual_rows(
        found.len(),
        LIST_ROW_H,
        move |_s: &App, i| Cow::Owned(format!("rf:{}", keys[i])),
        move |s: &App, i| {
            let path = &found[i];
            let meta = s.review.sessions.get(&sid_rows).and_then(|sr| sr.meta.iter().find(|m| &m.path == path));
            let on = selected.as_deref() == Some(path.as_str()) || highlighted.as_deref().map_or(i == 0, |h| h == path);
            let (dir, name) = (Some(opencode_kit::path::parent(path)).filter(|d| !d.is_empty()), opencode_kit::path::basename(path));
            let t = tree_icons::file(name);
            let mut r = row()
                .w_full()
                .h(LIST_ROW_H)
                .px(6.0)
                .gap(5.0)
                .center_y()
                .radius(5.0)
                .fill(if on { pal.row_active } else { Color::TRANSPARENT })
                .hover(pal.hover)
                .hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT)
                .cursor(Cursor::Pointer)
                .role("option")
                .child(icon(t.icon(), 16.0).color(if on { t.color(pal.dark) } else { pal.icon_muted }))
                .child(text(name.to_owned()).font(UI_SMALL).color(pal.text).shrink().min_w(0.0).truncate_fade());
            r = match dir {
                Some(d) => r.child(text(d.to_owned()).font(UI_SMALL).color(pal.faint).grow(1.0).shrink().min_w(0.0).truncate_fade()),
                None => r.child(el().grow(1.0)),
            };
            if let Some(m) = meta {
                r = r.child(counts(&pal, m.additions, m.deletions));
            }
            crate::open_in::context(picked(r, &pick, path), path.clone())
        },
    );
    // The filter's rows in the same Pierre tree as the folders, with its bar.
    crate::files::view::tree_scroll(p, "review-filter-scroll", list)
}

fn counts(p: &Pal, adds: u32, dels: u32) -> El {
    let mut r = row().key("stats").gap(4.0).center_y();
    if adds > 0 {
        r = r.child(text(format!("+{adds}")).dir(rune::Direction::Ltr).font(MONO_COUNT).color(p.success));
    }
    if dels > 0 {
        r = r.child(text(format!("\u{2212}{dels}")).font(MONO_COUNT).color(p.danger));
    }
    r
}
