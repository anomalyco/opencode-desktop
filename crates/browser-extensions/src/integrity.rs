//! Deterministic sorted tree SHA-256 digest, safe directory copy, and manifest key pinning.
//!
//! Produces byte-for-byte identical digests to `apps/browser-engine/src/extensions/integrity.rs`
//! and upstream `packages/gui-plugins/src/browser/profile/integrity.ts`:
//! - Strips `_metadata` (Chromium's compiled DNR rules cache) before hashing, refusing if
//!   `_metadata` is a symlink.
//! - Walks recursively (`depth <= 64`, `<= 10,000` entries, `<= 64 MiB` per file, `<= 256 MiB`
//!   total), rejecting symlinks and non-regular files, opening handles with `O_NOFOLLOW` +
//!   `(dev, ino)` verification on Unix and bounded `Read::take`, and failing closed if any worker
//!   thread fails to join.
//! - Hashes files in parallel with hardware-accelerated `sha2::Sha256`.

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::crx::{constant_time_eq, extension_id, is_valid_ext_id};

pub const MAX_DEPTH: usize = 64;
pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

struct FileJob {
    rel: String,
    path: PathBuf,
    size: u64,
    is_root_manifest: bool,
}

/// Opens a regular non-symlink file for reading (`O_NOFOLLOW` + `lstat`/`fstat` `(dev, ino)` match
/// on Unix), returning the handle and its observed byte size.
pub(crate) fn open_regular_no_follow(path: &Path) -> Option<(File, u64)> {
    let lmeta = std::fs::symlink_metadata(path).ok()?;
    if !lmeta.is_file() || lmeta.file_type().is_symlink() {
        return None;
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(path).ok()?;
    let fmeta = file.metadata().ok()?;
    if !fmeta.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if fmeta.dev() != lmeta.dev() || fmeta.ino() != lmeta.ino() {
            return None;
        }
    }
    Some((file, fmeta.len()))
}

/// Reads a regular non-symlink file with a hard byte cap (`Read::take(max_bytes + 1)`), eliminating
/// `stat`-then-`read` TOCTOU races.
pub(crate) fn read_regular_file_bounded(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    let (file, size) = open_regular_no_follow(path)?;
    if size > max_bytes {
        return None;
    }
    let mut buf = Vec::with_capacity((size as usize).min(1024 * 1024));
    file.take(max_bytes.saturating_add(1)).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > max_bytes {
        return None;
    }
    Some(buf)
}

/// Creates a brand-new directory with user-only (`0o700` on Unix) permissions.
pub(crate) fn create_dir_user_only(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

/// Ensures `dir` exists as a regular non-symlink directory with user-only permissions on Unix.
pub(crate) fn ensure_dir_no_symlink_user_only(dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) => {
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err("Directory is missing or is a symbolic link.".into());
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not create directory: {e}"))?;
            let meta = std::fs::symlink_metadata(dir)
                .map_err(|e| format!("Could not inspect directory: {e}"))?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err("Created path is not a regular directory.".into());
            }
        }
        Err(e) => return Err(format!("Could not inspect directory: {e}")),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Could not set user-only permissions: {e}"))?;
    }
    Ok(())
}

/// Removes Chromium's `_metadata` cache directory if present (refusing symlinks).
pub fn strip_metadata_dir(dir: &Path) -> bool {
    let metadata_dir = dir.join("_metadata");
    match std::fs::symlink_metadata(&metadata_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return false;
            }
            std::fs::remove_dir_all(&metadata_dir).is_ok()
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

/// Computes the deterministic sorted tree SHA-256 digest of `root`.
pub fn extension_tree_digest(root: &Path) -> Option<String> {
    extension_tree_digest_and_manifest(root).map(|(digest, _)| digest)
}

/// Verifies `dir` against `expected_id` (derived from the exact hashed `manifest.json` `key`) and
/// `expected_sha256`, stripping `_metadata` first.
pub fn verify_extension_dir(dir: &Path, expected_id: &str, expected_sha256: &str) -> bool {
    if !is_valid_ext_id(expected_id) || !is_hex_sha256(expected_sha256) {
        return false;
    }
    let Ok(root_meta) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    if !root_meta.is_dir() || root_meta.file_type().is_symlink() || !strip_metadata_dir(dir) {
        return false;
    }
    let Some((actual_digest, Some(manifest_bytes))) = extension_tree_digest_and_manifest(dir) else {
        return false;
    };
    if !constant_time_eq(actual_digest.as_bytes(), expected_sha256.to_ascii_lowercase().as_bytes()) {
        return false;
    }
    derive_id_from_manifest_bytes(&manifest_bytes).as_deref() == Some(expected_id)
}

/// Computes `(digest_hex, root_manifest_bytes)` using parallel hardware-accelerated SHA-256.
/// Fails closed if any entry is a symlink/non-regular file, exceeds caps, changes mid-read, or
/// if any worker thread panics.
pub fn extension_tree_digest_and_manifest(root: &Path) -> Option<(String, Option<Vec<u8>>)> {
    let mut dir_records: Vec<String> = Vec::new();
    let mut file_jobs: Vec<FileJob> = Vec::new();
    let mut count: usize = 0;
    let mut total_bytes: u64 = 0;

    fn collect(
        folder: &Path,
        prefix: &str,
        depth: usize,
        dir_records: &mut Vec<String>,
        file_jobs: &mut Vec<FileJob>,
        count: &mut usize,
        total_bytes: &mut u64,
    ) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let Ok(meta) = std::fs::symlink_metadata(folder) else {
            return false;
        };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(folder) else {
            return false;
        };
        for entry in entries {
            let Ok(entry) = entry else { return false };
            *count += 1;
            if *count > MAX_ENTRIES {
                return false;
            }
            let Ok(file_name) = entry.file_name().into_string() else {
                return false;
            };
            let rel = format!("{prefix}{file_name}");
            let target = entry.path();
            let Ok(ft) = entry.file_type() else {
                return false;
            };
            if ft.is_symlink() {
                return false;
            }
            if ft.is_dir() {
                dir_records.push(serde_json::json!([rel]).to_string());
                if !collect(&target, &format!("{rel}/"), depth + 1, dir_records, file_jobs, count, total_bytes) {
                    return false;
                }
                continue;
            }
            if !ft.is_file() {
                return false;
            }
            let Ok(info) = std::fs::symlink_metadata(&target) else {
                return false;
            };
            let size = info.len();
            if !info.is_file() || info.file_type().is_symlink() || size > MAX_FILE_BYTES {
                return false;
            }
            *total_bytes = total_bytes.saturating_add(size);
            if *total_bytes > MAX_TOTAL_BYTES {
                return false;
            }
            let is_root_manifest = depth == 0 && file_name == "manifest.json";
            file_jobs.push(FileJob { rel, path: target, size, is_root_manifest });
        }
        true
    }

    if !collect(root, "", 0, &mut dir_records, &mut file_jobs, &mut count, &mut total_bytes) {
        return None;
    }

    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get().clamp(1, 8))
        .min(file_jobs.len().max(1));
    let mut records = dir_records;
    let mut manifest_bytes: Option<Vec<u8>> = None;

    if workers <= 1 || file_jobs.len() < 8 {
        let mut chunk = vec![0u8; 64 * 1024];
        for job in &file_jobs {
            let (rec, mb) = hash_file_job(job, &mut chunk)?;
            records.push(rec);
            if mb.is_some() {
                manifest_bytes = mb;
            }
        }
    } else {
        let next_idx = AtomicUsize::new(0);
        let failed = AtomicBool::new(false);
        let partials: Vec<(Vec<String>, Option<Vec<u8>>)> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    let jobs_ref = &file_jobs;
                    let idx_ref = &next_idx;
                    let fail_ref = &failed;
                    s.spawn(move || {
                        let mut local = Vec::new();
                        let mut local_manifest = None;
                        let mut chunk = vec![0u8; 64 * 1024];
                        loop {
                            if fail_ref.load(Ordering::Relaxed) {
                                break;
                            }
                            let i = idx_ref.fetch_add(1, Ordering::Relaxed);
                            if i >= jobs_ref.len() {
                                break;
                            }
                            match hash_file_job(&jobs_ref[i], &mut chunk) {
                                Some((rec, mb)) => {
                                    local.push(rec);
                                    if mb.is_some() {
                                        local_manifest = mb;
                                    }
                                }
                                None => {
                                    fail_ref.store(true, Ordering::Relaxed);
                                    break;
                                }
                            }
                        }
                        (local, local_manifest)
                    })
                })
                .collect();
            let mut out = Vec::with_capacity(handles.len());
            for h in handles {
                match h.join() {
                    Ok(partial) => out.push(partial),
                    Err(_) => failed.store(true, Ordering::Relaxed),
                }
            }
            out
        });
        if failed.load(Ordering::Relaxed) || partials.len() != workers {
            return None;
        }
        for (mut local, mb) in partials {
            records.append(&mut local);
            if mb.is_some() {
                manifest_bytes = mb;
            }
        }
    }

    records.sort();
    let joined = records.join("\n");
    Some((to_hex(&Sha256::digest(joined.as_bytes())), manifest_bytes))
}

fn hash_file_job(job: &FileJob, chunk: &mut [u8]) -> Option<(String, Option<Vec<u8>>)> {
    let (file, size) = open_regular_no_follow(&job.path)?;
    if size != job.size || size > MAX_FILE_BYTES {
        return None;
    }
    let mut hasher = Sha256::new();
    let mut read_total: u64 = 0;
    let mut captured = (job.is_root_manifest && size <= MAX_MANIFEST_BYTES)
        .then(|| Vec::with_capacity(size as usize));
    let mut limited = (&file).take(size.saturating_add(1));
    loop {
        let n = limited.read(chunk).ok()?;
        if n == 0 {
            break;
        }
        read_total += n as u64;
        if read_total > size {
            return None;
        }
        hasher.update(&chunk[..n]);
        if let Some(buf) = &mut captured {
            buf.extend_from_slice(&chunk[..n]);
        }
    }
    let after = file.metadata().ok()?;
    if read_total != size || after.len() != size {
        return None;
    }
    Some((serde_json::json!([job.rel, to_hex(&hasher.finalize())]).to_string(), captured))
}

/// Copies a local or browser extension directory into `dst`, skipping `_metadata` and all symlinks
/// while enforcing entry, depth, and byte caps through opened non-symlink file handles
/// (`Read::take` + `create_new`, zero `std::fs::copy` symlink/size TOCTOU).
pub fn copy_dir_safe(src: &Path, dst: &Path) -> Result<(), String> {
    let src_meta = std::fs::symlink_metadata(src).map_err(|e| format!("Could not inspect source folder: {e}"))?;
    if !src_meta.is_dir() || src_meta.file_type().is_symlink() {
        return Err("Extension source is not a directory.".into());
    }
    ensure_dir_no_symlink_user_only(dst)?;
    // Refuse copying an ancestor of `dst` into `dst` (prevents recursive self-copy).
    if let (Ok(canon_src), Ok(canon_dst)) = (std::fs::canonicalize(src), std::fs::canonicalize(dst))
        && (canon_dst == canon_src || canon_dst.starts_with(&canon_src))
    {
        return Err("Extension source folder cannot contain the staging directory.".into());
    }
    let mut count: usize = 0;
    let mut total_bytes: u64 = 0;

    fn walk_copy(
        src: &Path,
        dst: &Path,
        depth: usize,
        count: &mut usize,
        total_bytes: &mut u64,
    ) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("Extension folder exceeds maximum nesting depth.".into());
        }
        let dir_meta = std::fs::symlink_metadata(src).map_err(|e| format!("Could not inspect folder: {e}"))?;
        if !dir_meta.is_dir() || dir_meta.file_type().is_symlink() {
            return Err("Extension source directory changed or became a symlink.".into());
        }
        let entries = std::fs::read_dir(src).map_err(|e| format!("Could not read extension folder: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("Could not read directory entry: {e}"))?;
            let name = entry.file_name();
            if name == "_metadata" {
                continue;
            }
            let Ok(name_str) = name.into_string() else {
                return Err("Extension contains non-UTF-8 file name.".into());
            };
            if name_str.is_empty()
                || name_str.len() > 255
                || name_str == "."
                || name_str == ".."
                || name_str.ends_with('.')
                || name_str.ends_with(' ')
                || name_str.chars().any(|c| {
                    c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                })
            {
                return Err("Extension contains an unsafe file name.".into());
            }
            let stem = name_str.split('.').next().unwrap_or(&name_str);
            if browser_policy::is_windows_device_name(stem) {
                return Err("Extension contains an unsafe reserved file name.".into());
            }
            let ft = entry.file_type().map_err(|e| format!("Could not inspect entry type: {e}"))?;
            if ft.is_symlink() {
                continue;
            }
            *count += 1;
            if *count > MAX_ENTRIES {
                return Err("Extension exceeds maximum file count.".into());
            }
            let from = entry.path();
            let to = dst.join(&name_str);
            if ft.is_dir() {
                create_dir_user_only(&to).map_err(|e| format!("Could not create directory: {e}"))?;
                walk_copy(&from, &to, depth + 1, count, total_bytes)?;
            } else if ft.is_file() {
                let Some((src_file, open_size)) = open_regular_no_follow(&from) else {
                    return Err("Refusing to copy symbolic link or non-regular file.".into());
                };
                let remaining_total = MAX_TOTAL_BYTES.saturating_sub(*total_bytes);
                let cap = MAX_FILE_BYTES.min(remaining_total);
                if open_size > cap {
                    return Err("Extension file exceeds maximum allowed size.".into());
                }
                let mut dst_opts = OpenOptions::new();
                dst_opts.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt as _;
                    dst_opts.custom_flags(libc::O_NOFOLLOW);
                }
                let mut dst_file = dst_opts
                    .open(&to)
                    .map_err(|e| format!("Could not create destination file: {e}"))?;
                let copied = io::copy(&mut (&src_file).take(cap.saturating_add(1)), &mut dst_file)
                    .map_err(|e| format!("Could not copy extension file: {e}"))?;
                if copied > cap || copied != open_size {
                    return Err("Extension file changed size or exceeded cap during copy.".into());
                }
                *total_bytes = total_bytes.saturating_add(copied);
                if *total_bytes > MAX_TOTAL_BYTES {
                    return Err("Extension exceeds maximum total size.".into());
                }
            }
        }
        Ok(())
    }

    walk_copy(src, dst, 0, &mut count, &mut total_bytes)
}

/// Pins the base64-encoded SPKI public key into `dir/manifest.json` (`extensions.ts:481-491`).
pub fn pin_manifest_key(dir: &Path, spki_der: &[u8]) -> Result<(), String> {
    let file = dir.join("manifest.json");
    let raw = read_regular_file_bounded(&file, MAX_MANIFEST_BYTES)
        .ok_or_else(|| "Extension manifest.json is missing or invalid.".to_owned())?;
    let text = std::str::from_utf8(&raw).map_err(|_| "Extension manifest.json is not valid UTF-8.".to_owned())?;
    let mut val: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|_| "Extension manifest.json is not valid JSON.".to_owned())?;
    let Some(obj) = val.as_object_mut() else {
        return Err("Extension manifest.json must be a JSON object.".into());
    };
    obj.insert(
        "key".into(),
        Value::String(base64::engine::general_purpose::STANDARD.encode(spki_der)),
    );
    let rendered = serde_json::to_string_pretty(&val)
        .map_err(|_| "Could not serialize manifest.json.".to_owned())?;
    let mut opts = OpenOptions::new();
    opts.write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut out = opts.open(&file).map_err(|e| format!("Could not open manifest.json: {e}"))?;
    out.write_all(rendered.as_bytes())
        .map_err(|e| format!("Could not write manifest.json: {e}"))
}

pub fn derive_id_from_manifest_bytes(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let val: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let key_b64 = val.get("key")?.as_str()?;
    let der = base64::engine::general_purpose::STANDARD.decode(key_b64.trim()).ok()?;
    Some(extension_id(&der))
}

fn is_hex_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
