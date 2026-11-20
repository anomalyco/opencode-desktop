//! Bounded, symlink-free ZIP validator and unpacker for verified CRX3 payloads.
//!
//! Mirrors `packages/gui-plugins/src/browser/profile/extensions.ts:353-475` with additional
//! hardening:
//! - Validates EVERY central directory entry and local header before writing any file.
//! - Hard caps: `<= 10,000` entries, `<= 64 MiB` per entry, `<= 256 MiB` total uncompressed.
//! - Rejects ZIP64, multi-disk archives, encrypted entries, Unix symlinks and special device
//!   nodes (FIFO/char/block/socket), unsupported compression methods (only Store 0 and Deflate 8),
//!   central/local filename mismatches, duplicate normalized paths, and file/directory collisions.
//! - Rejects path traversal (`..`), backslashes, drive/ADS colons, control chars, trailing dots
//!   or spaces, and Windows reserved device names (`CON`, `NUL`, `COM1..9`, `COM¹..³`, `LPT¹..³`).
//! - Inflates with `Read::take(entry.size + 1)` to stop zip bombs immediately and verifies CRC-32.

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::Path;

use flate2::read::DeflateDecoder;

pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

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
pub fn entry_path(name: &str) -> Option<Vec<String>> {
    let body = name.strip_suffix('/').unwrap_or(name);
    if body.starts_with('/')
        || body.chars().any(|c| {
            (c as u32) <= 0x1f || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
    {
        return None;
    }
    let mut segments = Vec::new();
    for seg in body.split('/') {
        if seg == "." {
            continue;
        }
        if seg.is_empty() || seg == ".." || seg.ends_with('.') || seg.ends_with(' ') {
            return None;
        }
        let stem = seg.split('.').next().unwrap_or(seg);
        if browser_policy::is_windows_device_name(stem) {
            return None;
        }
        segments.push(seg.to_owned());
    }
    Some(segments)
}

/// Parses and validates all entries of `zip` before touching disk (`extensions.ts:394-445`).
pub fn read_zip(zip: &[u8]) -> Option<Vec<ZipEntry>> {
    let eocd = find_eocd(zip)?;
    if eocd + 22 > zip.len() {
        return None;
    }
    let comment_len = read_u16_le(zip, eocd + 20)? as usize;
    if eocd + 22 + comment_len != zip.len() {
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
    if cd_end > eocd {
        return None;
    }

    let mut list = Vec::with_capacity(count);
    let mut offset = cd_start;
    let mut total_uncompressed: u64 = 0;
    // Case-insensitive path collision detection (prevents Windows/macOS overwrite attacks).
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
            let key = entry.segments.join("/").to_ascii_lowercase();
            if let Some(&prev_is_dir) = seen_kind.get(&key) {
                // Allow duplicate directory records, refuse duplicate files or dir/file clash.
                if !entry.directory || !prev_is_dir {
                    return None;
                }
            } else {
                seen_kind.insert(key.clone(), entry.directory);
            }
            // Ensure no ancestor prefix was already declared as a file.
            let mut prefix = String::new();
            for ancestor in &entry.segments[..entry.segments.len().saturating_sub(1)] {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(&ancestor.to_ascii_lowercase());
                if seen_kind.get(&prefix) == Some(&false) {
                    return None;
                }
                seen_dirs.insert(prefix.clone());
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

/// Unpacks a validated ZIP archive into `dest_dir`, refusing any zip bomb, CRC mismatch, or
/// symlink along the destination path (`extensions.ts:452-475`).
pub fn unpack_zip(zip: &[u8], dest_dir: &Path) -> Result<(), String> {
    let entries = read_zip(zip).ok_or_else(|| "The extension package is damaged or unsafe.".to_owned())?;
    std::fs::create_dir_all(dest_dir).map_err(|e| format!("Could not create staging folder: {e}"))?;
    let root_meta = std::fs::symlink_metadata(dest_dir)
        .map_err(|e| format!("Could not verify staging folder: {e}"))?;
    if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
        return Err("Staging folder is not a regular directory.".into());
    }

    for entry in entries {
        if entry.segments.is_empty() {
            continue;
        }
        let mut target = dest_dir.to_path_buf();
        for seg in &entry.segments {
            target.push(seg);
        }
        if entry.directory {
            create_dir_no_symlink(dest_dir, &entry.segments)?;
            continue;
        }
        if entry.segments.len() > 1 {
            create_dir_no_symlink(dest_dir, &entry.segments[..entry.segments.len() - 1])?;
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
        if let Ok(existing) = std::fs::symlink_metadata(&target)
            && existing.file_type().is_symlink()
        {
            return Err("Refusing to write through a symbolic link.".into());
        }
        std::fs::write(&target, &out).map_err(|e| format!("Could not unpack extension file: {e}"))?;
    }
    Ok(())
}

fn create_dir_no_symlink(root: &Path, segments: &[String]) -> Result<(), String> {
    let mut cur = root.to_path_buf();
    for seg in segments {
        cur.push(seg);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) => {
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    return Err("Refusing to traverse non-directory or symlink while unpacking.".into());
                }
            }
            Err(_) => {
                std::fs::create_dir(&cur).map_err(|e| format!("Could not create directory: {e}"))?;
            }
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
            if pos + 22 + comment == data.len() {
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
    if at + 46 > data.len() || read_u32_le(data, at)? != 0x0201_4b50 {
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
    let ext_attr = read_u32_le(data, at + 38)?;
    let local_u32 = read_u32_le(data, at + 42)?;

    if compressed_u32 == u32::MAX || size_u32 == u32::MAX || local_u32 == u32::MAX {
        return None;
    }
    let next = at.checked_add(46)?.checked_add(name_len)?.checked_add(extra_len)?.checked_add(comment_len)?;
    if next > data.len() {
        return None;
    }
    let name_bytes = &data[at + 46..at + 46 + name_len];
    let name = std::str::from_utf8(name_bytes).ok()?;
    let segments = entry_path(name)?;
    let directory = name.ends_with('/');

    // Reject encrypted entries.
    if (flags & 1) != 0 {
        return None;
    }
    // Reject Unix symlinks and special device/FIFO/socket file types.
    if (version_made_by >> 8) == 3 {
        let mode = (ext_attr >> 16) & 0o170_000;
        if mode != 0 && mode != 0o100_000 && mode != 0o040_000 {
            return None;
        }
    }
    if !directory && segments.is_empty() {
        return None;
    }
    let size = u64::from(size_u32);
    let compressed = compressed_u32 as usize;
    if directory && size != 0 {
        return None;
    }
    if !directory && (size > MAX_ENTRY_BYTES || !(method == 8 || (method == 0 && compressed_u32 == size_u32))) {
        return None;
    }

    let local = local_u32 as usize;
    if local.checked_add(30)? > cd_start || read_u32_le(data, local)? != 0x0403_4b50 {
        return None;
    }
    let local_flags = read_u16_le(data, local + 6)?;
    if (local_flags & 1) != 0 {
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

    Some((
        ZipEntry { segments, directory, method, crc32, data_start, compressed, size },
        next,
    ))
}

fn read_u16_le(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32_le(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
