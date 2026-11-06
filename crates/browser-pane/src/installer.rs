//! The engine's release, status and failure messages, and, with the `engine` feature on a Unix
//! host, resolving or downloading the signed `OpenCode Browser Engine.app` sidecar bundle.

#[cfg(all(feature = "engine", unix))]
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
};

#[cfg(all(feature = "engine", unix))]
use rune::Waker;
#[cfg(all(feature = "engine", unix))]
use sha2::{Digest, Sha256};

#[cfg(all(feature = "engine", unix))]
use crate::github_download::{self, GithubAuth};

pub const ENGINE_VERSION: &str = "156.0.14.1";
pub const ENGINE_BUNDLE_NAME: &str = "OpenCode Browser Engine.app";
pub const ENGINE_BIN_REL: &str = "Contents/MacOS/opencode-browser-engine";
/// Approximate archive size in decimal megabytes (`128_330_085` bytes), shown in the one-time download prompt.
pub const ENGINE_DOWNLOAD_MB: u64 = 128;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const ENGINE_DEFAULT_URL: Option<&str> = Some(
    "https://anoma.ly/release/opencode-rune/files/92291cae38cec0cf280e64ac3eb43cb77e0b44cdd3f304279a050fa8c69cfa18/OpenCode-Browser-Engine-156.0.14.1-macos-arm64.zip",
);
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
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
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
pub const ENGINE_DEFAULT_SHA256: Option<&str> = None;

pub const ERR_DOWNLOAD: &str =
    "Couldn't download the browser. Check your connection and try again.";
pub const ERR_SIGN_IN: &str =
    "Sign in to GitHub to download the browser.\nRun gh auth login in Terminal, then retry.";

pub fn err_no_access(owner: &str, repo: &str) -> String {
    format!("Your GitHub account can't see the browser download.\nAsk for access to {owner}/{repo}.")
}

#[cfg(all(feature = "engine", unix))]
static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq)]
pub enum EngineStatus {
    /// This build has no engine for its platform (it is built without the `engine` feature, or
    /// for a platform without a Unix-socket engine), so the pane never downloads or starts one.
    Unsupported,
    NotInstalled,
    Downloading { downloaded_bytes: u64, total_bytes: Option<u64> },
    Connecting,
    Ready,
    Failed(String),
}

/// The engine's folders, shared by every app that shows a browser pane (`rune::dirs`): its
/// state folder is `~/Library/Application Support/co.anomaly.opencode` on macOS and
/// `%LOCALAPPDATA%\co.anomaly.opencode` on Windows; its cache folder holds the sockets.
#[cfg(all(feature = "engine", unix))]
pub(crate) fn engine_dirs() -> Option<rune::dirs::AppDirs> {
    rune::dirs::AppDirs::new("co.anomaly.opencode")
}

#[cfg(all(feature = "engine", unix))]
pub fn engine_cache_root() -> PathBuf {
    engine_dirs()
        .map_or_else(|| std::env::temp_dir().join("co.anomaly.opencode"), |d| d.state().to_owned())
        .join("browser-engine")
        .join(ENGINE_VERSION)
}

#[cfg(all(feature = "engine", unix))]
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
#[cfg(all(feature = "engine", unix))]
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

#[cfg(all(feature = "engine", unix))]
pub fn socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("OPENCODE_BROWSER_SOCKET") {
        return PathBuf::from(custom);
    }
    let uid = unsafe { libc::geteuid() };
    let dir = engine_dirs().map_or_else(|| PathBuf::from("/tmp"), |d| d.cache().to_owned());
    let _ = create_private_dir(&dir);
    let exe = std::env::current_exe().unwrap_or_default();
    dir.join(socket_name_for(&exe, uid))
}

/// The engine serves one app at a time: the last to connect takes every frame and tab update,
/// and every app numbers its tabs from 1. So each app (by executable path) runs its own engine.
#[cfg(all(feature = "engine", unix))]
pub fn socket_name_for(exe: &Path, uid: u32) -> String {
    format!("browser-{uid}-{}.sock", exe_tag(exe))
}

/// Eight hex digits of the executable path's hash, pairing an app's socket with its profile.
#[cfg(all(feature = "engine", unix))]
pub(crate) fn exe_tag(exe: &Path) -> String {
    let digest = Sha256::digest(exe.as_os_str().as_encoded_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(all(feature = "engine", unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Downloads, SHA-256 verifies, and atomically installs `OpenCode Browser Engine.app` on a background thread.
#[cfg(all(feature = "engine", unix))]
pub fn start_download(
    state: Arc<Mutex<EngineStatus>>,
    waker: Option<Waker>,
    on_ready: Box<dyn FnOnce(PathBuf) + Send + 'static>,
) {
    let Some((url, expected_sha)) = resolve_download_source() else {
        *state.lock().unwrap() =
            EngineStatus::Failed("Browser isn't available for this platform yet.".into());
        if let Some(w) = waker {
            w();
        }
        return;
    };

    *state.lock().unwrap() = EngineStatus::Downloading {
        downloaded_bytes: 0,
        total_bytes: None,
    };
    if let Some(w) = &waker {
        w();
    }

    let dest_root = engine_cache_root();
    std::thread::Builder::new()
        .name("opencode-browser-install".into())
        .spawn(move || {
            let result = download_and_install_to(
                &url,
                expected_sha.as_deref(),
                &dest_root,
                &state,
                waker.as_ref(),
            );
            match result {
                Ok(bin) => on_ready(bin),
                Err(err) => {
                    *state.lock().unwrap() = EngineStatus::Failed(err);
                    if let Some(w) = waker {
                        w();
                    }
                }
            }
        })
        .ok();
}

#[cfg(all(feature = "engine", unix))]
const ERR_VERIFY: &str = "Download couldn't be verified. Try again.";
#[cfg(all(feature = "engine", unix))]
const ERR_INSTALL: &str = "Couldn't install the browser. Check available disk space and try again.";

/// Downloads `url`, checks `expected_sha256`, extracts the bundle, and swaps `dest_root` atomically.
/// A private GitHub release is fetched with the user's GitHub login.
#[cfg(all(feature = "engine", unix))]
pub fn download_and_install_to(
    url: &str,
    expected_sha256: Option<&str>,
    dest_root: &Path,
    state: &Arc<Mutex<EngineStatus>>,
    waker: Option<&Waker>,
) -> Result<PathBuf, String> {
    download_and_install_with(url, expected_sha256, dest_root, state, waker, &GithubAuth::user())
}

#[cfg(all(feature = "engine", unix))]
pub(crate) fn download_and_install_with(
    url: &str,
    expected_sha256: Option<&str>,
    dest_root: &Path,
    state: &Arc<Mutex<EngineStatus>>,
    waker: Option<&Waker>,
    github: &GithubAuth,
) -> Result<PathBuf, String> {
    let parent = dest_root
        .parent()
        .ok_or_else(|| ERR_INSTALL.to_owned())?;
    std::fs::create_dir_all(parent).map_err(|_| ERR_INSTALL.to_owned())?;

    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir_name = dest_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(ENGINE_VERSION);
    let staging = parent.join(format!(".{dir_name}.staging-{pid}-{seq}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|_| ERR_INSTALL.to_owned())?;

    let clean_url = url.split('?').next().unwrap_or(url);
    let ext = if clean_url.ends_with(".tar.zst") { "tar.zst" } else { "zip" };
    let archive_path = staging.join(format!("engine.{ext}"));

    let mut resp = match github_download::open(url, github).or_else(|primary_err| {
        match ENGINE_GITHUB_FALLBACK_URL.filter(|_| Some(url) == ENGINE_DEFAULT_URL) {
            Some(fallback) => github_download::open(fallback, github).map_err(|_| primary_err),
            None => Err(primary_err),
        }
    }) {
        Ok(r) => r,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };
    let total_bytes = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    *state.lock().unwrap() = EngineStatus::Downloading {
        downloaded_bytes: 0,
        total_bytes,
    };
    if let Some(w) = waker {
        w();
    }

    let mut reader = resp.body_mut().as_reader();
    let mut file = match std::fs::File::create(&archive_path) {
        Ok(f) => f,
        Err(_) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ERR_INSTALL.into());
        }
    };
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded = 0u64;
    let mut last_notify = 0u64;

    loop {
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(ERR_DOWNLOAD.into());
            }
        };
        if n == 0 {
            break;
        }
        if std::io::Write::write_all(&mut file, &buf[..n]).is_err() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ERR_INSTALL.into());
        }
        hasher.update(&buf[..n]);
        downloaded += n as u64;
        if downloaded - last_notify >= 256 * 1024 {
            last_notify = downloaded;
            *state.lock().unwrap() = EngineStatus::Downloading {
                downloaded_bytes: downloaded,
                total_bytes,
            };
            if let Some(w) = waker {
                w();
            }
        }
    }
    drop(file);

    *state.lock().unwrap() = EngineStatus::Downloading {
        downloaded_bytes: downloaded,
        total_bytes: total_bytes.or(Some(downloaded)),
    };
    if let Some(w) = waker {
        w();
    }

    if let Some(expected) = expected_sha256 {
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ERR_VERIFY.into());
        }
    }

    let extract_dir = staging.join("extracted");
    if std::fs::create_dir_all(&extract_dir).is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(ERR_INSTALL.into());
    }
    let status = if ext == "zip" {
        Command::new("ditto")
            .args(["-x", "-k"])
            .arg(&archive_path)
            .arg(&extract_dir)
            .status()
    } else {
        Command::new("tar")
            .args(["--zstd", "-xf"])
            .arg(&archive_path)
            .arg("-C")
            .arg(&extract_dir)
            .status()
    };
    match status {
        Ok(s) if s.success() => {}
        _ => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ERR_INSTALL.into());
        }
    }
    let _ = std::fs::remove_file(&archive_path);

    // Confirm the extracted bundle actually contains the executable before touching `dest_root`.
    let staged_bin = extract_dir.join(ENGINE_BUNDLE_NAME).join(ENGINE_BIN_REL);
    if !staged_bin.is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(ERR_INSTALL.into());
    }

    let backup = parent.join(format!(".{dir_name}.old-{pid}-{seq}"));
    let _ = std::fs::remove_dir_all(&backup);
    let had_existing = dest_root.exists();
    if had_existing && std::fs::rename(dest_root, &backup).is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(ERR_INSTALL.into());
    }
    if std::fs::rename(&extract_dir, dest_root).is_err() {
        if had_existing {
            let _ = std::fs::rename(&backup, dest_root);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(ERR_INSTALL.into());
    }
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(&staging);

    Ok(dest_root.join(ENGINE_BUNDLE_NAME).join(ENGINE_BIN_REL))
}
