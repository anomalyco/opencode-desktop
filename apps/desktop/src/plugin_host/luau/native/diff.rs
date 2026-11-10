//! `Diff` and `ui.diffView`: a session's changes as the review engine (`crate::review`) reads
//! them, drawn by the code view's diff (`code_view::DiffViewer`). One `DiffDocument` per session:
//! its source (`git`, `branch`, `turn`), its read, its files in the server's order and the file
//! the preview shows. `ui.diffView` previews that one file, under its header, as Desktop's
//! `SessionReviewFilePreviewV2`, or with `list` lists every file as the narrow Changes view.

use std::rc::Rc;

use mlua::{FromLua, Lua, Table, Value};
use opencode_plugin_spec as spec;
use rune::{El, KeyEvent, Tx, col};
use rune_kit::diff::{DiffMode, FileStatus};
use rune_luau::{Data, NodeArgs};

use super::super::cx::{self, Dep};
use super::super::services::{app_now, map, method, signal};
use super::super::vm::Vmx;
use super::{handle_field, object, p};
use crate::App;
use crate::code_view::{DiffViewer, ExpandMode};
use crate::review::{ChangeMode, SessionReview};

/// `Diff:Open(spec)`: session `sessionId`'s changes, read from `source`.
pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let v = vmx.clone();
    method(lua, t, "Open", move |lua, s: Value| {
        let s: spec::DiffOpenSpec = Data::from_lua(s, lua)?.decode().map_err(|e| mlua::Error::runtime(format!("DiffOpenSpec: {e}")))?;
        let app = app_now("Diff:Open")?;
        let mode = ChangeMode::parse(s.source.as_str()).unwrap_or_default();
        let sid = s.session_id;
        let engine = app.review.sessions.get(&sid);
        if engine.is_none_or(|sr| sr.mode != mode || sr.loaded_revision != Some((mode, app.review.revision))) {
            let id = sid.clone();
            app.later(Box::new(move |s: &mut App, _: &mut Tx| s.set_session_change_mode(&id, mode)));
        }
        document(lua, &v, &sid)
    })?;
    method(lua, t, "Mode", |_, ()| {
        cx::read(Dep::Diff(String::new()));
        Ok(if app_now("Diff:Mode")?.review.diff_mode == DiffMode::Split { "split" } else { "unified" })
    })?;
    method(lua, t, "SetMode", |_, mode: String| {
        let mode = if mode == "unified" { DiffMode::Unified } else { DiffMode::Split };
        app_now("Diff:SetMode")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.set_review_diff_mode(mode);
        }));
        Ok(())
    })
}

fn document(lua: &Lua, vmx: &Rc<Vmx>, sid: &str) -> mlua::Result<Table> {
    let m = lua.create_table()?;
    let with = |sid: &str| {
        let sid = sid.to_owned();
        move |what: &str| -> mlua::Result<(&'static App, String)> {
            cx::read(Dep::Diff(sid.clone()));
            Ok((app_now(what)?, sid.clone()))
        }
    };
    let read = with(sid);
    method(lua, &m, "Totals", move |lua, ()| {
        let (app, sid) = read("DiffDocument:Totals")?;
        let (files, added, removed) = app.review.sessions.get(&sid).map_or((0, 0, 0), |sr| {
            let (a, d) = sr.totals();
            (sr.meta.len(), a, d)
        });
        lua_data(lua, map([("files", Data::Integer(files as i64)), ("added", Data::Integer(i64::from(added))), ("removed", Data::Integer(i64::from(removed)))]))
    })?;
    let read = with(sid);
    method(lua, &m, "Files", move |lua, ()| {
        let (app, sid) = read("DiffDocument:Files")?;
        let files = app.review.sessions.get(&sid).map_or_else(Vec::new, files);
        lua_data(lua, Data::List(files))
    })?;
    let read = with(sid);
    method(lua, &m, "Source", move |_, ()| {
        let (app, sid) = read("DiffDocument:Source")?;
        Ok(app.review.sessions.get(&sid).map_or(ChangeMode::Git, |sr| sr.mode).name())
    })?;
    let read = with(sid);
    method(lua, &m, "Sources", move |lua, ()| {
        let (app, sid) = read("DiffDocument:Sources")?;
        let modes = app.review.sessions.get(&sid).map_or_else(|| vec![ChangeMode::Git, ChangeMode::Turn], SessionReview::available_modes);
        lua_data(lua, Data::List(modes.into_iter().map(|m| Data::String(m.name().into())).collect()))
    })?;
    let read = with(sid);
    method(lua, &m, "Status", move |_, ()| {
        let (app, sid) = read("DiffDocument:Status")?;
        Ok(status(app.review.sessions.get(&sid)))
    })?;
    let read = with(sid);
    method(lua, &m, "Loading", move |_, ()| {
        let (app, sid) = read("DiffDocument:Loading")?;
        Ok(status(app.review.sessions.get(&sid)) == "loading")
    })?;
    let read = with(sid);
    method(lua, &m, "Version", move |_, ()| {
        let (app, sid) = read("DiffDocument:Version")?;
        Ok(app.review.sessions.get(&sid).map_or(0, |sr| sr.stamp) as f64)
    })?;
    let read = with(sid);
    method(lua, &m, "Active", move |_, ()| {
        let (app, sid) = read("DiffDocument:Active")?;
        Ok(app.review.sessions.get(&sid).and_then(|sr| sr.active_path.clone()))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Focus", move |_, path: String| {
        let id = id.clone();
        app_now("DiffDocument:Focus")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if let Some(sr) = s.review.sessions.get_mut(&id) {
                sr.show(&path);
                sr.tree.reveal(&path, tx);
            }
        }));
        Ok(())
    })?;
    let id = sid.to_owned();
    method(lua, &m, "SetSource", move |_, (source, _message): (String, Option<String>)| {
        let (id, mode) = (id.clone(), ChangeMode::parse(&source).ok_or_else(|| mlua::Error::runtime(format!("DiffDocument:SetSource: no source {source:?}")))?);
        app_now("DiffDocument:SetSource")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.set_session_change_mode(&id, mode);
        }));
        Ok(())
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Retry", move |_, ()| {
        let id = id.clone();
        app_now("DiffDocument:Retry")?.later(Box::new(move |s: &mut App, _: &mut Tx| {
            s.review.revision += 1;
            s.ensure_session_review(&id);
        }));
        Ok(())
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Tree", move |lua, ()| object(lua, "FileTreeModel", &[("kind", Value::String(lua.create_string("diff")?)), ("sessionId", Value::String(lua.create_string(&id)?))], super::tree::methods(lua)?))?;
    method(lua, &m, "Close", |_, ()| Ok(()))?;
    m.raw_set("Changed", signal(lua, vmx, &format!("Diff.{sid}.Changed"))?)?;
    object(lua, "DiffDocument", &[("sessionId", Value::String(lua.create_string(sid)?))], m)
}

pub(super) fn lua_data(lua: &Lua, d: Data) -> mlua::Result<Value> {
    mlua::IntoLua::into_lua(d, lua)
}

/// How session `sr`'s read went (`DiffStatus`).
pub fn status(sr: Option<&SessionReview>) -> &'static str {
    match sr {
        Some(sr) if sr.ready && sr.no_git => "no-vcs",
        Some(sr) if sr.ready => "ready",
        _ => "loading",
    }
}

/// Its files, as the server listed them (`DiffFile`).
fn files(sr: &SessionReview) -> Vec<Data> {
    sr.listed
        .iter()
        .filter_map(|path| sr.meta.iter().find(|m| &m.path == path))
        .map(|m| {
            let status = match m.status {
                FileStatus::Added => "added",
                FileStatus::Deleted => "deleted",
                FileStatus::Renamed => "renamed",
                _ => "modified",
            };
            map([("path", Data::String(m.path.clone())), ("status", Data::String(status.into())), ("added", Data::Integer(i64::from(m.additions))), ("removed", Data::Integer(i64::from(m.deletions)))])
        })
        .collect()
}

/// The version of what `Dep::Diff(session)` reads.
pub fn version(app: &App, session: &str) -> (u64, bool, bool) {
    (app.review.sessions.get(session).map_or(0, |sr| sr.stamp), app.review.diff_mode == DiffMode::Split, app.review.expand_mode == ExpandMode::Expand)
}

/// `ui.diffView`: the preview of `file`, else every file one under another; with `list`, the
/// narrow Changes view, its children at its header's start.
pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    let app = cx::app().ok_or("ui.diffView draws only in a render")?;
    let pal = p(a);
    let sid = handle_field(a, "diff", "sessionId").ok_or("ui.diffView needs a diff from Diff:Open")?;
    let mode = if a.string("mode") == Some("unified") { DiffMode::Unified } else { DiffMode::Split };
    let expand = if a.flag("expand") { ExpandMode::Expand } else { ExpandMode::Collapse };
    let file = a.string("file").map(str::to_owned);
    if a.flag("list") {
        let title = if a.children.is_empty() { rune::el() } else { a.children.remove(0) };
        return Ok(crate::review::mobile::view(app, &pal, &sid, title));
    }
    let sr = app.review.sessions.get(&sid);
    let shown = sr.and_then(|sr| sr.diffs.only().and_then(|f| sr.diffs.files().get(f)).map(|f| f.path().to_owned()));
    let stale_file = file.is_some() && file != shown && sr.is_some_and(|sr| file.as_deref().and_then(|f| sr.diffs.file_index(f)).is_some());
    if mode != app.review.diff_mode || expand != app.review.expand_mode || stale_file {
        let id = sid.clone();
        app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.set_review_diff_mode(mode);
            s.set_review_expand_mode(expand);
            if let (Some(path), Some(sr)) = (file, s.review.sessions.get_mut(&id)) {
                sr.show(&path);
            }
        }));
    }
    let (get, get_mut) = (sid.clone(), sid.clone());
    let viewer = DiffViewer::new(
        format!("review-diff-{sid}"),
        move |s: &App| s.review.sessions.get(&get).map_or(&s.review.empty_diffs, |r| &r.diffs),
        move |s: &mut App| match s.review.sessions.get_mut(&get_mut) {
            Some(r) => &mut r.diffs,
            None => &mut s.review.empty_diffs,
        },
    )
    .on_fetch_text(|s: &mut App, f: usize| s.request_review_file_text(f))
    .build(app, &pal);
    Ok(col().w_full().grow(1.0).min_h(0.0).child(viewer))
}

/// Keys for the routed session's review while its tab shows and has the keyboard: the diff's
/// own (moving through hunks, copying the selection).
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if app.composer.focused || !app.side.focused || !app.is_review_active() {
        return false;
    }
    let Some(sid) = app.active_session_id().map(str::to_owned) else { return false };
    app.review.sessions.get_mut(&sid).is_some_and(|sr| sr.diffs.handle_key(ev, tx))
}
