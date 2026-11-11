//! Where Chromium keeps cookies, logins and site data.
//!
//! The engine that holds the lock on `co.anomaly.opencode/browser-profile` (in Application
//! Support on macOS, `%LOCALAPPDATA%` on Windows) keeps one profile across launches, so sign-ins
//! stick. CEF allows one process per profile, so a second engine running at the same time (a dev
//! build next to the installed app) gets a throwaway profile in Caches (`%TEMP%` on Windows)
//! that is deleted when it exits.

use std::fs::File;
use std::path::{Path, PathBuf};

pub struct Profile {
    /// CEF's `root_cache_path`.
    pub root: PathBuf,
    /// CEF's `cache_path`: the profile itself, inside `root`.
    pub cache: PathBuf,
    /// Deleted on exit (the throwaway profile).
    pub temporary: bool,
    _lock: Option<File>,
}

impl Profile {
    pub fn open() -> Self {
        let root = std::env::var_os("OPENCODE_BROWSER_PROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| state_dir().join("browser-profile"));
        if let Some(lock) = lock(&root) {
            return Self { cache: root.join("Default"), root, temporary: false, _lock: Some(lock) };
        }
        let scratch = scratch_dir();
        remove_dead_scratch(&scratch);
        let root = scratch.join(format!("pid-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        // Held while this engine runs, so another engine's sweep can tell it's alive.
        let lock = lock(&root);
        Self { cache: root.join("Default"), root, temporary: true, _lock: lock }
    }
}

#[cfg(unix)]
fn state_dir() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    home.join("Library/Application Support/co.anomaly.opencode")
}

#[cfg(unix)]
fn scratch_dir() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    home.join("Library/Caches/co.anomaly.opencode/cef-profile")
}

#[cfg(windows)]
fn state_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("co.anomaly.opencode")
}

#[cfg(windows)]
fn scratch_dir() -> PathBuf {
    std::env::temp_dir().join("co.anomaly.opencode").join("cef-profile")
}

/// An exclusive lock on `root`, held until the returned file closes (the process exits).
fn lock(root: &Path) -> Option<File> {
    std::fs::create_dir_all(root).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700));
    }
    // Not truncated: on Windows another engine's lock covers the file.
    let file = File::options().create(true).truncate(false).write(true).open(root.join("engine.lock")).ok()?;
    file.try_lock().is_ok().then_some(file)
}

/// Throwaway profiles left by engines that crashed before deleting their own.
fn remove_dead_scratch(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix("pid-")).and_then(|p| p.parse::<i32>().ok())
        else {
            continue;
        };
        if is_dead(&e.path(), pid) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(unix)]
fn is_dead(_dir: &Path, pid: i32) -> bool {
    (unsafe { libc::kill(pid, 0) }) != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// Each throwaway profile's engine holds its lock while it runs.
#[cfg(windows)]
fn is_dead(dir: &Path, _pid: i32) -> bool {
    lock(dir).is_some()
}

