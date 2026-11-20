//! Private owner-only temporary storage for authenticated remote media downloaded via
//! `Authorization` HTTP headers (matching official `anomalyco/opencode`
//! `packages/app/src/runtime/server/api.ts:45-56` & `packages/app/src/workspaces/files/model.tsx:228-241`:
//! credentials never go in a URL query parameter like `auth_token`).

#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(not(target_arch = "wasm32"))]
use super::Server;

#[cfg(not(target_arch = "wasm32"))]
const ALLOWED_VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "webm", "mkv", "ogv", "ogg", "avi", "wmv", "ts", "m2ts",
];

/// A downloaded remote media file stored in a private per-process directory with `0o600`
/// permissions (on Unix). The OS file handle is closed before returning so native players
/// (including Windows Media Foundation's `FILE_SHARE_READ`) can open it immediately; dropping the
/// last clone removes the file from disk automatically.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
pub struct TempMediaFile(Arc<tempfile::TempPath>);

#[cfg(not(target_arch = "wasm32"))]
impl TempMediaFile {
    pub fn path(&self) -> &Path {
        self.0.as_ref()
    }

    pub fn path_str(&self) -> Option<&str> {
        self.0.as_ref().to_str()
    }

    pub(crate) fn write(bytes: &[u8], safe_ext: &str) -> Result<Self, String> {
        let dir = private_media_dir()?;
        let mut file = tempfile::Builder::new()
            .prefix("opencode-media-")
            .suffix(&format!(".{safe_ext}"))
            .tempfile_in(&dir)
            .map_err(|e| format!("media cache temp file: {e}"))?;
        file.write_all(bytes)
            .and_then(|()| file.flush())
            .map_err(|e| format!("media cache write: {e}"))?;
        // Close the write handle immediately (`into_temp_path`) so Windows Media Foundation can
        // open the file with `FILE_SHARE_READ` without `ERROR_SHARING_VIOLATION`.
        Ok(Self(Arc::new(file.into_temp_path())))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn private_media_dir() -> Result<std::path::PathBuf, String> {
    static DIR: OnceLock<Mutex<Option<tempfile::TempDir>>> = OnceLock::new();
    let lock = DIR.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = guard.as_ref()
        && d.path().is_dir()
    {
        return Ok(d.path().to_path_buf());
    }
    let dir = tempfile::Builder::new()
        .prefix("opencode-media-cache-")
        .tempdir()
        .map_err(|e| format!("media cache dir: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("media cache chmod: {e}"))?;
    }
    let path = dir.path().to_path_buf();
    *guard = Some(dir);
    Ok(path)
}

#[cfg(not(target_arch = "wasm32"))]
impl Server {
    /// Downloads authenticated media from `path` via `Authorization` header (never `auth_token`
    /// query parameters) into a private owner-only [`TempMediaFile`] with an allow-listed video
    /// extension (`ext`).
    pub fn download_media_temp(&self, path: &str, ext: &str) -> Result<TempMediaFile, String> {
        let lower = ext.trim_start_matches('.').to_ascii_lowercase();
        let safe_ext = ALLOWED_VIDEO_EXTS
            .iter()
            .copied()
            .find(|&e| e == lower)
            .ok_or_else(|| "unsupported video extension".to_owned())?;
        let bytes = self.get_media_bytes(path).map_err(|e| e.to_string())?;
        TempMediaFile::write(&bytes, safe_ext)
    }
}
