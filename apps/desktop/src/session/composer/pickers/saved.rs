//! A session's picks across a relaunch: the agent and model picked in it while the server does
//! not run with them, and the model each of its agents last had. Desktop keeps them per
//! workspace (`providers/models/selection.tsx`, `Persist.serverWorkspace(.., "model-selection")`,
//! written on `selection-machine.ts`'s `persist.write`); here they are one app-wide row, by
//! session id, each entry in Desktop's shape with the time it was written.
//!
//! The row is read at launch from the app-wide keys the boot record already holds, so it stays
//! small: the sessions written longest ago go once it holds more than [`LIMIT`]. A new session's
//! draft keeps nothing (Desktop's draft selection is its provider's own signal).

use serde_json::{Value, json};

use super::DRAFT;
use crate::App;

const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "rune.model-selection";
/// An entry is about 200 bytes; a boot record's value should stay far under 64 KiB.
const LIMIT: usize = 64;

/// At launch: what each session kept.
pub(crate) fn restore(s: &mut App) {
    let Some(Value::Object(saved)) = crate::storage::json(GLOBAL, KEY) else { return };
    for (session, entry) in &saved {
        s.composer_ext.pickers.restore(session, entry);
    }
}

/// The picks of `scope` changed: its entry is written, or goes when it holds nothing now.
pub(crate) fn save(s: &App, scope: &str) {
    if scope == DRAFT || !s.persists() {
        return;
    }
    let entry = s.composer_ext.pickers.stored(scope);
    let at = *s.now_ms;
    crate::storage::update(GLOBAL, KEY, |saved| {
        let Some(mut entry) = entry else {
            saved.remove(scope);
            return;
        };
        entry["at"] = json!(at);
        saved.insert(scope.to_owned(), entry);
        while saved.len() > LIMIT {
            let Some(oldest) = saved.iter().min_by_key(|(_, e)| e["at"].as_i64().unwrap_or(0)).map(|(k, _)| k.clone()) else { break };
            saved.remove(&oldest);
        }
    });
}

/// `session.deleted`: the session's picks go (`selection-machine.ts` `session.deleted`).
pub(crate) fn forget(s: &mut App, session: &str) {
    s.composer_ext.pickers.forget_session(session);
    save(s, session);
}
