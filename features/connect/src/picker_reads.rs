//! The composer's server-wide catalog. Refresh it on the stream's new handshake, not by
//! replaying the launch (which also restores tabs and sessions). Each list recovers alone.

use std::time::Duration;

use opencode_kit::client::Server;
use rune::Tx;
use rune::machine::Attempt;
use serde_json::Value;

use crate::{ConnectHost, ConnectJob, ConnectReply, ConnectRequest};

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
    let generation = host.next_picker_reads_generation();
    for (list, reply) in LISTS.into_iter().zip(replies) {
        if !list.apply(host, reply.as_ref()) {
            request(host, generation, list, PickerRead::Retry(0));
        }
    }
}

/// Refreshes all four picker catalogs against the active server.
pub fn refresh(host: &mut dyn ConnectHost) {
    let generation = host.next_picker_reads_generation();
    for list in LISTS {
        request(host, generation, list, PickerRead::Fresh);
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
    if host.current_picker_reads_generation() != generation {
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
