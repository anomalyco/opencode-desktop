//! The composer's server-wide catalog. Refresh it on the stream's new handshake, not by
//! replaying the launch (which also restores tabs and sessions). Each list recovers alone.

use std::time::Duration;

use opencode_kit::client::Server;
use rune::Tx;
use rune::machine::{Attempt, Seq};
use serde_json::Value;

use crate::{ConnectHost, ConnectJob, ConnectReply, ConnectRequest};

/// Trailing debounce window for SSE `*.updated` bursts (seconds; matches official `settleMs` in
/// `packages/client/src/solid/data.ts:346-356`).
const SETTLE_SECS: f64 = 0.25;
/// Minimum interval between SSE-triggered refetches for the same catalog list (seconds).
const MIN_GAP_SECS: f64 = 1.0;

/// One of the four server-wide catalogs the composer's pickers read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerList {
    Providers,
    Config,
    Models,
    Agents,
}

const LISTS: [PickerList; 4] = [
    PickerList::Providers,
    PickerList::Config,
    PickerList::Models,
    PickerList::Agents,
];

impl PickerList {
    fn idx(self) -> usize {
        match self {
            Self::Providers => 0,
            Self::Config => 1,
            Self::Models => 2,
            Self::Agents => 3,
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Providers => "/api/provider",
            Self::Config => "/api/config",
            Self::Models => "/api/model",
            Self::Agents => "/api/agent",
        }
    }

    fn apply(self, host: &mut dyn ConnectHost, reply: Option<&Value>) -> bool {
        let Some(list) = reply.map(|v| v.get("data").unwrap_or(v)).filter(|v| v.is_array()) else {
            return false;
        };
        match self {
            Self::Providers => host.load_providers(list),
            Self::Config => host.load_config(list),
            Self::Models => host.load_models(list),
            Self::Agents => host.load_agents(list),
        }
        true
    }
}

/// Per-list generation & coalescing state for server-wide picker catalog reads. Each catalog has
/// its own generation so refreshing one list never drops another list's in-flight reply.
#[derive(Default)]
pub struct PickerReads {
    seq: [Seq; 4],
    due: [Option<f64>; 4],
    last_sent: [Option<f64>; 4],
}

impl PickerReads {
    fn next(&mut self, list: PickerList) -> Attempt {
        self.seq[list.idx()].next()
    }

    fn current(&self, list: PickerList) -> Attempt {
        self.seq[list.idx()].current()
    }
}

/// Retry schedule for one picker catalog read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerRead {
    Fresh,
    Retry(u8),
}

impl PickerRead {
    fn delay(self) -> Duration {
        match self {
            Self::Fresh => Duration::ZERO,
            Self::Retry(n) => Duration::from_millis(250 << n),
        }
    }

    fn failed(self) -> Option<Self> {
        match self {
            Self::Fresh => Some(Self::Retry(0)),
            Self::Retry(n) if n < 4 => Some(Self::Retry(n + 1)),
            _ => None,
        }
    }
}

/// Applies the launch read's `[providers, config, models, agents]` replies and schedules retries
/// for any list that didn't arrive.
pub fn first(host: &mut dyn ConnectHost, replies: [Option<Value>; 4]) {
    for (list, reply) in LISTS.into_iter().zip(replies) {
        let generation = {
            let pr = host.picker_reads_mut();
            pr.due[list.idx()] = None;
            pr.next(list)
        };
        if !list.apply(host, reply.as_ref()) {
            request(host, generation, list, PickerRead::Retry(0));
        }
    }
}

/// Refreshes all four picker catalogs against the active server immediately (reconnect / connect
/// completion / one-shot empty-model boot).
pub fn refresh(host: &mut dyn ConnectHost) {
    for list in LISTS {
        let generation = {
            let pr = host.picker_reads_mut();
            pr.due[list.idx()] = None;
            pr.next(list)
        };
        request(host, generation, list, PickerRead::Fresh);
    }
}

/// Coalesces an SSE `*.updated` notification for a single catalog list: multiple events within
/// [`SETTLE_SECS`] (or within [`MIN_GAP_SECS`] of the prior SSE-triggered read) collapse into one
/// HTTP GET without bumping generation mid-flight (`packages/client/src/solid/data.ts:346-356,1460-1528`).
pub fn schedule(host: &mut dyn ConnectHost, list: PickerList, now: f64) {
    let i = list.idx();
    let pr = host.picker_reads_mut();
    let min_at = pr.last_sent[i].map_or(0.0, |last| last + MIN_GAP_SECS);
    pr.due[i] = Some((now + SETTLE_SECS).max(min_at));
}

/// Dispatches any coalesced single-list catalog reads whose deadline has elapsed and schedules
/// the next wake for pending deadlines.
pub fn poll(host: &mut dyn ConnectHost, now: f64, tx: &mut Tx) {
    let mut fire = Vec::new();
    let mut next_due = None::<f64>;
    {
        let pr = host.picker_reads_mut();
        for list in LISTS {
            let i = list.idx();
            let Some(due) = pr.due[i] else { continue };
            if due <= now + 1e-9 {
                pr.due[i] = None;
                pr.last_sent[i] = Some(now);
                fire.push((list, pr.next(list)));
            } else {
                next_due = Some(next_due.map_or(due, |best| best.min(due)));
            }
        }
    }
    for (list, generation) in fire {
        request(host, generation, list, PickerRead::Fresh);
    }
    if let Some(due) = next_due {
        tx.wake_after(Duration::from_secs_f64((due - now).max(0.0)));
    }
}

fn request(host: &mut dyn ConnectHost, generation: Attempt, list: PickerList, read: PickerRead) {
    host.spawn_job(ConnectJob::Server {
        delay: read.delay(),
        request: ConnectRequest::PickerRead { generation, list, read },
    });
}

pub(crate) fn execute(server: &Server, generation: Attempt, list: PickerList, read: PickerRead) -> ConnectReply {
    let (value, client_error) = match server.get(list.path()) {
        Ok(v) => (Some(v), false),
        Err(e) => (None, e.status().is_some_and(|s| matches!(s, 400..=499) && s != 429)),
    };
    ConnectReply::PickerRead { generation, list, read, value, client_error }
}

pub(crate) fn apply(
    host: &mut dyn ConnectHost,
    generation: Attempt,
    list: PickerList,
    read: PickerRead,
    value: Option<Value>,
    client_error: bool,
    tx: &mut Tx,
) {
    if host.picker_reads().current(list) != generation {
        return;
    }
    if list.apply(host, value.as_ref()) {
        tx.instant();
        host.on_models_changed(tx);
    } else if !client_error
        && let Some(next) = read.failed()
    {
        request(host, generation, list, next);
    }
}
