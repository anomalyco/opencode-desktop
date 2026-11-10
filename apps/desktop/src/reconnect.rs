//! A lost link after the app has connected: the event stream dropped, went silent past
//! [`STREAM_IDLE`], or a read or write failed on the way (a timeout, a refusal). Everything stays
//! on screen; once it has lasted [`QUIET`] a quiet session header label says so, and it leaves
//! the moment the server answers again (`Incoming::Reconnected` also resyncs). The stream thread
//! in `opencode_kit::client` retries on its own with backoff.
//!
//! One [`Link`] per server, many at once (`Links`), as OpenCode Desktop keeps one event stream
//! and one sync state per server (`packages/app/src/runtime/server/server-sync/machine.ts`:
//! `idle | disconnected | awaiting-handshake | live`, and `packages/client/src/solid/connection.ts`).
//! Desktop shows nothing while a server is away; the header label is Kit's (`motion_gate` ALLOW).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use opencode_kit::client::Request;
use opencode_kit::model::{self, Message, Part, ToolStatus};
use rune::Tx;
use rune::machine::Effects;

use crate::{App, Route, Source};

/// A short drop says nothing; the stream thread already retries after 250 ms.
pub const QUIET: f64 = 2.0;
pub const RECONNECTING: &str = "Reconnecting…";
/// The event stream is reopened after this long with nothing on it. The server sends a
/// heartbeat every 15 s, so a tunnel that accepts but never answers is caught here.
pub const STREAM_IDLE: Duration = Duration::from_secs(25);

/// A server's key (`opencode_kit::servers::ServerKey`'s text: `"sidecar"`, an address, `wsl:…`).
pub type ServerId = String;

/// Whether a lost link says so yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// Too short to mention.
    Quiet,
    /// The header says Reconnecting.
    Shown,
}

/// One server's link, as this app sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Link {
    /// The stream is up and nothing failed for want of a link.
    #[default]
    Live,
    /// A read or write failed for want of a link at `since`, and the stream hasn't said either way.
    Suspect { since: f64, notice: Notice },
    /// The event stream dropped at `since`.
    Down { since: f64, notice: Notice },
}

#[derive(Clone, Copy, Debug)]
pub enum LinkEvent {
    /// The event stream reported a drop (`events: …`).
    StreamDropped { now: f64 },
    /// Another call failed on the link itself (not the server's answer).
    CallFailed { now: f64 },
    /// The server answered something.
    Heard,
    /// The event stream is back.
    StreamBack,
    /// The window polls at `now`.
    Tick { now: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LinkEffect {
    /// Poll again at this time.
    WakeAt(f64),
    /// What the header shows changed: commit at once (a status change doesn't animate).
    Repaint,
}

impl Link {
    pub fn step(&mut self, event: LinkEvent) -> Effects<LinkEffect> {
        use LinkEffect::*;
        match (*self, event) {
            (Link::Live, LinkEvent::StreamDropped { now }) => {
                *self = Link::Down { since: now, notice: Notice::Quiet };
                Effects::one(WakeAt(now + QUIET))
            }
            // A suspect link whose stream drops is down from now, and keeps what it shows.
            (Link::Suspect { notice, .. }, LinkEvent::StreamDropped { now }) => {
                *self = Link::Down { since: now, notice };
                if notice == Notice::Quiet { Effects::one(WakeAt(now + QUIET)) } else { Effects::none() }
            }
            (Link::Live, LinkEvent::CallFailed { now }) => {
                *self = Link::Suspect { since: now, notice: Notice::Quiet };
                Effects::one(WakeAt(now + QUIET))
            }
            (Link::Suspect { notice, .. }, LinkEvent::Heard) | (Link::Suspect { notice, .. } | Link::Down { notice, .. }, LinkEvent::StreamBack) => {
                *self = Link::Live;
                if notice == Notice::Shown { Effects::one(Repaint) } else { Effects::none() }
            }
            // A suspect link whose stream outlives `STREAM_IDLE` without dropping is alive after all.
            (Link::Suspect { since, .. }, LinkEvent::Tick { now }) if now - since >= STREAM_IDLE.as_secs_f64() + QUIET => {
                *self = Link::Live;
                Effects::one(Repaint)
            }
            (Link::Suspect { since, notice: Notice::Quiet } | Link::Down { since, notice: Notice::Quiet }, LinkEvent::Tick { now }) => {
                let left = since + QUIET - now;
                if left > 1e-3 {
                    return Effects::one(WakeAt(now + left));
                }
                let suspect = matches!(*self, Link::Suspect { .. });
                *self = if suspect { Link::Suspect { since, notice: Notice::Shown } } else { Link::Down { since, notice: Notice::Shown } };
                if suspect { Effects::two(Repaint, WakeAt(now + STREAM_IDLE.as_secs_f64())) } else { Effects::one(Repaint) }
            }
            _ => Effects::none(),
        }
    }

    /// The link is down or in doubt.
    pub fn down(&self) -> bool { !matches!(self, Link::Live) }

    /// The event stream dropped.
    pub fn dropped(&self) -> bool { matches!(self, Link::Down { .. }) }

    /// The header says Reconnecting.
    pub fn shown(&self) -> bool {
        matches!(self, Link::Suspect { notice: Notice::Shown, .. } | Link::Down { notice: Notice::Shown, .. })
    }
}

/// After a reconnect, the subagents read again: their step logs give way to the fresh page when
/// it lands, and the next list of running sessions may name subagents that started meanwhile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CatchUp {
    #[default]
    Idle,
    /// The next list of running sessions is read for subagents that started while away.
    AwaitingActive,
}

/// Every server's link, many at once, and the catch-up after a reconnect.
#[derive(Default)]
pub struct Links {
    links: HashMap<ServerId, Link>,
    catch_up: HashMap<ServerId, CatchUp>,
    /// Subagents whose pages were asked for again after a reconnect, by server.
    refreshing: HashMap<ServerId, HashSet<String>>,
}

impl Links {
    pub fn get(&self, server: &str) -> Link { self.links.get(server).copied().unwrap_or_default() }

    /// Steps `server`'s link; a live link leaves the map.
    pub fn step(&mut self, server: &str, event: LinkEvent) -> Effects<LinkEffect> {
        let mut link = self.get(server);
        let effects = link.step(event);
        if link == Link::Live { self.links.remove(server); } else { self.links.insert(server.to_owned(), link); }
        effects
    }

    /// Every server whose link is down or in doubt.
    pub fn troubled(&self) -> impl Iterator<Item = &ServerId> + '_ { self.links.keys() }

    /// A server that leaves this window forgets its link.
    pub fn forget(&mut self, server: &str) {
        self.links.remove(server);
        self.catch_up.remove(server);
        self.refreshing.remove(server);
    }

    #[cfg(test)]
    pub fn set(&mut self, server: &str, link: Link) { self.links.insert(server.to_owned(), link); }
}

/// The server the window is checked out on (`servers::runtime`): the key the stream's lines and
/// replies it takes now come under.
fn server_of(s: &App) -> Option<ServerId> {
    crate::launch::window_server(s)
}

fn apply(s: &mut App, server: &str, event: LinkEvent, tx: &mut Tx) -> bool {
    let mut changed = false;
    for effect in s.server_links.step(server, event) {
        match effect {
            LinkEffect::WakeAt(at) => tx.wake_after(Duration::from_secs_f64((at - s.clock).max(0.0))),
            LinkEffect::Repaint => {
                tx.instant();
                changed = true;
            }
        }
    }
    changed
}

/// A failure of the link itself, not the server's answer (a 5xx is a busy server, not a drop).
pub fn link_error(e: &str) -> bool {
    !e.contains("http status") && (opencode_kit::plain_error::is_transient(e) || opencode_kit::plain_error::failed_for_sure(e))
}

/// Notes a worker line (`Incoming::Status`). True when it was the event stream's own
/// (`"events: …"`), which it claims; another call that failed on the link only marks it suspect.
pub fn dropped(s: &mut App, e: &str, tx: &mut Tx) -> bool {
    let Some(server) = server_of(s) else { return false };
    let now = s.clock;
    if e.starts_with("events:") {
        apply(s, &server, LinkEvent::StreamDropped { now }, tx);
        s.status = e.to_owned();
        return true;
    }
    if link_error(e) {
        apply(s, &server, LinkEvent::CallFailed { now }, tx);
    }
    false
}

/// The stream is back.
pub fn back(s: &mut App) {
    let Some(server) = server_of(s) else { return };
    apply(s, &server, LinkEvent::StreamBack, &mut Tx::default());
}

/// The server answered something: a suspect link is fine. True when the header changed.
pub fn heard(s: &mut App) -> bool {
    let Some(server) = server_of(s) else { return false };
    apply(s, &server, LinkEvent::Heard, &mut Tx::default())
}

/// The window's server's link is down or in doubt.
pub fn down(s: &App) -> bool { s.server_links.get(crate::servers::runtime::active(s).as_str()).down() }

/// The header says Reconnecting for the window's server.
pub fn shown(s: &App) -> bool { s.server_links.get(crate::servers::runtime::active(s).as_str()).shown() }

/// Marks the header once trouble has lasted [`QUIET`], for every server in trouble.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let now = s.clock;
    let troubled: Vec<ServerId> = s.server_links.troubled().cloned().collect();
    troubled.iter().fold(false, |changed, server| apply(s, server, LinkEvent::Tick { now }, tx) | changed)
}

/// After a reconnect, reads again what the lost link may have hidden: the pages of the open
/// session's subagents that were running, so their rows say where they are now, not where they
/// were (or "Starting").
pub fn catch_up(s: &mut App) {
    let Some(server) = server_of(s) else { return };
    s.server_links.catch_up.insert(server.clone(), CatchUp::AwaitingActive);
    s.server_links.refreshing.remove(&server);
    let Route::Session { id: open, .. } = &s.route else { return };
    let root = s.root_of(open);
    let mut kids: Vec<String> = s.running.iter().filter(|id| id.as_str() != root && s.descends(id, root)).cloned().collect();
    for id in [root, open.as_str()] {
        for m in s.transcripts.get(id).map_or(&[][..], |t| &t.messages[..]) {
            let Message::Assistant(a) = m else { continue };
            for p in &a.parts {
                if let Part::Tool { name, status: ToolStatus::Running | ToolStatus::Pending, input, metadata, .. } = p
                    && matches!(name.as_str(), "task" | "subagent")
                    && let Some(child) = model::subagent_session(input, metadata)
                {
                    kids.push(child.to_owned());
                }
            }
        }
    }
    for id in kids {
        refresh(s, &server, id);
    }
}

fn refresh(s: &mut App, server: &str, id: String) {
    let Source::Live { requests: Some(r), .. } = &s.source else { return };
    if s.server_links.refreshing.entry(server.to_owned()).or_default().insert(id.clone()) {
        let _ = r.send(Request::Session(id));
    }
}

/// The server's running sessions: families we don't know are looked up, and right after a
/// reconnect each running subagent's page is read too.
pub fn active(s: &mut App, ids: &[String]) {
    let Some(server) = server_of(s) else { return };
    let catching_up = s.server_links.catch_up.remove(&server) == Some(CatchUp::AwaitingActive);
    let mut missing: Vec<String> = ids.iter().filter_map(|id| s.missing_ancestor(id)).collect();
    missing.sort();
    missing.dedup();
    for id in missing {
        s.fetch_info(&id);
    }
    if catching_up {
        let kids: Vec<String> = ids.iter().filter(|id| s.session(id).is_none_or(|i| i.parent.is_some())).cloned().collect();
        for id in kids {
            refresh(s, &server, id);
        }
    }
}

/// A page landed: a subagent read again after a reconnect shows its latest step from it.
pub fn page_in(s: &mut App, id: &str) {
    let Some(server) = server_of(s) else { return };
    if s.server_links.refreshing.get_mut(&server).is_some_and(|ids| ids.remove(id)) {
        s.forget_agent_steps(id);
    }
}

mod story {
    use crate::App;
    use crate::stories::{Story, Trigger, replay};

    use super::{Link, LinkEvent, Notice};

    /// The open session, its stream down long enough to say so.
    fn reconnecting(s: &mut App) {
        crate::open_scene(s);
        s.finish_stream();
        let server = super::server_of(s).unwrap_or_default();
        s.server_links.step(&server, LinkEvent::StreamDropped { now: s.clock - super::QUIET });
        s.server_links.step(&server, LinkEvent::Tick { now: s.clock });
        debug_assert_eq!(s.server_links.get(&server), Link::Down { since: s.clock - super::QUIET, notice: Notice::Shown });
    }

    const KEYS: &[Trigger] = &[
        replay(|s, _| {
            let server = super::server_of(s).unwrap_or_default();
            s.server_links.step(&server, LinkEvent::StreamDropped { now: s.clock });
        }),
        Trigger { key: 'b', label: "Stream back", run: |s, _| super::back(s) },
    ];

    rune_register::register!(crate::stories::STORY_LIST: Story = [Story::session("Session/Reconnecting", KEYS)]);
    rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("reconnecting", reconnecting)]);
}

#[cfg(test)]
#[path = "reconnect_tests.rs"]
mod tests;
