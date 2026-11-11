//! The boot record: everything the first frame is built from, in one row the writer keeps
//! current, readable with one positioned file read before SQLite is opened at all.
//!
//! Measured on blade (`proof\storage-1\floor-2.txt`, p50, warm / cold): a raw `CreateFileW` and one
//! 128 KiB `ReadFile` cost 288 / 978 µs; `sqlite3_initialize` + `sqlite3_open_v2` + one row cost
//! 2,581 / 2,645 µs (1,118 / 1,517 µs on a quieter run, `floor-1.txt`). So the record is read raw,
//! and SQLite, still the source of truth, opens afterwards for everything else.
//!
//! Where it is: the `boot` table is created first in a new file, so its root is page 2
//! (`FLAG_BOOT_PAGE2`), and the file's pages are 64 KiB, so the record (one row, `id = 1`) sits
//! whole on page 2 up to about 64 KiB; one read of the first two pages gets the header and the
//! record. A larger record continues on overflow pages, read one positioned read each.
//!
//! When it's trusted: the WAL holds no frames (the writer checkpoints when writes go quiet and
//! at quit, so after a clean quit it's empty), the header's `application_id` equals the record's
//! generation (both written in the same transaction), and the record's xxh3 matches. Otherwise
//! the record comes from SQLite with the rest ([`FastBoot::UseSqlite`]), never from a stale page.
//!
//! The page and record layouts follow sqlite.org/fileformat2.html.

use std::ops::Range;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use super::schema::{FLAG_BITS, FLAG_BOOT_PAGE2, SCHEMA_VERSION};
use crate::WindowFrame;

const MAGIC: &[u8; 4] = b"RNB1";
const HEAD: usize = 24;
/// A visible draft or session preview larger than this stays out of the record; the app reads
/// it from the full store after stage B.
const DOC_CAP: usize = 64 * 1024;
const PREVIEW_CAP: usize = 256 * 1024;
/// Window keys the first frame doesn't draw from (closed tabs, migration leftovers): they load
/// with the rest.
const LAZY_KEYS: [&str; 2] = ["tabs.closed", "legacy"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    State = 1,
    Document = 2,
    Window = 3,
    Plugin = 4,
    Trust = 5,
    Preview = 6,
}

#[derive(Clone, Debug)]
struct Entry {
    kind: u8,
    a: Range<usize>,
    b: Range<usize>,
    value: Range<usize>,
}

/// What stage A hands the UI thread: the record's bytes and an index into them. Lookups borrow;
/// nothing is decoded until asked for.
#[derive(Clone, Debug, Default)]
pub struct BootPacket {
    bytes: Vec<u8>,
    entries: Vec<Entry>,
    /// Where the record came from: a raw read, SQLite, or nowhere (a new profile).
    pub source: BootSource,
    /// Nanoseconds from the open's start to the packet, and the parts.
    pub read_ns: u64,
    pub decode_ns: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BootSource {
    #[default]
    Empty,
    Raw,
    Sqlite,
}

impl BootPacket {
    /// Indexes `bytes` (a whole record, header included); none when it's malformed.
    pub fn parse(bytes: Vec<u8>) -> Option<BootPacket> {
        let head = bytes.get(..HEAD)?;
        if &head[..4] != MAGIC {
            return None;
        }
        let count = u32::from_le_bytes(head[8..12].try_into().ok()?) as usize;
        let len = u32::from_le_bytes(head[12..16].try_into().ok()?) as usize;
        let sum = u64::from_le_bytes(head[16..24].try_into().ok()?);
        let payload = bytes.get(HEAD..HEAD + len)?;
        if twox_hash::XxHash3_64::oneshot(payload) != sum {
            return None;
        }
        let mut entries = Vec::with_capacity(count);
        let mut at = HEAD;
        for _ in 0..count {
            let h = bytes.get(at..at + 12)?;
            let (a, b, v) = (usize::from(u16::from_le_bytes([h[2], h[3]])), usize::from(u16::from_le_bytes([h[4], h[5]])), u32::from_le_bytes(h[8..12].try_into().ok()?) as usize);
            let start = at + 12;
            entries.push(Entry { kind: h[0], a: start..start + a, b: start + a..start + a + b, value: start + a + b..start + a + b + v });
            at = start + a + b + v;
        }
        (at == HEAD + len).then_some(BootPacket { bytes, entries, ..BootPacket::default() })
    }

    pub fn generation(&self) -> u32 {
        self.bytes.get(4..8).and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn str_of(&self, r: &Range<usize>) -> &str {
        std::str::from_utf8(&self.bytes[r.clone()]).unwrap_or_default()
    }

    fn of(&self, kind: Kind) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| e.kind == kind as u8)
    }

    /// A `state` value (JSON text) of a boot namespace.
    pub fn state(&self, name: &str, key: &str) -> Option<&str> {
        self.of(Kind::State).find(|e| self.str_of(&e.a) == name && self.str_of(&e.b) == key).map(|e| self.str_of(&e.value))
    }

    /// Every `(key, value)` of namespace `name` in the record.
    pub fn namespace<'a>(&'a self, name: &'a str) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
        self.of(Kind::State).filter(move |e| self.str_of(&e.a) == name).map(|e| (self.str_of(&e.b), self.str_of(&e.value)))
    }

    /// The `opencode.window.<id>.dat` namespaces in the record.
    pub fn window_namespaces(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.of(Kind::State).map(|e| self.str_of(&e.a)).filter(|n| n.starts_with("opencode.window.")).collect();
        names.dedup();
        names
    }

    /// A visible draft's text (`draft_document(window, tab_key)`).
    pub fn document(&self, key: &str) -> Option<&str> {
        self.of(Kind::Document).find(|e| self.str_of(&e.a) == key).map(|e| self.str_of(&e.value))
    }

    pub fn frame(&self, window: &str) -> Option<WindowFrame> {
        self.of(Kind::Window).find(|e| self.str_of(&e.a) == window).and_then(|e| decode_frame(&self.bytes[e.value.clone()]))
    }

    pub fn frames(&self) -> impl Iterator<Item = (&str, WindowFrame)> {
        self.of(Kind::Window).filter_map(|e| Some((self.str_of(&e.a), decode_frame(&self.bytes[e.value.clone()])?)))
    }

    /// The plugin enable overrides.
    pub fn plugins(&self) -> impl Iterator<Item = (&str, bool)> {
        self.of(Kind::Plugin).map(|e| (self.str_of(&e.a), self.bytes.get(e.value.start) == Some(&1)))
    }

    pub fn trusted(&self) -> impl Iterator<Item = (&str, &str)> {
        self.of(Kind::Trust).map(|e| (self.str_of(&e.a), self.str_of(&e.value)))
    }

    /// The preview of the session a window shows: its `updated_at` and bytes, borrowed.
    pub fn preview(&self, server: &str, session: &str) -> Option<(i64, &[u8])> {
        let e = self.of(Kind::Preview).find(|e| self.str_of(&e.a) == server && self.str_of(&e.b) == session)?;
        let v = &self.bytes[e.value.clone()];
        Some((i64::from_le_bytes(v.get(..8)?.try_into().ok()?), &v[8..]))
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

fn encode_frame(f: &WindowFrame) -> [u8; 36] {
    let mut out = [0u8; 36];
    let d = f.display.unwrap_or([0; 4]);
    let flags = u32::from(f.x.is_some()) | u32::from(f.y.is_some()) << 1 | u32::from(f.maximized) << 2 | u32::from(f.full_screen) << 3 | u32::from(f.display.is_some()) << 4;
    let words: [u32; 9] = [f.x.unwrap_or(0) as u32, f.y.unwrap_or(0) as u32, f.width, f.height, d[0] as u32, d[1] as u32, d[2] as u32, d[3] as u32, flags];
    for (i, w) in words.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn decode_frame(v: &[u8]) -> Option<WindowFrame> {
    let w = |i: usize| v.get(i * 4..i * 4 + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let flags = w(8)?;
    let int = |i: usize| w(i).map(|x| x as i32);
    Some(WindowFrame {
        x: (flags & 1 != 0).then_some(int(0)?),
        y: (flags & 2 != 0).then_some(int(1)?),
        width: w(2)?,
        height: w(3)?,
        maximized: flags & 4 != 0,
        full_screen: flags & 8 != 0,
        display: (flags & 16 != 0).then_some([int(4)?, int(5)?, int(6)?, int(7)?]),
    })
}

struct Writer {
    buf: Vec<u8>,
    count: u32,
}

impl Writer {
    /// The record: header (magic, generation, count, length, xxh3) then the entries.
    fn finish(mut self, generation: u32) -> Vec<u8> {
        let payload_len = (self.buf.len() - HEAD) as u32;
        let sum = twox_hash::XxHash3_64::oneshot(&self.buf[HEAD..]);
        self.buf[..4].copy_from_slice(MAGIC);
        self.buf[4..8].copy_from_slice(&generation.to_le_bytes());
        self.buf[8..12].copy_from_slice(&self.count.to_le_bytes());
        self.buf[12..16].copy_from_slice(&payload_len.to_le_bytes());
        self.buf[16..24].copy_from_slice(&sum.to_le_bytes());
        self.buf
    }

    fn push(&mut self, kind: Kind, a: &str, b: &str, value: &[u8]) {
        let (Ok(al), Ok(bl), Ok(vl)) = (u16::try_from(a.len()), u16::try_from(b.len()), u32::try_from(value.len())) else { return };
        self.buf.push(kind as u8);
        self.buf.push(0);
        self.buf.extend_from_slice(&al.to_le_bytes());
        self.buf.extend_from_slice(&bl.to_le_bytes());
        self.buf.extend_from_slice(&[0, 0]);
        self.buf.extend_from_slice(&vl.to_le_bytes());
        self.buf.extend_from_slice(a.as_bytes());
        self.buf.extend_from_slice(b.as_bytes());
        self.buf.extend_from_slice(value);
        self.count += 1;
    }
}

/// Rebuilds the record from the database inside the caller's write transaction, stores it as
/// `boot` row 1 and stamps the header's `application_id` with its generation. Returns its size.
pub fn rebuild(tx: &Connection) -> rusqlite::Result<usize> {
    let mut w = Writer { buf: vec![0u8; HEAD], count: 0 };
    let mut recent: Vec<(String, String)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT name, key, value FROM state WHERE name IN ('opencode.settings', 'opencode.global.dat', 'plugin.file', 'plugin.browser', 'plugin.updater') \
             OR (name >= 'opencode.window.' AND name < 'opencode.window/')",
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
    for (ns, tab) in &recent {
        let key = format!("{ns}:draft:{tab}");
        if let Some(text) = super::sql::read_document(tx, &key)?.filter(|t| t.len() <= DOC_CAP) {
            w.push(Kind::Document, &key, "", text.as_bytes());
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
    let generation = tx.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0)).map(|g| (g as u32).wrapping_add(1))?;
    let bytes = w.finish(generation);
    tx.prepare_cached("INSERT INTO boot (id, data) VALUES (1, ?1) ON CONFLICT (id) DO UPDATE SET data = excluded.data")?.execute([&bytes])?;
    // application_id is a signed 32-bit header field: the same bits.
    tx.execute_batch(&format!("PRAGMA application_id = {}", generation as i32))?;
    Ok(bytes.len())
}

/// The record built from memory (the database couldn't open), so the first frame still draws
/// what the user had.
pub(crate) fn from_cache(cache: &crate::cache::Cache) -> BootPacket {
    let mut w = Writer { buf: vec![0u8; HEAD], count: 0 };
    for (name, ns) in &cache.state {
        let crate::cache::Namespace::Loaded { rows, .. } = ns else { continue };
        if !crate::is_boot_namespace(name) {
            continue;
        }
        for (key, value) in rows {
            if name.starts_with("opencode.window.") && LAZY_KEYS.contains(&key.as_str()) {
                continue;
            }
            w.push(Kind::State, name, key, value.as_bytes());
            if key == "tabs.recent"
                && let Some(tab) = serde_json::from_str::<serde_json::Value>(value).ok().and_then(|v| v["key"].as_str().map(str::to_owned))
                && let Some(crate::cache::Doc::Present(text)) = cache.docs.get(&format!("{name}:draft:{tab}"))
                && text.len() <= DOC_CAP
            {
                w.push(Kind::Document, &format!("{name}:draft:{tab}"), "", text.as_bytes());
            }
        }
    }
    for (id, f) in &cache.windows {
        w.push(Kind::Window, id, "", &encode_frame(f));
    }
    for (id, on) in &cache.plugins {
        w.push(Kind::Plugin, id, "", &[u8::from(*on)]);
    }
    for (id, sha) in &cache.trust {
        w.push(Kind::Trust, id, "", sha.as_bytes());
    }
    BootPacket::parse(w.finish(0)).unwrap_or_default()
}

/// `(server, session)` of an encoded session tab key (`<server>\n/server/<b64>/session/<id>`).
pub fn session_of(tab_key: &str) -> Option<(&str, &str)> {
    let (server, href) = tab_key.split_once('\n')?;
    Some((server, href.rsplit_once("/session/")?.1))
}

/// The record from SQLite (stage A's fallback, and stage B).
pub fn from_sqlite(conn: &Connection) -> rusqlite::Result<Option<Vec<u8>>> {
    conn.prepare_cached("SELECT data FROM boot WHERE id = 1")?.query_row([], |r| r.get(0)).optional()
}

#[derive(Debug, PartialEq, Eq)]
pub enum FastBoot {
    Record(Vec<u8>),
    /// The fast path can't answer (no file, a WAL with frames, another layout, a stale or torn
    /// record): the record comes from SQLite.
    UseSqlite(&'static str),
}

/// Reads the boot record from `db` without SQLite: one `CreateFileW`, one read of the first two
/// pages (and one per overflow page past them), and a size check of the WAL.
pub fn read_raw(db: &Path) -> FastBoot {
    if wal_len(&PathBuf::from(format!("{}-wal", db.display()))) > 0 {
        return FastBoot::UseSqlite("the WAL holds frames");
    }
    let Ok(file) = std::fs::File::open(db) else { return FastBoot::UseSqlite("no database file") };
    let mut first = vec![0u8; 2 * 65536];
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
    if version >> FLAG_BITS != SCHEMA_VERSION || version & FLAG_BOOT_PAGE2 == 0 {
        return FastBoot::UseSqlite("boot isn't on page 2");
    }
    // Pages smaller than 64 KiB: page 2 is already in the first read.
    if n < 2 * page_size {
        return FastBoot::UseSqlite("short read");
    }
    let Some(record) = leaf_blob(&file, &first[page_size..2 * page_size], page_size, usable) else { return FastBoot::UseSqlite("unexpected page layout") };
    let stamped = record.get(4..8).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    if stamped != Some(generation) {
        return FastBoot::UseSqlite("stale record");
    }
    FastBoot::Record(record)
}

/// The WAL's length without opening it: `GetFileAttributesExW` reads the directory entry, where
/// `std::fs::metadata` opens and closes a handle (an antivirus-scanned open on Windows).
#[allow(unsafe_code)]
fn wal_len(path: &Path) -> u64 {
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

fn read_some(file: &std::fs::File, buf: &mut [u8], offset: u64) -> Option<usize> {
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
fn leaf_blob(file: &std::fs::File, page: &[u8], page_size: usize, usable: usize) -> Option<Vec<u8>> {
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
            if next == 0 || read_some(file, &mut overflow, u64::from(next - 1) * page_size as u64)? != page_size {
                return None;
            }
            next = u32::from_be_bytes(overflow[..4].try_into().ok()?);
            let take = (payload - bytes.len()).min(usable - 4);
            bytes.extend_from_slice(&overflow[4..4 + take]);
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
fn local_size(payload: usize, usable: usize) -> usize {
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
fn varint(b: &[u8]) -> Option<(u64, usize)> {
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
