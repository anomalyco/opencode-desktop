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
use super::super::services::{map, method, signal, with_app};
use super::super::vm::Vmx;
use super::{handle_field, object, p};
use crate::App;
use crate::code_view::find_bar::{FindBar, find_bar, over};
use crate::code_view::{DiffViewer, ExpandMode};
use crate::review::{ChangeMode, SessionReview};

/// A routed session with the built-in Review already on screen needs its saved source in
/// the navigation update, before its label asks for the count. Don't start an inactive VM
/// or apply the bundled plugin's store schema to a third-party panel.
pub(crate) fn prepare_route(app: &mut App, sid: &str) {
    if !app.review_shown() { return; }
    if !app.plugins.running().any(|(_, p)| p.tier == super::super::Tier::Bundled && p.manifest.id == "review") {
        // Other providers keep their own choice of source and their existing refresh timing.
        if app.side_open() { app.workspace_panes.review.refresh.step(crate::review::RefreshEvent::Immediate); }
        return;
    }
    let mode = if let Some(sr) = app.workspace_panes.review.sessions.get(sid) {
        sr.mode
    } else {
        use crate::plugin_host::{services::store, stores};
        let ns = opencode_storage::plugin_namespace("review");
        let key = stores::key("session", &store::scope_session(app, sid));
        store::session_text("review", &ns, "session", sid, &key)
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|v| ChangeMode::parse(v.get("mode")?.as_str()?)).unwrap_or_default()
    };
    app.set_session_change_mode(sid, mode);
}

/// `Diff:Open(spec)`: session `sessionId`'s changes, read from `source`.
pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let v = vmx.clone();
    method(lua, t, "Open", move |lua, s: Value| {
        let s: spec::DiffOpenSpec = Data::from_lua(s, lua)?.decode().map_err(|e| mlua::Error::runtime(format!("DiffOpenSpec: {e}")))?;
        let mode = ChangeMode::parse(s.source.as_str()).unwrap_or_default();
        let sid = s.session_id;
        with_app("Diff:Open", |app| {
            if !app.workspace_panes.review.has_read(&sid, mode) {
                let id = sid.clone();
                app.later(Box::new(move |s: &mut App, _: &mut Tx| s.set_session_change_mode(&id, mode)));
            }
            Ok(())
        })?;
        document(lua, &v, &sid)
    })?;
    method(lua, t, "Mode", |_, ()| {
        cx::read(Dep::DiffMode);
        with_app("Diff:Mode", |app| Ok(if app.workspace_panes.review.diff_mode == DiffMode::Split { "split" } else { "unified" }))
    })?;
    method(lua, t, "SetMode", |_, mode: String| {
        let mode = if mode == "unified" { DiffMode::Unified } else { DiffMode::Split };
        with_app("Diff:SetMode", |app| {
            app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.set_review_diff_mode(mode);
            }));
            Ok(())
        })
    })
}

fn read_diff<R>(sid: &str, what: &str, f: impl FnOnce(&App) -> mlua::Result<R>) -> mlua::Result<R> {
    cx::read(Dep::Diff(sid.to_owned()));
    with_app(what, f)
}

fn read_meta<R>(sid: &str, what: &str, f: impl FnOnce(&App) -> mlua::Result<R>) -> mlua::Result<R> {
    cx::read(Dep::DiffMeta(sid.to_owned()));
    with_app(what, f)
}

fn document(lua: &Lua, vmx: &Rc<Vmx>, sid: &str) -> mlua::Result<Table> {
    let m = lua.create_table()?;
    let sid_totals = sid.to_owned();
    method(lua, &m, "Totals", move |lua, ()| {
        cx::read(Dep::DiffTotals(sid_totals.clone()));
        let (files, added, removed) = with_app("DiffDocument:Totals", |app| {
            Ok(app.workspace_panes.review.sessions.get(&sid_totals).map_or((0, 0, 0), |sr| {
                let (a, d) = sr.totals();
                (sr.root_count, a, d)
            }))
        })?;
        lua_data(lua, map([("files", Data::Integer(files as i64)), ("added", Data::Integer(i64::from(added))), ("removed", Data::Integer(i64::from(removed)))]))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Files", move |lua, ()| {
        let files = read_diff(&id, "DiffDocument:Files", |app| Ok(app.workspace_panes.review.sessions.get(&id).map_or_else(Vec::new, files)))?;
        lua_data(lua, Data::List(files))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Source", move |_, ()| {
        read_meta(&id, "DiffDocument:Source", |app| Ok(app.workspace_panes.review.sessions.get(&id).map_or(ChangeMode::Git, |sr| sr.mode).name()))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Sources", move |lua, ()| {
        let modes = read_meta(&id, "DiffDocument:Sources", |app| Ok(app.workspace_panes.review.sessions.get(&id).map_or_else(|| vec![ChangeMode::Git, ChangeMode::Turn], SessionReview::available_modes)))?;
        lua_data(lua, Data::List(modes.into_iter().map(|m| Data::String(m.name().into())).collect()))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Status", move |_, ()| {
        read_meta(&id, "DiffDocument:Status", |app| Ok(status(app.workspace_panes.review.sessions.get(&id))))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Error", move |_, ()| {
        read_meta(&id, "DiffDocument:Error", |app| Ok(app.workspace_panes.review.sessions.get(&id).and_then(|sr| sr.load.failed().map(str::to_owned))))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Loading", move |_, ()| {
        read_meta(&id, "DiffDocument:Loading", |app| Ok(status(app.workspace_panes.review.sessions.get(&id)) == "loading"))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Version", move |_, ()| {
        read_diff(&id, "DiffDocument:Version", |app| Ok(app.workspace_panes.review.sessions.get(&id).map_or(0, |sr| sr.stamp) as f64))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Active", move |_, ()| {
        read_meta(&id, "DiffDocument:Active", |app| Ok(app.workspace_panes.review.sessions.get(&id).and_then(|sr| sr.active_path.clone())))
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Focus", move |_, path: String| {
        let id = id.clone();
        with_app("DiffDocument:Focus", |app| {
            app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                if let Some(sr) = s.workspace_panes.review.sessions.get_mut(&id) {
                    sr.show(&path);
                    sr.tree.reveal(&path, tx);
                }
            }));
            Ok(())
        })
    })?;
    let id = sid.to_owned();
    method(lua, &m, "SetSource", move |_, (source, _message): (String, Option<String>)| {
        let (id, mode) = (id.clone(), ChangeMode::parse(&source).ok_or_else(|| mlua::Error::runtime(format!("DiffDocument:SetSource: no source {source:?}")))?);
        with_app("DiffDocument:SetSource", |app| {
            app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.set_session_change_mode(&id, mode);
            }));
            Ok(())
        })
    })?;
    let id = sid.to_owned();
    method(lua, &m, "Retry", move |_, ()| {
        let id = id.clone();
        with_app("DiffDocument:Retry", |app| {
            app.later(Box::new(move |s: &mut App, _: &mut Tx| {
                s.workspace_panes.review.revision += 1;
                s.ensure_session_review(&id);
            }));
            Ok(())
        })
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
        Some(sr) if sr.load.no_git() => "no-vcs",
        Some(sr) if sr.load.ready() => "ready",
        Some(sr) if sr.load.offline() => "offline",
        Some(sr) if sr.load.failed().is_some() => "error",
        _ => "loading",
    }
}

/// Its files, as the server listed them (`DiffFile`).
fn files(sr: &SessionReview) -> Vec<Data> {
    sr.listed
        .iter()
        .filter_map(|path| sr.tree.get(path)?.data.as_ref())
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

/// The version of what `Dep::DiffTotals(session)` reads (`DiffDocument:Totals`).
pub fn totals_version(app: &App, session: &str) -> (usize, u32, u32) {
    app.workspace_panes.review.sessions.get(session).map_or((0, 0, 0), |sr| {
        let (a, d) = sr.totals();
        (sr.root_count, a, d)
    })
}

/// The version of what `Dep::DiffMeta(session)` reads: source, status, error, active path and
/// available sources, without hunk line counts or sticky headers.
pub fn meta_version(app: &App, session: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    let sr = app.workspace_panes.review.sessions.get(session);
    (
        sr.map_or(ChangeMode::Git, |r| r.mode).name(),
        status(sr),
        sr.and_then(|r| r.load.failed()),
        sr.and_then(|r| r.active_path.as_deref()),
        sr.map(SessionReview::available_modes),
    )
        .hash(&mut h);
    h.finish()
}

/// The version of what `Dep::DiffTree(session)` reads: the visible change-tree rows, their
/// folder expansion and counts, and each changed file's status kind (`add`, `del`, `mix`).
pub fn tree_version(app: &App, session: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    if let Some(sr) = app.workspace_panes.review.sessions.get(session) {
        let len = sr.tree.visible_len();
        len.hash(&mut h);
        for i in 0..len {
            if let Some((node, tr)) = sr.tree.row(i) {
                (tr.id, node.dir, tr.depth, node.dir && sr.tree.is_expanded(tr.id), sr.folder_label(tr.id)).hash(&mut h);
            }
        }
        sr.listed.len().hash(&mut h);
        for path in &sr.listed {
            let st = sr.tree.get(path).and_then(|n| n.data.as_ref()).map(|m| m.status);
            (path, st).hash(&mut h);
        }
    }
    h.finish()
}

pub type FindVersion = (usize, usize, String, bool);
pub type DiffVersion = (u64, bool, bool, Option<usize>, Option<FindVersion>);

/// The version of what `Dep::Diff(session)` reads, the file heading stuck over the scrolled
/// diff included: scrolling past a file's heading sticks it without changing the diff.
pub fn version(app: &App, session: &str) -> DiffVersion {
    let sr = app.workspace_panes.review.sessions.get(session);
    // The find bar the diff draws over itself: open, its matches, its field.
    let find = sr.map(|sr| &sr.diffs).filter(|d| d.find_open()).map(|d| (d.find_count(), d.find_index(), d.find.input.text.clone(), d.find.input.focused));
    (sr.map_or(0, |sr| sr.stamp), app.workspace_panes.review.diff_mode == DiffMode::Split, app.workspace_panes.review.expand_mode == ExpandMode::Expand, sr.and_then(|sr| sr.diffs.sticky()), find)
}

/// The version of what `Dep::NarrowReview(session)` reads: the files open in the narrow list
/// and the session's line comments.
pub fn narrow_version(app: &App, session: &str) -> (Vec<String>, u64) {
    let open = app.workspace_panes.review.sessions.get(session).map(|sr| sr.narrow.open_paths()).unwrap_or_default();
    (open, app.composer_ext.line_comments.version(session))
}

/// `ui.diffView`: the preview of `file`, else every file one under another; with `list`, the
/// narrow Changes view, its children at its header's start.
pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    cx::with_app(|app| {
        let pal = p(a);
        let sid = handle_field(a, "diff", "sessionId").ok_or("ui.diffView needs a diff from Diff:Open")?;
        let mode = if a.string("mode") == Some("unified") { DiffMode::Unified } else { DiffMode::Split };
        let expand = if a.flag("expand") { ExpandMode::Expand } else { ExpandMode::Collapse };
        let file = a.string("file").map(str::to_owned);
        if a.flag("list") {
            // Its rows are built here, not as they lay out: what they show is read here.
            cx::read(Dep::NarrowReview(sid.clone()));
            let title = if a.children.is_empty() { rune::el() } else { a.children.remove(0) };
            let notes = crate::line_comments::host::Notes::new(&sid, crate::line_comments::Surface::Narrow)
                as Rc<dyn crate::code_view::notes::LineNotes<App>>;
            return Ok(crate::review::mobile::view(app, &pal, &sid, title, Some(notes)));
        }
        cx::read(Dep::Diff(sid.clone()));
        let sr = app.workspace_panes.review.sessions.get(&sid);
        let shown = sr.and_then(|sr| sr.diffs.only().and_then(|f| sr.diffs.files().get(f)).map(|f| f.path().to_owned()));
        let stale_file = file.is_some() && file != shown && sr.is_some_and(|sr| file.as_deref().and_then(|f| sr.diffs.file_index(f)).is_some());
        if mode != app.workspace_panes.review.diff_mode || expand != app.workspace_panes.review.expand_mode || stale_file {
            let id = sid.clone();
            app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.set_review_diff_mode(mode);
                s.set_review_expand_mode(expand);
                if let (Some(path), Some(sr)) = (file, s.workspace_panes.review.sessions.get_mut(&id)) {
                    sr.show(&path);
                }
            }));
        }
        let (get, get_mut) = (sid.clone(), sid.clone());
        let viewer = DiffViewer::new(
            format!("review-diff-{sid}"),
            move |s: &App| s.workspace_panes.review.sessions.get(&get).map_or(&s.workspace_panes.review.empty_diffs, |r| &r.diffs),
            move |s: &mut App| {
                let review = &mut *s.workspace_panes.review;
                match review.sessions.get_mut(&get_mut) {
                    Some(r) => &mut r.diffs,
                    None => &mut review.empty_diffs,
                }
            },
        )
        .on_fetch_text(|s: &mut App, f: usize| s.request_review_file_text(f))
        .notes(crate::line_comments::host::Notes::new(&sid, crate::line_comments::Surface::Review))
        .build(app, &pal);
        let bar = sr.filter(|sr| sr.diffs.find_open()).map(|sr| {
            let d = &sr.diffs;
            let bar = FindBar {
                key: "review-find-bar",
                // Under the sticky file header (--session-title-height), 4 up.
                top: crate::code_view::HEADER_H - 4.0,
                edit: diff_find_input,
                changed: |s, sid, tx| {
                    tx.instant();
                    if let Some(sr) = s.workspace_panes.review.sessions.get_mut(sid) {
                        sr.diffs.sync_find_query();
                    }
                },
                step: |s, sid, forward, tx| {
                    if let Some(sr) = s.workspace_panes.review.sessions.get_mut(sid) {
                        sr.diffs.sync_find_query();
                        sr.diffs.step_find(forward, tx);
                    }
                },
                close: |s, sid, tx| {
                    tx.instant();
                    if let Some(sr) = s.workspace_panes.review.sessions.get_mut(sid) {
                        sr.diffs.set_find_open(false);
                    }
                },
                target: sid.clone(),
            };
            find_bar(&pal, &d.find.input, d.find_count(), d.find_index(), app.settings.spellcheck, bar)
        });
        Ok(col().w_full().grow(1.0).min_h(0.0).child(over(viewer, bar)))
    })
    .ok_or_else(|| "ui.diffView draws only in a render".to_owned())?
}

/// Session `sid`'s diff find field.
fn diff_find_input<'a>(s: &'a mut App, sid: &str) -> &'a mut rune::TextEdit {
    let review = &mut *s.workspace_panes.review;
    match review.sessions.get_mut(sid) {
        Some(sr) => &mut sr.diffs.find.input,
        None => &mut review.empty_diffs.find.input,
    }
}

/// Keys for the routed session's review while its tab shows and has the keyboard: the diff's
/// own (moving through hunks, copying the selection).
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if app.composer.focused || !app.workspace_panes.side.focus.is_focused() || !app.is_review_active() {
        return false;
    }
    let Some(sid) = app.active_session_id().map(str::to_owned) else { return false };
    app.workspace_panes.review.sessions.get_mut(&sid).is_some_and(|sr| sr.diffs.handle_key(ev, tx, &crate::code_view::DesktopKeys))
}
