//! The install's registration on Windows: what `register` writes, `refresh` updates after an
//! update, and `unregister` takes away.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{EXE, Product};

mod folders;
mod process;
mod registry;
mod shortcut;

pub use process::{close_running, running_under};

/// Where the setup installs `p` by default: `%LOCALAPPDATA%\Programs\<folder>`.
pub fn default_root(p: &Product) -> Option<PathBuf> {
    Some(folders::user_programs()?.join(p.folder()))
}

/// Other builds of the app installed for the user, which share its settings and caches.
pub fn other_installs(p: &Product) -> Vec<String> {
    registry::other_entries(p)
}

/// The root `p`'s uninstall entry names, when it's installed.
pub fn installed_root(p: &Product) -> Option<PathBuf> {
    registry::entry_root(p)
}

/// Registers the install at `root`: the Start menu shortcut (and, when asked, the desktop
/// shortcut), the uninstall entry, the app's URL scheme and its notification identity. Only the
/// setup and the updater write registration; at launch the registered install only puts its own
/// scheme back when it went missing (`repair_scheme`). Returns what it
/// changed besides, for the log: `opencode://` handed back when an earlier build had taken it.
pub fn register(p: &Product, root: &Path, desktop_shortcut: bool) -> Result<Vec<String>, String> {
    let exe = root.join(EXE);
    if !exe.is_file() {
        return Err(format!("{} has no {EXE}", root.display()));
    }
    let workdir = folders::profile().unwrap_or_else(|| root.to_path_buf());
    let start = folders::programs().ok_or("no Start menu folder")?.join(link_name(p));
    shortcut::create(&start, &exe, p.id(), p.name(), &workdir)?;
    if desktop_shortcut {
        let desktop = folders::desktop().ok_or("no Desktop folder")?.join(link_name(p));
        shortcut::create(&desktop, &exe, p.id(), p.name(), &workdir)?;
    }
    registry::write_entry(p, root, size_kb(root))?;
    registry::write_scheme(p.scheme(), p.name(), &exe)?;
    registry::write_identity(p, root)?;
    let done = registry::hand_back_opencode().into_iter().collect();
    folders::associations_changed();
    Ok(done)
}

/// After an update put a new version in `root`: its version and size in Apps & features, its
/// notification identity, and `opencode://` handed back if an earlier build took it. Does nothing
/// when `root` isn't `p`'s registered install (a portable copy, a development build).
pub fn refresh(p: &Product, root: &Path) -> Result<Vec<String>, String> {
    if !registry::entry_root(p).is_some_and(|r| same_path(&r, root)) {
        return Ok(Vec::new());
    }
    registry::write_entry(p, root, size_kb(root))?;
    registry::write_identity(p, root)?;
    let done: Vec<String> = registry::hand_back_opencode().into_iter().collect();
    if !done.is_empty() {
        folders::associations_changed();
    }
    Ok(done)
}

/// Where a running copy stands: the registered install (`root` is its folder), which the setup
/// named for toasts, or another copy (a zip, a build folder, a harness run). It only reads: a
/// launch writes no registration, so a copy's toasts show under whatever the setup (or, for a
/// proof, its script) registered for its id. Call it off the UI thread, after the first frame.
pub fn settle_identity(p: &Product, root: &Path) -> Identity {
    if registry::entry_root(p).is_some_and(|r| same_path(&r, root)) {
        return Identity::Installed;
    }
    Identity::Copy
}

/// The registered install's URL scheme, put back when it went missing or now starts another
/// program, as OpenCode Desktop registers its scheme at every launch
/// (`app.setAsDefaultProtocolClient`) besides its installer. Only the install the uninstall entry
/// names writes it: another copy (a zip, a build folder, a harness run) never touches the user's
/// registration. Returns what it changed, for the log. Call it off the UI thread.
pub fn repair_scheme(p: &Product, root: &Path) -> Option<String> {
    if !registry::entry_root(p).is_some_and(|r| same_path(&r, root)) {
        return None;
    }
    let exe = root.join(EXE);
    if !exe.is_file() || registry::scheme_starts(p.scheme(), &exe) {
        return None;
    }
    registry::write_scheme(p.scheme(), p.name(), &exe).ok()?;
    folders::associations_changed();
    Some(format!("pointed {}:// at {}", p.scheme(), exe.display()))
}

/// What `settle_identity` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    /// The registered install: its scheme and toast identity are the setup's.
    Installed,
    /// Another copy.
    Copy,
}

/// Takes away everything `register` wrote outside `root`: the shortcuts (and a taskbar pin to the
/// app), the uninstall entry, the app's scheme while it still points into `root`, `opencode://`
/// when an earlier build had taken it, and the notification identity and toasts. Returns what it
/// changed, for the log.
pub fn unregister(p: &Product, root: &Path) -> Vec<String> {
    let mut done = Vec::new();
    let exe = root.join(EXE);
    let links = [folders::programs(), folders::desktop(), folders::taskbar_pins()];
    for link in links.into_iter().flatten().map(|dir| dir.join(link_name(p))) {
        let ours = shortcut::target_of(&link).is_some_and(|t| same_path(&t, &exe));
        if ours && std::fs::remove_file(&link).is_ok() {
            done.push(format!("removed {}", link.display()));
        }
    }
    done.extend(registry::remove_entry(p));
    done.extend(registry::remove_scheme_if_ours(p.scheme(), root));
    done.extend(registry::hand_back_opencode());
    done.extend(registry::remove_identity(p));
    if clear_notifications(p) {
        done.push(format!("cleared {}'s notifications from the notification center", p.id()));
    }
    folders::associations_changed();
    done
}

/// Takes the app's toasts out of the notification center, which keeps them after the app goes.
fn clear_notifications(p: &Product) -> bool {
    use windows::UI::Notifications::ToastNotificationManager;
    let id = windows::core::HSTRING::from(p.id());
    ToastNotificationManager::History().and_then(|h| h.ClearWithId(&id)).is_ok()
}

/// Removes `dir` and everything in it, retrying while a scanner or an exiting process still
/// holds a file. True once it's gone.
pub fn remove_dir(dir: &Path) -> bool {
    for _ in 0..40 {
        if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

fn link_name(p: &Product) -> String {
    format!("{}.lnk", p.name())
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// What the install takes on disk, for Apps & features.
fn size_kb(root: &Path) -> u32 {
    fn walk(dir: &Path) -> u64 {
        std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => walk(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        }).sum()
    }
    u32::try_from(walk(root) / 1024).unwrap_or(u32::MAX)
}
