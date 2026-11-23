//! Official Desktop's `drafts.sqlite` as SQLite reads it with its WAL applied, built in memory.
//!
//! Desktop running (or killed) leaves a `-wal` with committed frames beside the database, so the
//! file alone is not the database. A new profile's first frame waits for the seed's rows, and a
//! scratch copy of both files wrote every byte to disk first: 380,867 us to the first frame for
//! a 126 MB database with a small WAL, launches of 1,560,618 and 4,160,481 us while the disk was
//! busy, and 2,096,977 us to the packet for a 67 MB database with a 64 MB WAL (112,149 us here).
//! Here nothing is written: the database's pages are read once, straight into the memory SQLite
//! then opens ([`rusqlite::Connection::deserialize_read_exact`]), with the page of each
//! committed WAL frame in place of the file's. That is what a checkpoint would leave in the file
//! (`walIndexRecover` and `walCheckpoint` in `sqlite3.c`; the format is "WAL File Format" in
//! <https://www.sqlite.org/fileformat2.html>).
//!
//! Neither file is locked and Desktop's folder is not written. What was read counts only when
//! both files look the same before and after ([`super::snapshot::read_stamp`]): the WAL is read
//! first, so a checkpoint that runs meanwhile only moves pages the image already takes from the
//! WAL, and a commit or a WAL restart meanwhile changes the stamp.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek as _, SeekFrom};
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::snapshot::{MAX_SEED_DB_BYTES, MAX_SEED_WAL_BYTES, read_stamp};

const ATTEMPTS: u32 = 3;
const WAL_HEADER: usize = 32;
const FRAME_HEADER: usize = 24;

fn be(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn page_size_ok(size: usize) -> bool {
    size.is_power_of_two() && (512..=65536).contains(&size)
}

/// SQLite's WAL checksum (`walChecksumBytes`) over `bytes` (a multiple of 8 long), carried on
/// from `sum`. `big`: the WAL's magic says its 32-bit words are big-endian.
fn checksum(big: bool, bytes: &[u8], mut sum: (u32, u32)) -> (u32, u32) {
    let word = |b: &[u8]| {
        let b = [b[0], b[1], b[2], b[3]];
        if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
    };
    for pair in bytes.chunks_exact(8) {
        sum.0 = sum.0.wrapping_add(word(&pair[..4])).wrapping_add(sum.1);
        sum.1 = sum.1.wrapping_add(word(&pair[4..])).wrapping_add(sum.0);
    }
    sum
}

/// The committed part of a WAL.
struct Wal {
    bytes: Vec<u8>,
    page_size: usize,
    /// Page number to where its newest committed frame's page starts in `bytes`.
    at: HashMap<u32, usize>,
    /// The database's size in pages after the last commit.
    pages: u32,
}

/// The frames SQLite's recovery accepts, up to the last commit: each has the header's salts and
/// a checksum that carries on from the frame before it. None when no commit is in the WAL (an
/// empty one, one from before a restart, or not a WAL): the database file is then the database.
fn committed(bytes: Vec<u8>) -> Option<Wal> {
    if bytes.len() < WAL_HEADER {
        return None;
    }
    let magic = be(&bytes, 0);
    let page_size = be(&bytes, 8) as usize;
    if magic & !1 != 0x377f_0682 || be(&bytes, 4) != 3_007_000 || !page_size_ok(page_size) {
        return None;
    }
    let big = magic & 1 == 1;
    let mut sum = checksum(big, &bytes[..24], (0, 0));
    if sum != (be(&bytes, 24), be(&bytes, 28)) {
        return None;
    }
    let (mut at, mut pending, mut pages) = (HashMap::new(), Vec::new(), 0u32);
    let mut frame = WAL_HEADER;
    while frame + FRAME_HEADER + page_size <= bytes.len() {
        let data = frame + FRAME_HEADER;
        let page = be(&bytes, frame);
        if page == 0 || bytes[frame + 8..frame + 16] != bytes[16..24] {
            break;
        }
        sum = checksum(big, &bytes[data..data + page_size], checksum(big, &bytes[frame..frame + 8], sum));
        if sum != (be(&bytes, frame + 16), be(&bytes, frame + 20)) {
            break;
        }
        pending.push((page, data));
        let size = be(&bytes, frame + 4);
        if size != 0 {
            at.extend(pending.drain(..));
            pages = size;
        }
        frame = data + page_size;
    }
    (pages != 0).then_some(Wal { bytes, page_size, at, pages })
}

/// The image, read front to back: page by page from the WAL where it has the page, from the
/// file otherwise (one read for every run of pages between two the WAL has).
struct Image {
    db: File,
    db_len: u64,
    wal: Option<Wal>,
    /// The WAL's pages, sorted.
    in_wal: Vec<u32>,
    page_size: u64,
    len: u64,
    pos: u64,
}

impl Read for Image {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let page = u32::try_from(self.pos / self.page_size + 1).map_err(io::Error::other)?;
        let within = (self.pos % self.page_size) as usize;
        let framed = self.wal.as_ref().and_then(|w| Some((w, *w.at.get(&page)?)));
        let n = if let Some((wal, at)) = framed {
            let n = buf.len().min(self.page_size as usize - within);
            buf[..n].copy_from_slice(&wal.bytes[at + within..at + within + n]);
            n
        } else {
            let next = self.in_wal.partition_point(|&p| p < page);
            let end = self.in_wal.get(next).map_or(self.len, |&p| (u64::from(p) - 1) * self.page_size).min(self.len);
            let n = buf.len().min(usize::try_from(end - self.pos).unwrap_or(usize::MAX));
            if self.pos >= self.db_len {
                // A page the last commit counts and neither file holds.
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a page is in neither the database nor its WAL"));
            }
            let n = n.min(usize::try_from(self.db_len - self.pos).unwrap_or(usize::MAX));
            self.db.seek(SeekFrom::Start(self.pos))?;
            self.db.read(&mut buf[..n])?
        };
        // The header's two format bytes say "WAL" (2) in a database Desktop has open; SQLite has no
        // WAL for a database in memory and refuses to open one that says so. 1 is "rollback journal".
        for byte in 18..20u64 {
            if (self.pos..self.pos + n as u64).contains(&byte) {
                buf[(byte - self.pos) as usize] = 1;
            }
        }
        self.pos += n as u64;
        Ok(n)
    }
}

fn attempt(db: &Path, wal_file: &Path) -> Result<Connection, String> {
    let before = read_stamp(db, wal_file)?;
    if before.db_len > MAX_SEED_DB_BYTES || before.wal_len > MAX_SEED_WAL_BYTES {
        return Err(format!("{}: over the size cap ({} B, WAL {} B)", db.display(), before.db_len, before.wal_len));
    }
    // A rollback journal with content: a writer without a WAL is in the middle of a commit.
    if fs::symlink_metadata(format!("{}-journal", db.display())).is_ok_and(|m| m.len() > 0) {
        return Err(format!("{}: a rollback journal is beside it", db.display()));
    }
    let wal = match before.wal_len {
        0 => None,
        len => {
            let mut bytes = Vec::with_capacity(len as usize);
            File::open(wal_file).and_then(|f| f.take(len).read_to_end(&mut bytes)).map_err(|e| format!("read {}: {e}", wal_file.display()))?;
            committed(bytes)
        }
    };
    let mut file = File::open(db).map_err(|e| format!("open {}: {e}", db.display()))?;
    let page_size = match &wal {
        Some(wal) => wal.page_size,
        None => {
            let mut header = [0u8; 100];
            file.read_exact(&mut header).map_err(|e| format!("read header {}: {e}", db.display()))?;
            if &header[..16] != b"SQLite format 3\0" {
                return Err(format!("{}: not a database", db.display()));
            }
            // 1 stands for 65536.
            match u16::from_be_bytes([header[16], header[17]]) {
                1 => 65536,
                size => usize::from(size),
            }
        }
    };
    if !page_size_ok(page_size) {
        return Err(format!("{}: page size {page_size}", db.display()));
    }
    let page_size = page_size as u64;
    let pages = wal.as_ref().map_or(before.db_len / page_size, |w| u64::from(w.pages));
    let len = pages * page_size;
    if len == 0 || len > MAX_SEED_DB_BYTES {
        return Err(format!("{}: {pages} pages of {page_size} B", db.display()));
    }
    let mut in_wal: Vec<u32> = wal.as_ref().map(|w| w.at.keys().copied().collect()).unwrap_or_default();
    in_wal.sort_unstable();
    let image = Image { db: file, db_len: before.db_len, wal, in_wal, page_size, len, pos: 0 };

    let mut conn = Connection::open_in_memory_with_flags(OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| format!("open in memory: {e}"))?;
    conn.deserialize_read_exact("main", image, len as usize, true).map_err(|e| format!("read {}: {e}", db.display()))?;
    if read_stamp(db, wal_file)? != before {
        return Err(format!("{}: written while it was read", db.display()));
    }
    match conn.query_row("PRAGMA integrity_check(1)", [], |r| r.get::<_, String>(0)) {
        Ok(s) if s.eq_ignore_ascii_case("ok") => Ok(conn),
        Ok(other) => Err(format!("integrity_check failed: {other}")),
        Err(e) => Err(format!("integrity_check error: {e}")),
    }
}

fn wal_path(db: &Path) -> String {
    format!("{}-wal", db.display())
}

/// `official_dir/drafts.sqlite` with its WAL applied, read-only, in memory. An error when it
/// can't be read this way (written three times while it was read, over a cap, damaged).
pub(super) fn open(official_dir: &Path) -> Result<Connection, String> {
    let db = official_dir.join("drafts.sqlite");
    let wal = wal_path(&db);
    let mut last = String::new();
    for attempt_no in 0..ATTEMPTS {
        if attempt_no > 0 {
            std::thread::sleep(Duration::from_millis(10u64 << attempt_no));
        }
        match attempt(&db, wal.as_ref()) {
            Ok(conn) => return Ok(conn),
            Err(why) => last = why,
        }
    }
    Err(last)
}
