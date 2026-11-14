//! Search only becomes available after actual frame presentation. A migration failure
//! rolls back without taking the authoritative stores offline.
mod tokenizer;
pub(crate) mod index;
pub(crate) mod query;
pub use query::{Hit, Scope, SearchSurface};

#[derive(Default)]
pub(crate) enum State { #[default] BeforeFrame, Ready(Work), Unavailable(String) }

pub(crate) enum Work { Pending, Drained }

impl State {
    pub(crate) fn changed(&mut self) { if let Self::Ready(work) = self { *work = Work::Pending; } }
}

use rusqlite::{Connection, TransactionBehavior};

use super::schema::{self, FLAG_BITS, FLAG_MASK};

pub(crate) fn initialize(c: &mut Connection, now: i64) -> rusqlite::Result<()> {
    tokenizer::register(c)?;
    let version = schema::user_version(c)?;
    if version >> FLAG_BITS == schema::SCHEMA_VERSION { return Ok(()) }
    if version >> FLAG_BITS != schema::BASE_SCHEMA { return Err(rusqlite::Error::InvalidQuery) }
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("migrate.sql"))?;
    tx.execute("INSERT INTO migration(id,time_completed) VALUES ('20261114000000_storage-search',?1)", [now])?;
    tx.pragma_update(None, "user_version", (schema::SCHEMA_VERSION << FLAG_BITS) | (version & FLAG_MASK))?;
    tx.commit()
}
