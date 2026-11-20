//! Bounded, symlink-free ZIP validator and unpacker for verified CRX3 payloads.
//!
//! Mirrors `packages/gui-plugins/src/browser/profile/extensions.ts:353-475` with stricter
//! hardening:
//! - Validates EVERY central directory entry and local file header before touching disk.
//! - Hard caps: `<= 10,000` entries, `<= 64 MiB` per entry, `<= 256 MiB` total uncompressed,
//!   path `<= 1024` bytes, segment `<= 255` bytes, nesting depth `<= 64`.
//! - Rejects ZIP64, multi-disk archives, unreferenced gaps before EOCD (`cd_end == eocd`),
//!   encrypted / strongly-encrypted entries, Unix/macOS symlinks and special device nodes,
//!   Windows reparse points, unsupported compression methods (only Store 0 and Deflate 8),
//!   central-vs-local header differentials (method, flags, filename, and non-DD CRC/sizes),
//!   full Unicode case + NFD normalized collisions, Windows 8.3 short-name aliases, and
//!   file/directory clashes.
//! - Rejects path traversal (`..`), backslashes, colons, all Unicode control chars (C0, DEL, C1),
//!   trailing dots/spaces, and Windows reserved device names (`CON`, `NUL`, `COM0..9`, `COM¹..³`,
//!   `LPT0..9`, `LPT¹..³`).
//! - Creates files atomically with `OpenOptions::create_new(true)` (plus `O_NOFOLLOW` on Unix)
//!   so filesystem-level collisions or symlink races fail closed instead of overwriting.
//! - Inflates with `Read::take(entry.size + 1)` to stop zip bombs immediately and verifies CRC-32.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::path::Path;

use flate2::read::DeflateDecoder;
use icu_normalizer::DecomposingNormalizer;

pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_PATH_BYTES: usize = 1024;
pub const MAX_SEGMENT_BYTES: usize = 255;
pub const MAX_DEPTH: usize = 64;

/// Flag bits that must agree between central directory and local file header:
/// bit 0 (encrypted), bit 3 (data descriptor), bit 6 (strong encryption), bit 11 (UTF-8).
const CRITICAL_FLAG_MASK: u16 = (1 << 0) | (1 << 3) | (1 << 6) | (1 << 11);

#[derive(Clone, Debug)]
pub struct ZipEntry {
    pub segments: Vec<String>,
    pub directory: bool,
    pub method: u16,
    pub crc32: u32,
    pub data_start: usize,
    pub compressed: usize,
    pub size: u64,
}

/// Validates and splits a ZIP entry name into OS-safe path segments (`extensions.ts:372-387`).
/// Enforces `<= 1024` path bytes, `<= 255` segment bytes, `<= 64` depth, no control chars (C0/DEL/C1),
/// no Windows reserved device names (incl. `COM0`/`LPT0`/superscripts), and no Windows 8.3 aliases.
pub fn entry_path(name: &str) -> Option<Vec<String>> {
    let body = name.strip_suffix('/').unwrap_or(name);
    if body.len() > MAX_PATH_BYTES
        || body.starts_with('/')
        || body
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return None;
    }
    let mut segments = Vec::new();
    for seg in body.split('/') {
        if seg == "." {
            continue;
        }
        if seg.is_empty()
            || seg.len() > MAX_SEGMENT_BYTES
            || seg == ".."
            || seg.ends_with('.')
            || seg.ends_with(' ')
        {
            return None;
        }
        let stem = seg.split('.').next().unwrap_or(seg);
        if browser_policy::is_windows_device_name(stem) || is_windows_short_name(seg) {
            return None;
        }
        segments.push(seg.to_owned());
        if segments.len() > MAX_DEPTH {
            return None;
        }
    }
    Some(segments)
}

/// Detects Windows 8.3 short-name alias patterns (`NAME~1.EXT`) so an archive cannot pair a long
/// filename with its 8.3 alias on NTFS/FAT volumes that generate short names.
fn is_windows_short_name(seg: &str) -> bool {
    let (stem, ext) = match seg.split_once('.') {
        Some((s, e)) => (s, Some(e)),
        None => (seg, None),
    };
    if stem.len() > 8 || ext.is_some_and(|e| e.len() > 3 || e.contains('.')) {
        return false;
    }
    let Some((prefix, digits)) = stem.rsplit_once('~') else {
        return false;
    };
    !prefix.is_empty() && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Parses and validates all entries of `zip` before touching disk (`extensions.ts:394-445`).
pub fn read_zip(zip: &[u8]) -> Option<Vec<ZipEntry>> {
    let eocd = find_eocd(zip)?;
    if eocd.checked_add(22)? > zip.len() {
        return None;
    }
    let comment_len = read_u16_le(zip, eocd + 20)? as usize;
    if eocd.checked_add(22)?.checked_add(comment_len)? != zip.len() {
        return None;
    }
    let disk_num = read_u16_le(zip, eocd + 4)?;
    let cd_disk = read_u16_le(zip, eocd + 6)?;
    let disk_entries = read_u16_le(zip, eocd + 8)?;
    let total_entries = read_u16_le(zip, eocd + 10)?;
    let cd_size = read_u32_le(zip, eocd + 12)?;
    let cd_offset = read_u32_le(zip, eocd + 16)?;
    // Reject multi-disk archives and ZIP64 sentinels.
    if disk_num != 0
        || cd_disk != 0
        || disk_entries != total_entries
        || total_entries == u16::MAX
        || cd_size == u32::MAX
        || cd_offset == u32::MAX
    {
        return None;
    }
    let count = total_entries as usize;
    if count > MAX_ENTRIES {
        return None;
    }
    let cd_start = cd_offset as usize;
    let cd_end = cd_start.checked_add(cd_size as usize)?;
    // Require central directory to sit flush against EOCD (no unreferenced gap bytes).
    if cd_end != eocd {
        return None;
    }

    let nfd = DecomposingNormalizer::new_nfd();
    let mut list = Vec::with_capacity(count);
    let mut offset = cd_start;
    let mut total_uncompressed: u64 = 0;
    // Full Unicode case-folded + NFD-normalized collision detection (catches NTFS Unicode case
    // collisions and macOS APFS/HFS+ NFC/NFD collisions before touching disk).
    let mut seen_kind: HashMap<String, bool> = HashMap::with_capacity(count);
    let mut seen_dirs: HashSet<String> = HashSet::new();

    for _ in 0..count {
        let (entry, next) = parse_cd_entry(zip, offset, cd_start)?;
        if next > cd_end {
            return None;
        }
        total_uncompressed = total_uncompressed.checked_add(entry.size)?;
        if total_uncompressed > MAX_TOTAL_BYTES {
            return None;
        }
        if !entry.segments.is_empty() {
            let norm_segs: Vec<String> = entry
                .segments
                .iter()
                .map(|s| nfd.normalize(&s.to_lowercase()).into_owned())
                .collect();
            let key = norm_segs.join("/");
            if let Some(&prev_is_dir) = seen_kind.get(&key) {
                // Allow duplicate directory records, refuse duplicate files or dir/file clash.
                if !entry.directory || !prev_is_dir {
                    return None;
                }
            } else {
                seen_kind.insert(key.clone(), entry.directory);
            }
            // Ensure no ancestor prefix was already declared as a file; insert only unseen prefixes.
            let mut prefix = String::with_capacity(key.len());
            for ancestor in &norm_segs[..norm_segs.len().saturating_sub(1)] {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(ancestor);
                if seen_kind.get(&prefix) == Some(&false) {
                    return None;
                }
                if !seen_dirs.contains(&prefix) {
                    seen_dirs.insert(prefix.clone());
                }
            }
            if !entry.directory && seen_dirs.contains(&key) {
                return None;
            }
        }
        list.push(entry);
        offset = next;
    }
    if offset != cd_end {
        return None;
    }
    Some(list)
}

/// Unpacks a validated ZIP archive into a dedicated empty `dest_dir`, refusing any zip bomb,
/// CRC mismatch, filesystem collision, or symlink along the destination path.
/// On mid-archive error, removes every entry it created inside `dest_dir`.
pub fn unpack_zip(zip: &[u8], dest_dir: &Path) -> Result<(), String> {
    let entries = read_zip(zip).ok_or_else(|| "The extension package is damaged or unsafe.".to_owned())?;
    std::fs::create_dir_all(dest_dir).map_err(|e| format!("Could not create staging folder: {e}"))?;
    let root_meta = std::fs::symlink_metadata(dest_dir)
        .map_err(|e| format!("Could not verify staging folder: {e}"))?;
    if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
        return Err("Staging folder is not a regular directory.".into());
    }

    let mut created_files: Vec<std::path::PathBuf> = Vec::new();
    let mut created_dirs: Vec<std::path::PathBuf> = Vec::new();

    let result = (|| -> Result<(), String> {
        for entry in entries {
            if entry.segments.is_empty() {
                continue;
            }
            let mut target = dest_dir.to_path_buf();
            for seg in &entry.segments {
                target.push(seg);
            }
            if entry.directory {
                create_dir_no_symlink(dest_dir, &entry.segments, &mut created_dirs)?;
                continue;
            }
            if entry.segments.len() > 1 {
                create_dir_no_symlink(
                    dest_dir,
                    &entry.segments[..entry.segments.len() - 1],
                    &mut created_dirs,
                )?;
            }
            let body = &zip[entry.data_start..entry.data_start + entry.compressed];
            let out: Vec<u8> = if entry.method == 0 {
                body.to_vec()
            } else {
                let mut decoded = Vec::with_capacity((entry.size as usize).min(1024 * 1024));
                DeflateDecoder::new(body)
                    .take(entry.size + 1)
                    .read_to_end(&mut decoded)
                    .map_err(|_| "The extension package is damaged.".to_owned())?;
                decoded
            };
            if out.len() as u64 != entry.size {
                return Err("The extension package size did not match its header.".into());
            }
            if crc32fast::hash(&out) != entry.crc32 {
                return Err("The extension package failed CRC-32 verification.".into());
            }
            // Atomic exclusive creation (`O_CREAT | O_EXCL | O_NOFOLLOW` on Unix, `CREATE_NEW` on
            // Windows): never follows symlinks and never silently overwrites colliding filenames.
            let mut file = open_new_no_follow(&target)
                .map_err(|e| format!("Could not create extension file cleanly: {e}"))?;
            created_files.push(target);
            file.write_all(&out)
                .map_err(|e| format!("Could not write extension file: {e}"))?;
        }
        Ok(())
    })();

    if result.is_err() {
        for f in created_files.into_iter().rev() {
            std::fs::remove_file(f).ok();
        }
        for d in created_dirs.into_iter().rev() {
            std::fs::remove_dir(d).ok();
        }
    }
    result
}

fn open_new_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path)
}

fn create_dir_no_symlink(
    root: &Path,
    segments: &[String],
    created_dirs: &mut Vec<std::path::PathBuf>,
) -> Result<(), String> {
    let mut cur = root.to_path_buf();
    for seg in segments {
        cur.push(seg);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) => {
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    return Err("Refusing to traverse non-directory or symlink while unpacking.".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&cur).map_err(|e| format!("Could not create directory: {e}"))?;
                created_dirs.push(cur.clone());
            }
            Err(e) => return Err(format!("Could not inspect directory while unpacking: {e}")),
        }
    }
    Ok(())
}

fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let min_pos = data.len().saturating_sub(22 + 65_535);
    let mut pos = data.len() - 22;
    loop {
        if &data[pos..pos + 4] == b"PK\x05\x06" {
            let comment = read_u16_le(data, pos + 20)? as usize;
            if pos.checked_add(22)?.checked_add(comment)? == data.len() {
                return Some(pos);
            }
        }
        if pos == min_pos {
            return None;
        }
        pos -= 1;
    }
}

fn parse_cd_entry(data: &[u8], at: usize, cd_start: usize) -> Option<(ZipEntry, usize)> {
    if at.checked_add(46)? > data.len() || read_u32_le(data, at)? != 0x0201_4b50 {
        return None;
    }
    let version_made_by = read_u16_le(data, at + 4)?;
    let flags = read_u16_le(data, at + 8)?;
    let method = read_u16_le(data, at + 10)?;
    let crc32 = read_u32_le(data, at + 16)?;
    let compressed_u32 = read_u32_le(data, at + 20)?;
    let size_u32 = read_u32_le(data, at + 24)?;
    let name_len = read_u16_le(data, at + 28)? as usize;
    let extra_len = read_u16_le(data, at + 30)? as usize;
    let comment_len = read_u16_le(data, at + 32)? as usize;
    let disk_start = read_u16_le(data, at + 34)?;
    let ext_attr = read_u32_le(data, at + 38)?;
    let local_u32 = read_u32_le(data, at + 42)?;

    if disk_start != 0 || compressed_u32 == u32::MAX || size_u32 == u32::MAX || local_u32 == u32::MAX {
        return None;
    }
    let next = at
        .checked_add(46)?
        .checked_add(name_len)?
        .checked_add(extra_len)?
        .checked_add(comment_len)?;
    if next > data.len() {
        return None;
    }
    let name_bytes = &data[at + 46..at + 46 + name_len];
    let name = std::str::from_utf8(name_bytes).ok()?;
    let segments = entry_path(name)?;
    let directory = name.ends_with('/');

    // Reject encrypted (bit 0) and strongly encrypted (bit 6) entries.
    if (flags & ((1 << 0) | (1 << 6))) != 0 {
        return None;
    }
    // Reject Unix (3) and macOS (19) symlinks & special device/FIFO/socket file types.
    let host_os = version_made_by >> 8;
    if host_os == 3 || host_os == 19 {
        let mode = (ext_attr >> 16) & 0o170_000;
        if mode != 0 && mode != 0o100_000 && mode != 0o040_000 {
            return None;
        }
    }
    // Reject Windows/DOS/NTFS reparse points (symlinks / junctions, FILE_ATTRIBUTE_REPARSE_POINT).
    if matches!(host_os, 0 | 10 | 11 | 14) && (ext_attr & 0x0400) != 0 {
        return None;
    }
    if !directory && segments.is_empty() {
        return None;
    }
    if method != 0 && method != 8 {
        return None;
    }
    let size = u64::from(size_u32);
    let compressed = compressed_u32 as usize;
    if directory {
        if size != 0 || crc32 != 0 {
            return None;
        }
        // Google CWS stores empty Deflate directory entries (`compressed == 2` for `[0x03, 0x00]`);
        // allow only tiny Deflate streams that actually inflate to 0 bytes, and 0 for Store.
        if method == 0 && compressed != 0 {
            return None;
        }
        if method == 8 && compressed > 16 {
            return None;
        }
    } else if size > MAX_ENTRY_BYTES || (method == 0 && compressed_u32 != size_u32) {
        return None;
    }

    // Strict local-file-header consistency check (blocks Janus / differential ZIP parsers).
    let local = local_u32 as usize;
    if local.checked_add(30)? > cd_start || read_u32_le(data, local)? != 0x0403_4b50 {
        return None;
    }
    let local_flags = read_u16_le(data, local + 6)?;
    let local_method = read_u16_le(data, local + 8)?;
    let local_crc32 = read_u32_le(data, local + 14)?;
    let local_comp = read_u32_le(data, local + 18)?;
    let local_size = read_u32_le(data, local + 22)?;
    if (local_flags & ((1 << 0) | (1 << 6))) != 0
        || (local_flags & CRITICAL_FLAG_MASK) != (flags & CRITICAL_FLAG_MASK)
        || local_method != method
    {
        return None;
    }
    // When data-descriptor bit 3 is unset, local CRC and sizes must match central directory.
    if (flags & (1 << 3)) == 0
        && (local_crc32 != crc32 || local_comp != compressed_u32 || local_size != size_u32)
    {
        return None;
    }

    let local_name_len = read_u16_le(data, local + 26)? as usize;
    let local_extra_len = read_u16_le(data, local + 28)? as usize;
    let local_name_end = local.checked_add(30)?.checked_add(local_name_len)?;
    if local_name_end > cd_start || &data[local + 30..local_name_end] != name_bytes {
        return None;
    }
    let data_start = local_name_end.checked_add(local_extra_len)?;
    let data_end = data_start.checked_add(compressed)?;
    if data_end > cd_start {
        return None;
    }
    // Verify Deflate directory entries genuinely decompress to 0 bytes (zero hidden payload).
    if directory && method == 8 && compressed > 0 {
        let mut probe = Vec::new();
        DeflateDecoder::new(&data[data_start..data_end])
            .take(1)
            .read_to_end(&mut probe)
            .ok()?;
        if !probe.is_empty() {
            return None;
        }
    }

    Some((
        ZipEntry { segments, directory, method, crc32, data_start, compressed, size },
        next,
    ))
}

fn read_u16_le(data: &[u8], off: usize) -> Option<u16> {
    let end = off.checked_add(2)?;
    let b = data.get(off..end)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32_le(data: &[u8], off: usize) -> Option<u32> {
    let end = off.checked_add(4)?;
    let b = data.get(off..end)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
