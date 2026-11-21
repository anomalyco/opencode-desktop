//! Hardware-accelerated parallel SHA-256 tree digest verification and inert-stub exemption builder.
//!
//! Security invariants:
//! - Strips `_metadata` (refusing symlinks) before hashing.
//! - Walks recursively (`depth <= 64`, `<= 10,000` entries, `<= 64 MiB` per file, `<= 256 MiB` total),
//!   opens every file with `O_NOFOLLOW` + `(dev, ino)` verification on Unix and `Read::take`,
//!   re-checks length after read, and fails closed if any worker thread panics.
//! - Parses `ExtensionManifest` strictly from the exact hashed `manifest.json` bytes (zero
//!   post-verification re-read TOCTOU) and refuses MV2 / themes / disallowed native permissions.
//! - Uses a comma-free stub directory path so `--disable-extensions-except` never splits on commas.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::extensions::manifest::{
    DISALLOWED_REQUIRED_PERMS, ExtensionManifest, is_valid_ext_id, strip_disallowed_perms_from_manifest,
};

const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 10_000;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const STUB_PREFIX: &str = ".ext-exempt-";
static STAGE_SEQ: AtomicU64 = AtomicU64::new(1);

/// A parsed `--extension <id>:<sha256>:<dir>` specification awaiting full SHA-256 verification.
#[derive(Clone, Debug)]
pub struct ExtensionCandidate {
    pub id: String,
    pub sha256: String,
    pub dir: PathBuf,
}

/// Result of fast pre-CEF stub preparation (<1 ms).
#[derive(Debug, Default)]
pub struct PreparedStartup {
    pub stub_root: Option<PathBuf>,
    /// Exclusive lock held on `<stub_root>/.lock` for the lifetime of the CEF runtime so shared
    /// comma-fallback directories (`%LOCALAPPDATA%` / `$TMPDIR`) sweep dead runs safely.
    pub stub_lock: Option<File>,
    pub stub_dirs: Vec<PathBuf>,
    pub stub_dirs_by_id: HashMap<String, PathBuf>,
    pub candidates: Vec<ExtensionCandidate>,
    pub pre_rejected_ids: Vec<String>,
}

/// A cryptographically verified startup extension ready for async `Extensions.loadUnpacked`.
#[derive(Clone, Debug)]
pub struct VerifiedExtension {
    pub id: String,
    pub sha256: String,
    /// Original immutable user-approved extension directory (re-hashed on reload).
    pub dir: PathBuf,
    /// Directory passed to Chromium `Extensions.loadUnpacked` — either `dir` directly, or a
    /// private per-PID runtime mirror inside `stub_root` whose `manifest.json` has
    /// [`crate::extensions::manifest::STRIPPED_NATIVE_PERMS`] removed so Chromium C++ never grants them.
    pub load_dir: PathBuf,
    pub manifest: ExtensionManifest,
}

/// Outcome of background parallel SHA-256 verification for all `--extension` candidates.
#[derive(Clone, Debug, Default)]
pub struct StartupExtensions {
    pub verified: Vec<VerifiedExtension>,
    pub rejected_ids: Vec<String>,
}

/// Opens a regular non-symlink file (`O_NOFOLLOW` + `(dev, ino)` on Unix;
/// `FILE_FLAG_OPEN_REPARSE_POINT` + `FILE_ATTRIBUTE_REPARSE_POINT` check on Windows).
pub(crate) fn open_regular_no_follow(path: &Path) -> Option<(File, u64)> {
    let lmeta = std::fs::symlink_metadata(path).ok()?;
    if !lmeta.is_file() || is_reparse_or_symlink(&lmeta) {
        return None;
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_FLAG_OPEN_REPARSE_POINT (0x0020_0000): opens the link itself without following.
        opts.custom_flags(0x0020_0000);
    }
    let file = opts.open(path).ok()?;
    let fmeta = file.metadata().ok()?;
    if !fmeta.is_file() || is_reparse_or_symlink(&fmeta) {
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

fn is_reparse_or_symlink(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        if (meta.file_attributes() & 0x0400) != 0 {
            return true;
        }
    }
    false
}

/// Reads a regular non-symlink file with a hard byte cap (`Read::take(max_bytes + 1)`).
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

pub fn parse_spec(raw: &str) -> Option<ExtensionCandidate> {
    let (id, rest) = raw.split_once(':')?;
    let (sha, dir_str) = rest.split_once(':')?;
    if !is_valid_ext_id(id) || !is_hex_sha256(sha) || dir_str.is_empty() {
        return None;
    }
    Some(ExtensionCandidate {
        id: id.to_owned(),
        sha256: sha.to_ascii_lowercase(),
        dir: PathBuf::from(dir_str),
    })
}

/// Fast (<1 ms) pre-CEF preparation: reads only each candidate's `manifest.json` (MV3, key -> id,
/// and no disallowed required permissions) and emits a 100-byte inert stub manifest (0 perms,
/// 0 scripts, 0 DNR) in a comma-free user-only directory for `--disable-extensions-except`.
pub fn prepare_startup_stubs(specs: &[String], profile_root: &Path) -> PreparedStartup {
    sweep_stale_stubs(profile_root, false);
    if let Some(fallback_parent) = comma_fallback_parent()
        && fallback_parent != profile_root
    {
        sweep_stale_stubs(&fallback_parent, true);
    }
    let parsed: Vec<ExtensionCandidate> = specs.iter().filter_map(|s| parse_spec(s)).collect();
    if parsed.is_empty() {
        return PreparedStartup::default();
    }
    // Chromium splits `--disable-extensions-except` on commas; if `profile_root` contains a comma,
    // pick a user-owned comma-free parent (never rely on profile-overwritten `TMP` on Windows, and
    // create an atomic non-predictable `0o700` dir on Unix; fail closed if unavailable).
    let base_parent = if profile_root.to_string_lossy().contains(',') {
        match comma_fallback_parent() {
            Some(p) => p,
            None => {
                return PreparedStartup {
                    pre_rejected_ids: parsed.into_iter().map(|c| c.id).collect(),
                    ..PreparedStartup::default()
                };
            }
        }
    } else {
        profile_root.to_path_buf()
    };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let stub_root = base_parent.join(format!("{STUB_PREFIX}{}-{nonce:x}", std::process::id()));
    if stub_root.to_string_lossy().contains(',') {
        return PreparedStartup {
            pre_rejected_ids: parsed.into_iter().map(|c| c.id).collect(),
            ..PreparedStartup::default()
        };
    }
    let root_created = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            std::fs::DirBuilder::new().mode(0o700).create(&stub_root)
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir(&stub_root)
        }
    };
    if root_created.is_err() {
        return PreparedStartup {
            pre_rejected_ids: parsed.into_iter().map(|c| c.id).collect(),
            ..PreparedStartup::default()
        };
    }
    let stub_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(stub_root.join(".lock"))
        .ok()
        .and_then(|f| f.try_lock().ok().map(|()| f));
    let mut out = PreparedStartup::default();
    for cand in parsed {
        let Some((version, key)) = read_mv3_stub_fields(&cand.dir, &cand.id) else {
            out.pre_rejected_ids.push(cand.id);
            continue;
        };
        let stub_dir = stub_root.join(&cand.id);
        let dir_created = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                std::fs::DirBuilder::new().mode(0o700).create(&stub_dir)
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir(&stub_dir)
            }
        };
        if dir_created.is_err() {
            out.pre_rejected_ids.push(cand.id);
            continue;
        }
        let stub = json!({
            "manifest_version": 3,
            "name": "_",
            "version": version,
            "key": key,
        });
        if std::fs::write(stub_dir.join("manifest.json"), stub.to_string()).is_err() {
            out.pre_rejected_ids.push(cand.id);
            continue;
        }
        out.stub_dirs.push(stub_dir.clone());
        out.stub_dirs_by_id.insert(cand.id.clone(), stub_dir);
        out.candidates.push(cand);
    }
    if out.stub_dirs.is_empty() {
        drop(stub_lock);
        let _ = std::fs::remove_dir_all(&stub_root);
    } else {
        out.stub_root = Some(stub_root);
        out.stub_lock = stub_lock;
    }
    out
}

fn comma_fallback_parent() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|p| !p.to_string_lossy().contains(','))
    }
    #[cfg(not(windows))]
    {
        let p = std::env::temp_dir();
        (!p.to_string_lossy().contains(',')).then_some(p)
    }
}

/// Removes the engine's temporary inert-stub directory on shutdown (called after `cef::shutdown()`).
pub fn cleanup_stub_root(stub_root: &Path) {
    if let Ok(meta) = std::fs::symlink_metadata(stub_root)
        && !is_reparse_or_symlink(&meta)
    {
        let _ = std::fs::remove_dir_all(stub_root);
    }
}

/// Removes one staged `_rt/<ext_id>-<seq>` runtime mirror directory on uninstall or failed verify.
pub fn cleanup_staged_dir(staged_dir: &Path) {
    if let Ok(meta) = std::fs::symlink_metadata(staged_dir)
        && meta.is_dir()
        && !is_reparse_or_symlink(&meta)
    {
        let _ = std::fs::remove_dir_all(staged_dir);
    }
}

/// Sweeps any leftover `.ext-exempt-<pid>` stub folders from earlier crashed engine processes.
/// In a shared comma-fallback parent (`%LOCALAPPDATA%` / `$TMPDIR`), requires acquiring the
/// directory's `.lock` file (or age > 1 h for legacy lockless dirs) so live sibling engines
/// are never disturbed.
fn sweep_stale_stubs(parent: &Path, is_shared_parent: bool) {
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    let my_pid = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(pid_part) = name.strip_prefix(STUB_PREFIX) else { continue };
        let pid_str = pid_part.split('-').next().unwrap_or(pid_part);
        let path = entry.path();
        if !pid_str.parse::<u32>().ok().is_some_and(|p| p != my_pid) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if !meta.is_dir() || is_reparse_or_symlink(&meta) {
            continue;
        }
        if is_shared_parent {
            let lock_path = path.join(".lock");
            let can_remove = match std::fs::symlink_metadata(&lock_path) {
                Ok(lm) if lm.is_file() && !is_reparse_or_symlink(&lm) => {
                    OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&lock_path)
                        .is_ok_and(|f| f.try_lock().is_ok())
                }
                Ok(_) => false,
                Err(_) => meta
                    .modified()
                    .ok()
                    .and_then(|m| m.elapsed().ok())
                    .is_some_and(|age| age > std::time::Duration::from_secs(3600)),
            };
            if !can_remove {
                continue;
            }
        }
        let _ = std::fs::remove_dir_all(path);
    }
}

fn read_mv3_stub_fields(dir: &Path, expected_id: &str) -> Option<(String, String)> {
    let dir_meta = std::fs::symlink_metadata(dir).ok()?;
    if !dir_meta.is_dir() || is_reparse_or_symlink(&dir_meta) {
        return None;
    }
    let bytes = read_regular_file_bounded(&dir.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let val: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let obj = val.as_object()?;
    if obj.get("manifest_version")?.as_u64()? != 3 {
        return None;
    }
    if obj.get("theme").is_some_and(|v| !v.is_null()) {
        return None;
    }
    let perms = super::manifest::checked_string_array(obj.get("permissions"))?;
    if perms.iter().any(|p| DISALLOWED_REQUIRED_PERMS.contains(&p.as_str())) {
        return None;
    }
    let _hosts = super::manifest::checked_string_array(obj.get("host_permissions"))?;
    let version = obj.get("version")?.as_str()?.to_owned();
    let key = obj.get("key")?.as_str()?.trim().to_owned();
    if derive_id_from_key_b64(&key).as_deref() != Some(expected_id) {
        return None;
    }
    Some((version, key))
}

/// Cryptographically verifies all startup candidates in parallel (per-file hardware SHA-256),
/// parsing each manifest directly from the exact hashed `manifest.json` bytes (zero re-read) and
/// staging a sanitized runtime mirror when `STRIPPED_NATIVE_PERMS` are declared.
pub fn verify_candidates_parallel(
    candidates: Vec<ExtensionCandidate>,
    stub_root: Option<&Path>,
) -> StartupExtensions {
    let mut out = StartupExtensions::default();
    for cand in candidates {
        if let Some((manifest, load_dir)) =
            verify_extension_dir_and_stage(&cand.dir, &cand.id, &cand.sha256, stub_root)
        {
            out.verified.push(VerifiedExtension {
                id: cand.id,
                sha256: cand.sha256,
                dir: cand.dir,
                load_dir,
                manifest,
            });
        } else {
            out.rejected_ids.push(cand.id);
        }
    }
    out
}

/// Verifies `dir` against `expected_id` and `expected_sha256`, parses `ExtensionManifest` from
/// the exact hashed `manifest.json` bytes, and if the manifest declares any of
/// `STRIPPED_NATIVE_PERMS` (required or optional), stages an inode-isolated, symlink-free runtime
/// copy under `<stub_root>/_rt/<expected_id>`, verifies the SHA-256 tree digest of the isolated
/// copy itself (zero post-verify TOCTOU), and replaces only `manifest.json` with the stripped copy.
pub fn verify_extension_dir_and_stage(
    dir: &Path,
    expected_id: &str,
    expected_sha256: &str,
    stub_root: Option<&Path>,
) -> Option<(ExtensionManifest, PathBuf)> {
    if !is_valid_ext_id(expected_id) || !is_hex_sha256(expected_sha256) {
        return None;
    }
    let root_meta = std::fs::symlink_metadata(dir).ok()?;
    if !root_meta.is_dir() || is_reparse_or_symlink(&root_meta) {
        return None;
    }
    let metadata_dir = dir.join("_metadata");
    if let Ok(meta) = std::fs::symlink_metadata(&metadata_dir) {
        if is_reparse_or_symlink(&meta) {
            return None;
        }
        std::fs::remove_dir_all(&metadata_dir).ok()?;
    }
    let (actual_digest, Some(manifest_bytes)) = extension_tree_digest_and_manifest(dir)? else {
        return None;
    };
    if !constant_time_eq(actual_digest.as_bytes(), expected_sha256.to_ascii_lowercase().as_bytes()) {
        return None;
    }
    if derive_id_from_manifest_bytes(&manifest_bytes).as_deref() != Some(expected_id) {
        return None;
    }
    let manifest = ExtensionManifest::from_verified_bytes(dir, &manifest_bytes)?;
    let (needs_strip, stripped_bytes) = strip_disallowed_perms_from_manifest(&manifest_bytes)?;
    if !needs_strip {
        return Some((manifest, dir.to_path_buf()));
    }
    let rt_root = stub_root?.join("_rt");
    let staged = stage_sanitized_mirror(dir, &rt_root, expected_id, expected_sha256, &stripped_bytes)?;
    Some((manifest, staged))
}

fn stage_sanitized_mirror(
    src_root: &Path,
    rt_root: &Path,
    ext_id: &str,
    expected_sha256: &str,
    stripped_manifest: &[u8],
) -> Option<PathBuf> {
    if !is_valid_ext_id(ext_id) {
        return None;
    }
    std::fs::create_dir_all(rt_root).ok()?;
    let rt_meta = std::fs::symlink_metadata(rt_root).ok()?;
    if !rt_meta.is_dir() || is_reparse_or_symlink(&rt_meta) {
        return None;
    }
    // Use a monotonic `<ext_id>-<seq>` subdir so live reloads on Windows never fail with
    // `ERROR_SHARING_VIOLATION` while Chromium still holds handles inside an earlier stage dir.
    let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    let dst = rt_root.join(format!("{ext_id}-{seq}"));
    if std::fs::symlink_metadata(&dst).is_ok() {
        return None;
    }
    std::fs::create_dir(&dst).ok()?;
    let mut entries = 0usize;
    let mut total_bytes = 0u64;
    if !mirror_tree(src_root, &dst, 0, &mut entries, &mut total_bytes) {
        let _ = std::fs::remove_dir_all(&dst);
        return None;
    }
    // Re-hash the private inode-isolated copy BEFORE replacing `manifest.json` so no concurrent
    // write to `src_root` can slip unverified bytes into the staged tree.
    let Some((staged_digest, _)) = extension_tree_digest_and_manifest(&dst) else {
        let _ = std::fs::remove_dir_all(&dst);
        return None;
    };
    if !constant_time_eq(staged_digest.as_bytes(), expected_sha256.to_ascii_lowercase().as_bytes()) {
        let _ = std::fs::remove_dir_all(&dst);
        return None;
    }
    if std::fs::write(dst.join("manifest.json"), stripped_manifest).is_err() {
        let _ = std::fs::remove_dir_all(&dst);
        return None;
    }
    Some(dst)
}

fn mirror_tree(src: &Path, dst: &Path, depth: usize, entries: &mut usize, total_bytes: &mut u64) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let Ok(rd) = std::fs::read_dir(src) else { return false };
    let mut buf = [0u8; 64 * 1024];
    for entry in rd {
        let Ok(entry) = entry else { return false };
        *entries += 1;
        if *entries > MAX_ENTRIES {
            return false;
        }
        let Ok(name) = entry.file_name().into_string() else { return false };
        if depth == 0 && name == "_metadata" {
            continue;
        }
        let from = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&from) else { return false };
        if is_reparse_or_symlink(&meta) {
            return false;
        }
        let to = dst.join(&name);
        if meta.is_dir() {
            if std::fs::create_dir(&to).is_err() {
                return false;
            }
            if !mirror_tree(&from, &to, depth + 1, entries, total_bytes) {
                return false;
            }
        } else if meta.is_file() {
            // Open with `open_regular_no_follow` + `Read::take(MAX_FILE_BYTES + 1)` instead of
            // `std::fs::copy` to close transient symlink/size TOCTOU before the post-copy hash.
            let Some((file, sz)) = open_regular_no_follow(&from) else { return false };
            if sz > MAX_FILE_BYTES {
                return false;
            }
            let Ok(mut out) = OpenOptions::new().write(true).create_new(true).open(&to) else {
                return false;
            };
            let mut copied: u64 = 0;
            let mut limited = (&file).take(MAX_FILE_BYTES.saturating_add(1));
            loop {
                let Ok(n) = limited.read(&mut buf) else { return false };
                if n == 0 {
                    break;
                }
                copied = copied.saturating_add(n as u64);
                if copied > MAX_FILE_BYTES {
                    return false;
                }
                *total_bytes = total_bytes.saturating_add(n as u64);
                if *total_bytes > MAX_TOTAL_BYTES {
                    return false;
                }
                if out.write_all(&buf[..n]).is_err() {
                    return false;
                }
            }
        } else {
            return false;
        }
    }
    true
}

struct FileJob {
    rel: String,
    path: PathBuf,
    size: u64,
    is_root_manifest: bool,
}

/// Computes the deterministic sorted tree SHA-256 digest of `root` using parallel worker threads,
/// returning `(digest_hex, root_manifest_bytes)` from the exact bytes hashed.
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
        if !meta.is_dir() || is_reparse_or_symlink(&meta) {
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
            let Ok(info) = std::fs::symlink_metadata(&target) else {
                return false;
            };
            if is_reparse_or_symlink(&info) {
                return false;
            }
            if info.is_dir() {
                dir_records.push(serde_json::json!([rel]).to_string());
                if !collect(&target, &format!("{rel}/"), depth + 1, dir_records, file_jobs, count, total_bytes) {
                    return false;
                }
                continue;
            }
            let size = info.len();
            if !info.is_file() || size > MAX_FILE_BYTES {
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

    let next_idx = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let mut records = dir_records;
    let mut manifest_bytes: Option<Vec<u8>> = None;

    if workers <= 1 || file_jobs.len() < 8 {
        let mut chunk = vec![0u8; 64 * 1024];
        for job in &file_jobs {
            let (rec, mbytes) = hash_file_job(job, &mut chunk)?;
            records.push(rec);
            if let Some(mb) = mbytes {
                manifest_bytes = Some(mb);
            }
        }
    } else {
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
                                Some((rec, mbytes)) => {
                                    local.push(rec);
                                    if let Some(mb) = mbytes {
                                        local_manifest = Some(mb);
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
        for (mut local, mbytes) in partials {
            records.append(&mut local);
            if let Some(mb) = mbytes {
                manifest_bytes = Some(mb);
            }
        }
    }

    records.sort();
    let joined = records.join("\n");
    let final_hash = Sha256::digest(joined.as_bytes());
    Some((to_hex(&final_hash), manifest_bytes))
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
    let hex = to_hex(&hasher.finalize());
    Some((serde_json::json!([job.rel, hex]).to_string(), captured))
}

/// Derives the 32-char Chromium extension ID (`a..p`) from raw `manifest.json` bytes.
pub fn derive_id_from_manifest_bytes(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let val: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let key_b64 = val.get("key")?.as_str()?;
    derive_id_from_key_b64(key_b64)
}

fn derive_id_from_key_b64(key_b64: &str) -> Option<String> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(key_b64.trim())
        .ok()?;
    let digest = Sha256::digest(&der);
    let mut id = String::with_capacity(32);
    for &b in &digest[..16] {
        id.push((b'a' + (b >> 4)) as char);
        id.push((b'a' + (b & 0x0f)) as char);
    }
    Some(id)
}

fn is_hex_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
