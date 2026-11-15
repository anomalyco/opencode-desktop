//! Local installation signals for callers offering to install Hex. Dictation uses `presence`
//! separately: finding a desktop entry doesn't mean Hex can take a recording.

use std::path::Path;

pub fn installed(support: &Path) -> bool {
    super::presence(&super::discovery(support), support) != super::Presence::Missing || packaged()
}

#[cfg(target_os = "macos")]
fn packaged() -> bool {
    Path::new("/Applications/Hex.app").is_dir() || std::env::home_dir().is_some_and(|h| h.join("Applications/Hex.app").is_dir())
}

#[cfg(windows)]
fn packaged() -> bool {
    // Inno's per-user Apps & Features key (anomalyco/hex: packaging/windows/hex.iss).
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{6ED48C92-5177-41F4-B8D6-37B7B02CFDF4}_is1";
    windows_registry::CURRENT_USER.open(KEY).is_ok()
}

#[cfg(target_os = "linux")]
fn packaged() -> bool {
    use std::{env, os::unix::fs::PermissionsExt, path::PathBuf};

    fn desktop(dir: &Path) -> bool {
        ["applications/anomaly.hex.desktop", "applications/hex.desktop"].iter().any(|entry| dir.join(entry).is_file())
    }
    fn binary(dir: &Path) -> bool {
        dir.join("hex").metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }

    // .github/release/install-linux.sh (including the old launcher name) and packaging/linux/nfpm.yaml.
    let home = env::home_dir();
    let data = env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| Some(home.as_ref()?.join(".local/share")));
    let dirs = env::var_os("XDG_DATA_DIRS").filter(|p| !p.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data.as_deref().is_some_and(desktop)
        || env::split_paths(&dirs).any(|p| p.is_absolute() && desktop(&p))
        || home.is_some_and(|h| binary(&h.join(".local/bin")))
        || env::var_os("PATH").is_some_and(|paths| env::split_paths(&paths).any(|p| p.is_absolute() && binary(&p)))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn packaged() -> bool {
    false
}
