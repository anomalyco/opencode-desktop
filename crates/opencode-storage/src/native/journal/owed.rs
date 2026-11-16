//! What a dead owner's journal still owes the database, read without taking the journal over.
//! A memory-only window that regains the workspace must not save over keystrokes that only the
//! journal holds: the next owner would trust them (`Kind::JournalOwner`), and a rebuilt boot record
//! without that owner would make them unreadable for good.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use super::{Found, Recovered, Region, Seg, location, namespace_of, still_the_row};

/// The drafts an *owned*, unclean journal holds that are newer than the database's revision and
/// whose window is still saved: exactly what stage B's `reconcile` would write on the next open.
/// Empty when no record names the journal as its run's, or the journal ended clean.
pub(crate) fn owed(conn: &Connection, db: &Path) -> Result<Vec<Recovered>, String> {
    let Some(loc) = location(conn).map_err(|e| e.to_string())? else { return Ok(Vec::new()) };
    let owner = super::super::boot_record::from_sqlite(conn).ok().flatten().and_then(crate::boot_packet::BootPacket::parse).and_then(|p| p.journal_owner());
    if owner != Some(loc.instance) {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(db).map_err(|e| e.to_string())?;
    let ps = u64::from(loc.page_size);
    let (lo, hi) = (loc.pages.iter().min().copied().ok_or("no pages")?, loc.pages.iter().max().copied().ok_or("no pages")?);
    let from = (u64::from(lo) - 1) * ps;
    let len = (u64::from(hi) * ps - from) as usize;
    if file.metadata().map_err(|e| e.to_string())?.len() < u64::from(hi) * ps {
        return Err("the file is shorter than the journal".into());
    }
    // SAFETY: a private copy-on-write view; nothing is written through it to the file.
    #[allow(unsafe_code)]
    let mut map = unsafe { memmap2::MmapOptions::new().offset(from).len(len).map_copy(&file) }.map_err(|e| e.to_string())?;
    if !still_the_row(&file, &loc, &map, from) {
        return Err("the recorded place isn't the journal row's any more".into());
    }
    let n = loc.pages.len();
    let segs: Vec<Seg> = loc.pages.iter().enumerate().map(|(i, p)| {
        let at = ((u64::from(*p) - 1) * ps - from) as usize + 4;
        let full = loc.page_size as usize - 4;
        Seg { at, len: if i + 1 == n { (loc.last as usize).min(full) / 4 * 4 } else { full / 4 * 4 } }
    }).collect();
    if segs.len() < 3 {
        return Err("too few pages".into());
    }
    let region = Region { base: map.as_mut_ptr(), segs };
    let Some(h) = region.read_header().filter(|h| h.instance == loc.instance) else { return Err("the journal at the recorded place isn't this database's".into()) };
    if h.clean {
        return Ok(Vec::new());
    }
    let mut found: Vec<Found> = Vec::new();
    for half in 0..2 {
        region.scan(half, h.gens[half], &mut found);
    }
    let mut newest: HashMap<String, Found> = HashMap::new();
    for f in found {
        if newest.get(&f.key).is_none_or(|n| n.seq < f.seq) {
            newest.insert(f.key.clone(), f);
        }
    }
    let mut owed = Vec::new();
    for f in newest.into_values() {
        let Some(text) = f.text else { continue };
        let rev: u64 = conn.query_row("SELECT rev FROM draft_rev WHERE key = ?1", [&f.key], |x| x.get::<_, i64>(0)).optional().map_err(|e| e.to_string())?.map_or(0, |v| v as u64);
        let ns = namespace_of(&f.key).unwrap_or_default();
        let window: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM state WHERE name = ?1)", [ns], |x| x.get(0)).map_err(|e| e.to_string())?;
        if window && f.seq > rev {
            owed.push(Recovered { key: f.key, seq: f.seq, text });
        }
    }
    drop(map);
    Ok(owed)
}
