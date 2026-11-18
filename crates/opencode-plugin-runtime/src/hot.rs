//! Save-to-reload: a watcher over every Luau plugin folder. Its thread rereads the files a save
//! changed and recompiles the modules among them; meanwhile the UI thread builds the next VM's
//! base (`Script::prewarm`), and the poll after the compile only swaps the VM.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, channel};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use notify::{RecursiveMode, Watcher as _};
use rune::time::Instant;
use rune_luau::{Compiled, Sources};

use crate::catalogs::Catalogs;
use crate::machines::HotEvent;
use crate::sources::{self, Origin, PrecompiledModules};

/// A plugin compiled off the UI thread, ready to swap in.
pub struct Prepared {
    pub sources: Sources,
    pub compiled: Compiled,
    pub compile: Duration,
    /// Its message catalogs, read off the UI thread too.
    pub catalogs: Catalogs,
    /// The files it was compiled from, and their SHA-256 (what a trust OK is for).
    pub files: Vec<(String, String)>,
    pub hash: String,
}

/// What the watcher's thread saw, in order: a save, then what compiling it gave.
#[cfg(not(target_arch = "wasm32"))]
type Ready = Arc<Mutex<Vec<(usize, HotEvent)>>>;

pub struct Watcher {
    #[cfg(not(target_arch = "wasm32"))]
    _watcher: notify::RecommendedWatcher,
    #[cfg(not(target_arch = "wasm32"))]
    ready: Ready,
    #[cfg(target_arch = "wasm32")]
    _private: (),
}

impl Watcher {
    /// Watches `dirs` (plugin index, folder) using the default parent `.luaurc`.
    pub fn spawn(dirs: Vec<(usize, PathBuf)>, waker: Option<rune::Waker>) -> Option<Watcher> {
        Self::spawn_with_luaurc(dirs, sources::default_luaurc(), waker)
    }

    /// Watches `dirs` (plugin index, folder) with an explicit parent `.luaurc`.
    pub fn spawn_with_luaurc(
        dirs: Vec<(usize, PathBuf)>,
        parent_luaurc: impl Into<String>,
        waker: Option<rune::Waker>,
    ) -> Option<Watcher> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (dirs, parent_luaurc, waker);
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
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
            let luaurc = parent_luaurc.into();
            std::thread::Builder::new()
                .name("luau-hot".to_owned())
                .spawn(move || compile_saves(&folders, &saved, &slot, &luaurc, waker.as_ref()))
                .ok()?;
            for (_, dir) in &dirs {
                watcher.watch(dir, RecursiveMode::Recursive).ok();
            }
            Some(Watcher { _watcher: watcher, ready })
        }
    }

    /// What the watcher's thread saw since the last call.
    pub fn take(&self) -> Vec<(usize, HotEvent)> {
        #[cfg(target_arch = "wasm32")]
        {
            Vec::new()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ready.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default()
        }
    }
}

/// The watcher's thread: one save is several file events (an editor truncates, writes,
/// renames), so it waits until the folder is quiet for a moment, rereads the files those events
/// named, and compiles once, unless the files read as they did at the last compile.
#[cfg(not(target_arch = "wasm32"))]
fn compile_saves(
    folders: &[(usize, PathBuf)],
    saved: &Receiver<(usize, PathBuf)>,
    slot: &Ready,
    parent_luaurc: &str,
    waker: Option<&rune::Waker>,
) {
    let mut files: HashMap<usize, BTreeMap<String, String>> = folders
        .iter()
        .map(|(i, dir)| (*i, sources::read_files(dir).into_iter().collect()))
        .collect();
    // What each folder compiled to last, so a save recompiles only the modules it changed. The
    // first compile happens here, before any save.
    let mut last: HashMap<usize, (String, Compiled)> = folders
        .iter()
        .filter_map(|(i, dir)| {
            let list: Vec<(String, String)> = files.get(i)?.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
            let prepared = compile_folder(dir, &list, None, parent_luaurc).ok()?;
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
            let prepared = compile_folder(dir, &list, last.get(i).map(|(_, c)| c), parent_luaurc);
            if let Ok(p) = &prepared {
                last.insert(*i, (hash, p.compiled.clone()));
            }
            push(vec![(*i, HotEvent::Compiled(prepared.map(Box::new)))]);
        }
    }
}

/// Compiles plugin folder `dir`'s window half from its `files`, reusing `previous`'s bytecode
/// for the modules that didn't change.
#[cfg(not(target_arch = "wasm32"))]
fn compile_folder(
    dir: &std::path::Path,
    files: &[(String, String)],
    previous: Option<&Compiled>,
    parent_luaurc: &str,
) -> Result<Prepared, String> {
    let start = Instant::now();
    let m = files
        .iter()
        .find(|(p, _)| p == "plugin.toml")
        .and_then(|(_, t)| sources::parse(t).ok())
        .ok_or("plugin.toml doesn't read")?;
    let folder = dir.file_name().map_or_else(|| m.id.clone(), |n| n.to_string_lossy().into_owned());
    let script = sources::script_sources_with_luaurc(files, &folder, &m.entry, parent_luaurc);
    let compiled = match previous {
        Some(previous) => script.compile_from(previous),
        None => script.compile(),
    }
    .map_err(|e| e.message)?;
    Ok(Prepared {
        sources: script,
        compiled,
        compile: start.elapsed(),
        catalogs: Catalogs::folder(files),
        files: files.to_vec(),
        hash: sources::content_hash(files),
    })
}

/// Brings plugin folder `dir`'s `files` up to date with what happened to `paths`: a file read
/// again or gone, or, for a folder, the whole plugin read again.
#[cfg(not(target_arch = "wasm32"))]
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

/// Compiles `files` into a [`Prepared`] plugin half using `build_sources` and `catalogs`.
pub fn prepare_files(
    files: Vec<(String, String)>,
    catalogs: Catalogs,
    build_sources: impl FnOnce(&[(String, String)]) -> Sources,
) -> Result<Prepared, String> {
    let start = Instant::now();
    let sources = build_sources(&files);
    let compiled = sources.compile().map_err(|e| e.message)?;
    Ok(Prepared {
        sources,
        compiled,
        compile: start.elapsed(),
        catalogs,
        hash: sources::content_hash(&files),
        files,
    })
}

/// Reads plugin `m`'s files again (a folder's from the disk: call it off the app's thread) and
/// compiles its window half using the default parent `.luaurc`.
pub fn prepare(
    origin: &Origin,
    m: &opencode_plugin_spec::Manifest,
    precompiled: PrecompiledModules,
) -> Result<Prepared, String> {
    prepare_with_luaurc(origin, m, sources::default_luaurc(), precompiled)
}

/// [`prepare`] with an explicit parent `.luaurc`.
pub fn prepare_with_luaurc(
    origin: &Origin,
    m: &opencode_plugin_spec::Manifest,
    parent_luaurc: &str,
    precompiled: PrecompiledModules,
) -> Result<Prepared, String> {
    let files = origin.dir().map_or_else(|| origin.files(), sources::read_files);
    let folder = origin
        .dir()
        .and_then(|d| d.file_name())
        .map_or_else(|| m.id.clone(), |n| n.to_string_lossy().into_owned());
    let catalogs = origin.catalogs(&files);
    prepare_files(files, catalogs, |f| origin.scripts_with_luaurc(f, &folder, &m.entry, parent_luaurc, precompiled))
}
