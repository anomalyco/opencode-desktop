//! What the open owes the windows while it decides the draft journal, and the journal's attach.

use std::time::Instant;

use super::super::{StorageConfig, boot_record, journal};
use super::{BootReport, ns};
use crate::boot_packet::BootPacket;

/// What the open still owes the windows, and what the draft journal brought.
#[derive(Default)]
pub(super) struct Owe {
    /// A packet read before SQLite (or a new profile's empty one), sent once the journal exists.
    pub packet: Option<BootPacket>,
    /// What was sent from inside stage B.
    pub sent: Option<BootPacket>,
    /// Where a record said the journal is.
    pub hint: Option<journal::Location>,
    /// The drafts the journal held at launch.
    pub recovered: Vec<journal::Recovered>,
    /// The journal was reserved by this open: the boot record must name it.
    pub reserved: bool,
    /// This run keeps a journal (not one that shows another process's file, saving nothing).
    pub want_journal: bool,
}

pub(super) fn attach_journal(config: &StorageConfig, report: &mut BootReport, owe: &mut Owe, loc: &journal::Location, fresh: bool) -> bool {
    let t = Instant::now();
    let now_us = u64::try_from((config.clock)()).unwrap_or(0).saturating_mul(1000);
    let ok = match journal::attach(&config.path, loc, fresh, now_us) {
        Ok(r) => {
            report.journal_recovered = r.len();
            owe.recovered = r;
            true
        }
        Err(e) => {
            report.journal_error = Some(e);
            false
        }
    };
    report.journal_ns += ns(t);
    rune::marks::mark("storage.journal");
    ok
}

pub(super) fn record_names_journal(conn: &rusqlite::Connection) -> bool {
    boot_record::from_sqlite(conn).ok().flatten().and_then(BootPacket::parse).is_some_and(|p| p.journal_location().is_some())
}
