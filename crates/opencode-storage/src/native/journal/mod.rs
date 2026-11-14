//! The draft journal: every draft change is copied into a memory-mapped region of the database
//! file before it goes to the writer, so a kill loses at most the key in flight even when the
//! writer is stuck behind a busy disk. It's the `journal` row's overflow pages, reserved once
//! (a zeroed 1 MiB blob, written without a sync) and checkpointed, so they're in the main file
//! and never in the WAL; SQLite never writes them again. Recorded on the UI thread with a
//! memory copy (no system call, lock or allocation for a draft it has seen); read back before the
//! first frame, where a record newer than the database's (`draft_rev`) is the draft shown.
//!
//! Lifetime: [`attach`] maps it on the open's thread after the instance claim (before any key);
//! the writer marks what it committed [`settle`]d; [`finish`] at close marks it clean when
//! everything is, so the next launch reads only its header. A power cut can lose what the OS
//! hadn't written of the mapped pages; the database's synced checkpoints bound that as before.

mod layout;

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use layout::{Found, Header, Pos, Region, Seg};
use rusqlite::{Connection, OptionalExtension, params};

/// The reserved blob's size.
const BYTES: i64 = 1 << 20;

/// Where the journal is: what the boot record carries (`Kind::Journal`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub instance: u64,
    pub page_size: u32,
    /// Every overflow page, in order, and how many bytes of the last one are the blob's.
    pub pages: Vec<u32>,
    pub last: u32,
    /// The `journal` table's root page: [`attach`] checks the row is still there and its chain is
    /// still these pages (a row deleted by an outside tool leaves its pages on the freelist).
    pub root: u32,
}

impl Location {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(20 + self.pages.len() * 4);
        v.extend_from_slice(&self.instance.to_le_bytes());
        v.extend_from_slice(&self.page_size.to_le_bytes());
        v.extend_from_slice(&self.last.to_le_bytes());
        v.extend_from_slice(&(self.pages.len() as u32).to_le_bytes());
        for p in &self.pages {
            v.extend_from_slice(&p.to_le_bytes());
        }
        v.extend_from_slice(&self.root.to_le_bytes());
        v
    }

    pub(crate) fn decode(b: &[u8]) -> Option<Location> {
        let w = |i: usize| b.get(i..i + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes);
        let n = w(16)? as usize;
        let pages = (0..n).map(|i| w(20 + i * 4)).collect::<Option<Vec<_>>>()?;
        // A place recorded before the root was (`root` 0) fails `attach`'s check: reserved again.
        let root = w(20 + n * 4).unwrap_or(0);
        Some(Location { instance: u64::from_le_bytes(b.get(..8)?.try_into().ok()?), page_size: w(8)?, last: w(12)?, pages, root })
    }
}

/// A draft the journal holds: its newest text.
#[derive(Clone, Debug)]
pub struct Recovered {
    pub key: String,
    pub seq: u64,
    pub text: String,
}

struct Entry {
    seq: u64,
    /// Where its newest text is; none when its newest change has no record (cleared, too large).
    at: Option<(usize, Pos, usize)>,
}

struct Inner {
    _map: memmap2::MmapMut,
    region: Region,
    header: Header,
    gen_counter: u64,
    head: Pos,
    index: HashMap<String, Entry>,
}

pub(crate) struct Journal {
    /// The UI thread's one writer at a time: a write that finds it taken is skipped, never waited.
    busy: AtomicBool,
    open: AtomicBool,
    seq: AtomicU64,
    inner: UnsafeCell<Inner>,
    /// What the writer has committed: key → the newest seq in the database.
    settled: Mutex<HashMap<String, u64>>,
}

// SAFETY: `inner` is only touched by whoever holds `busy` (or the open's thread before the
// journal is published).
#[allow(unsafe_code)]
unsafe impl Sync for Journal {}
// SAFETY: the mapping's pointer stays valid wherever the journal is; see `Sync`.
#[allow(unsafe_code)]
unsafe impl Send for Journal {}

static JOURNAL: OnceLock<Journal> = OnceLock::new();

/// Whether `key` is a window's draft document (`opencode.window.<id>.dat:draft:<tab>`).
pub fn is_draft_key(key: &str) -> bool {
    key.starts_with("opencode.window.") && key.contains(".dat:draft:")
}

/// The window namespace a draft key belongs to.
pub(crate) fn namespace_of(key: &str) -> Option<&str> {
    key.split_once(":draft:").map(|(ns, _)| ns)
}

/// Records draft `key`'s new text (none: cleared) and returns its revision, for the writer to
/// store beside it; 0 without a journal (a run in memory, a profile that couldn't reserve one).
pub fn record(key: &str, text: Option<&str>) -> u64 {
    if !is_draft_key(key) {
        return 0;
    }
    if JOURNAL.get().is_none() && !(HOLDING.load(Ordering::Acquire) && hold(key, text)) {
        return 0;
    }
    let Some(j) = JOURNAL.get().filter(|j| j.open.load(Ordering::Acquire)) else { return 0 };
    let seq = j.seq.fetch_add(1, Ordering::AcqRel) + 1;
    if j.busy.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return seq;
    }
    // SAFETY: `busy` is ours.
    #[allow(unsafe_code)]
    let inner = unsafe { &mut *j.inner.get() };
    inner.write(j, key, seq, text);
    j.busy.store(false, Ordering::Release);
    seq
}

/// The revision of draft `key`'s newest change (what a send that takes its words settles).
pub fn latest(key: &str) -> u64 {
    let Some(j) = JOURNAL.get() else { return 0 };
    if j.busy.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return 0;
    }
    #[allow(unsafe_code)]
    let seq = unsafe { &*j.inner.get() }.index.get(key).map_or(0, |e| e.seq);
    j.busy.store(false, Ordering::Release);
    seq
}

/// The database now holds these drafts at these revisions (the writer, after a commit).
pub(crate) fn settle<'a>(done: impl IntoIterator<Item = (&'a str, u64)>) {
    let Some(j) = JOURNAL.get() else { return };
    let mut s = j.settled.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for (key, rev) in done {
        let e = s.entry(key.to_owned()).or_insert(0);
        *e = (*e).max(rev);
    }
}

/// Window `id`'s drafts went with it: nothing of theirs is kept for a next launch.
pub(crate) fn forget_window(namespace: &str) {
    let Some(j) = JOURNAL.get() else { return };
    let prefix = format!("{namespace}:draft:");
    let keys: Vec<(String, u64)> = {
        if j.busy.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return;
        }
        #[allow(unsafe_code)]
        let inner = unsafe { &*j.inner.get() };
        let keys = inner.index.iter().filter(|(k, _)| k.starts_with(&prefix)).map(|(k, e)| (k.clone(), e.seq)).collect();
        j.busy.store(false, Ordering::Release);
        keys
    };
    settle(keys.iter().map(|(k, seq)| (k.as_str(), *seq)));
}

/// The storage closes: when the database holds every draft the journal does, the journal is
/// marked clean (the next launch reads its header and nothing else). No more records after this.
pub(crate) fn finish(saved: bool) {
    let Some(j) = JOURNAL.get() else { return };
    // Closed already: abandoned with a damaged file, which is left as it was found.
    if !j.open.swap(false, Ordering::AcqRel) {
        return;
    }
    while j.busy.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        std::hint::spin_loop();
    }
    #[allow(unsafe_code)]
    let inner = unsafe { &mut *j.inner.get() };
    let settled = j.settled.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let all = inner.index.iter().all(|(k, e)| e.at.is_none() || settled.get(k).is_some_and(|s| *s >= e.seq));
    drop(settled);
    if saved && all {
        inner.reset(j.seq.load(Ordering::Acquire));
    }
    j.busy.store(false, Ordering::Release);
}

impl Inner {
    fn write(&mut self, j: &Journal, key: &str, seq: u64, text: Option<&str>) {
        let at = text.and_then(|t| self.append(j, key, seq, t));
        match self.index.get_mut(key) {
            Some(e) => *e = Entry { seq, at },
            None => {
                self.index.insert(key.to_owned(), Entry { seq, at });
            }
        }
    }

    /// Appends a record in the active half, moving to the other half when it's full.
    fn append(&mut self, j: &Journal, key: &str, seq: u64, text: &str) -> Option<(usize, Pos, usize)> {
        let len = layout::record_len(key.len(), text.len());
        if self.header.clean {
            self.header.clean = false;
            self.header.hseq += 1;
            self.region.write_header(&self.header);
        }
        let active = self.header.active as usize;
        let (start, end) = match self.region.place(active, self.head, len) {
            Some(p) => p,
            None => {
                self.switch(j, key);
                let active = self.header.active as usize;
                self.region.place(active, self.head, len)?
            }
        };
        let active = self.header.active as usize;
        self.region.write_record(start, self.header.gens[active], seq, key, text);
        self.head = end;
        Some((active, start, len))
    }

    /// The other half becomes active under a new generation; the newest record of every draft
    /// the database may not hold yet is copied into it (`except`: the one being written now).
    fn switch(&mut self, j: &Journal, except: &str) {
        let to = 1 - self.header.active as usize;
        self.gen_counter += 1;
        self.header.gens[to] = self.gen_counter;
        self.header.active = to as u32;
        self.header.hseq += 1;
        self.header.seq_hint = j.seq.load(Ordering::Acquire);
        self.region.write_header(&self.header);
        self.head = self.region.start(to);
        let settled = j.settled.try_lock().ok();
        let mut keep: Vec<(&String, &mut Entry)> = self.index.iter_mut().filter(|(k, e)| e.at.is_some() && *k != except && settled.as_ref().is_none_or(|s| s.get(*k).is_none_or(|r| *r < e.seq))).collect();
        keep.sort_by_key(|(_, e)| std::cmp::Reverse(e.seq));
        for (_, e) in keep {
            let Some((from_half, from, len)) = e.at else { continue };
            if from_half == to {
                continue;
            }
            match self.region.place(to, self.head, len) {
                Some((start, end)) => {
                    self.region.copy_record(from, len, start, self.header.gens[to]);
                    self.head = end;
                    e.at = Some((to, start, len));
                }
                None => e.at = None,
            }
        }
    }

    /// Both halves get new generations (every record before is gone) and the header says clean.
    fn reset(&mut self, seq: u64) {
        self.gen_counter += 2;
        self.header.gens = [self.gen_counter - 1, self.gen_counter];
        self.header.active = 0;
        self.header.clean = true;
        self.header.hseq += 1;
        self.header.seq_hint = seq;
        self.region.write_header(&self.header);
        self.head = self.region.start(0);
        self.index.clear();
    }
}

/// Maps the journal at `loc` in `db` and reads what it holds; then publishes it for [`record`].
/// `fresh`: just reserved, so its header is written now. Returns each draft's newest text.
/// `now_us`: seqs start above it, so every launch's revisions are above the last one's.
pub(crate) fn attach(db: &Path, loc: &Location, fresh: bool, now_us: u64) -> Result<Vec<Recovered>, String> {
    if JOURNAL.get().is_some() {
        return Err("the journal is already open".into());
    }
    let file = std::fs::OpenOptions::new().read(true).write(true).open(db).map_err(|e| e.to_string())?;
    let ps = u64::from(loc.page_size);
    let (lo, hi) = (loc.pages.iter().min().copied().ok_or("no pages")?, loc.pages.iter().max().copied().ok_or("no pages")?);
    let from = (u64::from(lo) - 1) * ps;
    let len = (u64::from(hi) * ps - from) as usize;
    if file.metadata().map_err(|e| e.to_string())?.len() < u64::from(hi) * ps {
        return Err("the file is shorter than the journal".into());
    }
    // SAFETY: the pages are the journal row's overflow pages, which nothing else in this process
    // writes; other processes don't write them either (single instance).
    #[allow(unsafe_code)]
    let mut map = unsafe { memmap2::MmapOptions::new().offset(from).len(len).map_mut(&file) }.map_err(|e| e.to_string())?;
    let base = map.as_mut_ptr();
    rune::marks::mark("journal.mapped");
    // Still the row's pages: its table's root holds row 1 pointing at the first, and each page
    // points at the next (SQLite's overflow chain, the pages' first 4 bytes). A row an outside tool
    // deleted or rewrote fails this, so no record goes into a page SQLite may give to other data.
    if !still_the_row(&file, loc, &map, from) {
        return Err("the recorded place isn't the journal row's any more".into());
    }
    let n = loc.pages.len();
    let segs: Vec<Seg> = loc
        .pages
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let at = ((u64::from(*p) - 1) * ps - from) as usize + 4;
            let full = loc.page_size as usize - 4;
            Seg { at, len: if i + 1 == n { (loc.last as usize).min(full) / 4 * 4 } else { full / 4 * 4 } }
        })
        .collect();
    if segs.len() < 3 {
        return Err("too few pages".into());
    }
    let region = Region { base, segs };
    let (header, found) = match region.read_header() {
        Some(h) if !fresh && h.instance == loc.instance => {
            let mut found = Vec::new();
            if !h.clean {
                for half in 0..2 {
                    region.scan(half, h.gens[half], &mut found);
                }
            }
            (h, found)
        }
        _ if fresh => {
            let h = Header { hseq: 1, instance: loc.instance, gens: [1, 2], active: 0, clean: true, seq_hint: 0 };
            region.write_header(&h);
            (h, Vec::new())
        }
        _ => return Err("the journal at the recorded place isn't this database's".into()),
    };
    let top = found.iter().map(|f| f.seq).max().unwrap_or(0).max(header.seq_hint).max(now_us);
    let mut newest: HashMap<String, Found> = HashMap::new();
    for f in found {
        if newest.get(&f.key).is_none_or(|n| n.seq < f.seq) {
            newest.insert(f.key.clone(), f);
        }
    }
    let gen_counter = header.gens[0].max(header.gens[1]);
    let mut inner = Inner { _map: map, head: region.start(header.active as usize), region, header, gen_counter, index: HashMap::new() };
    let recovered: Vec<Recovered> = newest.values().filter_map(|f| Some(Recovered { key: f.key.clone(), seq: f.seq, text: f.text.clone()? })).collect();
    for f in newest.into_values() {
        inner.index.insert(f.key, Entry { seq: f.seq, at: Some((f.half, f.at, f.len)) });
    }
    let j = Journal { busy: AtomicBool::new(false), open: AtomicBool::new(true), seq: AtomicU64::new(top), inner: UnsafeCell::new(inner), settled: Mutex::new(HashMap::new()) };
    // A journal that held records starts this run on a fresh half: they're copied forward.
    if !recovered.is_empty() {
        #[allow(unsafe_code)]
        unsafe { &mut *j.inner.get() }.switch(&j, "");
    }
    // What the windows typed while this was decided goes in first, then it's published (under the
    // lock `hold` takes, so no key falls between the two and every later key has a later seq).
    let mut e = early();
    if let Early::Holding(held) = std::mem::replace(&mut *e, Early::Decided) {
        #[allow(unsafe_code)]
        let inner = unsafe { &mut *j.inner.get() };
        for (key, text) in held {
            let seq = j.seq.fetch_add(1, Ordering::AcqRel) + 1;
            inner.write(&j, &key, seq, text.as_deref());
        }
    }
    HOLDING.store(false, Ordering::Release);
    let published = JOURNAL.set(j).map_err(|_| "the journal is already open".to_owned());
    drop(e);
    published?;
    Ok(recovered)
}

/// The file the journal is in was damaged and is moved aside: nothing more is written to it
/// (it's kept as it was found), and this run has no journal.
pub(crate) fn abandon() {
    if let Some(j) = JOURNAL.get() {
        j.open.store(false, Ordering::Release);
    }
    decided();
}

/// Whether a journal is mapped for this run.
pub(crate) fn attached() -> bool {
    JOURNAL.get().is_some()
}

/// The keys that come before the journal is decided (a new profile, the first launch after the
/// update: it's reserved beside the first frame, and the first input never waits for it). They
/// go to the database as always; their newest text per draft is held here and copied into the
/// journal the moment it's mapped. Until then they're in memory only (the guarantees table's one
/// exception).
enum Early {
    /// No open was started (a run in memory): nothing is held.
    Idle,
    /// The open is deciding: each draft's newest change since it started.
    Holding(HashMap<String, Option<String>>),
    /// Mapped (what was held is in it) or this run has none.
    Decided,
}

static EARLY: Mutex<Early> = Mutex::new(Early::Idle);
/// `EARLY` is `Holding`: read without the lock on every key of a run without a journal.
static HOLDING: AtomicBool = AtomicBool::new(false);

fn early() -> std::sync::MutexGuard<'static, Early> {
    EARLY.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An open started: keys until it decides are held for the journal.
pub(crate) fn pending() {
    *early() = Early::Holding(HashMap::new());
    HOLDING.store(true, Ordering::Release);
}

/// This run has no journal (or has it already): nothing more is held.
pub(crate) fn decided() {
    let mut e = early();
    *e = Early::Decided;
    HOLDING.store(false, Ordering::Release);
}

/// No journal yet: holds the key while the open is deciding. True when the journal was
/// published meanwhile (the publish takes the same lock), so the caller records it there.
fn hold(key: &str, text: Option<&str>) -> bool {
    let mut e = early();
    if JOURNAL.get().is_some() {
        return true;
    }
    if let Early::Holding(held) = &mut *e {
        held.insert(key.to_owned(), text.map(str::to_owned));
    }
    false
}

/// Makes sure the database has its journal (the first launch of a new profile, the first after
/// the update that brought it): the tables, the zeroed blob, then a checkpoint so its pages are
/// in the main file. No sync anywhere: a kill here leaves a profile that reserves again. Returns
/// where it is, and whether it was just reserved.
pub(crate) fn provision(conn: &Connection, db: &Path, clock: fn() -> i64) -> rusqlite::Result<(Location, bool)> {
    if let Some(loc) = location(conn)?.filter(|l| row_ok(conn, l)) {
        return Ok((loc, false));
    }
    // Unsynced throughout, the tables included: one transaction, no sync on the way to the
    // first frame (the next synced checkpoint makes it durable, as with any kill-safe commit).
    conn.execute_batch("PRAGMA synchronous = OFF")?;
    let reserved = (|| {
        conn.execute_batch(
            "BEGIN IMMEDIATE; \
             CREATE TABLE IF NOT EXISTS boot (id INTEGER PRIMARY KEY, data BLOB NOT NULL); \
             CREATE TABLE IF NOT EXISTS journal (id INTEGER PRIMARY KEY, data BLOB NOT NULL); \
             CREATE TABLE IF NOT EXISTS journal_meta (id INTEGER PRIMARY KEY, location BLOB NOT NULL); \
             CREATE TABLE IF NOT EXISTS draft_rev (key TEXT PRIMARY KEY, rev INTEGER NOT NULL) WITHOUT ROWID; \
             DELETE FROM journal;",
        )?;
        rune::marks::mark("journal.tables");
        conn.execute("INSERT INTO journal (id, data) VALUES (1, zeroblob(?1))", [BYTES])?;
        conn.execute_batch("COMMIT")?;
        rune::marks::mark("journal.inserted");
        // In WAL mode the pages are in the WAL now: into the main file with them. (A new file
        // has no WAL yet: they went there directly.)
        if conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))? == "wal" {
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0)).and_then(|busy| if busy == 0 { Ok(()) } else { Err(rusqlite::Error::InvalidQuery) })?;
            rune::marks::mark("journal.checkpointed");
        }
        let mut loc = overflow(conn, db).ok_or(rusqlite::Error::InvalidQuery)?;
        rune::marks::mark("journal.located");
        loc.instance = instance(clock);
        conn.execute("INSERT INTO journal_meta (id, location) VALUES (1, ?1) ON CONFLICT (id) DO UPDATE SET location = excluded.location", [loc.encode()])?;
        // The boot record as it stood when the journal was reserved (row 2): until this build
        // rebuilds it (naming the journal), an unchanged record means nobody else did either.
        conn.execute("INSERT INTO journal_meta (id, location) VALUES (2, ?1) ON CONFLICT (id) DO UPDATE SET location = excluded.location", [boot_sum(conn)?.to_le_bytes()])?;
        Ok(loc)
    })();
    if reserved.is_err() {
        conn.execute_batch("ROLLBACK").ok();
    }
    conn.execute_batch("PRAGMA synchronous = NORMAL")?;
    reserved.map(|l| (l, true))
}

/// The boot record's checksum (0 without one).
fn boot_sum(conn: &Connection) -> rusqlite::Result<u64> {
    let data: Option<Vec<u8>> = conn.query_row("SELECT data FROM boot WHERE id = 1", [], |r| r.get(0)).optional()?;
    Ok(data.map_or(0, |d| twox_hash::XxHash3_64::oneshot(&d)))
}

/// Whether the journal's records can be trusted against the database: the boot record names it
/// (this build rebuilt it since the reservation), or it's the record the reservation saw
/// (nothing rebuilt it since). Otherwise a build without the journal rebuilt it (a rollback): it
/// may have saved newer text than the journal holds.
pub(crate) fn current(conn: &Connection, record_names_it: bool) -> bool {
    if record_names_it {
        return true;
    }
    let seen: Option<Vec<u8>> = conn.query_row("SELECT location FROM journal_meta WHERE id = 2", [], |r| r.get(0)).optional().ok().flatten();
    seen.and_then(|b| b.try_into().ok()).map(u64::from_le_bytes).is_some_and(|s| boot_sum(conn).is_ok_and(|now| now == s))
}

/// The journal's place as last recorded, when the table has one.
pub(crate) fn location(conn: &Connection) -> rusqlite::Result<Option<Location>> {
    let exists: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = 'journal_meta')", [], |r| r.get(0))?;
    if !exists {
        return Ok(None);
    }
    let row: Option<Vec<u8>> = conn.query_row("SELECT location FROM journal_meta WHERE id = 1", [], |r| r.get(0)).optional()?;
    Ok(row.as_deref().and_then(Location::decode))
}

/// A random id for this journal: the boot record names it, so a recorded place that no longer
/// holds it (an outside tool rewrote the file) isn't trusted.
fn instance(clock: fn() -> i64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_i64(clock());
    h.finish() | 1
}

/// Whether the file still has the journal row at `loc`: the file's page size, the table's root
/// page holding row 1 whose overflow chain starts at the first page, and each mapped page pointing
/// at the next. Read from the file, which is current when the WAL is empty (the raw path); with
/// frames in it, [`row_ok`] asks SQLite too.
fn still_the_row(file: &std::fs::File, loc: &Location, map: &memmap2::MmapMut, from: u64) -> bool {
    let ps = loc.page_size as usize;
    let mut head = [0u8; 100];
    let Some(100) = super::boot_record::read_some(file, &mut head, 0) else { return false };
    let file_ps = match u16::from_be_bytes([head[16], head[17]]) {
        1 => 65536,
        n => usize::from(n),
    };
    if file_ps != ps || loc.root == 0 {
        return false;
    }
    let mut page = vec![0u8; ps];
    if super::boot_record::read_some(file, &mut page, u64::from(loc.root - 1) * ps as u64) != Some(ps) {
        return false;
    }
    if first_overflow(&page, ps - usize::from(head[20])).is_none_or(|(first, payload)| first != loc.pages[0] || payload < BYTES as usize) {
        return false;
    }
    loc.pages.iter().enumerate().all(|(i, p)| {
        let at = ((u64::from(*p) - 1) * ps as u64 - from) as usize;
        let next = map.get(at..at + 4).map_or(u32::MAX, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        next == loc.pages.get(i + 1).copied().unwrap_or(0)
    })
}

/// SQLite's own view: the `journal` table's root is still `loc.root` and its one row is the
/// reserved blob (a row deleted or rewritten since, by anything, isn't).
pub(crate) fn row_ok(conn: &Connection, loc: &Location) -> bool {
    let root = conn.query_row("SELECT rootpage FROM sqlite_schema WHERE type = 'table' AND name = 'journal'", [], |r| r.get::<_, i64>(0)).optional().ok().flatten();
    let len = conn.query_row("SELECT length(data) FROM journal WHERE id = 1 AND (SELECT count(*) FROM journal) = 1", [], |r| r.get::<_, i64>(0)).optional().ok().flatten();
    root == Some(i64::from(loc.root)) && len == Some(BYTES)
}

/// A leaf table page with one cell, row 1: its first overflow page and payload size.
fn first_overflow(page: &[u8], usable: usize) -> Option<(u32, usize)> {
    if page[0] != 0x0D || u16::from_be_bytes([page[3], page[4]]) != 1 {
        return None;
    }
    let cell = usize::from(u16::from_be_bytes([page[8], page[9]]));
    let (payload, a) = super::boot_record::varint(page.get(cell..)?)?;
    let (rowid, b) = super::boot_record::varint(page.get(cell + a..)?)?;
    if rowid != 1 {
        return None;
    }
    let payload = usize::try_from(payload).ok()?;
    let ptr = cell + a + b + super::boot_record::local_size(payload, usable);
    Some((u32::from_be_bytes(page.get(ptr..ptr + 4)?.try_into().ok()?), payload))
}

/// The journal row's overflow pages, read from the file (they're in it after the checkpoint).
fn overflow(conn: &Connection, db: &Path) -> Option<Location> {
    let root: i64 = conn.query_row("SELECT rootpage FROM sqlite_schema WHERE name = 'journal'", [], |r| r.get(0)).ok()?;
    let file = std::fs::File::open(db).ok()?;
    let mut head = [0u8; 100];
    super::boot_record::read_some(&file, &mut head, 0)?;
    let ps = match u16::from_be_bytes([head[16], head[17]]) {
        1 => 65536,
        n => usize::from(n),
    };
    let usable = ps - usize::from(head[20]);
    let mut page = vec![0u8; ps];
    let at = (u64::try_from(root).ok()? - 1) * ps as u64;
    if super::boot_record::read_some(&file, &mut page, at)? != ps || page[0] != 0x0D || u16::from_be_bytes([page[3], page[4]]) != 1 {
        return None;
    }
    let cell = usize::from(u16::from_be_bytes([page[8], page[9]]));
    let (payload, a) = super::boot_record::varint(page.get(cell..)?)?;
    let (rowid, b) = super::boot_record::varint(page.get(cell + a..)?)?;
    if rowid != 1 {
        return None;
    }
    let payload = usize::try_from(payload).ok()?;
    let local = super::boot_record::local_size(payload, usable);
    let ptr = cell + a + b + local;
    let mut next = u32::from_be_bytes(page.get(ptr..ptr + 4)?.try_into().ok()?);
    let mut pages = Vec::new();
    let mut left = payload - local;
    let mut word = [0u8; 4];
    while next != 0 && left > 0 {
        pages.push(next);
        left = left.saturating_sub(usable - 4);
        super::boot_record::read_some(&file, &mut word, u64::from(next - 1) * ps as u64)?;
        next = u32::from_be_bytes(word);
    }
    let last = (payload - local - (pages.len().saturating_sub(1)) * (usable - 4)) as u32;
    Some(Location { instance: 0, page_size: ps as u32, pages, last, root: u32::try_from(root).ok()? })
}

/// Stage B: each draft the journal holds that's newer than the database's copy (and whose window
/// is still saved) is written to the database at its revision; the rest are settled. Returns the
/// keys written.
pub(crate) fn reconcile(conn: &Connection, recovered: &[Recovered], now_ms: i64) -> Vec<String> {
    if recovered.is_empty() {
        return Vec::new();
    }
    let decide = |r: &Recovered| -> rusqlite::Result<bool> {
        let rev: u64 = conn.query_row("SELECT rev FROM draft_rev WHERE key = ?1", [&r.key], |x| x.get::<_, i64>(0)).optional()?.map_or(0, |v| v as u64);
        let ns = namespace_of(&r.key).unwrap_or_default();
        let window: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM state WHERE name = ?1)", [ns], |x| x.get(0))?;
        Ok(window && r.seq > rev)
    };
    let mut written = Vec::new();
    let mut settled = Vec::new();
    let result = (|| -> rusqlite::Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        for r in recovered {
            if decide(r)? {
                let (stored, chunks) = super::sql::encode_text(&r.text);
                for (id, data) in &chunks {
                    if !super::sql::insert_blob(&tx, id, data, now_ms)? {
                        super::sql::touch_blob(&tx, id, now_ms)?;
                    }
                }
                super::sql::put_document(&tx, &r.key, Some(&stored))?;
                put_rev(&tx, &r.key, r.seq)?;
                written.push(r.key.clone());
            }
            settled.push((r.key.as_str(), r.seq));
        }
        tx.commit()
    })();
    match result {
        Ok(()) => settle(settled),
        Err(e) => {
            eprintln!("opencode-storage: couldn't save the drafts the journal kept ({e}); they stay in it");
            written.clear();
        }
    }
    written
}

/// Stores draft `key`'s revision (never lowering it).
pub(crate) fn put_rev(c: &Connection, key: &str, rev: u64) -> rusqlite::Result<()> {
    c.prepare_cached("INSERT INTO draft_rev (key, rev) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET rev = max(rev, excluded.rev)")?.execute(params![key, rev as i64]).map(drop)
}
