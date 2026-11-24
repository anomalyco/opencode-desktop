//! Word, Excel and PowerPoint previews for the Microsoft Office plugin, which drives them through
//! the `Office` service and the `ui.officePages` and `ui.officeSheet` nodes. Each open file runs
//! on a thread of its own (`worker.rs`) under a state machine the windows share
//! (`registry.rs`); pages draw as Vello scenes into effect slots (`compositor.rs`).

#[cfg(target_arch = "wasm32")]
extern crate ureq_web as ureq;

mod cjk;
mod compositor;
#[cfg(feature = "library")]
mod library;
pub mod pages;
pub mod registry;
pub mod sheet;
mod worker;

use std::path::PathBuf;
use std::sync::Mutex;

pub use compositor::OfficeCompositor;
pub use worker::{Scene, SheetFrame};

/// Host hooks the Office viewer needs from its embedding Rune app.
pub trait OfficeHost: opencode_kernel::AppEnv + opencode_kernel::AppFeedback {}

/// Process-wide configuration set by the host app at startup.
#[derive(Clone, Copy)]
pub struct OfficeEnv {
    /// Returns the base cache directory under which optional CJK font packs live (`office-fonts/`).
    pub cache_dir: fn() -> Option<PathBuf>,
    /// Optional wall-clock milliseconds since Unix epoch (for deterministic headless captures);
    /// falls back to `rune::time::SystemTime::now()`.
    pub wall_ms: Option<fn() -> i64>,
}

enum CacheDir {
    Unset,
    Fn(fn() -> Option<PathBuf>),
    Fixed(PathBuf),
}

struct EnvState {
    cache_dir: CacheDir,
    wall_ms: Option<fn() -> i64>,
}

static ENV: Mutex<EnvState> = Mutex::new(EnvState { cache_dir: CacheDir::Unset, wall_ms: None });

/// Configures the Office feature's cache directory resolver and optional wall-clock source.
pub fn configure(env: OfficeEnv) {
    let mut state = ENV.lock().unwrap_or_else(|e| e.into_inner());
    state.cache_dir = CacheDir::Fn(env.cache_dir);
    state.wall_ms = env.wall_ms;
}

/// Sets a fixed base cache directory for optional CJK font packs (`<dir>/office-fonts`).
pub fn set_cache_dir(dir: PathBuf) {
    let mut state = ENV.lock().unwrap_or_else(|e| e.into_inner());
    state.cache_dir = CacheDir::Fixed(dir);
}

pub(crate) fn cache_dir() -> Option<PathBuf> {
    let state = ENV.lock().unwrap_or_else(|e| e.into_inner());
    match &state.cache_dir {
        CacheDir::Unset => None,
        CacheDir::Fn(f) => f(),
        CacheDir::Fixed(p) => Some(p.clone()),
    }
}

#[allow(clippy::disallowed_methods)]
pub(crate) fn wall_ms() -> i64 {
    let clock = ENV.lock().unwrap_or_else(|e| e.into_inner()).wall_ms;
    if let Some(f) = clock {
        return f();
    }
    rune::time::SystemTime::now().duration_since(rune::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Whether the engines are a library of their own in this build (`library` feature), loading it
/// now if they are: the error says what is wrong with the library. Blocking.
pub fn engines_library() -> Result<bool, String> {
    #[cfg(feature = "library")]
    return library::ready().map(|()| true);
    #[cfg(not(feature = "library"))]
    Ok(false)
}

/// Takes the file threads' and the font fetch's news, and does what a sheet's grid asked of the
/// window (a copy, a toast, a scroll); true when a view should draw again.
pub fn poll(app: &mut dyn OfficeHost, tx: &mut rune::Tx) -> bool {
    let changed = registry::poll() | cjk::poll();
    for effect in registry::take_ui() {
        sheet::run(app, effect, tx);
    }
    changed
}
