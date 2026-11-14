//! The open's raw packet, and its wait for an instance that's still quitting: its windows are
//! gone and the single-instance claim with them, its last write (the WAL's sync, the record's
//! copy) still to come.

use std::time::Instant;

use rusqlite::Connection;

use super::super::boot_record::{self, BootPacket, BootSource, FastBoot};
use super::super::{StorageConfig, import};
use super::{BootReport, Lock, Owe, is_held, ns, owned_journal, stage_b};
use crate::cache::Cache;

/// The first frame's packet from a record read raw: `pending` when it's the copy the last quit
/// kept (its WAL is checkpointed after the first frame, `WalEvent::Opened`). A record that names a
/// journal waits for it (after the claim): what it holds may be newer than the record, and the
/// first frame shows the newer. One that names none (the first launch after the update) has
/// nothing to wait for: the journal is reserved beside the first frame, before the first key.
#[allow(clippy::too_many_arguments)]
pub(super) fn raw_packet(bytes: Vec<u8>, pending: bool, report: &mut BootReport, owe: &mut Owe, packet: &mut Option<BootPacket>, t0: Instant, send: &dyn Fn(BootPacket)) {
    report.raw_read_ns = ns(t0);
    let t = Instant::now();
    report.boot_bytes = bytes.len();
    let Some(mut p) = BootPacket::parse(bytes) else {
        report.raw_miss = Some("torn record");
        return;
    };
    if pending {
        report.wal_pending = true;
        report.raw_miss = None;
        rune::marks::mark("storage.pending");
    }
    report.decode_ns = ns(t);
    report.boot_source = BootSource::Raw;
    report.plugin_bytes = p.plugin_bytes();
    p.source = BootSource::Raw;
    p.read_ns = report.raw_read_ns;
    p.decode_ns = report.decode_ns;
    report.stage_a_ns = ns(t0);
    owe.hint = owned_journal(&p);
    if owe.hint.is_some() {
        owe.packet = Some(p);
    } else {
        *packet = Some(p.clone());
        send(p);
    }
}

/// How long a launch waits for an instance that's still quitting (its windows gone, the claim
/// handed on, its WAL's sync and the record's copy still to come) before it opens as for any
/// other process holding the file (`held_open`).
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

pub(super) enum Quitting {
    /// The file was free (or failed otherwise): SQLite's open, as it went.
    Opened(rusqlite::Result<(Connection, Cache, import::Deferred)>),
    /// The quitting instance left its WAL synced with the record's copy beside it.
    Copy(Vec<u8>),
    /// Neither within `QUIT_WAIT`.
    Waited,
}

/// The WAL holds frames with no copy of the record for them, after the claim: an instance that's
/// still quitting is writing its last (`Writer::quit_finish`). Whichever comes first, its lock
/// going (SQLite opens: the record from it) or the record's copy (the first frame from it, SQLite
/// after), for up to `QUIT_WAIT`.
pub(super) fn await_quit(config: &StorageConfig, report: &mut BootReport, t0: Instant, send: &dyn Fn(BootPacket), owe: &mut Owe, early: &mut Vec<super::super::EarlyCache>) -> Quitting {
    let until = Instant::now() + QUIT_WAIT;
    loop {
        let result = stage_b(config, report, t0, Some(send), owe, early, Lock::Probe);
        if result.as_ref().is_ok() || result.as_ref().is_err_and(|e| !is_held(e, config)) {
            return Quitting::Opened(result);
        }
        if let FastBoot::Pending(bytes, _) = boot_record::read_raw(&config.path) {
            return Quitting::Copy(bytes);
        }
        if Instant::now() >= until {
            return Quitting::Waited;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
