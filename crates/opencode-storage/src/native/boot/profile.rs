//! The profile's lease at the open (`native::lease`): taken after the instance claim and before
//! the journal is mapped or SQLite opens the file.
//!
//! - **Held at once:** the open goes on as before. A copy of a build from before the lease (the
//!   first update into this one) never takes it, but holds SQLite's lock for its whole life: when
//!   it does (macOS), what the record showed is held, as for any other process holding the file.
//! - **Busy** (another copy, or one still closing its storage: a relaunch's old copy, a launch
//!   while the app quits): the first frame waits for the lease a few seconds (`SHOW`; a relaunch,
//!   which Rune already held up to 10 s for its old copy, `SHOW_RELAUNCH`), since a copy that's
//!   quitting writes its last state and a frame from before that is one the file no longer shows.
//!   Then the held path: what was read is shown, held (with "OpenCode is finishing a save"), the
//!   lease is tried again for as long as the open waits for SQLite's lock, and once it's taken the
//!   open goes on; what the windows were shown is kept only if the file still shows it
//!   (`shows_the_same`), else the run keeps its changes in memory. Still held after `WAIT`: memory,
//!   saying another copy is running.
//! - **Locking unsupported** (a network home): the open goes on without a lease, as before it.
//! - **The file can't be made** (a read-only volume, a full disk): memory, saying why.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::{BootReport, Owe, ns};
use crate::cache::OutboxCache;
use crate::native::boot_record::{BootPacket, BootSource};
use crate::native::{Backend, StorageConfig, journal, lease};
use crate::Storage;

/// How long a launch waits for another copy to let go of the profile: as long as the open waits
/// for SQLite's lock (`BUSY_TRIES` tries of `busy_timeout`).
const WAIT: Duration = Duration::from_secs(30);
/// How long a busy launch's first frame waits for the lease before it shows what was read, held.
const SHOW: Duration = Duration::from_secs(5);
/// The same for a relaunch (Restart, an update's), which Rune held up to 10 s for its old copy.
const SHOW_RELAUNCH: Duration = Duration::from_secs(2);

/// A relaunch (Restart, an update's): Rune names the copy it replaces and waits for it at most 10 s.
pub(super) fn relaunch() -> bool {
    std::env::var_os("RUNE_RELAUNCH_AFTER").is_some()
}

/// Why this run keeps its changes in memory, when it does.
pub(super) enum Hold {
    Go,
    Memory(String),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn hold(config: &StorageConfig, report: &mut BootReport, t0: Instant, send: &dyn Fn(BootPacket), owe: &mut Owe, packet: &mut Option<BootPacket>, held: &mut Option<BootPacket>) -> Hold {
    let mut waited: Option<Instant> = None;
    let until = Instant::now() + WAIT;
    let show = if relaunch() { SHOW_RELAUNCH } else { SHOW };
    loop {
        match lease::try_take(&config.path) {
            lease::Outcome::Held => break,
            lease::Outcome::Unsupported(e) => {
                eprintln!("opencode-storage: this volume can't lock the profile ({e}); it opens without the lease");
                break;
            }
            lease::Outcome::Unavailable(e) => {
                lease::decide(true);
                return Hold::Memory(e.to_string());
            }
            lease::Outcome::Busy if Instant::now() >= until => {
                lease::decide(false);
                return Hold::Memory(lease::IN_USE.into());
            }
            lease::Outcome::Busy => {
                let since = *waited.get_or_insert_with(|| {
                    rune::marks::mark("storage.lease.busy");
                    hold_what_was_read(owe, packet, held);
                    Instant::now()
                });
                // The first frame waits `show` for the lease, then gets what was read, held.
                if packet.is_none() && since.elapsed() >= show {
                    let p = held.take().unwrap_or_else(BootPacket::held_empty);
                    show_held(report, t0, owe, packet, send, p);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
    lease::decide(true);
    // A relaunch read the record before Rune waited for its old copy, which may have saved since:
    // what nothing was shown from yet is read again, through SQLite now that the file is this
    // process's (the journal's place is still checked there).
    if relaunch() && packet.is_none() {
        owe.packet = None;
        *held = None;
        report.raw_miss = Some("a relaunch reads the record once its old copy has let go");
    }
    if waited.is_some() {
        rune::marks::mark("storage.lease.held");
    } else if lease::sqlite_held_elsewhere(&config.path) {
        // A copy of a build without the lease still has the file: what was read is held.
        rune::marks::mark("storage.lease.sqlite_held");
        hold_what_was_read(owe, packet, held);
    }
    Hold::Go
}

/// What another process may still change is held: a packet not yet sent becomes the held
/// fallback, one already sent is compared before anything is kept, and the journal is mapped only
/// through SQLite (stage B checks its row there).
fn hold_what_was_read(owe: &mut Owe, packet: &mut Option<BootPacket>, held: &mut Option<BootPacket>) {
    lease::note_held_by_copy();
    owe.hint = None;
    if let Some(p) = packet.as_mut() {
        p.source = BootSource::Checkpoint;
    }
    if let Some(mut p) = owe.packet.take() {
        p.source = BootSource::Checkpoint;
        *held = Some(p);
    } else if let Some(p) = held.as_mut() {
        p.source = BootSource::Checkpoint;
    }
}

fn show_held(report: &mut BootReport, t0: Instant, owe: &mut Owe, packet: &mut Option<BootPacket>, send: &dyn Fn(BootPacket), p: BootPacket) {
    report.boot_source = p.source;
    report.boot_bytes = p.len();
    report.stage_a_ns = ns(t0);
    *packet = Some(p.clone());
    send(p);
    // Nothing it changes is saved unless the file still shows it once it's open: no journal.
    owe.want_journal = false;
    journal::decided();
}

/// The run in memory: the windows keep what they were shown (or what was read, held).
pub(super) fn in_memory(report: &mut BootReport, t0: Instant, send: &dyn Fn(BootPacket), owe: &mut Owe, packet: Option<BootPacket>, held: Option<BootPacket>, why: String) -> (Storage, BootReport) {
    report.error = Some(why);
    report.total_ns = ns(t0);
    journal::decided();
    let mut cache = crate::cache::Cache { complete: true, outbox: OutboxCache::Loaded(BTreeMap::new()), ..Default::default() };
    match packet {
        Some(p) => p.fill_cache(&mut cache),
        None => {
            let p = owe.packet.take().or(held).map(|mut p| {
                p.source = BootSource::Checkpoint;
                p
            });
            let p = p.unwrap_or_else(BootPacket::held_empty);
            p.fill_cache(&mut cache);
            send(p);
        }
    }
    eprintln!("opencode-storage: {}; this run keeps its changes in memory", report.error.as_deref().unwrap_or_default());
    (Storage::from_parts(crate::native::shared(cache), Backend::memory()), std::mem::take(report))
}
