//! Host path and runtime capability descriptors passed explicitly into feature crates in place
//! of global `dirs` lookups.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Resolved filesystem directories supplied by the host shell.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformPaths {
    pub data_dir: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    pub logs_dir: Option<PathBuf>,
    pub updates_dir: Option<PathBuf>,
    pub downloads_dir: Option<PathBuf>,
    pub home_dir: Option<PathBuf>,
}

impl PlatformPaths {
    /// Creates a descriptor with the primary storage, cache, and update roots.
    pub fn new(data_dir: Option<PathBuf>, cache_dir: Option<PathBuf>, updates_dir: Option<PathBuf>) -> Self {
        Self {
            data_dir,
            config_dir: None,
            cache_dir,
            logs_dir: None,
            updates_dir,
            downloads_dir: None,
            home_dir: None,
        }
    }

    /// Sets the user configuration directory.
    pub fn with_config_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.config_dir = dir;
        self
    }

    /// Sets the diagnostics and log directory.
    pub fn with_logs_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.logs_dir = dir;
        self
    }

    /// Sets the user downloads directory.
    pub fn with_downloads_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.downloads_dir = dir;
        self
    }

    /// Sets the user home directory.
    pub fn with_home_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.home_dir = dir;
        self
    }

    /// Path to the motion log (`motion.jsonl`) inside `logs_dir`, if configured.
    pub fn motion_log(&self) -> Option<PathBuf> {
        Some(self.logs_dir.as_ref()?.join("motion.jsonl"))
    }
}

/// Host runtime environment metadata passed down to decoupled feature crates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub paths: PlatformPaths,
    pub is_headless: bool,
    pub is_rooted: bool,
    pub is_wasm: bool,
}

impl PlatformInfo {
    /// Creates a [`PlatformInfo`] for the current target architecture.
    pub fn new(paths: PlatformPaths, is_headless: bool) -> Self {
        Self {
            paths,
            is_headless,
            is_rooted: false,
            is_wasm: cfg!(target_arch = "wasm32"),
        }
    }

    /// Marks whether every host path lives under an isolated scratch root (`RUNE_DIRS_ROOT`).
    pub const fn with_rooted(mut self, is_rooted: bool) -> Self {
        self.is_rooted = is_rooted;
        self
    }

    /// Application data directory, if available.
    pub fn data_dir(&self) -> Option<&Path> {
        self.paths.data_dir.as_deref()
    }

    /// User configuration directory, if available.
    pub fn config_dir(&self) -> Option<&Path> {
        self.paths.config_dir.as_deref()
    }

    /// Rebuildable cache directory, if available.
    pub fn cache_dir(&self) -> Option<&Path> {
        self.paths.cache_dir.as_deref()
    }

    /// Diagnostics and logs directory, if available.
    pub fn logs_dir(&self) -> Option<&Path> {
        self.paths.logs_dir.as_deref()
    }

    /// Updater downloads and records directory, if available.
    pub fn updates_dir(&self) -> Option<&Path> {
        self.paths.updates_dir.as_deref()
    }

    /// User downloads directory, if available.
    pub fn downloads_dir(&self) -> Option<&Path> {
        self.paths.downloads_dir.as_deref()
    }

    /// User home directory, if available.
    pub fn home_dir(&self) -> Option<&Path> {
        self.paths.home_dir.as_deref()
    }
}
