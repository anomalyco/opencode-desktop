//! The boot packet: everything the first frame is built from, as one record of entries the
//! UI thread borrows from, nothing decoded until asked. The native backend keeps it as a row the
//! writer rebuilds (`native::boot_record`) and reads it raw before SQLite opens; the web backend
//! builds it from the state it read at open. Same bytes, same lookups, on both.

use std::ops::Range;

use crate::WindowFrame;

/// The record's format. It moves on when a build changes what the record holds, so a record an
/// older build wrote isn't trusted: it reads as malformed and that launch reads SQLite, which
/// rebuilds it. `RNB2`: the plugin slice holds the shown sessions' values (`RNB1` never did, and
/// answered "nothing stored" for them).
pub(crate) const MAGIC: &[u8; 4] = b"RNB2";
pub(crate) const HEAD: usize = 24;
/// A visible draft or session preview larger than this stays out of the record; the app reads
/// it from the full store after stage B.
pub(crate) const DOC_CAP: usize = 64 * 1024;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const PREVIEW_CAP: usize = 256 * 1024;
/// Window keys the first frame doesn't draw from (closed tabs, migration leftovers): they load
/// with the rest.
pub(crate) const LAZY_KEYS: [&str; 3] = ["tabs.closed", "legacy", "side.rest"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    State = 1,
    Document = 2,
    Window = 3,
    Plugin = 4,
    Trust = 5,
    Preview = 6,
    /// A plugin value the first frame may read (`plugins::boot_slice`).
    PluginValue = 7,
    /// A plugin value too large for the record: only the database has it.
    Omitted = 8,
    /// The record answers for the plugins (their files were imported).
    Covered = 9,
    /// A session a window shows: its plugin values are in the record.
    Shown = 10,
    /// Where the draft journal is (native: `native::journal::Location`).
    #[cfg(not(target_arch = "wasm32"))]
    Journal = 12,
    /// A shown draft's revision in the database (`draft_rev`), whether or not it has a document.
    #[cfg(not(target_arch = "wasm32"))]
    DraftRev = 13,
    /// The journal instance the run that wrote this record kept (its 8 bytes): a record rebuilt by
    /// a run without the journal (an older build, a platform it was off on) has none.
    #[cfg(not(target_arch = "wasm32"))]
    JournalOwner = 14,
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    kind: u8,
    pub(crate) a: Range<usize>,
    pub(crate) b: Range<usize>,
    pub(crate) value: Range<usize>,
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
    /// Shown drafts the journal held newer than the record: the first frame shows these.
    pub(crate) overlay: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BootSource {
    #[default]
    Empty,
    Raw,
    Sqlite,
    /// Another process holds the file: the last checkpoint's record, read raw. Nothing this run
    /// changes is saved.
    Checkpoint,
    /// Another process holds the file and won't let it be read: a first-run screen. The saved
    /// state is untouched; nothing this run changes is saved.
    Denied,
}

impl BootSource {
    /// The run paints what isn't the saved state as it is, and saves nothing.
    pub fn held(self) -> bool {
        matches!(self, BootSource::Checkpoint | BootSource::Denied)
    }
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

    pub(crate) fn str_of(&self, r: &Range<usize>) -> &str {
        std::str::from_utf8(&self.bytes[r.clone()]).unwrap_or_default()
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn bytes_of(&self, e: &Entry) -> &[u8] {
        &self.bytes[e.value.clone()]
    }

    pub(crate) fn of(&self, kind: Kind) -> impl Iterator<Item = &Entry> {
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
        if let Some((_, text)) = self.overlay.iter().find(|(k, _)| k == key) {
            return Some(text);
        }
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

    /// Puts what the packet holds into `cache` where it has nothing (an open that failed after
    /// the first frame was built from the packet: memory keeps what the screen shows).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn fill_cache(&self, cache: &mut crate::cache::Cache) {
        for e in self.of(Kind::State) {
            let (name, key) = (self.str_of(&e.a), self.str_of(&e.b));
            if matches!(cache.get(name, key), crate::Lookup::Ready(None)) {
                cache.set(name, key.to_owned(), Some(self.str_of(&e.value).into()));
            }
        }
        for (key, text) in &self.overlay {
            cache.docs.entry(key.clone()).or_insert_with(|| crate::cache::Doc::Present(text.as_str().into()));
        }
        for e in self.of(Kind::Document) {
            cache.docs.entry(self.str_of(&e.a).to_owned()).or_insert_with(|| crate::cache::Doc::Present(self.str_of(&e.value).into()));
        }
        for (id, f) in self.frames() {
            cache.windows.entry(id.to_owned()).or_insert(f);
        }
        for (id, on) in self.plugins() {
            cache.plugins.entry(id.to_owned()).or_insert(on);
        }
        for (id, sha) in self.trusted() {
            cache.trust.entry(id.to_owned()).or_insert_with(|| sha.to_owned());
        }
        for (name, key, value) in self.plugin_values() {
            if matches!(cache.get(name, key), crate::Lookup::Ready(None)) {
                cache.set(name, key.to_owned(), Some(value.into()));
            }
        }
    }
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// A first-run screen for a file another process won't let this one read.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn held_empty() -> BootPacket {
        BootPacket { source: BootSource::Denied, ..BootPacket::default() }
    }

    /// The entries a first frame is built from: two records with the same ones show the same
    /// first frame. The journal's bookkeeping (where it is, which run kept it) is left out: a
    /// record rebuilt by a run without the journal (a held run, `owe.want_journal`) has none.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn shown_entries(&self) -> impl Iterator<Item = (u8, &[u8], &[u8], &[u8])> + '_ {
        self.entries
            .iter()
            .filter(|e| e.kind != Kind::Journal as u8 && e.kind != Kind::JournalOwner as u8)
            .map(|e| (e.kind, &self.bytes[e.a.clone()], &self.bytes[e.b.clone()], &self.bytes[e.value.clone()]))
    }
}

pub(crate) fn encode_frame(f: &WindowFrame) -> [u8; 36] {
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

pub(crate) struct Writer {
    pub(crate) buf: Vec<u8>,
    pub(crate) count: u32,
}

impl Writer {
    /// The record: header (magic, generation, count, length, xxh3) then the entries.
    pub(crate) fn finish(mut self, generation: u32) -> Vec<u8> {
        let payload_len = (self.buf.len() - HEAD) as u32;
        let sum = twox_hash::XxHash3_64::oneshot(&self.buf[HEAD..]);
        self.buf[..4].copy_from_slice(MAGIC);
        self.buf[4..8].copy_from_slice(&generation.to_le_bytes());
        self.buf[8..12].copy_from_slice(&self.count.to_le_bytes());
        self.buf[12..16].copy_from_slice(&payload_len.to_le_bytes());
        self.buf[16..24].copy_from_slice(&sum.to_le_bytes());
        self.buf
    }

    pub(crate) fn push(&mut self, kind: Kind, a: &str, b: &str, value: &[u8]) {
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

/// The record built from memory (the database couldn't open), so the first frame still draws
/// what the user had.
pub(crate) fn from_cache(cache: &crate::cache::Cache) -> BootPacket {
    let mut w = Writer { buf: vec![0u8; HEAD], count: 0 };
    let mut shown: Vec<String> = Vec::new();
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
            let tab = (key == "tabs.recent").then(|| serde_json::from_str::<serde_json::Value>(value).ok().and_then(|v| v["key"].as_str().map(str::to_owned))).flatten();
            if let Some(tab) = &tab
                && let Some((_, session)) = session_of(tab)
            {
                shown.push(session.to_owned());
            }
            if let Some(tab) = tab
                && let Some(crate::cache::Doc::Present(text)) = cache.docs.get(&format!("{name}:draft:{tab}"))
                && text.len() <= DOC_CAP
            {
                w.push(Kind::Document, &format!("{name}:draft:{tab}"), "", text.as_bytes());
            }
        }
    }
    // Memory holds every namespace it knows whole: the plugin values it has are all there are.
    let shown: Vec<&str> = shown.iter().map(String::as_str).collect();
    let rows = cache.state.iter().filter_map(|(name, ns)| match ns {
        crate::cache::Namespace::Loaded { rows, .. } => Some(rows.iter().map(move |(k, v)| (name.as_str(), k.as_str(), &**v))),
        crate::cache::Namespace::Unloaded { .. } => None,
    });
    let rows: Vec<(&str, &str, &str)> = rows.flatten().filter(|(n, k, _)| crate::plugins::wanted(n, k, &shown)).collect();
    crate::plugins::boot_slice(&mut w, cache.complete, &shown, rows.into_iter());
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