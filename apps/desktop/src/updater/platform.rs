//! What the machine's platform effects run (Desktop's `platform.ts`): blocking, on the updater's
//! worker thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait Platform: Send + Sync + 'static {
    /// The running build's version, as the feed names releases.
    fn version(&self) -> &str;

    /// The release to install, `None` when the running build is current.
    fn check(&self) -> Result<Option<String>, String>;

    /// Downloads, verifies and stages the release the last check found; `progress` hears the
    /// download's bytes so far and its total.
    fn stage(&self, progress: &dyn Fn(u64, u64)) -> Result<(), String>;

    /// Hands off to the staged update; once this returns `Ok` the app must quit.
    fn install(&self) -> Result<(), String>;

    /// The app quits with an update staged: install it on the way out, without starting again.
    fn install_on_quit(&self) {}

    /// Install as soon as an update is ready.
    fn auto_install(&self) -> bool {
        false
    }
}

/// This build's updater, or `None` where none runs (a development build, a build without a
/// release feed, a bare `cargo` binary on macOS).
pub fn make(exe: &Path, version: &str, build_channel: &str, updates: PathBuf) -> Option<Arc<dyn Platform>> {
    #[cfg(target_os = "macos")]
    {
        let _ = version;
        super::mac::platform(exe, build_channel, updates).map(|p| Arc::new(p) as Arc<dyn Platform>)
    }
    #[cfg(windows)]
    {
        super::windows::platform(exe, version, build_channel, updates).map(|p| Arc::new(p) as Arc<dyn Platform>)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (exe, version, build_channel, updates);
        None
    }
}
