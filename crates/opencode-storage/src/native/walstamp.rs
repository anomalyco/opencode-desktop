//! Where SQLite's WAL ends: its header's salts and the last frame's checksums, read from the file
//! (SQLite's WAL format: a 32-byte header, then frames of a 24-byte header and a page). Two small
//! reads; the stamp changes with every frame written after it, and with every checkpoint's restart.

use std::fs::File;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub page_size: u32,
    pub salt1: u32,
    pub salt2: u32,
    pub frames: u32,
    pub ck1: u32,
    pub ck2: u32,
}

impl Stamp {
    pub(crate) fn encode(&self) -> [u8; 24] {
        let mut b = [0u8; 24];
        for (i, v) in [self.page_size, self.salt1, self.salt2, self.frames, self.ck1, self.ck2].iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b
    }

    pub(crate) fn decode(b: &[u8]) -> Option<Stamp> {
        let w = |i: usize| b.get(i * 4..i * 4 + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes);
        Some(Stamp { page_size: w(0)?, salt1: w(1)?, salt2: w(2)?, frames: w(3)?, ck1: w(4)?, ck2: w(5)? })
    }
}

/// The WAL's stamp, when it holds whole frames ending in a commit (a frame whose header names
/// the database's size after it); none for an empty, torn or mid-transaction WAL.
pub(crate) fn read(wal: &File) -> Option<Stamp> {
    let len = wal.metadata().ok()?.len();
    let mut head = [0u8; 32];
    if super::boot_record::read_some(wal, &mut head, 0)? != 32 {
        return None;
    }
    let be = |b: &[u8], i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    if !matches!(be(&head, 0), 0x377f_0682 | 0x377f_0683) {
        return None;
    }
    let page_size = be(&head, 8);
    let frame = 24 + u64::from(page_size);
    if page_size == 0 || len < 32 + frame || (len - 32) % frame != 0 {
        return None;
    }
    let frames = u32::try_from((len - 32) / frame).ok()?;
    let mut last = [0u8; 24];
    if super::boot_record::read_some(wal, &mut last, 32 + (u64::from(frames) - 1) * frame)? != 24 {
        return None;
    }
    let (salt1, salt2) = (be(&head, 16), be(&head, 20));
    if be(&last, 4) == 0 || be(&last, 8) != salt1 || be(&last, 12) != salt2 {
        return None;
    }
    Some(Stamp { page_size, salt1, salt2, frames, ck1: be(&last, 16), ck2: be(&last, 20) })
}
