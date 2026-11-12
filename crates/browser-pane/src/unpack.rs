//! Unpacks the engine archive as it arrives: zstd (at most an 8 MB window, so unpacking needs
//! little memory) around a ustar archive of plain files under `OpenCode Browser Engine/`. Any
//! other entry (a link, a device, an absolute or `..` path) fails the install, so an archive can
//! only ever write inside the folder it unpacks into.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::download::Cut;
use crate::installer::{ENGINE_BUNDLE_NAME, ERR_VERIFY, err_write};

/// The largest zstd window this unpacks (package-windows.ps1 compresses at level 19: 8 MB).
const MAX_WINDOW: u64 = 8 << 20;

pub(crate) enum UnpackError {
    /// It isn't an archive this unpacks, or the stream under it failed (the download's reader
    /// then knows why).
    Format,
    /// Writing a file failed: the message to show.
    Write(String),
}

/// Unpacks `archive` into `dest` (which must exist), calling `unpacked` with each file's path once
/// it's whole and closed. Returns once the archive's end marker is read.
pub(crate) fn tar_zst(archive: &mut dyn Read, dest: &Path, unpacked: &mut dyn FnMut(&Path)) -> Result<(), UnpackError> {
    let mut zstd = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(archive, MAX_WINDOW).map_err(|_| UnpackError::Format)?;
    let mut made: HashSet<PathBuf> = HashSet::new();
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
            Kind::Dir => {
                std::fs::create_dir_all(&path).map_err(|e| UnpackError::Write(err_write(&e)))?;
                made.insert(path);
            }
            Kind::File => {
                if let Some(parent) = path.parent()
                    && made.insert(parent.to_owned())
                {
                    std::fs::create_dir_all(parent).map_err(|e| UnpackError::Write(err_write(&e)))?;
                }
                let mut out = BufWriter::with_capacity(1 << 20, File::create(&path).map_err(|e| UnpackError::Write(err_write(&e)))?);
                let mut left = entry.size;
                while left > 0 {
                    let n = buf.len().min(left as usize);
                    read_exact(&mut zstd, &mut buf[..n])?;
                    out.write_all(&buf[..n]).map_err(|e| UnpackError::Write(err_write(&e)))?;
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
    File,
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
    let kind = match h[156] {
        b'0' | 0 => Kind::File,
        b'5' => Kind::Dir,
        _ => return None,
    };
    let (prefix, name) = (field(345..500)?, field(0..100)?);
    let full = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
    let path = PathBuf::from(full.trim_end_matches('/'));
    let mut parts = path.components();
    let inside = parts.next() == Some(Component::Normal(ENGINE_BUNDLE_NAME.as_ref())) && parts.all(|c| matches!(c, Component::Normal(_)));
    inside.then(|| Entry { size: if matches!(kind, Kind::Dir) { 0 } else { octal(124..136).unwrap_or(u64::MAX) }, path, kind }).filter(|e| e.size != u64::MAX)
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
