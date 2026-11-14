//! The boot record's copy beside the database (`<db>-boot`), for a launch after a quit that left
//! the WAL pending: the quit syncs the WAL rather than checkpointing it, so the record in the main
//! file is behind, and this copy, stamped with where the WAL ended when it was written, is what the
//! next launch's first frame reads instead of opening SQLite for it.
//!
//! - **Written** by the quit, after its last commit and the WAL's sync: a 64-byte header (length,
//!   the WAL's stamp, both checksums) and the record, in one write. Not synced: a copy the OS never
//!   wrote, or a torn one, fails its checksums and the launch reads the record through SQLite as
//!   before.
//! - **Trusted** only when the WAL still ends exactly where its stamp says (the same salts, frame
//!   count and last frame's checksums): any frame written since, a checkpoint, a kill before the
//!   copy was written, and it's ignored. Nothing else reads it: an older build never looks.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::walstamp::Stamp;

const MAGIC: u32 = u32::from_le_bytes(*b"RNBC");
const HEAD: usize = 64;
/// The first read: the header and, for every record within budget, all of it.
const FIRST_READ: usize = 64 * 1024;

pub(crate) fn path(db: &Path) -> PathBuf {
    PathBuf::from(format!("{}-boot", db.display()))
}

fn sum(b: &[u8]) -> u64 {
    twox_hash::XxHash3_64::oneshot(b)
}

/// The copy of `record` with the WAL at `stamp`, over whatever copy was there.
pub(crate) fn write(db: &Path, record: &[u8], stamp: &Stamp) -> std::io::Result<()> {
    let mut h = [0u8; HEAD];
    h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    h[4..8].copy_from_slice(&1u32.to_le_bytes());
    h[8..12].copy_from_slice(&u32::try_from(record.len()).map_err(|_| std::io::ErrorKind::InvalidInput)?.to_le_bytes());
    h[12..36].copy_from_slice(&stamp.encode());
    h[40..48].copy_from_slice(&sum(record).to_le_bytes());
    let s = sum(&h[..56]);
    h[56..64].copy_from_slice(&s.to_le_bytes());
    let mut bytes = Vec::with_capacity(HEAD + record.len());
    bytes.extend_from_slice(&h);
    bytes.extend_from_slice(record);
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path(db))?;
    f.write_all(&bytes)
}

/// The copy beside `db`, when it's there and whole: its WAL stamp and its bytes.
pub(crate) fn read(db: &Path) -> Option<(Stamp, Vec<u8>)> {
    let file = File::open(path(db)).ok()?;
    let mut buf = vec![0u8; FIRST_READ];
    let n = super::boot_record::read_some(&file, &mut buf, 0)?;
    if n < HEAD {
        return None;
    }
    let h = &buf[..HEAD];
    let w = |i: usize| u32::from_le_bytes(h[i..i + 4].try_into().unwrap_or_default());
    let l = |i: usize| u64::from_le_bytes(h[i..i + 8].try_into().unwrap_or_default());
    if w(0) != MAGIC || w(4) != 1 || sum(&h[..56]) != l(56) {
        return None;
    }
    let (len, want, stamp) = (w(8) as usize, l(40), Stamp::decode(&h[12..36])?);
    let mut body = buf.split_off(HEAD);
    body.truncate(n - HEAD);
    if body.len() < len {
        let have = body.len();
        body.resize(len, 0);
        if super::boot_record::read_some(&file, &mut body[have..], (HEAD + have) as u64)? != len - have {
            return None;
        }
    }
    body.truncate(len);
    (sum(&body) == want).then_some((stamp, body))
}
