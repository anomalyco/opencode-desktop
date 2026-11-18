//! The composer's server-wide catalog. Refresh it on the stream's new handshake, not by
//! replaying the launch (which also restores tabs and sessions). Each list recovers alone.

use std::time::Duration;

use rune::machine::Attempt;
use serde_json::Value;

use crate::App;

#[derive(Clone, Copy)]
enum List { Providers, Config, Models, Agents }
use List::*;
const LISTS: [List; 4] = [Providers, Config, Models, Agents];

impl List {
    fn path(self) -> &'static str {
        match self { Providers => "/api/provider", Config => "/api/config", Models => "/api/model", Agents => "/api/agent" }
    }

    fn apply(self, app: &mut App, reply: Option<&Value>) -> bool {
        let Some(list) = reply.map(|v| v.get("data").unwrap_or(v)).filter(|v| v.is_array()) else { return false };
        match self {
            Providers => app.pickers.load_providers(list),
            Config => app.pickers.load_config(list),
            Models => app.pickers.load_models(list),
            Agents => app.pickers.load_agents(list),
        }
        true
    }
}

pub(crate) fn first(app: &mut App, replies: [Option<Value>; 4]) {
    let generation = app.jobs.picker_reads.next();
    for (list, reply) in LISTS.into_iter().zip(replies) {
        if !list.apply(app, reply.as_ref()) { request(app, generation, list, Read::Retry(0)); }
    }
}

pub(crate) fn refresh(app: &mut App) {
    let generation = app.jobs.picker_reads.next();
    for list in LISTS { request(app, generation, list, Read::Fresh); }
}

#[derive(Clone, Copy)]
enum Read { Fresh, Retry(u8) }

impl Read {
    fn delay(self) -> Duration {
        match self { Self::Fresh => Duration::ZERO, Self::Retry(n) => Duration::from_millis(250 << n) }
    }

    fn failed(self) -> Option<Self> {
        match self { Self::Fresh => Some(Self::Retry(0)), Self::Retry(n) if n < 4 => Some(Self::Retry(n + 1)), _ => None }
    }
}

fn request(app: &App, generation: Attempt, list: List, read: Read) {
    app.job_after(read.delay(), move |server| {
        let result = server.get(list.path());
        Box::new(move |app, tx| {
            if app.jobs.picker_reads.current() != generation { return; }
            if list.apply(app, result.as_ref().ok()) {
                tx.instant();
            } else if !result.is_err_and(|e| e.status().is_some_and(|s| matches!(s, 400..=499) && s != 429))
                && let Some(next) = read.failed() {
                request(app, generation, list, next);
            }
        })
    });
}
