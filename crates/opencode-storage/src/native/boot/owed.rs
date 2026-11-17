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
    /// What the writer has still to do for the journal.
    pub journal: super::super::writer::OpenJournal,
    /// This run keeps a journal (not one that shows another process's file, saving nothing).
    pub want_journal: bool,
}

pub(super) fn attach_journal(config: &StorageConfig, report: &mut BootReport, owe: &mut Owe, loc: &journal::Location, fresh: bool) -> bool {
    let t = Instant::now();
    let now_us = u64::try_from((config.clock)()).unwrap_or(0).saturating_mul(1000);
    let ok = match journal::attach(&config.path, loc, fresh, now_us) {
        Ok(r) => {
            report.journal_recovered = r.len();
            if !fresh {
                report.journal_replayed = true;
            }
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

/// The journal a record names, when the run that wrote the record kept it (`Kind::JournalOwner`).
/// A record rebuilt by a run without it (an older build, or a platform it was off on, which still
/// names the place) is no hint: what that run saved may be newer than the journal's records.
pub(super) fn owned_journal(p: &BootPacket) -> Option<journal::Location> {
    p.journal_location().filter(|l| p.journal_owner() == Some(l.instance))
}

/// Whether the stored boot record was written by a run that kept the journal `instance`.
pub(super) fn record_owns_journal(conn: &rusqlite::Connection, instance: u64) -> bool {
    boot_record::from_sqlite(conn).ok().flatten().and_then(BootPacket::parse).is_some_and(|p| p.journal_owner() == Some(instance))
}
