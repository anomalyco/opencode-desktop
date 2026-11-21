//! The boot record: everything the first frame is built from, in one row the writer keeps
//! current, readable with one positioned file read before SQLite is opened at all.
//!
//! Measured on a Windows workstation (p50, warm / cold): a raw `CreateFileW` and one
//! 128 KiB `ReadFile` cost 288 / 978 µs; `sqlite3_initialize` + `sqlite3_open_v2` + one row cost
//! 2,581 / 2,645 µs (1,118 / 1,517 µs on a quieter run). So the record is read raw,
//! and SQLite, still the source of truth, opens afterwards for everything else.
//!
//! Where it is: the `boot` table is created first in a new file, so its root is page 2
//! (`FLAG_BOOT_PAGE2`). The record (one row, `id = 1`) starts there and continues on overflow
//! pages: one read of the file's first 128 KiB gets the header, page 2 and, with today's 16 KiB
//! pages, the overflow pages that lie in it; any other is one positioned read. (Files made
//! before 4 KiB pages have 64 KiB ones: the record whole on page 2, up to about 64 KiB.)
//!
//! When it's trusted: the WAL holds no frames (the writer checkpoints when writes go quiet and
//! at quit, so after a clean quit it's empty), the header's `application_id` equals the record's
//! generation (both written in the same transaction), and the record's xxh3 matches. Otherwise
//! the record comes from SQLite with the rest ([`FastBoot::UseSqlite`]). The one exception: when
//! another process holds the file so SQLite can't open it, the first frame is painted from the
//! last checkpoint's record ([`FastBoot::Checkpointed`]) rather than wait, and nothing is saved.
//!
//! The page and record layouts follow sqlite.org/fileformat2.html.


use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use super::schema::{FLAG_BITS, FLAG_BOOT_PAGE2, SCHEMA_VERSION};
pub use crate::boot_packet::{BootPacket, BootSource, Kind, session_of};
pub(crate) use crate::boot_packet::from_cache;
use crate::WindowFrame;
use crate::boot_packet::{DOC_CAP, HEAD, LAZY_KEYS, PREVIEW_CAP, Writer, encode_frame};

pub fn rebuild(tx: &Connection) -> rusqlite::Result<usize> {
    let mut w = Writer { buf: vec![0u8; HEAD], count: 0 };
    let mut recent: Vec<(String, String)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            crate::boot_state_query(),
        )?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            let (name, key, value) = (r.get_ref(0)?.as_str()?, r.get_ref(1)?.as_str()?, r.get_ref(2)?.as_str()?);
            if name.starts_with("opencode.window.") && LAZY_KEYS.contains(&key) {
                continue;
            }
            if key == "tabs.recent"
                && let Some(k) = serde_json::from_str::<serde_json::Value>(value).ok().and_then(|v| v["key"].as_str().map(str::to_owned))
            {
                recent.push((name.to_owned(), k));
            }
            w.push(Kind::State, name, key, value.as_bytes());
        }
    }
    let revs = table_exists(tx, "draft_rev")?;
    for (ns, tab) in &recent {
        let key = format!("{ns}:draft:{tab}");
        if let Some(text) = super::sql::read_document(tx, &key)?.filter(|t| t.len() <= DOC_CAP) {
            w.push(Kind::Document, &key, "", text.as_bytes());
        }
        if revs {
            let rev: i64 = tx.prepare_cached("SELECT rev FROM draft_rev WHERE key = ?1")?.query_row([&key], |r| r.get(0)).optional()?.unwrap_or(0);
            w.push(Kind::DraftRev, &key, "", &rev.to_le_bytes());
        }
        if let Some((server, session)) = session_of(tab) {
            let row: Option<(i64, Vec<u8>)> = tx.prepare_cached("SELECT updated_at, data FROM session_preview WHERE server = ?1 AND session = ?2")?.query_row([server, session], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            if let Some((at, data)) = row.filter(|(_, d)| d.len() <= PREVIEW_CAP) {
                let mut v = at.to_le_bytes().to_vec();
                v.extend_from_slice(&data);
                w.push(Kind::Preview, server, session, &v);
            }
        }
    }
    let frames: Vec<(String, WindowFrame)> = tx
        .prepare_cached("SELECT id, x, y, width, height, maximized, full_screen, display_x, display_y, display_w, display_h FROM window_state")?
        .query_map([], |r| {
            let display = match (r.get::<_, Option<i32>>(7)?, r.get::<_, Option<i32>>(8)?, r.get::<_, Option<i32>>(9)?, r.get::<_, Option<i32>>(10)?) {
                (Some(x), Some(y), Some(w), Some(h)) => Some([x, y, w, h]),
                _ => None,
            };
            Ok((r.get(0)?, WindowFrame { x: r.get(1)?, y: r.get(2)?, width: r.get(3)?, height: r.get(4)?, maximized: r.get(5)?, full_screen: r.get(6)?, display }))
        })?
        .collect::<Result<_, _>>()?;
    for (id, f) in &frames {
        w.push(Kind::Window, id, "", &encode_frame(f));
    }
    let plugins: Vec<(String, bool)> = tx.prepare_cached("SELECT id, enabled FROM plugin")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    for (id, on) in &plugins {
        w.push(Kind::Plugin, id, "", &[u8::from(*on)]);
    }
    let trust: Vec<(String, String)> = tx.prepare_cached("SELECT id, sha256 FROM plugin_trust")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    for (id, sha) in &trust {
        w.push(Kind::Trust, id, "", sha.as_bytes());
    }
    if let Some(loc) = super::journal::location(tx)? {
        w.push(Kind::Journal, "", "", &loc.encode());
        // This run keeps that journal: its records are this run's to trust next launch.
        if super::journal::kept_instance() == Some(loc.instance) {
            w.push(Kind::JournalOwner, "", "", &loc.instance.to_le_bytes());
        }
    }
    let shown: Vec<&str> = recent.iter().filter_map(|(_, tab)| Some(session_of(tab)?.1)).collect();
    let covered = super::schema::user_version(tx)? & super::schema::FLAG_PLUGINS != 0;
    let rows = crate::plugins::native::slice_rows(tx, &shown)?;
    crate::plugins::boot_slice(&mut w, covered, &shown, rows.iter().map(|(n, k, v)| (n.as_str(), k.as_str(), v.as_str())));
    let generation = tx.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0)).map(|g| (g as u32).wrapping_add(1))?;
    let bytes = w.finish(generation);
    tx.prepare_cached("INSERT INTO boot (id, data) VALUES (1, ?1) ON CONFLICT (id) DO UPDATE SET data = excluded.data")?.execute([&bytes])?;
    // application_id is a signed 32-bit header field: the same bits.
    tx.execute_batch(&format!("PRAGMA application_id = {}", generation as i32))?;
    Ok(bytes.len())
}

fn table_exists(c: &Connection, name: &str) -> rusqlite::Result<bool> {
    c.prepare_cached("SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)")?.query_row([name], |r| r.get(0))
}

impl BootPacket {
    /// Where the draft journal is, as the record names it.
    pub(crate) fn journal_location(&self) -> Option<super::journal::Location> {
        self.of(Kind::Journal).next().and_then(|e| super::journal::Location::decode(self.bytes_of(e)))
    }

    /// The journal instance the run that wrote this record kept, when it kept one.
    pub(crate) fn journal_owner(&self) -> Option<u64> {
        self.of(Kind::JournalOwner).next().and_then(|e| Some(u64::from_le_bytes(self.bytes_of(e).try_into().ok()?)))
    }

    /// A shown draft's revision in the database; none when the record doesn't say.
    pub(crate) fn draft_rev(&self, key: &str) -> Option<u64> {
        self.of(Kind::DraftRev).find(|e| self.str_of(&e.a) == key).and_then(|e| Some(i64::from_le_bytes(self.bytes_of(e).try_into().ok()?) as u64))
    }

    /// The drafts the journal holds that are newer than what the record shows: the first frame
    /// shows them. Only drafts the record knows the revision of (shown ones); stage B decides
    /// the rest against the database.
    pub(crate) fn apply_journal(&mut self, recovered: &[super::journal::Recovered]) {
        for r in recovered {
            if self.draft_rev(&r.key).is_some_and(|rev| r.seq > rev) {
                self.overlay.push((r.key.clone(), r.text.clone()));
            }
        }
    }
}

/// The record from SQLite (stage A's fallback, and stage B).
pub fn from_sqlite(conn: &Connection) -> rusqlite::Result<Option<Vec<u8>>> {
    conn.prepare_cached("SELECT data FROM boot WHERE id = 1")?.query_row([], |r| r.get(0)).optional()
}

#[derive(Debug, PartialEq, Eq)]
pub enum FastBoot {
    Record(Vec<u8>),
    /// The WAL holds frames, so the record on file is the last checkpoint's: possibly a few
    /// writes behind. Used only when SQLite can't open the file (another process holds it).
    Checkpointed(Vec<u8>),
    /// The file is there but this process may not read it (another process opened it without
    /// sharing).
    Denied,
    /// The fast path can't answer (no file, a WAL with frames, another layout, a stale or torn
    /// record): the record comes from SQLite.
    UseSqlite(&'static str),
}

/// How much of the file's start the raw read takes in one read.
const FIRST_READ: usize = 128 * 1024;

/// Reads the boot record from `db` without SQLite: one `CreateFileW`, one read of the first two
/// pages (and one per overflow page past them), and a size check of the WAL.
pub fn read_raw(db: &Path) -> FastBoot {
    let wal_frames = wal_len(&PathBuf::from(format!("{}-wal", db.display()))) > 0;
    let file = match std::fs::File::open(db) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return FastBoot::UseSqlite("no database file"),
        Err(_) => return FastBoot::Denied,
    };
    // The first 128 KiB: the header, page 2 and, with small pages, most of a record's overflow.
    let mut first = vec![0u8; FIRST_READ];
    let Some(n) = read_some(&file, &mut first, 0) else { return FastBoot::UseSqlite("unreadable") };
    if n < 100 || &first[..16] != b"SQLite format 3\0" {
        return FastBoot::UseSqlite("not a database");
    }
    let page_size = match u16::from_be_bytes([first[16], first[17]]) {
        1 => 65536,
        n => usize::from(n),
    };
    let usable = page_size - usize::from(first[20]);
    let version = u32::from_be_bytes([first[60], first[61], first[62], first[63]]);
    let generation = u32::from_be_bytes([first[68], first[69], first[70], first[71]]);
    if !(super::schema::BASE_SCHEMA..=SCHEMA_VERSION).contains(&(version >> FLAG_BITS)) || version & FLAG_BOOT_PAGE2 == 0 {
        return FastBoot::UseSqlite("boot isn't on page 2");
    }
    if version & super::schema::FLAG_SEED_SETTLED == 0 {
        return FastBoot::UseSqlite("seed unsettled");
    }
    // Pages smaller than 64 KiB: page 2 is already in the first read.
    if n < 2 * page_size {
        return FastBoot::UseSqlite("short read");
    }
    let first = &first[..n];
    let Some(record) = leaf_blob(&file, first, page_size, usable) else { return FastBoot::UseSqlite("unexpected page layout") };
    let stamped = record.get(4..8).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    if stamped != Some(generation) {
        return FastBoot::UseSqlite("stale record");
    }
    if wal_frames { FastBoot::Checkpointed(record) } else { FastBoot::Record(record) }
}

/// The WAL's length without opening it: `GetFileAttributesExW` reads the directory entry, where
/// `std::fs::metadata` opens and closes a handle (an antivirus-scanned open on Windows).
#[allow(unsafe_code)]
pub(crate) fn wal_len(path: &Path) -> u64 {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesExW, GetFileExInfoStandard, WIN32_FILE_ATTRIBUTE_DATA};
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        // SAFETY: an all-zero WIN32_FILE_ATTRIBUTE_DATA is a valid value of the plain C struct.
        let mut data: WIN32_FILE_ATTRIBUTE_DATA = unsafe { std::mem::zeroed() };
        // SAFETY: `wide` is NUL-terminated and outlives the call; `data` is the struct the
        // GetFileExInfoStandard level writes.
        let ok = unsafe { GetFileAttributesExW(wide.as_ptr(), GetFileExInfoStandard, (&raw mut data).cast()) };
        if ok == 0 {
            return 0;
        }
        (u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow)
    }
    #[cfg(not(windows))]
    std::fs::metadata(path).map_or(0, |m| m.len())
}

pub(super) fn read_some(file: &std::fs::File, buf: &mut [u8], offset: u64) -> Option<usize> {
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;
    #[cfg(unix)]
    use std::os::unix::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        #[cfg(windows)]
        let n = file.seek_read(&mut buf[done..], offset + done as u64).ok()?;
        #[cfg(unix)]
        let n = file.read_at(&mut buf[done..], offset + done as u64).ok()?;
        if n == 0 {
            break;
        }
        done += n;
    }
    Some(done)
}

/// The `data` column of row 1 on table leaf page `page`, following its overflow chain.
/// The boot row's `data` from page 2, following its overflow pages: from `first` (the file's
/// start, already read) when they're in it, one positioned read each when not.
fn leaf_blob(file: &std::fs::File, first: &[u8], page_size: usize, usable: usize) -> Option<Vec<u8>> {
    let page = first.get(page_size..2 * page_size)?;
    if page[0] != 0x0D || u16::from_be_bytes([page[3], page[4]]) == 0 {
        return None;
    }
    let at = usize::from(u16::from_be_bytes([page[8], page[9]]));
    let (payload, n) = varint(page.get(at..)?)?;
    let (rowid, m) = varint(page.get(at + n..)?)?;
    if rowid != 1 {
        return None;
    }
    let payload = usize::try_from(payload).ok()?;
    let start = at + n + m;
    let local = local_size(payload, usable);
    let cell = page.get(start..start + local)?;
    // The usual case: the whole record on the page, its value copied out once.
    if local == payload {
        let (from, len) = data_column(cell)?;
        return Some(cell.get(from..from + len)?.to_vec());
    }
    let mut bytes = Vec::with_capacity(payload);
    bytes.extend_from_slice(cell);
    {
        let mut next = u32::from_be_bytes(page.get(start + local..start + local + 4)?.try_into().ok()?);
        let mut overflow = vec![0u8; page_size];
        while bytes.len() < payload {
            let at = usize::try_from(next).ok()?.checked_sub(1)? * page_size;
            let page: &[u8] = match first.get(at..at + page_size) {
                Some(p) => p,
                None if read_some(file, &mut overflow, at as u64)? == page_size => &overflow,
                None => return None,
            };
            next = u32::from_be_bytes(page[..4].try_into().ok()?);
            let take = (payload - bytes.len()).min(usable - 4);
            bytes.extend_from_slice(&page[4..4 + take]);
        }
    }
    let (from, len) = data_column(&bytes)?;
    bytes.truncate(from + len);
    bytes.drain(..from);
    Some(bytes)
}

/// Where the `data` column lies in a `boot` record: its header of serial types, then the
/// values; `id` (the rowid alias) is NULL.
fn data_column(record: &[u8]) -> Option<(usize, usize)> {
    let (header, h) = varint(record)?;
    let header = usize::try_from(header).ok()?;
    let (t0, k0) = varint(record.get(h..)?)?;
    let (t1, _) = varint(record.get(h + k0..)?)?;
    if t1 < 12 || t1 % 2 != 0 {
        return None;
    }
    Some((header + serial_len(t0)?, usize::try_from((t1 - 12) / 2).ok()?))
}

/// How much of a table leaf cell's payload stays on the page (fileformat2 §1.6).
pub(super) fn local_size(payload: usize, usable: usize) -> usize {
    let max = usable - 35;
    if payload <= max {
        return payload;
    }
    let min = (usable - 12) * 32 / 255 - 23;
    let k = min + (payload - min) % (usable - 4);
    if k <= max { k } else { min }
}

fn serial_len(t: u64) -> Option<usize> {
    Some(match t {
        0 | 8 | 9 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 | 7 => 8,
        n if n >= 12 => usize::try_from((n - 12 - n % 2) / 2).ok()?,
        _ => return None,
    })
}

/// A big-endian base-128 varint (up to 9 bytes): its value and length.
pub(super) fn varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut v: u64 = 0;
    for (i, byte) in b.iter().take(9).enumerate() {
        if i == 8 {
            return Some(((v << 8) | u64::from(*byte), 9));
        }
        v = (v << 7) | u64::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}
