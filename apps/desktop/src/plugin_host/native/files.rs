//! `Workspace:OpenTree`, `Workspace:OpenDocument` and `ui.codeView`: the Files panel's workspace
//! tree and open file, read from the app's file engine (`crate::files`), which lists folders,
//! loads files, highlights them and finds in them. A tree or a document belongs to the session
//! whose folder it reads: the routed one, else the first open in that folder.

use std::hash::{Hash, Hasher};

use mlua::{Lua, Table, Value};
use rune::{El, KeyEvent, Tx, col};
use rune_luau::NodeArgs;

use super::super::cx::{self, Dep};
use super::super::services::{app_now, method};
use super::{handle_field, object, p};
use crate::App;

/// `Workspace:OpenTree` and `Workspace:OpenDocument`.
pub fn build(lua: &Lua, t: &Table) -> mlua::Result<()> {
    method(lua, t, "OpenTree", |lua, directory: String| {
        let app = app_now("Workspace:OpenTree")?;
        let sid = session_for(app, &directory).ok_or_else(|| mlua::Error::runtime(format!("Workspace:OpenTree: no session is open in {directory}")))?;
        if !app.files.sessions.get(&sid).is_some_and(|w| w.ready) {
            let id = sid.clone();
            app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.ensure_workspace_files(&id);
            }));
        }
        let fields = [("kind", Value::String(lua.create_string("workspace")?)), ("sessionId", Value::String(lua.create_string(&sid)?)), ("Root", Value::String(lua.create_string(&directory)?))];
        object(lua, "FileTreeModel", &fields, super::tree::methods(lua)?)
    })?;
    method(lua, t, "OpenDocument", |lua, path: String| {
        let app = app_now("Workspace:OpenDocument")?;
        let sid = app.active_session_id().ok_or_else(|| mlua::Error::runtime("Workspace:OpenDocument: no session is open"))?.to_owned();
        let path = crate::review::data::normalize_path(&path);
        // A file not read yet, or one a failed refresh left stale, reads as it shows.
        if !app.files.sessions.get(&sid).is_some_and(|w| w.docs.contains_key(&path)) || app.files.reads.stale(&sid, &path) {
            let (id, at) = (sid.clone(), path.clone());
            app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.ensure_file_loaded(&id, &at);
            }));
        }
        let fields = [("Path", Value::String(lua.create_string(&path)?)), ("sessionId", Value::String(lua.create_string(&sid)?))];
        object(lua, "CodeDocument", &fields, document(lua)?)
    })?;
    method(lua, t, "OpenFile", |lua, (sid, path, options): (String, String, Value)| {
        let o: opencode_plugin_spec::OpenFileOptions = if options.is_nil() { Default::default() } else { mlua::FromLua::from_lua(options, lua).and_then(|d: rune_luau::Data| d.decode().map_err(mlua::Error::runtime))? };
        let lines = o.start_line.map(|a| {
            let a = a.max(1) as u32;
            (a, o.end_line.map_or(a, |b| (b.max(1) as u32).max(a)))
        });
        let preview = o.preview == Some(true);
        let path = crate::review::data::normalize_path(&path);
        app_now("Workspace:OpenFile")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.open_workspace_file_for_session(&sid, &path, preview, lines, false, tx);
        }));
        Ok(())
    })
}

/// The methods of a `CodeDocument`.
fn document(lua: &Lua) -> mlua::Result<Table> {
    let m = lua.create_table()?;
    let read = |this: &Table, what: &str| -> mlua::Result<(&'static App, String, String)> {
        let (sid, path): (String, String) = (this.raw_get("sessionId")?, this.raw_get("Path")?);
        cx::read(Dep::Files(sid.clone()));
        Ok((app_now(what)?, sid, path))
    };
    m.raw_set(
        "Version",
        lua.create_function(move |_, this: Table| {
            let (app, sid, _) = read(&this, "CodeDocument:Version")?;
            Ok((version(app, &sid) >> 11) as f64)
        })?,
    )?;
    for (name, field) in [("Missing", 0), ("Binary", 1)] {
        m.raw_set(
            name,
            lua.create_function(move |_, this: Table| {
                let (app, sid, path) = read(&this, "CodeDocument")?;
                let doc = app.files.sessions.get(&sid).and_then(|w| w.docs.get(&path));
                Ok(doc.is_some_and(|d| if field == 0 { d.missing() } else { d.binary() }))
            })?,
        )?;
    }
    m.raw_set(
        "Reload",
        lua.create_function(|_, this: Table| {
            let (sid, path): (String, String) = (this.raw_get("sessionId")?, this.raw_get("Path")?);
            // Read again in place, as a change on disk reads it: the view keeps where it is.
            app_now("CodeDocument:Reload")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                if s.files.sessions.get(&sid).is_some_and(|w| w.docs.contains_key(&path)) {
                    crate::files::watch::reload(s, &sid, &path);
                } else {
                    s.ensure_file_loaded(&sid, &path);
                }
            }));
            Ok(())
        })?,
    )?;
    method(lua, &m, "Close", |_, _: mlua::MultiValue| Ok(()))?;
    Ok(m)
}

/// The session a tree of `directory` reads: the routed one when it's there (or no folder is
/// given), else the first session open in it.
fn session_for(app: &App, directory: &str) -> Option<String> {
    let routed = app.active_session_id().and_then(|id| app.session(id));
    if let Some(s) = routed.filter(|s| directory.is_empty() || s.directory == directory) {
        return Some(s.id.clone());
    }
    app.tabs.iter().filter_map(|t| app.session(&t.id)).find(|s| s.directory == directory).map(|s| s.id.clone())
}

/// The version of what `Dep::Files(session)` reads: the tree, the finds and every open file's
/// state, with where its code is scrolled (a scroll asks for the rows it brings into view).
pub fn version(app: &App, session: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    if let Some(w) = app.files.sessions.get(session) {
        (w.ready, w.tree.visible_len(), w.known_files.len(), w.known_dirs.len(), w.expanded_dirs.len(), &w.search, &w.highlighted_filter_path).hash(&mut h);
        let mut docs: Vec<_> = w.docs.iter().collect();
        docs.sort_by(|a, b| a.0.cmp(b.0));
        for (path, d) in docs {
            (path, d.ready(), d.missing(), d.binary(), d.size_bytes(), d.wrap(), d.line_count(), d.find_open(), d.match_count(), d.active_match_index(), d.text().len()).hash(&mut h);
            (&d.find_input.text, d.find_input.focused).hash(&mut h);
            if d.find_open() { w.file_matches(path,d).hash(&mut h); }
            if let Some(r) = rune::memo::layout(&format!("file-code-{session}-{path}-scroll")) {
                format!("{r:?}").hash(&mut h);
            }
            w.md.get(path).map(|v| (v.rendered, v.matches.len(), v.active)).hash(&mut h);
            w.code_paths.get(path).map(|c| c.rev).hash(&mut h);
        }
        // A picture's decode, its zoom and pan, and the stage it fits.
        let mut images: Vec<_> = w.images.iter().collect();
        images.sort_by(|a, b| a.0.cmp(b.0));
        for (path, img) in images {
            (path, img.image.is_ready(), img.image.size(), img.shows(), format!("{:?}", img.zoom)).hash(&mut h);
        }
        // A video's state and controls, and its time at Chromium's `timeupdate` step (0.25 s).
        let mut videos: Vec<_> = w.videos.iter().collect();
        videos.sort_by(|a, b| a.0.cmp(b.0));
        for (path, v) in videos {
            let p = &v.player;
            (path, format!("{:?} {:?} {:?}", p.state(), v.overlay, v.scrub), p.is_playing(), p.muted(), p.has_sound(), p.natural_size(), p.duration().map(f64::to_bits), (p.time() * 4.0) as u64).hash(&mut h);
        }
        app.files.stage_size.get().map(f32::to_bits).hash(&mut h);
    }
    h.finish()
}

/// `ui.codeView`: the open file, its find bar and its code (or a Markdown file's rendered page).
pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    let app = cx::app().ok_or("ui.codeView draws only in a render")?;
    let pal = p(a);
    let sid = handle_field(a, "doc", "sessionId").ok_or("ui.codeView needs a document from Workspace:OpenDocument")?;
    let path = handle_field(a, "doc", "Path").unwrap_or_default();
    cx::read(Dep::Files(sid.clone()));
    let empty = crate::files::SessionFiles::default();
    let ws = app.files.sessions.get(&sid).unwrap_or(&empty);
    Ok(col().w_full().grow(1.0).min_h(0.0).child(crate::files::viewer::code_area(app, &pal, &sid, ws, &path, true)))
}

/// Lands what the file engine read.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    crate::files::poll_files(app, tx)
}

/// The open file's keys.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    crate::files::key_files(app, ev, tx)
}
