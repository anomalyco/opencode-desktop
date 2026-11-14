//! The draft journal's bytes: a header and two halves of records, laid over the data areas of
//! the `journal` row's overflow pages (each page's first 4 bytes are SQLite's pointer to the next
//! one, so the journal is a list of segments, not one run of bytes). SQLite never writes those
//! pages again unless the row is changed, which nothing does.
//!
//! - **Header:** two 64-byte slots at the start of segment 0, each with its own sequence and
//!   xxh3; the valid slot with the higher sequence is current, so a header torn by a kill or a
//!   power cut leaves the previous one standing.
//! - **Records:** a 40-byte head (magic, length, the half's generation, seq, key and text
//!   lengths, xxh3), the key, the text. The magic is stored last, so a record cut short by a kill
//!   has none; a record from an earlier use of the half has another generation. Either ends the
//!   scan. A head never spans two segments.

use std::sync::atomic::{AtomicU32, Ordering, fence};

const HEADER_MAGIC: u32 = u32::from_le_bytes(*b"RNJH");
const RECORD_MAGIC: u32 = u32::from_le_bytes(*b"RNJR");
pub(super) const REC_HEAD: usize = 40;
const SLOT: usize = 64;

/// Where a byte range of the journal lives in the mapping.
#[derive(Clone, Copy, Debug)]
pub(super) struct Seg {
    pub at: usize,
    pub len: usize,
}

/// A place in a half: its segment (index into `Region::segs`) and the offset in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pos {
    pub seg: usize,
    pub off: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Header {
    pub hseq: u64,
    pub instance: u64,
    pub gens: [u64; 2],
    pub active: u32,
    /// No record in either half has to be read (the last run ended with everything saved).
    pub clean: bool,
    pub seq_hint: u64,
}

/// A record as read back.
pub(super) struct Found {
    pub half: usize,
    pub at: Pos,
    pub len: usize,
    pub seq: u64,
    pub key: String,
    pub text: Option<String>,
}

pub(super) struct Region {
    pub base: *mut u8,
    /// Segment 0 holds the header; the rest split into the two halves.
    pub segs: Vec<Seg>,
}

#[allow(unsafe_code)]
impl Region {
    /// The halves' segment ranges.
    pub fn half(&self, h: usize) -> std::ops::Range<usize> {
        let m = 1 + (self.segs.len() - 1) / 2;
        if h == 0 { 1..m } else { m..self.segs.len() }
    }

    pub fn start(&self, h: usize) -> Pos {
        Pos { seg: self.half(h).start, off: 0 }
    }

    fn ptr(&self, p: Pos) -> *mut u8 {
        // SAFETY: callers keep `p` inside segment `p.seg`, which lies inside the mapping.
        unsafe { self.base.add(self.segs[p.seg].at + p.off) }
    }

    /// Where a record of `len` bytes starting at or after `p` goes in half `h`, and where it ends;
    /// none when it doesn't fit.
    pub fn place(&self, h: usize, mut p: Pos, len: usize) -> Option<(Pos, Pos)> {
        let half = self.half(h);
        if self.segs[p.seg].len - p.off < REC_HEAD {
            p = Pos { seg: p.seg + 1, off: 0 };
        }
        if !half.contains(&p.seg) {
            return None;
        }
        let (start, mut q, mut left) = (p, p, len);
        loop {
            let room = self.segs[q.seg].len - q.off;
            if left <= room {
                q.off += left;
                return Some((start, q));
            }
            left -= room;
            q = Pos { seg: q.seg + 1, off: 0 };
            if !half.contains(&q.seg) {
                return None;
            }
        }
    }

    /// Copies `bytes` to `p` onward, across segments; returns the position after them.
    fn put(&self, mut p: Pos, mut bytes: &[u8]) -> Pos {
        while !bytes.is_empty() {
            let room = self.segs[p.seg].len - p.off;
            if room == 0 {
                p = Pos { seg: p.seg + 1, off: 0 };
                continue;
            }
            let n = room.min(bytes.len());
            // SAFETY: `n` bytes fit in segment `p.seg` at `p.off`; the source is a live slice.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr(p), n) };
            p.off += n;
            bytes = &bytes[n..];
        }
        p
    }

    fn get(&self, mut p: Pos, out: &mut [u8]) -> Option<Pos> {
        let mut done = 0;
        while done < out.len() {
            if p.seg >= self.segs.len() {
                return None;
            }
            let room = self.segs[p.seg].len - p.off;
            if room == 0 {
                p = Pos { seg: p.seg + 1, off: 0 };
                continue;
            }
            let n = room.min(out.len() - done);
            // SAFETY: as in `put`, reading.
            unsafe { std::ptr::copy_nonoverlapping(self.ptr(p), out[done..].as_mut_ptr(), n) };
            p.off += n;
            done += n;
        }
        Some(p)
    }

    /// Writes one record at `start` (from [`Region::place`]); the magic goes last.
    pub fn write_record(&self, start: Pos, gen_: u64, seq: u64, key: &str, text: &str) {
        let len = record_len(key.len(), text.len());
        let mut head = [0u8; REC_HEAD];
        head[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        head[8..16].copy_from_slice(&gen_.to_le_bytes());
        head[16..24].copy_from_slice(&seq.to_le_bytes());
        head[24..28].copy_from_slice(&(key.len() as u32).to_le_bytes());
        head[28..32].copy_from_slice(&(text.len() as u32).to_le_bytes());
        head[32..40].copy_from_slice(&sum(seq, key.as_bytes(), text.as_bytes()).to_le_bytes());
        let body = Pos { seg: start.seg, off: start.off + 4 };
        let after = self.put(body, &head[4..]);
        let after = self.put(after, key.as_bytes());
        self.put(after, text.as_bytes());
        self.publish(start);
    }

    /// Copies the record at `from` (`len` bytes) to `to`, giving it generation `gen_`.
    pub fn copy_record(&self, from: Pos, len: usize, to: Pos, gen_: u64) {
        let mut buf = [0u8; 4096];
        let (mut src, mut dst, mut left) = (Pos { seg: from.seg, off: from.off + 4 }, Pos { seg: to.seg, off: to.off + 4 }, len - 4);
        let mut first = true;
        while left > 0 {
            let n = left.min(buf.len());
            let Some(next) = self.get(src, &mut buf[..n]) else { return };
            if first {
                // Bytes 8..16 of the record (4..12 of what's copied) are its generation.
                buf[4..12].copy_from_slice(&gen_.to_le_bytes());
                first = false;
            }
            dst = self.put(dst, &buf[..n]);
            src = next;
            left -= n;
        }
        self.publish(to);
    }

    fn publish(&self, start: Pos) {
        fence(Ordering::Release);
        // SAFETY: record heads start 4-byte aligned inside one segment (segments start 4 bytes
        // into a page, and every record length is a multiple of 4).
        unsafe { AtomicU32::from_ptr(self.ptr(start).cast()).store(RECORD_MAGIC, Ordering::Release) };
    }

    /// Every valid record of half `h` written under generation `gen_`, in order.
    pub fn scan(&self, h: usize, gen_: u64, out: &mut Vec<Found>) {
        let mut p = self.start(h);
        let half = self.half(h);
        loop {
            if self.segs[p.seg].len - p.off < REC_HEAD {
                p = Pos { seg: p.seg + 1, off: 0 };
            }
            if !half.contains(&p.seg) {
                return;
            }
            let mut head = [0u8; REC_HEAD];
            if self.get(p, &mut head).is_none() {
                return;
            }
            let word = |i: usize| u32::from_le_bytes(head[i..i + 4].try_into().unwrap_or_default());
            let long = |i: usize| u64::from_le_bytes(head[i..i + 8].try_into().unwrap_or_default());
            let (len, seq, klen, tlen) = (word(4) as usize, long(16), word(24) as usize, word(28) as usize);
            if word(0) != RECORD_MAGIC || long(8) != gen_ || len != record_len(klen, tlen) || self.place(h, p, len).is_none_or(|(s, _)| s != p) {
                return;
            }
            let mut body = vec![0u8; klen + tlen];
            if self.get(Pos { seg: p.seg, off: p.off + REC_HEAD }, &mut body).is_none() || sum(seq, &body[..klen], &body[klen..]) != long(32) {
                return;
            }
            let text = String::from_utf8(body.split_off(klen)).ok();
            let Ok(key) = String::from_utf8(body) else { return };
            let Some((_, end)) = self.place(h, p, len) else { return };
            out.push(Found { half: h, at: p, len, seq, key, text });
            p = end;
        }
    }

    pub fn read_header(&self) -> Option<Header> {
        (0..2).filter_map(|i| self.slot(i)).max_by_key(|h| h.hseq)
    }

    fn slot(&self, i: usize) -> Option<Header> {
        let mut b = [0u8; SLOT];
        self.get(Pos { seg: 0, off: i * SLOT }, &mut b)?;
        let word = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap_or_default());
        let long = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or_default());
        if word(0) != HEADER_MAGIC || word(4) != 1 || twox_hash::XxHash3_64::oneshot(&b[..56]) != long(56) {
            return None;
        }
        Some(Header { hseq: long(8), instance: long(16), gens: [long(24), long(32)], active: word(40), clean: word(44) != 0, seq_hint: long(48) })
    }

    /// Writes `h` into the slot the current header isn't in.
    pub fn write_header(&self, h: &Header) {
        let current = |i: usize| self.slot(i).map_or(0, |s| s.hseq + 1);
        let i = usize::from(current(0) >= current(1));
        let mut b = [0u8; SLOT];
        b[0..4].copy_from_slice(&HEADER_MAGIC.to_le_bytes());
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        b[8..16].copy_from_slice(&h.hseq.to_le_bytes());
        b[16..24].copy_from_slice(&h.instance.to_le_bytes());
        b[24..32].copy_from_slice(&h.gens[0].to_le_bytes());
        b[32..40].copy_from_slice(&h.gens[1].to_le_bytes());
        b[40..44].copy_from_slice(&h.active.to_le_bytes());
        b[44..48].copy_from_slice(&u32::from(h.clean).to_le_bytes());
        b[48..56].copy_from_slice(&h.seq_hint.to_le_bytes());
        let s = twox_hash::XxHash3_64::oneshot(&b[..56]);
        b[56..64].copy_from_slice(&s.to_le_bytes());
        self.put(Pos { seg: 0, off: i * SLOT }, &b);
    }
}

pub(super) fn record_len(klen: usize, tlen: usize) -> usize {
    (REC_HEAD + klen + tlen).div_ceil(4) * 4
}

fn sum(seq: u64, key: &[u8], text: &[u8]) -> u64 {
    twox_hash::XxHash3_64::oneshot_with_seed(twox_hash::XxHash3_64::oneshot_with_seed(seq, key), text)
}
