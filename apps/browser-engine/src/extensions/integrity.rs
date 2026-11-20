//! Hardware-accelerated parallel SHA-256 tree digest verification and inert-stub exemption builder.
//!
//! Matches upstream `packages/gui-plugins/src/browser/profile/integrity.ts`:
//! - Strips `_metadata` (Chromium's compiled DNR rules cache) before hashing.
//! - Walks recursively (depth <= 64, <= 10,000 entries, <= 64 MiB per file, <= 256 MiB total).
//! - Refuses any symlink or non-regular file/directory.
//! - Re-checks file length after reading (detects concurrent growth/mutation).
//! - Captures `manifest.json` bytes during the hash walk so the derived key ID is checked against
//!   the exact bytes that were hashed (zero re-read / zero TOCTOU between digest and key check).
//! - Hashes sorted JSON records `["dir"]` and `["rel/path","<sha256>"]` joined by `\n`.
//!
//! Fast-boot architecture:
//! - Before `cef::initialize()`, `prepare_startup_stubs` validates only each candidate's small
//!   `manifest.json` (MV3 + key derives `id`) and emits a 100-byte inert stub manifest (0 perms,
//!   0 scripts, 0 DNR rules) for `--disable-extensions-except` so Chromium starts in ~170 ms with
//!   0 registry scans.
//! - Full parallel SHA-256 tree verification runs on a background thread concurrently with
//!   `cef::initialize()`; verified extensions then load via `Extensions.loadUnpacked`, while any
//!   tampered extension has its inert stub uninstalled immediately and never loads.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::extensions::manifest::{ExtensionManifest, is_valid_ext_id};

const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 10_000;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const STUB_PREFIX: &str = ".ext-exempt-";

/// A parsed `--extension <id>:<sha256>:<dir>` specification awaiting full SHA-256 verification.
#[derive(Clone, Debug)]
pub struct ExtensionCandidate {
    pub id: String,
    pub sha256: String,
    pub dir: PathBuf,
}

/// Result of fast pre-CEF stub preparation (<1 ms).
#[derive(Clone, Debug, Default)]
pub struct PreparedStartup {
    pub stub_root: Option<PathBuf>,
    pub stub_dirs: Vec<PathBuf>,
    pub candidates: Vec<ExtensionCandidate>,
    pub pre_rejected_ids: Vec<String>,
}

/// A cryptographically verified startup extension ready for async `Extensions.loadUnpacked`.
#[derive(Clone, Debug)]
pub struct VerifiedExtension {
    pub id: String,
    pub sha256: String,
    pub dir: PathBuf,
    pub manifest: ExtensionManifest,
}

/// Outcome of background parallel SHA-256 verification for all `--extension` candidates.
#[derive(Clone, Debug, Default)]
pub struct StartupExtensions {
    pub verified: Vec<VerifiedExtension>,
    pub rejected_ids: Vec<String>,
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

/// Fast (<1 ms) pre-CEF preparation: reads only each candidate's `manifest.json` (MV3 + key -> id),
/// rejecting malformed/MV2/mismatched specs up front, and writes a 100-byte inert stub manifest
/// (zero permissions, zero scripts, zero DNR) per valid candidate so `--disable-extensions-except`
/// registers their exact IDs in Chromium's exemption set without delaying `cef::initialize()`.
pub fn prepare_startup_stubs(specs: &[String], profile_root: &Path) -> PreparedStartup {
    sweep_stale_stubs(profile_root);
    let parsed: Vec<ExtensionCandidate> = specs.iter().filter_map(|s| parse_spec(s)).collect();
    if parsed.is_empty() {
        return PreparedStartup::default();
    }
    let stub_root = profile_root.join(format!("{STUB_PREFIX}{}", std::process::id()));
    let mut out = PreparedStartup::default();
    for cand in parsed {
        let Some((version, key)) = read_mv3_stub_fields(&cand.dir, &cand.id) else {
            out.pre_rejected_ids.push(cand.id);
            continue;
        };
        let stub_dir = stub_root.join(&cand.id);
        if std::fs::create_dir_all(&stub_dir).is_err() {
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
        out.stub_dirs.push(stub_dir);
        out.candidates.push(cand);
    }
    if !out.stub_dirs.is_empty() {
        out.stub_root = Some(stub_root);
    }
    out
}

/// Removes the engine's temporary inert-stub directory on shutdown.
pub fn cleanup_stub_root(stub_root: &Path) {
    if let Ok(meta) = std::fs::symlink_metadata(stub_root)
        && !meta.file_type().is_symlink()
    {
        let _ = std::fs::remove_dir_all(stub_root);
    }
}

/// Sweeps any leftover `.ext-exempt-<pid>` stub folders from earlier crashed engine processes.
fn sweep_stale_stubs(profile_root: &Path) {
    let Ok(entries) = std::fs::read_dir(profile_root) else { return };
    let my_pid = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(pid_str) = name.strip_prefix(STUB_PREFIX) else { continue };
        if pid_str.parse::<u32>().ok().is_some_and(|p| p != my_pid)
            && entry.file_type().is_ok_and(|ft| ft.is_dir() && !ft.is_symlink())
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn read_mv3_stub_fields(dir: &Path, expected_id: &str) -> Option<(String, String)> {
    let dir_meta = std::fs::symlink_metadata(dir).ok()?;
    if !dir_meta.is_dir() || dir_meta.file_type().is_symlink() {
        return None;
    }
    let manifest_path = dir.join("manifest.json");
    let m_meta = std::fs::symlink_metadata(&manifest_path).ok()?;
    if !m_meta.is_file() || m_meta.file_type().is_symlink() || m_meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let bytes = std::fs::read(&manifest_path).ok()?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    let val: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let obj = val.as_object()?;
    if obj.get("manifest_version")?.as_u64()? != 3 {
        return None;
    }
    let version = obj.get("version")?.as_str()?.to_owned();
    let key = obj.get("key")?.as_str()?.trim().to_owned();
    if derive_id_from_key_b64(&key).as_deref() != Some(expected_id) {
        return None;
    }
    Some((version, key))
}

/// Cryptographically verifies all startup candidates in parallel (per-file hardware SHA-256).
pub fn verify_candidates_parallel(candidates: Vec<ExtensionCandidate>) -> StartupExtensions {
    let mut out = StartupExtensions::default();
    for cand in candidates {
        if verify_extension_dir(&cand.dir, &cand.id, &cand.sha256)
            && let Some(manifest) = ExtensionManifest::read(&cand.dir)
        {
            out.verified.push(VerifiedExtension {
                id: cand.id,
                sha256: cand.sha256,
                dir: cand.dir,
                manifest,
            });
        } else {
            out.rejected_ids.push(cand.id);
        }
    }
    out
}

/// Verifies `dir` against `expected_id` (derived from the hashed `manifest.json` `key`) and
/// `expected_sha256`. Strips `_metadata` first (Chromium's compiled DNR cache is never trusted).
pub fn verify_extension_dir(dir: &Path, expected_id: &str, expected_sha256: &str) -> bool {
    if !is_valid_ext_id(expected_id) || !is_hex_sha256(expected_sha256) {
        return false;
    }
    let Ok(root_meta) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
        return false;
    }
    // Strip Chromium's compiled DNR rules cache before hashing (upstream `profile/index.ts:239`).
    // Refuse immediately if `_metadata` was replaced with a symlink.
    let metadata_dir = dir.join("_metadata");
    if let Ok(meta) = std::fs::symlink_metadata(&metadata_dir) {
        if meta.file_type().is_symlink() {
            return false;
        }
        if std::fs::remove_dir_all(&metadata_dir).is_err() {
            return false;
        }
    }
    let Some((actual_digest, manifest_bytes)) = extension_tree_digest_and_manifest(dir) else {
        return false;
    };
    if !constant_time_eq(actual_digest.as_bytes(), expected_sha256.as_bytes()) {
        return false;
    }
    let Some(bytes) = manifest_bytes else {
        return false;
    };
    derive_id_from_manifest_bytes(&bytes).as_deref() == Some(expected_id)
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
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        if failed.load(Ordering::Relaxed) {
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
    let mut file = File::open(&job.path).ok()?;
    let info = file.metadata().ok()?;
    let size = info.len();
    if !info.is_file() || size != job.size || size > MAX_FILE_BYTES {
        return None;
    }
    let mut hasher = Sha256::new();
    let mut read_total: u64 = 0;
    let mut captured = (job.is_root_manifest && size <= MAX_MANIFEST_BYTES)
        .then(|| Vec::with_capacity(size as usize));
    loop {
        let n = file.read(chunk).ok()?;
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

pub fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
