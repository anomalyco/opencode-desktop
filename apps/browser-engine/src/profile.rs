//! Where Chromium keeps cookies, logins and site data.
//!
//! The engine that holds the lock on its profile (`OPENCODE_BROWSER_PROFILE`, which the app always
//! passes; else `co.anomaly.opencode/browser-profile` in Application Support on macOS,
//! `%LOCALAPPDATA%` on Windows) keeps it across launches, so sign-ins stick. CEF allows one process
//! per profile, so a second engine running at the same time gets a throwaway profile beside it
//! (`<profile>-throwaway\pid-<pid>`), deleted when it exits.

use std::fs::File;
use std::path::{Path, PathBuf};

pub struct Profile {
    /// CEF's `root_cache_path`.
    pub root: PathBuf,
    /// The lasting profile root (even when running on a throwaway profile), used to locate
    /// sibling ephemeral isolated downloads and validate human Save destinations.
    pub lasting_root: PathBuf,
    /// Sibling directory outside `root` for ephemeral isolated-context agent downloads.
    pub isolated_downloads_base: PathBuf,
    /// CEF's `cache_path`: the profile itself, inside `root`.
    pub cache: PathBuf,
    /// Deleted on exit (the throwaway profile).
    pub temporary: bool,
    _lock: Option<File>,
}

impl Profile {
    pub fn open() -> Self {
        let lasting_root = std::env::var_os("OPENCODE_BROWSER_PROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| state_dir().join("browser-profile"));
        let isolated_downloads_base = isolated_downloads_dir(&lasting_root);
        #[cfg(target_os = "macos")]
        let key = crate::mac_app::decide_cookie_key();
        let lasting = acquire_lock(&lasting_root);
        #[cfg(target_os = "macos")]
        let lasting = lasting.filter(|_| cookie_key_fits(&lasting_root, key));
        if let Some(lock) = lasting {
            return Self {
                cache: lasting_root.join("Default"),
                root: lasting_root.clone(),
                lasting_root,
                isolated_downloads_base,
                temporary: false,
                _lock: Some(lock),
            };
        }
        let scratch = scratch_dir(&lasting_root);
        remove_dead_scratch(&scratch);
        let root = scratch.join(format!("pid-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        // Held while this engine runs, so another engine's sweep can tell it's alive.
        let lock = acquire_lock(&root);
        Self {
            cache: root.join("Default"),
            root,
            lasting_root,
            isolated_downloads_base,
            temporary: true,
            _lock: lock,
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable).map(PathBuf::from).filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("set HOME or XDG directories")).join(fallback))
        .join("co.anomaly.opencode")
}

#[cfg(target_os = "linux")]
fn state_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state")
}

#[cfg(target_os = "macos")]
fn state_dir() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    home.join("Library/Application Support/co.anomaly.opencode")
}

/// macOS: whether this run may use the lasting profile. The key that encrypts its cookies is
/// recorded beside them (`opencode-cookie-key`). A run with the other key (an unsigned or
/// re-signed build, `OPENCODE_BROWSER_MOCK_KEYCHAIN=1`, a locked or missing login keychain)
/// could read none of the saved sign-ins and would write ones the next normal run cannot read, so
/// it gets a throwaway profile and leaves the lasting one untouched (READER-9 #378 should-fix 3).
#[cfg(target_os = "macos")]
fn cookie_key_fits(root: &Path, key: crate::mac_app::CookieKey) -> bool {
    let marker = root.join("opencode-cookie-key");
    let recorded = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|s| crate::mac_app::CookieKey::parse(s.trim()))
        // A profile from before the record: written with the key this build is meant to use (the
        // Keychain on Developer ID builds since `156.0.26-3`, the mock key on local builds).
        .or_else(|| root.join("Default").join("Cookies").exists().then(crate::mac_app::intended_cookie_key));
    if let Some(recorded) = recorded.filter(|r| *r != key) {
        eprintln!(
            "engine-profile: the profile's cookies use the {} key and this run the {} key; using a throwaway profile",
            recorded.name(),
            key.name()
        );
        return false;
    }
    let _ = std::fs::write(&marker, key.name());
    true
}

/// Throwaway profiles go beside the lasting one, under the same root.
pub(crate) fn scratch_dir(root: &Path) -> PathBuf {
    let name = root.file_name().map_or_else(|| "browser-profile".into(), |n| n.to_string_lossy().into_owned());
    root.with_file_name(format!("{name}-throwaway"))
}

/// Ephemeral isolated-tab agent downloads go beside the lasting profile, never inside it.
pub(crate) fn isolated_downloads_dir(lasting_root: &Path) -> PathBuf {
    let name = lasting_root
        .file_name()
        .map_or_else(|| "browser-profile".into(), |n| n.to_string_lossy().into_owned());
    lasting_root.with_file_name(format!("{name}-isolated-downloads"))
}

#[cfg(windows)]
fn state_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("co.anomaly.opencode")
}


/// The user's home directory, used to reject overly broad workspace `file_roots`.
pub fn home_dir() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .and_then(|v| v.into_string().ok())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .and_then(|v| v.into_string().ok())
            .filter(|s| !s.is_empty())
    }
}

/// An exclusive lock on `root`, held until the returned file closes (the process exits).
pub(crate) fn acquire_lock(root: &Path) -> Option<File> {
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
        if is_pid_dead(&e.path(), pid) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(unix)]
pub(crate) fn is_pid_dead(_dir: &Path, pid: i32) -> bool {
    (unsafe { libc::kill(pid, 0) }) != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// Each throwaway profile's engine holds its lock while it runs.
#[cfg(windows)]
pub(crate) fn is_pid_dead(dir: &Path, _pid: i32) -> bool {
    acquire_lock(dir).is_some()
}
