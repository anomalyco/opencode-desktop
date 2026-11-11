//! Save-to-reload: a watcher over every Luau plugin folder. Its thread rereads the files a save
//! changed and recompiles the modules among them; meanwhile the UI thread builds the next VM's
//! base (`Script::prewarm`), and the poll after the compile only swaps the VM
//! (`super::reload`).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher as _};
use rune::time::Instant;
use rune_luau::{Compiled, Sources};

use super::machines::HotEvent;
use super::sources::{self, Origin};

/// A plugin compiled off the UI thread, ready to swap in.
pub struct Prepared {
    pub sources: Sources,
    pub compiled: Compiled,
    pub compile: Duration,
    /// Its message catalogs, read off the UI thread too.
    pub catalogs: super::sources::Catalogs,
    /// The files it was compiled from, and their SHA-256 (what a trust OK is for).
    pub files: Vec<(String, String)>,
    pub hash: String,
}

/// What the watcher's thread saw, in order: a save, then what compiling it gave.
type Ready = Arc<Mutex<Vec<(usize, HotEvent)>>>;

pub struct Watcher {
    _watcher: notify::RecommendedWatcher,
    ready: Ready,
}

impl Watcher {
    /// Watches `dirs` (plugin index, folder).
    pub fn spawn(dirs: Vec<(usize, PathBuf)>, waker: Option<rune::Waker>) -> Option<Watcher> {
        // A browser has no plugin folders on disk to watch.
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        if dirs.is_empty() {
            return None;
        }
        let ready: Ready = Arc::default();
        let (saves, saved) = channel::<(usize, PathBuf)>();
        let watched = dirs.clone();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if !(event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove()) {
                return;
            }
            for path in event.paths {
                if let Some((i, _)) = watched.iter().find(|(_, dir)| path.starts_with(dir)) {
                    saves.send((*i, path)).ok();
                }
            }
        })
        .ok()?;
        let slot = ready.clone();
        let folders = dirs.clone();
        std::thread::Builder::new().name("luau-hot".to_owned()).spawn(move || compile_saves(&folders, &saved, &slot, waker.as_ref())).ok()?;
        for (_, dir) in &dirs {
            watcher.watch(dir, RecursiveMode::Recursive).ok();
        }
        Some(Watcher { _watcher: watcher, ready })
    }

    /// What the watcher's thread saw since the last call.
    pub fn take(&self) -> Vec<(usize, HotEvent)> {
        self.ready.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default()
    }
}

/// The watcher's thread: one save is several file events (an editor truncates, writes,
/// renames), so it waits until the folder is quiet for a moment, rereads the files those events
/// named, and compiles once, unless the files read as they did at the last compile.
fn compile_saves(folders: &[(usize, PathBuf)], saved: &Receiver<(usize, PathBuf)>, slot: &Ready, waker: Option<&rune::Waker>) {
    let mut files: HashMap<usize, BTreeMap<String, String>> = folders.iter().map(|(i, dir)| (*i, sources::read_files(dir).into_iter().collect())).collect();
    // What each folder compiled to last, so a save recompiles only the modules it changed. The
    // first compile happens here, before any save.
    let mut last: HashMap<usize, (String, Compiled)> = folders
        .iter()
        .filter_map(|(i, dir)| {
            let list: Vec<(String, String)> = files.get(i)?.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
            let prepared = compile_folder(dir, &list, None).ok()?;
            Some((*i, (sources::content_hash(&list), prepared.compiled)))
        })
        .collect();
    let push = |events: Vec<(usize, HotEvent)>| {
        if let Ok(mut r) = slot.lock() {
            r.extend(events);
        }
        if let Some(w) = waker {
            w();
        }
    };
    while let Ok(first) = saved.recv() {
        let mut changed = vec![first];
        while let Ok(more) = saved.recv_timeout(Duration::from_millis(40)) {
            changed.push(more);
        }
        for (i, dir) in folders {
            let paths: Vec<&PathBuf> = changed.iter().filter(|(j, _)| j == i).map(|(_, p)| p).collect();
            if paths.is_empty() {
                continue;
            }
            let Some(current) = files.get_mut(i) else { continue };
            reread(dir, current, &paths);
            let list: Vec<(String, String)> = current.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
            let hash = sources::content_hash(&list);
            if last.get(i).is_some_and(|(h, _)| *h == hash) {
                continue;
            }
            push(vec![(*i, HotEvent::Changed)]);
            let prepared = compile_folder(dir, &list, last.get(i).map(|(_, c)| c));
            if let Ok(p) = &prepared {
                last.insert(*i, (hash, p.compiled.clone()));
            }
            push(vec![(*i, HotEvent::Compiled(prepared))]);
        }
    }
}

/// Compiles plugin folder `dir`'s window half from its `files`, reusing `previous`'s bytecode
/// for the modules that didn't change.
fn compile_folder(dir: &std::path::Path, files: &[(String, String)], previous: Option<&Compiled>) -> Result<Prepared, String> {
    let start = Instant::now();
    let m = files.iter().find(|(p, _)| p == "plugin.toml").and_then(|(_, t)| sources::parse(t).ok()).ok_or("plugin.toml doesn't read")?;
    let folder = dir.file_name().map_or_else(|| m.id.clone(), |n| n.to_string_lossy().into_owned());
    let script = sources::script_sources(files, &folder, &m.entry);
    let compiled = match previous {
        Some(previous) => script.compile_from(previous),
        None => script.compile(),
    }
    .map_err(|e| e.message)?;
    Ok(Prepared { sources: script, compiled, compile: start.elapsed(), catalogs: sources::catalogs(files), files: files.to_vec(), hash: sources::content_hash(files) })
}

/// Brings plugin folder `dir`'s `files` up to date with what happened to `paths`: a file read
/// again or gone, or, for a folder, the whole plugin read again.
fn reread(dir: &std::path::Path, files: &mut BTreeMap<String, String>, paths: &[&PathBuf]) {
    for path in paths {
        if path.is_dir() || *path == dir {
            *files = sources::read_files(dir).into_iter().collect();
            return;
        }
        let Some(rel) = sources::relative(dir, path).filter(|r| sources::kept(r)) else { continue };
        match std::fs::read_to_string(path) {
            Ok(text) => files.insert(rel, text),
            Err(_) => files.remove(&rel),
        };
    }
}

/// Reads plugin `m`'s files again (a folder's from the disk: call it off the app's thread) and
/// compiles its window half.
pub fn prepare(origin: &Origin, m: &opencode_plugin_spec::Manifest) -> Result<Prepared, String> {
    let start = Instant::now();
    let files = origin.dir().map_or_else(|| origin.files(), sources::read_files);
    let folder = origin.dir().and_then(|d| d.file_name()).map_or_else(|| m.id.clone(), |n| n.to_string_lossy().into_owned());
    let sources = sources::script_sources(&files, &folder, &m.entry);
    let compiled = sources.compile().map_err(|e| e.message)?;
    Ok(Prepared { sources, compiled, compile: start.elapsed(), catalogs: sources::catalogs(&files), hash: sources::content_hash(&files), files })
}
