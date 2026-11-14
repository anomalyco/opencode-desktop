//! Search only becomes available after actual frame presentation. A migration failure
//! rolls back without taking the authoritative stores offline.
mod tokenizer;
pub(crate) mod index;
pub(crate) mod query;
pub use query::{Coverage, Hit, Scope, SearchSurface};

#[derive(Default)]
pub(crate) enum State { #[default] BeforeFrame, Ready(Work), Unavailable(String) }

/// The index lane's work. It never starts a synced checkpoint (its rows can be rebuilt): it
/// waits at `WalFull` until one that something else started empties the WAL, and stops for good
/// at the quit (the backlog is queued in the file, for the next launch).
pub(crate) enum Work { Pending, Drained, WalFull, Quitting }

impl State {
    /// A write queued an entity: there's work, unless the lane is waiting on the WAL or stopped.
    pub(crate) fn changed(&mut self) {
        if let Self::Ready(work @ (Work::Pending | Work::Drained)) = self {
            *work = Work::Pending;
        }
    }

    /// A checkpoint emptied the WAL: a lane that waited on it goes on.
    pub(crate) fn checkpointed(&mut self) {
        if let Self::Ready(work @ Work::WalFull) = self {
            *work = Work::Pending;
        }
    }

    /// The quit: no more index steps this run.
    pub(crate) fn quitting(&mut self) {
        if let Self::Ready(work) = self {
            *work = Work::Quitting;
        }
    }
}

use rusqlite::{Connection, TransactionBehavior};

use super::schema::{self, FLAG_BITS, FLAG_MASK};

/// Whose the file is, for schema 2: made by this open, or found (an existing profile, which is
/// this build's when it has its journal).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Made,
    Found,
}

/// Registers the tokenizer and brings the file to schema 2; false when it stays at 1 (a profile
/// from before the journal, the switch off: `schema::upgrade_existing`).
pub(crate) fn initialize(c: &mut Connection, now: i64, origin: Origin) -> rusqlite::Result<bool> {
    tokenizer::register(c)?;
    let version = schema::user_version(c)?;
    if version >> FLAG_BITS == schema::SCHEMA_VERSION { return Ok(true) }
    if version >> FLAG_BITS != schema::BASE_SCHEMA { return Err(rusqlite::Error::InvalidQuery) }
    if origin == Origin::Found && super::journal::location(c)?.is_none() && !schema::upgrade_existing() { return Ok(false) }
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Schema 2's tables at version 1: an older build opened the file since and set its version
    // back. Its writes aren't indexed, so the derived tables go and are made again.
    let again: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = 'search_meta')", [], |r| r.get(0))?;
    if again {
        tx.execute_batch("DROP TABLE IF EXISTS search_fts; DROP TABLE IF EXISTS search_meta; DROP INDEX IF EXISTS blob_by_touch; DELETE FROM migration WHERE id = '20261114000000_storage-search';")?;
    }
    tx.execute_batch(include_str!("migrate.sql"))?;
    tx.execute("INSERT INTO migration(id,time_completed) VALUES ('20261114000000_storage-search',?1)", [now])?;
    tx.pragma_update(None, "user_version", (schema::SCHEMA_VERSION << FLAG_BITS) | (version & FLAG_MASK))?;
    tx.commit()?;
    Ok(true)
}
