//! Unpacks the engine archive as it arrives: zstd (at most an 8 MB window, so unpacking needs
//! little memory) around a ustar archive of plain files under `OpenCode Browser Engine/`. Any
//! other entry (a link, a device, an absolute or `..` path) fails the install, so an archive can
//! only ever write inside the folder it unpacks into.

use std::collections::HashSet;
use std::fs::{DirBuilder, OpenOptions};
use std::io::{BufWriter, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::download::Cut;
use crate::installer::{ENGINE_BUNDLE_NAME, ERR_VERIFY, err_write};

/// The largest zstd window this unpacks, as a power of two (package-windows.ps1 compresses at
/// level 19: 8 MB).
const MAX_WINDOW_LOG: u32 = 23;

pub(crate) enum UnpackError {
    /// It isn't an archive this unpacks, or the stream under it failed (the download's reader
    /// then knows why).
    Format,
    /// Writing a file failed: the message to show.
    Write(String),
}

/// Unpacks `archive` into `dest` (a fresh staging directory owned by the caller), calling `unpacked` with each file's path once
/// it's whole and closed. Returns once the archive's end marker is read.
pub(crate) fn tar_zst(archive: &mut dyn Read, dest: &Path, unpacked: &mut dyn FnMut(&Path)) -> Result<(), UnpackError> {
    // The decoder reads its input in large pieces (a download's reads are syscalls).
    let mut zstd = zstd::stream::read::Decoder::new(archive).map_err(|_| UnpackError::Format)?;
    zstd.window_log_max(MAX_WINDOW_LOG).map_err(|_| UnpackError::Format)?;
    if !std::fs::symlink_metadata(dest).map_err(|e| UnpackError::Write(err_write(&e)))?.is_dir() {
        return Err(UnpackError::Format);
    }
    let mut made = HashSet::from([dest.to_owned()]);
    let mut buf = vec![0u8; 1 << 20];
    let mut header = [0u8; 512];
    loop {
        read_exact(&mut zstd, &mut header)?;
        if header.iter().all(|&b| b == 0) {
            return Ok(());
        }
        let entry = parse(&header).ok_or(UnpackError::Format)?;
        let path = dest.join(&entry.path);
        match entry.kind {
            Kind::Dir => make_dir(&path, &mut made)?,
            Kind::File { mode } => {
                make_dir(path.parent().ok_or(UnpackError::Format)?, &mut made)?;
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                // Only the execute bits come from the archive; creation respects the user's umask.
                #[cfg(unix)]
                options.mode(0o644 | (mode & 0o111));
                #[cfg(not(unix))]
                let _ = mode;
                let mut out = BufWriter::with_capacity(1 << 20, options.open(&path).map_err(create_error)?);
                let mut left = entry.size;
                while left > 0 {
                    let n = left.min(buf.len() as u64) as usize;
                    read_exact(&mut zstd, &mut buf[..n])?;
                    write(&mut out, &buf[..n]).map_err(|e| UnpackError::Write(err_write(&e)))?;
                    left -= n as u64;
                }
                out.flush().map_err(|e| UnpackError::Write(err_write(&e)))?;
                drop(out);
                unpacked(&path);
            }
        }
        // Entries end on a 512-byte boundary.
        let pad = (512 - entry.size % 512) % 512;
        read_exact(&mut zstd, &mut buf[..pad as usize])?;
    }
}

/// Create each component ourselves, including parents omitted by the packager. An existing
/// object in staging could be a link, a duplicate file, or a filesystem alias of an earlier name.
fn make_dir(path: &Path, made: &mut HashSet<PathBuf>) -> Result<(), UnpackError> {
    if made.contains(path) {
        return Ok(());
    }
    make_dir(path.parent().ok_or(UnpackError::Format)?, made)?;
    let mut dir = DirBuilder::new();
    #[cfg(unix)]
    dir.mode(0o755);
    dir.recursive(false).create(path).map_err(create_error)?;
    made.insert(path.to_owned());
    Ok(())
}

fn create_error(e: std::io::Error) -> UnpackError {
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        UnpackError::Format
    } else {
        UnpackError::Write(err_write(&e))
    }
}

fn write(out: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(feature = "faults")]
    fault::write()?;
    out.write_all(bytes)
}

/// Harness builds only (the `faults` feature): `OPENCODE_BROWSER_FAULT_DISK_FULL=<n>` makes the
/// process's nth unpack write fail as a full disk does (ERROR_DISK_FULL, ENOSPC), once, so the
/// pane's disk-full case can be shown without filling a disk.
#[cfg(feature = "faults")]
mod fault {
    use std::sync::atomic::{AtomicU64, Ordering};

    static WRITES: AtomicU64 = AtomicU64::new(0);

    pub(super) fn write() -> std::io::Result<()> {
        let fail_at = std::env::var_os("OPENCODE_BROWSER_FAULT_DISK_FULL").and_then(|n| n.to_str()?.parse::<u64>().ok());
        if fail_at != Some(WRITES.fetch_add(1, Ordering::Relaxed) + 1) {
            return Ok(());
        }
        #[cfg(windows)]
        return Err(std::io::Error::from_raw_os_error(112));
        #[cfg(unix)]
        return Err(std::io::Error::from_raw_os_error(libc::ENOSPC));
    }
}

fn read_exact(r: &mut dyn Read, buf: &mut [u8]) -> Result<(), UnpackError> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => return Err(UnpackError::Format),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(UnpackError::Format),
        }
    }
    Ok(())
}

enum Kind {
    File { mode: u32 },
    Dir,
}

struct Entry {
    path: PathBuf,
    size: u64,
    kind: Kind,
}

/// A ustar header: a plain file or a folder under the engine's folder, its checksum right.
fn parse(h: &[u8; 512]) -> Option<Entry> {
    let field = |range: std::ops::Range<usize>| {
        let f = &h[range];
        std::str::from_utf8(&f[..f.iter().position(|&b| b == 0).unwrap_or(f.len())]).ok()
    };
    let octal = |range| u64::from_str_radix(field(range)?.trim(), 8).ok();
    if &h[257..262] != b"ustar" {
        return None;
    }
    let sum: u64 = h.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { u64::from(b) }).sum();
    if octal(148..156)? != sum {
        return None;
    }
    let size = octal(124..136)?;
    let kind = match h[156] {
        b'0' | 0 => Kind::File { mode: u32::try_from(octal(100..108)?).ok()? },
        b'5' if size == 0 => Kind::Dir,
        _ => return None,
    };
    let (prefix, name) = (field(345..500)?, field(0..100)?);
    let full = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
    let full = match kind {
        Kind::Dir => full.strip_suffix('/').unwrap_or(&full),
        Kind::File { .. } => full.as_str(),
    };
    let mut parts = full.split('/');
    if parts.next()? != ENGINE_BUNDLE_NAME || !parts.all(safe_name) {
        return None;
    }
    if matches!(kind, Kind::File { .. }) && full == ENGINE_BUNDLE_NAME {
        return None;
    }
    Some(Entry { path: PathBuf::from(full), size, kind })
}

fn safe_name(name: &str) -> bool {
    // Both packagers use portable filenames. Exclude dot components, alternate streams,
    // DOS device names and names Windows would silently trim or interpret as a separator.
    if name.is_empty() || name.ends_with(['.', ' ']) || name.contains(['\\', ':', '<', '>', '"', '|', '?', '*']) || name.chars().any(char::is_control) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ').to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return false;
    }
    let port = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT"));
    !port.is_some_and(|n| matches!(n.as_bytes(), [b'0'..=b'9']) || matches!(n, "¹" | "²" | "³"))
}

impl UnpackError {
    /// How the install ends (when the download's reader didn't fail first): a broken archive is
    /// never resumed, a failed write is.
    pub(crate) fn cut(self) -> Cut {
        match self {
            UnpackError::Format => Cut::Rejected(ERR_VERIFY.into()),
            UnpackError::Write(msg) => Cut::Failed(msg),
        }
    }
}
