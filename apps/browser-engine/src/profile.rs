//! Where Chromium keeps cookies, logins and site data.
//!
//! The engine that holds the lock on `Application Support/co.anomaly.opencode/browser-profile`
//! keeps one profile across launches, so sign-ins stick. CEF allows one process per profile, so
//! a second engine running at the same time (a dev build next to the installed app) gets a
//! throwaway profile in Caches that is deleted when it exits.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

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
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
        let root = std::env::var_os("OPENCODE_BROWSER_PROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Library/Application Support/co.anomaly.opencode/browser-profile"));
        if let Some(lock) = lock(&root) {
            return Self { cache: root.join("Default"), root, temporary: false, _lock: Some(lock) };
        }
        let scratch = home.join("Library/Caches/co.anomaly.opencode/cef-profile");
        remove_dead_scratch(&scratch);
        let root = scratch.join(format!("pid-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        Self { cache: root.join("Default"), root, temporary: true, _lock: None }
    }
}

/// An exclusive lock on `root`, held until the returned file closes (the process exits).
fn lock(root: &PathBuf) -> Option<File> {
    std::fs::create_dir_all(root).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700));
    }
    let file = File::create(root.join("engine.lock")).ok()?;
    let ok = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    ok.then_some(file)
}

/// Throwaway profiles left by engines that crashed before deleting their own.
fn remove_dead_scratch(dir: &PathBuf) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix("pid-")).and_then(|p| p.parse::<i32>().ok())
        else {
            continue;
        };
        if unsafe { libc::kill(pid, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_engine_owns_the_profile_and_a_second_gets_a_throwaway() {
        let root = std::env::temp_dir().join(format!("engine-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let first = lock(&root).expect("first engine locks the profile");
        assert!(lock(&root).is_none(), "a second engine can't take it while the first runs");
        drop(first);
        assert!(lock(&root).is_some(), "free again once the first exits");
        let _ = std::fs::remove_dir_all(&root);
    }
}
