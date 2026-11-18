//! Desktop hot-reload wrapper delegating to `opencode_plugin_runtime::hot`.

pub use opencode_plugin_runtime::hot::{Prepared, Watcher};

use std::path::PathBuf;

use super::sources::{self, Origin};

/// Watches `dirs` (plugin index, folder) using the desktop repository's parent `.luaurc`.
pub fn spawn(dirs: Vec<(usize, PathBuf)>, waker: Option<rune::Waker>) -> Option<Watcher> {
    Watcher::spawn_with_luaurc(dirs, sources::parent_luaurc(), waker)
}

/// Reads plugin `m`'s files again (a folder's from disk) and compiles its window half.
pub fn prepare(origin: &Origin, m: &opencode_plugin_spec::Manifest) -> Result<Prepared, String> {
    opencode_plugin_runtime::hot::prepare_with_luaurc(origin, m, sources::parent_luaurc(), sources::precompiled())
}
