//! Save-to-reload: a watcher over every Luau plugin folder. Its thread rereads the files a save
//! changed and recompiles the modules among them; meanwhile the UI thread builds the next VM's
//! base (`Script::prewarm`), and the poll after the compile only swaps the VM. A folder whose
//! `plugin.toml` was deleted is reported removed instead, for the poll to withdraw it.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, Sender, channel};
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

/// A plugin folder as the watcher keeps it: its index, the path the host gave it (which names it
/// in a compile), and that path absolute (which is how notify names an event).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct Folder {
    index: usize,
    path: PathBuf,
    root: PathBuf,
}

pub struct Watcher {
    /// Held so a folder that went away can be watched again when it comes back.
    #[cfg(not(target_arch = "wasm32"))]
    watcher: Mutex<notify::RecommendedWatcher>,
    /// Every watched folder.
    #[cfg(not(target_arch = "wasm32"))]
    folders: Vec<Folder>,
    /// The saves channel, to have a folder put back read again.
    #[cfg(not(target_arch = "wasm32"))]
    saves: Sender<(usize, PathBuf)>,
    /// The folders whose watch went with them, to watch again once they are there.
    #[cfg(not(target_arch = "wasm32"))]
    lost: Arc<Mutex<Vec<PathBuf>>>,
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
            // notify names an event by an absolute path even when the folder was watched by a
            // relative one (`--plugin-dir ./plugins`), so the watches, the paths compared with
            // them and the paths watched again are absolute. The host's path is kept beside it,
            // so a folder keeps the name its Luau root gets.
            let folders: Vec<Folder> = dirs
                .into_iter()
                .map(|(index, path)| Folder { index, root: absolute(&path), path })
                .collect();
            let ready: Ready = Arc::default();
            let (saves, saved) = channel::<(usize, PathBuf)>();
            let send = saves.clone();
            let lost: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
            let gone = lost.clone();
            let watched = folders.clone();
            let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else { return };
                if !(event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove()) {
                    return;
                }
                for path in event.paths {
                    let Some(folder) = watched.iter().find(|f| path.starts_with(&f.root)) else { continue };
                    // The folder itself went: its watch went with it, so `take` puts one back once
                    // the folder is there again.
                    if watch_lost(&event.kind, &path, &folder.root) {
                        if let Ok(mut lost) = gone.lock() {
                            lost.push(folder.root.clone());
                        }
                    }
                    send.send((folder.index, path)).ok();
                }
            })
            .ok()?;
            let slot = ready.clone();
            let thread_folders = folders.clone();
            let luaurc = parent_luaurc.into();
            std::thread::Builder::new()
                .name("luau-hot".to_owned())
                .spawn(move || compile_saves(&thread_folders, &saved, &slot, &luaurc, waker.as_ref()))
                .ok()?;
            for folder in &folders {
                watcher.watch(&folder.root, RecursiveMode::Recursive).ok();
            }
            // A folder's own removal reaches a watch on that folder on Linux and Windows; on macOS
            // the event can arrive on the parent instead, so watch each parent one level down. A
            // parent that is itself a watched plugin folder already covers its children, and
            // folders that share a parent watch it once.
            let mut parents: Vec<&std::path::Path> = folders.iter().filter_map(|f| f.root.parent()).collect();
            parents.sort();
            parents.dedup();
            for parent in parents {
                if folders.iter().any(|f| f.root.as_path() == parent) {
                    continue;
                }
                watcher.watch(parent, RecursiveMode::NonRecursive).ok();
            }
            Some(Watcher { watcher: Mutex::new(watcher), folders, saves, lost, ready })
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
            self.rearm();
            self.ready.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default()
        }
    }

    /// Watches again the folders whose watch went with them, once they are there: a new watch for
    /// the folder as it is now, and its files read again. The ones still missing wait for a later
    /// poll, so a folder removed and made again stays live.
    #[cfg(not(target_arch = "wasm32"))]
    fn rearm(&self) {
        let gone = {
            let Ok(mut lost) = self.lost.lock() else { return };
            let mut gone = std::mem::take(&mut *lost);
            gone.sort();
            gone.dedup();
            gone
        };
        for dir in gone {
            if !dir.is_dir() {
                if let Ok(mut lost) = self.lost.lock() {
                    lost.push(dir);
                }
                continue;
            }
            let folder = self.folders.iter().find(|f| f.root.as_path() == dir.as_path());
            let Ok(mut watcher) = self.watcher.lock() else { continue };
            let armed = watcher.watch(&dir, RecursiveMode::Recursive).is_ok();
            // Its parent was watched to see the folder go: it goes with the folder.
            if let Some(parent) = dir.parent()
                && !self.folders.iter().any(|f| f.root.as_path() == parent)
            {
                watcher.watch(parent, RecursiveMode::NonRecursive).ok();
            }
            drop(watcher);
            if !armed {
                if let Ok(mut lost) = self.lost.lock() {
                    lost.push(dir);
                }
                continue;
            }
            // Its files are read again, so what it holds now is what runs.
            if let Some(folder) = folder {
                self.saves.send((folder.index, folder.root.clone())).ok();
            }
        }
    }
}

/// The watcher's thread: one save is several file events (an editor truncates, writes,
/// renames), so it waits until the folder is quiet for a moment, rereads the files those events
/// named, and compiles once, unless the files read as they did at the last compile.
#[cfg(not(target_arch = "wasm32"))]
fn compile_saves(
    folders: &[Folder],
    saved: &Receiver<(usize, PathBuf)>,
    slot: &Ready,
    parent_luaurc: &str,
    waker: Option<&rune::Waker>,
) {
    let mut files: HashMap<usize, BTreeMap<String, String>> = folders
        .iter()
        .map(|f| (f.index, sources::read_files(&f.root).into_iter().collect()))
        .collect();
    // What each folder compiled to last, so a save recompiles only the modules it changed. The
    // first compile happens here, before any save.
    let mut last: HashMap<usize, (String, Compiled)> = folders
        .iter()
        .filter_map(|f| {
            let list: Vec<(String, String)> = files.get(&f.index)?.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
            let prepared = compile_folder(&f.path, &list, None, parent_luaurc).ok()?;
            Some((f.index, (sources::content_hash(&list), prepared.compiled)))
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
        for folder in folders {
            let i = folder.index;
            let paths: Vec<&PathBuf> = changed.iter().filter(|(j, _)| *j == i).map(|(_, p)| p).collect();
            if paths.is_empty() {
                continue;
            }
            // Its manifest is gone, and the plugin with it: nothing compiles. Only a missing file
            // counts, so an editor that truncates and rewrites (the file is there, empty) or a
            // folder unreadable for a moment is still a save.
            if missing(&folder.root.join("plugin.toml")) {
                // Nothing it compiled to survives; the folder's slot and what is left on disk
                // stay, so the save that puts the manifest back compiles it afresh.
                last.remove(&i);
                if let Some(current) = files.get_mut(&i) {
                    *current = sources::read_files(&folder.root).into_iter().collect();
                }
                push(vec![(i, HotEvent::Removed)]);
                continue;
            }
            let Some(current) = files.get_mut(&i) else { continue };
            reread(&folder.root, current, &paths);
            let list: Vec<(String, String)> = current.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
            let hash = sources::content_hash(&list);
            if last.get(&i).is_some_and(|(h, _)| *h == hash) {
                continue;
            }
            push(vec![(i, HotEvent::Changed)]);
            let prepared = compile_folder(&folder.path, &list, last.get(&i).map(|(_, c)| c), parent_luaurc);
            if let Ok(p) = &prepared {
                last.insert(i, (hash, p.compiled.clone()));
            }
            push(vec![(i, HotEvent::Compiled(prepared.map(Box::new)))]);
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

/// Whether `kind` for `path` says the folder at `root` went away with its watch, for [`Watcher::rearm`]
/// to take back once the folder is there again: removed, or renamed away. notify names a rename's
/// source `Modify(Name(From))` at the old path (inotify on `MOVE_SELF`/`MOVED_FROM`, Windows on the
/// old name) and drops the watch it had there. A rename's destination is `Name(To)` or, on macOS
/// where FSEvents gives no side, `Name(Any)`; it doesn't take the folder's place away.
#[cfg(not(target_arch = "wasm32"))]
fn watch_lost(kind: &notify::EventKind, path: &std::path::Path, root: &std::path::Path) -> bool {
    path == root
        && (kind.is_remove()
            || matches!(kind, notify::EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::From))))
}

/// Whether `path` is gone. Only `NotFound` counts: an editor's truncate-then-write leaves the
/// file there (empty), and any other error is the folder failing to read for a moment.
#[cfg(not(target_arch = "wasm32"))]
fn missing(path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

/// `dir` as an absolute path: notify names an event by one even when it watched a relative path,
/// and this is the path the folder is read again from. A symlink resolves to what it points at;
/// a folder that isn't there (yet) keeps the place it would be, against the current directory.
#[cfg(not(target_arch = "wasm32"))]
fn absolute(dir: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|cwd| lexical(cwd.join(dir)))
            .unwrap_or_else(|_| dir.to_path_buf())
    })
}

/// `path` with `.` dropped and `..` folded into the name before it, so two spellings of one folder
/// compare equal.
#[cfg(not(target_arch = "wasm32"))]
fn lexical(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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
    let fresh = if origin.dir().is_some() {
        let (_, source) = files.iter().find(|(path, _)| path == "plugin.toml").ok_or("plugin.toml is missing")?;
        Some(sources::parse(source)?)
    } else {
        None
    };
    let m = fresh.as_ref().unwrap_or(m);
    let folder = origin
        .dir()
        .and_then(|d| d.file_name())
        .map_or_else(|| m.id.clone(), |n| n.to_string_lossy().into_owned());
    let catalogs = origin.catalogs(&files);
    prepare_files(files, catalogs, |f| origin.scripts_with_luaurc(f, &folder, &m.entry, parent_luaurc, precompiled))
}
