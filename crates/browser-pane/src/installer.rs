//! Resolves or downloads the on-demand `OpenCode Browser Engine.app` sidecar bundle.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use rune::Waker;
use sha2::{Digest, Sha256};

pub const ENGINE_VERSION: &str = "156.0.14";
pub const ENGINE_BUNDLE_NAME: &str = "OpenCode Browser Engine.app";
pub const ENGINE_BIN_REL: &str = "Contents/MacOS/opencode-browser-engine";

#[derive(Clone, Debug, PartialEq)]
pub enum EngineStatus {
    NotInstalled,
    Downloading { downloaded_bytes: u64, total_bytes: Option<u64> },
    Connecting,
    Ready,
    Failed(String),
}

pub fn engine_cache_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(home)
            .join("Library/Application Support/co.anomaly.opencode/browser-engine")
            .join(ENGINE_VERSION)
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from(home)
            .join(".local/share/opencode/browser-engine")
            .join(ENGINE_VERSION)
    }
}

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

pub fn socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("OPENCODE_BROWSER_SOCKET") {
        return PathBuf::from(custom);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let uid = unsafe { libc::geteuid() };
    #[cfg(target_os = "macos")]
    let dir = PathBuf::from(home).join("Library/Caches/co.anomaly.opencode");
    #[cfg(not(target_os = "macos"))]
    let dir = PathBuf::from(home).join(".cache/opencode");
    let _ = create_private_dir(&dir);
    dir.join(format!("browser-{uid}.sock"))
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Downloads and extracts a signed `.tar.zst` or `.zip` engine archive on a background thread.
///
/// The URL is read from `OPENCODE_BROWSER_ENGINE_URL` (and optional hex `OPENCODE_BROWSER_ENGINE_SHA256`),
/// allowing release builds to point at the CDN while dev builds install locally via
/// `apps/browser-engine/scripts/install-local.sh`.
pub fn start_download(
    state: Arc<Mutex<EngineStatus>>,
    waker: Option<Waker>,
    on_ready: Box<dyn FnOnce(PathBuf) + Send + 'static>,
) {
    let Ok(url) = std::env::var("OPENCODE_BROWSER_ENGINE_URL") else {
        *state.lock().unwrap() = EngineStatus::Failed(
            "Browser engine is not installed yet. Run `apps/browser-engine/scripts/install-local.sh` in dev, or set OPENCODE_BROWSER_ENGINE_URL."
                .into(),
        );
        if let Some(w) = waker {
            w();
        }
        return;
    };
    let expected_sha = std::env::var("OPENCODE_BROWSER_ENGINE_SHA256").ok();

    *state.lock().unwrap() = EngineStatus::Downloading { downloaded_bytes: 0, total_bytes: None };
    if let Some(w) = &waker {
        w();
    }

    std::thread::Builder::new()
        .name("opencode-browser-install".into())
        .spawn(move || {
            let result = download_and_extract(&url, expected_sha.as_deref(), &state, waker.as_ref());
            match result {
                Ok(bin) => {
                    on_ready(bin);
                }
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

fn download_and_extract(
    url: &str,
    expected_sha256: Option<&str>,
    state: &Arc<Mutex<EngineStatus>>,
    waker: Option<&Waker>,
) -> Result<PathBuf, String> {
    let dest_root = engine_cache_root();
    let staging = dest_root.with_extension(format!("staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("create staging dir: {e}"))?;

    let ext = if url.ends_with(".zip") { "zip" } else { "tar.zst" };
    let archive_path = staging.join(format!("engine.{ext}"));

    let mut resp = ureq::get(url).call().map_err(|e| format!("download failed: {e}"))?;
    let total_bytes = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(&archive_path).map_err(|e| format!("create archive: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded = 0u64;
    let mut last_notify = 0u64;

    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("read stream: {e}"))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| format!("write archive: {e}"))?;
        hasher.update(&buf[..n]);
        downloaded += n as u64;
        if downloaded - last_notify >= 512 * 1024 {
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

    if let Some(expected) = expected_sha256 {
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("SHA-256 mismatch: expected {expected}, got {actual}"));
        }
    }

    let extract_dir = staging.join("extracted");
    std::fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;
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
    }
    .map_err(|e| format!("spawn extractor: {e}"))?;

    if !status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("archive extraction exited with {status}"));
    }
    let _ = std::fs::remove_file(&archive_path);

    if let Some(parent) = dest_root.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(&dest_root);
    std::fs::rename(&extract_dir, &dest_root).map_err(|e| format!("install rename: {e}"))?;
    let _ = std::fs::remove_dir_all(&staging);

    let bin = dest_root.join(ENGINE_BUNDLE_NAME).join(ENGINE_BIN_REL);
    if bin.is_file() {
        Ok(bin)
    } else {
        Err(format!("extracted bundle missing {}", bin.display()))
    }
}
