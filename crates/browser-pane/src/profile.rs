//! Where each app's engine keeps cookies, logins and site data.
//!
//! CEF lets one engine use a profile at a time, and the engine falls back to a throwaway profile
//! when its profile is locked. So each app (by executable path, like its socket) keeps its own
//! lasting profile. An app can choose another dir with `BrowserHost::set_profile_dir`, and
//! `OPENCODE_BROWSER_PROFILE` in the app's environment wins over both.

use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;
use rune::time::Instant;

use crate::installer::exe_tag;

pub const PROFILE_ENV: &str = "OPENCODE_BROWSER_PROFILE";

/// How long a spawn waits for a held profile to free up: the engine from before per-app
/// engines stays up 15 s after its last app disconnects, holding the shared profile.
pub(crate) const UNLOCK_WAIT: Duration = Duration::from_secs(20);

fn profiles_root() -> PathBuf {
    crate::installer::engine_dirs().map_or_else(|| PathBuf::from("/tmp"), |d| d.state().to_owned())
}

/// The profile every app shared before each got its own; the engine's own default.
pub fn shared_profile_dir() -> PathBuf {
    profiles_root().join("browser-profile")
}

/// The app's own lasting profile, tagged like its socket (`browser-profile-<tag>`).
pub fn profile_dir_for(root: &Path, exe: &Path) -> PathBuf {
    root.join(format!("browser-profile-{}", exe_tag(exe)))
}

/// The profile the engine gets: the environment's, then the app's choice, then its own.
pub(crate) fn resolve(env: Option<OsString>, chosen: Option<&Path>, exe: &Path) -> PathBuf {
    if let Some(dir) = env.filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    chosen.map_or_else(|| profile_dir_for(&profiles_root(), exe), Path::to_path_buf)
}

pub(crate) fn for_spawn(chosen: Option<&Path>) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    resolve(std::env::var_os(PROFILE_ENV), chosen, &exe)
}

/// Waits up to `timeout` for no engine to hold `dir`'s lock; true once it's free.
pub(crate) fn wait_until_free(dir: &Path, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if is_free(dir) {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether no engine holds `dir/engine.lock` (the engine's `File::lock`: `flock` on Unix,
/// `LockFileEx` on Windows).
pub(crate) fn is_free(dir: &Path) -> bool {
    let Ok(file) = File::open(dir.join("engine.lock")) else {
        return true;
    };
    file.try_lock().is_ok()
}
