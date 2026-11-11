//! The engine's release, its availability state, and failure messages, and, with the `engine`
//! feature, resolving, verifying and installing the signed `OpenCode Browser Engine.app` sidecar
//! bundle (on Windows, an `OpenCode Browser Engine` folder holding the executable and CEF's
//! runtime files). `download` fetches the archive and `prefetch` runs the download job.

#[cfg(feature = "engine")]
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
};
use rune::time::Instant;

#[cfg(feature = "engine")]
use rune::Waker;
#[cfg(feature = "engine")]
use sha2::{Digest, Sha256};

#[cfg(feature = "engine")]
use crate::download::{self, Fetched, Pace};
#[cfg(feature = "engine")]
use crate::github_download::GithubAuth;

pub const ENGINE_VERSION: &str = "156.0.14.1";
#[cfg(not(windows))]
pub const ENGINE_BUNDLE_NAME: &str = "OpenCode Browser Engine.app";
#[cfg(not(windows))]
pub const ENGINE_BIN_REL: &str = "Contents/MacOS/opencode-browser-engine";
#[cfg(windows)]
pub const ENGINE_BUNDLE_NAME: &str = "OpenCode Browser Engine";
#[cfg(windows)]
pub const ENGINE_BIN_REL: &str = "opencode-browser-engine.exe";
/// Approximate archive size in decimal megabytes (`128_330_085` bytes), shown in the one-time download prompt.
#[cfg(not(windows))]
pub const ENGINE_DOWNLOAD_MB: u64 = 128;
/// Approximate archive size in decimal megabytes: a local `package-release.ps1` build is
/// `178_461_776` bytes. Check it again with the release build.
#[cfg(windows)]
pub const ENGINE_DOWNLOAD_MB: u64 = 178;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const ENGINE_DEFAULT_URL: Option<&str> = Some(
    "https://anoma.ly/release/opencode-rune/files/92291cae38cec0cf280e64ac3eb43cb77e0b44cdd3f304279a050fa8c69cfa18/OpenCode-Browser-Engine-156.0.14.1-macos-arm64.zip",
);
// TODO(IMPORTANT before release): build + sign (Authenticode) the Windows engine in CI with package-release.ps1, upload to the release CDN, then set ENGINE_DEFAULT_URL/SHA256 here.
// It will live next to the macOS one:
// https://anoma.ly/release/opencode-rune/files/<sha256>/OpenCode-Browser-Engine-156.0.14.1-windows-x64.zip
// Until then the background download skips itself, and `install-local.ps1` installs a local build.
#[cfg(all(windows, target_arch = "x86_64"))]
pub const ENGINE_DEFAULT_URL: Option<&str> = None;
#[cfg(not(any(all(target_os = "macos", target_arch = "aarch64"), all(windows, target_arch = "x86_64"))))]
pub const ENGINE_DEFAULT_URL: Option<&str> = None;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const ENGINE_GITHUB_FALLBACK_URL: Option<&str> = Some(
    "https://github.com/anomalyco/rune-desktop/releases/download/browser-engine-156.0.14.1/OpenCode-Browser-Engine-156.0.14.1-macos-arm64.zip",
);
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
pub const ENGINE_GITHUB_FALLBACK_URL: Option<&str> = None;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const ENGINE_DEFAULT_SHA256: Option<&str> =
    Some("92291cae38cec0cf280e64ac3eb43cb77e0b44cdd3f304279a050fa8c69cfa18");
// TODO(IMPORTANT before release): build + sign (Authenticode) the Windows engine in CI with package-release.ps1, upload to the release CDN, then set ENGINE_DEFAULT_URL/SHA256 here.
#[cfg(all(windows, target_arch = "x86_64"))]
pub const ENGINE_DEFAULT_SHA256: Option<&str> = None;
#[cfg(not(any(all(target_os = "macos", target_arch = "aarch64"), all(windows, target_arch = "x86_64"))))]
pub const ENGINE_DEFAULT_SHA256: Option<&str> = None;

pub const ERR_DOWNLOAD: &str =
    "Couldn't download the browser. Check your connection and try again.";
pub const ERR_SIGN_IN: &str =
    "Sign in to GitHub to download the browser.\nRun gh auth login in Terminal, then retry.";
pub(crate) const ERR_VERIFY: &str = "Download couldn't be verified. Try again.";
pub(crate) const ERR_INSTALL: &str = "Couldn't install the browser. Check available disk space and try again.";
pub(crate) const ERR_NO_SOURCE: &str = "Browser isn't available for this platform yet.";

pub fn err_no_access(owner: &str, repo: &str) -> String {
    format!("Your GitHub account can't see the browser download.\nAsk for access to {owner}/{repo}.")
}

#[cfg(feature = "engine")]
static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);

/// The engine's availability, one state machine that the pane and Settings read:
///
/// ```text
/// NotInstalled ─► Downloading ─► Verifying ─► Installed ─► Connecting ─► Ready
///                  │   ▲    │                     ▲
///                  ▼   │    ▼                     └─ found on disk at launch
///             Paused(reason)  Failed(retry_at) ─► Downloading (at retry_at, or Retry)
/// ```
///
/// `Downloading` is the same job whether the idle prefetch started it or the pane asked for it;
/// opening the pane only lifts its throttle.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineStatus {
    /// This build has no engine for its platform (it is built without the `engine` feature, or
    /// for a platform without a Unix-socket engine), so the pane never downloads or starts one.
    Unsupported,
    /// Not looked for yet: a launch touches nothing of the engine (its files, its socket) until a
    /// page opens or the background download starts, which then find out.
    Unchecked,
    /// Looking for an installed engine, off the UI thread (`BrowserHost::status_checked`).
    Checking,
    NotInstalled,
    Downloading { downloaded_bytes: u64, total_bytes: Option<u64> },
    /// The archive is in; its SHA-256 is checked and it unpacks into place.
    Verifying,
    /// The background download waits for the OS to allow it. Opening the pane goes ahead anyway.
    Paused(PauseReason),
    /// On disk, not running.
    Installed,
    Connecting,
    Ready,
    Failed(Failure),
}

/// Why a download or start failed, and when the background download tries again (`None`: it
/// waits for Retry).
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub message: String,
    pub retry_at: Option<Instant>,
}

impl Failure {
    pub fn retrying(message: impl Into<String>, at: Instant) -> Self {
        Self { message: message.into(), retry_at: Some(at) }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { message, retry_at: None }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Why the background download is paused, as the OS reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseReason {
    /// The connection is metered (or roaming, or over its data limit).
    Metered,
    /// The computer runs on its battery.
    OnBattery,
}

impl PauseReason {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Metered => "Paused on a metered connection",
            Self::OnBattery => "Paused on battery power",
        }
    }
}

/// The engine's folders, shared by every app that shows a browser pane (`rune::dirs`): its
/// state folder is `~/Library/Application Support/co.anomaly.opencode` on macOS and
/// `%LOCALAPPDATA%\co.anomaly.opencode` on Windows; its cache folder holds the sockets.
#[cfg(feature = "engine")]
pub(crate) fn engine_dirs() -> Option<rune::dirs::AppDirs> {
    rune::dirs::AppDirs::new("co.anomaly.opencode")
}

#[cfg(feature = "engine")]
pub fn engine_cache_root() -> PathBuf {
    engine_dirs()
        .map_or_else(|| std::env::temp_dir().join("co.anomaly.opencode"), |d| d.state().to_owned())
        .join("browser-engine")
        .join(ENGINE_VERSION)
}

/// Where a download in progress keeps its bytes, beside the versioned folders, so a download cut
/// short (the app quit, the connection dropped) resumes from where it stopped. The name carries
/// the archive's SHA-256, so a new release never resumes into an old one's bytes.
#[cfg(feature = "engine")]
pub(crate) fn partial_path(dest_root: &Path, url: &str, sha256: Option<&str>) -> PathBuf {
    let tag: String = match sha256 {
        Some(sha) => sha.chars().take(16).collect(),
        None => Sha256::digest(url.as_bytes())[..8].iter().map(|b| format!("{b:02x}")).collect(),
    };
    let ext = archive_ext(url);
    let parent = dest_root.parent().unwrap_or(dest_root);
    parent.join(format!(".{ENGINE_VERSION}-{tag}.{ext}.part"))
}

#[cfg(feature = "engine")]
pub fn resolve_installed_executable() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("OPENCODE_BROWSER_ENGINE_BIN") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(app) = std::env::var("OPENCODE_BROWSER_ENGINE_APP") {
        let bin = PathBuf::from(app).join(ENGINE_BIN_REL);
        if bin.is_file() {
            return Some(bin);
        }
    }
    let cached = engine_cache_root().join(ENGINE_BUNDLE_NAME).join(ENGINE_BIN_REL);
    if cached.is_file() {
        return Some(cached);
    }
    None
}

/// Resolves `(download_url, expected_sha256)` from env overrides or baked-in release constants.
/// `OPENCODE_BROWSER_ENGINE_URL` and `OPENCODE_BROWSER_ENGINE_SHA256` point it at another
/// archive, such as a local build served for testing.
#[cfg(feature = "engine")]
pub fn resolve_download_source() -> Option<(String, Option<String>)> {
    if let Ok(custom_url) = std::env::var("OPENCODE_BROWSER_ENGINE_URL") {
        let trimmed = custom_url.trim();
        if !trimmed.is_empty() {
            let custom_sha = std::env::var("OPENCODE_BROWSER_ENGINE_SHA256")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            return Some((trimmed.to_owned(), custom_sha));
        }
    }
    let url = ENGINE_DEFAULT_URL?.to_owned();
    let sha = std::env::var("OPENCODE_BROWSER_ENGINE_SHA256")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| ENGINE_DEFAULT_SHA256.map(str::to_owned));
    Some((url, sha))
}

#[cfg(feature = "engine")]
pub fn socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("OPENCODE_BROWSER_SOCKET") {
        return PathBuf::from(custom);
    }
    // Windows: the folder under `%LOCALAPPDATA%` is already the user's own.
    #[cfg(unix)]
    let uid = unsafe { libc::geteuid() };
    #[cfg(windows)]
    let uid = 0;
    let dir = engine_dirs().map_or_else(|| PathBuf::from("/tmp"), |d| d.cache().to_owned());
    let _ = create_private_dir(&dir);
    let exe = std::env::current_exe().unwrap_or_default();
    dir.join(socket_name_for(&exe, uid))
}

/// The engine serves one app at a time: the last to connect takes every frame and tab update,
/// and every app numbers its tabs from 1. So each app (by executable path) runs its own engine.
#[cfg(feature = "engine")]
pub fn socket_name_for(exe: &Path, uid: u32) -> String {
    format!("browser-{uid}-{}.sock", exe_tag(exe))
}

/// Eight hex digits of the executable path's hash, pairing an app's socket with its profile.
#[cfg(feature = "engine")]
pub(crate) fn exe_tag(exe: &Path) -> String {
    let digest = Sha256::digest(exe.as_os_str().as_encoded_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(feature = "engine")]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Downloads `url` in one go, checks `expected_sha256`, extracts the bundle, and swaps `dest_root`
/// atomically. A private GitHub release is fetched with the user's GitHub login.
#[cfg(feature = "engine")]
pub fn download_and_install_to(
    url: &str,
    expected_sha256: Option<&str>,
    dest_root: &Path,
    state: &Arc<Mutex<EngineStatus>>,
    waker: Option<&Waker>,
) -> Result<PathBuf, String> {
    download_and_install_with(url, expected_sha256, dest_root, state, waker, &GithubAuth::user())
}

#[cfg(feature = "engine")]
pub(crate) fn download_and_install_with(
    url: &str,
    expected_sha256: Option<&str>,
    dest_root: &Path,
    state: &Arc<Mutex<EngineStatus>>,
    waker: Option<&Waker>,
    github: &GithubAuth,
) -> Result<PathBuf, String> {
    let parent = dest_root.parent().ok_or_else(|| ERR_INSTALL.to_owned())?;
    std::fs::create_dir_all(parent).map_err(|_| ERR_INSTALL.to_owned())?;
    let staging = staging_dir(dest_root, "download");
    std::fs::create_dir_all(&staging).map_err(|_| ERR_INSTALL.to_owned())?;
    let archive = staging.join(format!("engine.{}", archive_ext(url)));
    let notify = |downloaded_bytes, total_bytes| {
        *state.lock().unwrap() = EngineStatus::Downloading { downloaded_bytes, total_bytes };
        if let Some(w) = waker {
            w();
        }
    };
    let result = match download::fetch(url, &archive, github, notify, &mut || Pace::Full) {
        Ok(Fetched::Done { sha256 }) => install_archive(url, &archive, expected_sha256, &sha256, dest_root),
        Ok(Fetched::Stopped) => Err(ERR_DOWNLOAD.to_owned()),
        Err(err) => Err(err),
    };
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// `zip` or `tar.zst`, from the archive URL.
#[cfg(feature = "engine")]
pub(crate) fn archive_ext(url: &str) -> &'static str {
    let clean_url = url.split('?').next().unwrap_or(url);
    if clean_url.ends_with(".tar.zst") { "tar.zst" } else { "zip" }
}

#[cfg(feature = "engine")]
fn staging_dir(dest_root: &Path, kind: &str) -> PathBuf {
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir_name = dest_root.file_name().and_then(|n| n.to_str()).unwrap_or(ENGINE_VERSION);
    let staging = dest_root.parent().unwrap_or(dest_root).join(format!(".{dir_name}.{kind}-{pid}-{seq}"));
    let _ = std::fs::remove_dir_all(&staging);
    staging
}

/// Removes unpacking folders left by an app that quit mid-install (older than an hour, so another
/// app's install in progress keeps its own).
#[cfg(feature = "engine")]
pub(crate) fn sweep_stale_staging(dest_root: &Path) {
    let (Some(parent), Some(name)) = (dest_root.parent(), dest_root.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    let prefixes = [format!(".{name}.staging-"), format!(".{name}.download-")];
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > std::time::Duration::from_secs(3600)));
        let file_name = entry.file_name();
        let ours = file_name.to_str().is_some_and(|n| prefixes.iter().any(|p| n.starts_with(p.as_str())));
        if ours && stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Checks the downloaded `archive` (`actual_sha256`) against `expected_sha256`, unpacks it, and
/// swaps it in as `dest_root` atomically: the old version stays until the new one is in place.
#[cfg(feature = "engine")]
pub(crate) fn install_archive(
    url: &str,
    archive: &Path,
    expected_sha256: Option<&str>,
    actual_sha256: &str,
    dest_root: &Path,
) -> Result<PathBuf, String> {
    if let Some(expected) = expected_sha256
        && !actual_sha256.eq_ignore_ascii_case(expected.trim())
    {
        return Err(ERR_VERIFY.into());
    }
    let parent = dest_root.parent().ok_or_else(|| ERR_INSTALL.to_owned())?;
    std::fs::create_dir_all(parent).map_err(|_| ERR_INSTALL.to_owned())?;
    let staging = staging_dir(dest_root, "staging");
    let result = extract_and_swap(url, archive, &staging, dest_root);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[cfg(feature = "engine")]
fn extract_and_swap(url: &str, archive: &Path, staging: &Path, dest_root: &Path) -> Result<PathBuf, String> {
    let extract_dir = staging.join("extracted");
    std::fs::create_dir_all(&extract_dir).map_err(|_| ERR_INSTALL.to_owned())?;
    let ext = archive_ext(url);
    // Windows' own bsdtar reads zip archives too (a GNU tar earlier on PATH doesn't).
    let status = if ext == "zip" && cfg!(windows) {
        let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        Command::new(system.join("System32").join("tar.exe"))
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(&extract_dir)
            .status()
    } else if ext == "zip" {
        Command::new("ditto").args(["-x", "-k"]).arg(archive).arg(&extract_dir).status()
    } else {
        Command::new("tar").args(["--zstd", "-xf"]).arg(archive).arg("-C").arg(&extract_dir).status()
    };
    if !status.is_ok_and(|s| s.success()) {
        return Err(ERR_INSTALL.into());
    }

    // Confirm the extracted bundle actually contains the executable before touching `dest_root`.
    if !extract_dir.join(ENGINE_BUNDLE_NAME).join(ENGINE_BIN_REL).is_file() {
        return Err(ERR_INSTALL.into());
    }

    let backup = staging.join("old");
    let had_existing = dest_root.exists();
    if had_existing && std::fs::rename(dest_root, &backup).is_err() {
        return Err(ERR_INSTALL.into());
    }
    if std::fs::rename(&extract_dir, dest_root).is_err() {
        if had_existing {
            let _ = std::fs::rename(&backup, dest_root);
        }
        return Err(ERR_INSTALL.into());
    }
    Ok(dest_root.join(ENGINE_BUNDLE_NAME).join(ENGINE_BIN_REL))
}
