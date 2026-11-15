//! Install-tip signals from `anomalyco/opencode@141be750b2`,
//! `packages/desktop/src/main/files/hex.ts` (`hexPaths` and `detectHex`).

#[cfg(any(target_os = "linux", windows))]
use std::path::PathBuf;
#[cfg(any(unix, windows))]
use std::{env, path::Path};

#[cfg(any(target_os = "linux", windows))]
fn directory(key: &str, default: PathBuf) -> PathBuf {
    env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default)
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(unix)]
fn on_path() -> bool {
    env::var_os("PATH").is_some_and(|dirs| {
        env::split_paths(&dirs).any(|p| p.is_absolute() && executable(&p.join("hex")))
    })
}

#[cfg(target_os = "macos")]
pub fn installed() -> bool {
    const BINARY: &str = "Applications/Hex.app/Contents/MacOS/hex";
    executable(&Path::new("/").join(BINARY))
        || env::home_dir().is_some_and(|home| executable(&home.join(BINARY)))
        || on_path()
}

#[cfg(windows)]
pub fn installed() -> bool {
    let roaming = directory(
        "APPDATA",
        env::home_dir().unwrap_or_default().join("AppData/Roaming"),
    );
    roaming.join("hex/current-version").is_file()
        || roaming.join("voice-control/current-version").is_file()
        || env::var("PATH")
            .unwrap_or_default()
            .split(';')
            .map(Path::new)
            .filter(|dir| dir.has_root())
            .any(|dir| dir.join("hex.com").is_file() || dir.join("hex.exe").is_file())
}

#[cfg(target_os = "linux")]
pub fn installed() -> bool {
    use std::os::unix::fs::FileTypeExt;
    let home = env::home_dir().unwrap_or_default();
    let data = directory("XDG_DATA_HOME", home.join(".local/share"));
    let support = directory("HEX_APPLICATION_SUPPORT_DIR", data.join("hex"));
    let bin = directory("HEX_INSTALL_DIR", home.join(".local/bin"));
    executable(Path::new("/usr/bin/hex"))
        || Path::new("/usr/share/applications/anomaly.hex.desktop").is_file()
        || data.join("applications/anomaly.hex.desktop").is_file()
        || executable(&bin.join("hex"))
        || executable(&support.join("current/hex"))
        || support
            .join("service.sock")
            .metadata()
            .is_ok_and(|m| m.file_type().is_socket())
        || on_path()
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
pub fn installed() -> bool {
    on_path()
}

#[cfg(not(any(unix, windows)))]
pub fn installed() -> bool {
    false
}
