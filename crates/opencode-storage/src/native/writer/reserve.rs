//! The draft journal's reservation when the open couldn't use one (the first launch after the
//! update that brought it): made after the first frame, so nothing of it is read or written before
//! the frame. One step per writer turn, with the queue drained between them, so the keys typed
//! meanwhile are committed kill-safe as they were before the update (`Storage::commit_soon`); the
//! journal holds their newest text from the start and takes it in when it's mapped.

use std::time::{Duration, Instant};

use rusqlite::{Transaction, TransactionBehavior};

use super::super::{boot_record, journal};
use super::Writer;
use crate::machine::checkpoint::WalEvent;
use crate::machine::reservation;

/// The draft journal as the open leaves it to the writer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OpenJournal {
    /// Nothing to do: mapped from a place the database holds, or none this run.
    #[default]
    Settled,
    /// The open reserved it, or wrote what it held into the database, without a sync: the next
    /// checkpoint's record names it, and the first frame's work syncs the file.
    Reserved,
    /// The database has no place for it: reserved after the first frame (`ReserveStep`).
    Owed,
}

/// The draft journal's own reservation, when the open left it owed.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Reserving {
    /// Nothing to do: the open had a place for it, or made one before the frame.
    Done,
    Owed(ReserveStep),
    /// No journal this run: it couldn't be reserved.
    Unavailable,
}
/// Where an owed reservation is: each step is one writer turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ReserveStep {
    /// The first frame isn't up yet.
    AfterFrame,
    /// Next: the tables and the zeroed blob (`journal::reserve`).
    Insert,
    /// Next: the boot record current, the blob's pages into the main file, its place recorded
    /// (`journal::locate`).
    Locate,
    /// Next: mapped, with the keys held meanwhile copied in (`journal::attach`).
    Attach(journal::Location),
}

/// Between two steps: long enough that the writer's next turn drains the queue first.
const BETWEEN: Duration = Duration::from_millis(1);

impl Writer {
    /// The first frame is up: an owed reservation starts.
    pub(super) fn reserve_after_frame(&mut self) {
        if self.reserving == Reserving::Owed(ReserveStep::AfterFrame) {
            self.next_step(ReserveStep::Insert);
        }
    }

    fn next_step(&mut self, step: ReserveStep) {
        self.reserving = Reserving::Owed(step);
        let attempt = self.seq.next();
        self.timers.insert("reserve", Some((attempt, Instant::now() + BETWEEN)));
    }

    /// The `reserve` timer: the owed reservation's next step.
    pub(super) fn reserve_step(&mut self) {
        let Reserving::Owed(step) = std::mem::replace(&mut self.reserving, Reserving::Unavailable) else { return };
        let Some(db) = self.conn.path().map(std::path::PathBuf::from) else { return self.no_journal("the database has no path".into()) };
        match step {
            ReserveStep::AfterFrame => self.reserving = Reserving::Owed(ReserveStep::AfterFrame),
            ReserveStep::Insert => match journal::reserve(&self.conn) {
                Ok(()) => {
                    self.wal_event(WalEvent::Committed { boot: false });
                    self.next_step(ReserveStep::Locate);
                }
                Err(e) => self.no_journal(e.to_string()),
            },
            // The record goes current first: the checkpoint empties the WAL, and whenever the WAL is
            // empty the record on page 2 is the truth. `journal_meta` row 2 then keeps this record's
            // sum, so a kill before the next rebuild leaves a place the next launch trusts.
            ReserveStep::Locate => match self.rebuild_record().and_then(|()| journal::locate(&self.conn, &db, self.clock)) {
                Ok(loc) => {
                    self.wal_event(WalEvent::Committed { boot: false });
                    self.next_step(ReserveStep::Attach(loc));
                }
                Err(e) => self.no_journal(e.to_string()),
            },
            ReserveStep::Attach(loc) => {
                let now_us = u64::try_from((self.clock)()).unwrap_or(0).saturating_mul(1000);
                match journal::attach(&db, &loc, true, now_us) {
                    Ok(_) => {
                        rune::marks::mark("journal.attached");
                        // The next checkpoint's record names it, kept by this run.
                        self.wal_event(WalEvent::Committed { boot: true });
                        self.reserving = Reserving::Done;
                        // Its pages reached the file through an unsynced checkpoint: one sync makes them durable.
                        reservation::step(&mut self.reservation, reservation::Event::Reserved);
                        self.sync_reservation();
                    }
                    Err(e) => self.no_journal(e),
                }
            }
        }
    }

    fn rebuild_record(&self) -> rusqlite::Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        boot_record::rebuild(&tx)?;
        tx.commit()
    }

    /// This run has no journal: the keys go to the database alone, and the next launch reserves
    /// it again (a place this run left half made is made again).
    fn no_journal(&mut self, why: String) {
        eprintln!("opencode-storage: couldn't reserve the draft journal after the first frame ({why}); this run keeps drafts without it, and the next launch tries again");
        self.reserving = Reserving::Unavailable;
        journal::decided();
    }

    /// The writer stops with the reservation still owed: nothing more is held for it.
    pub(super) fn reserve_stopped(&mut self) {
        if matches!(self.reserving, Reserving::Owed(_)) {
            self.timers.insert("reserve", None);
            self.reserving = Reserving::Unavailable;
            journal::decided();
        }
    }
}
