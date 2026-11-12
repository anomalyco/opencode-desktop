//! A lost link after the app has connected: the event stream dropped, went silent past
//! [`STREAM_IDLE`], or a read or write failed on the way (a timeout, a refusal). Everything stays
//! on screen; once it has lasted [`QUIET`] a quiet session header label says so, and it leaves
//! the moment the server answers again (`Incoming::Reconnected` also resyncs). The stream thread
//! in `opencode_kit::client` retries on its own with backoff.
//!
//! One [`Link`] per server (`opencode_kit::servers::link`), many at once (`Links`), as OpenCode
//! Desktop keeps one event stream and one sync state per server
//! (`packages/app/src/runtime/server/server-sync/machine.ts`: `idle | disconnected |
//! awaiting-handshake | live`, and `packages/client/src/solid/connection.ts`). What the stream,
//! the other calls, the launch, the health probe and the OS say steps it; the header reads it.
//! Desktop shows nothing while a server is away; the header label is Kit's (`motion_gate` ALLOW).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use opencode_kit::client::Request;
use opencode_kit::model::{self, Message, Part, ToolStatus};
use opencode_kit::servers::link::{Event, Link};
use rune::Tx;

use crate::{App, Route, Source};

/// A short drop says nothing; the stream thread already retries after 250 ms.
pub const QUIET: f64 = opencode_kit::servers::link::QUIET;
pub const RECONNECTING: &str = "Reconnecting…";
/// The event stream is reopened after this long with nothing on it. The server sends a
/// heartbeat every 15 s, so a tunnel that accepts but never answers is caught here.
pub const STREAM_IDLE: Duration = Duration::from_secs(25);

/// A server's key (`opencode_kit::servers::ServerKey`'s text: `"sidecar"`, an address, `wsl:…`).
pub type ServerId = String;

/// After a reconnect, the subagents read again: their step logs give way to the fresh page when
/// it lands, and the next list of running sessions may name subagents that started meanwhile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CatchUp {
    #[default]
    Idle,
    /// The next list of running sessions is read for subagents that started while away.
    AwaitingActive,
}

/// Every server's link (`opencode_kit::servers::link`), many at once, and the catch-up after a
/// reconnect.
#[derive(Default)]
pub struct Links {
    /// Each server's link that isn't online; one not listed is.
    states: HashMap<ServerId, Link>,
    /// What the launch and the health probe last said of each server, as the link last heard it.
    heard: HashMap<ServerId, Probe>,
    catch_up: HashMap<ServerId, CatchUp>,
    /// Subagents whose pages were asked for again after a reconnect, by server.
    refreshing: HashMap<ServerId, HashSet<String>>,
}

impl Links {
    /// Server `server`'s link as the header tells it.
    pub fn state(&self, server: &str) -> Link {
        self.states.get(server).cloned().unwrap_or(Link::Online)
    }

    /// A server that leaves this window forgets its link.
    pub fn forget(&mut self, server: &str) {
        self.states.remove(server);
        self.heard.remove(server);
        self.catch_up.remove(server);
        self.refreshing.remove(server);
    }
}

/// The server the window is checked out on (`servers::runtime`): the key the stream's lines and
/// replies it takes now come under.
fn server_of(s: &App) -> Option<ServerId> {
    crate::launch::window_server(s)
}

/// Steps server `server`'s link (`opencode_kit::servers::link`) and carries out what it asks.
pub fn step_link(s: &mut App, server: &str, event: opencode_kit::servers::link::Event, tx: &mut Tx) -> bool {
    use opencode_kit::servers::link::Effect;
    let mut state = s.server_links.state(server);
    let was = state.dropped();
    let effects = state.step(event);
    let dropped = state.dropped();
    if state == Link::Online {
        s.server_links.states.remove(server);
    } else {
        s.server_links.states.insert(server.to_owned(), state);
    }
    // The stream dropping or reopening is the first word that the server went or came back.
    if was != dropped {
        crate::servers::health::nudge(s, &opencode_kit::servers::ServerKey::new(server), tx);
    }
    let mut changed = false;
    for effect in effects {
        match effect {
            Effect::Repaint => {
                tx.instant();
                changed = true;
            }
            Effect::WakeAt(at) => tx.wake_after(Duration::from_secs_f64((at - *s.clock).max(0.0))),
            Effect::Reconnect => {
                opencode_kit::client::Server::wake_reconnects();
                crate::servers::health::nudge(s, &opencode_kit::servers::ServerKey::new(server), tx);
            }
        }
    }
    changed
}

/// Server `server`'s link as the header tells it.
pub fn link_of(s: &App, server: &str) -> Link {
    s.server_links.state(server)
}

/// An `events: <why>` line's typed error, when the client wrote one.
fn stream_error(e: &str) -> Option<opencode_kit::wire_error::WireError> {
    opencode_kit::wire_error::WireError::parse(&format!("GET /api/event: {}", e.strip_prefix("events: ")?))
}

/// A failure of the link itself, not the server's answer (a 5xx is a busy server, not a drop).
pub fn link_error(e: &str) -> bool {
    !e.contains("http status") && (opencode_kit::plain_error::is_transient(e) || opencode_kit::plain_error::failed_for_sure(e))
}

/// Notes a worker line (`Incoming::Status`). True when it was the event stream's own
/// (`"events: …"`), which it claims; another call that failed on the link only marks it suspect.
pub fn dropped(s: &mut App, e: &str, tx: &mut Tx) -> bool {
    let Some(server) = server_of(s) else { return false };
    let now = *s.clock;
    if e.starts_with("events:") {
        // What was mid-text when it dropped grows again only from whole text (the barrier).
        for (sid, t) in s.transcripts.iter() {
            s.stream_gaps.entry(sid.clone()).or_default().extend(t.open_steps());
        }
        step_link(s, &server, Event::StreamDown { now, error: stream_error(e) }, tx);
        *s.status = e.to_owned();
        return true;
    }
    if link_error(e) {
        step_link(s, &server, Event::CallFailed { now }, tx);
    }
    false
}

/// The stream is back.
pub fn back(s: &mut App) {
    let Some(server) = server_of(s) else { return };
    step_link(s, &server, Event::StreamUp, &mut Tx::default());
}

/// The server answered something: a suspect link is fine. True when the header changed.
pub fn heard(s: &mut App) -> bool {
    let Some(server) = server_of(s) else { return false };
    if !matches!(s.server_links.state(&server), Link::Suspect { .. }) {
        return false;
    }
    step_link(s, &server, Event::Heard, &mut Tx::default())
}

/// The window's server's link is anything but online: nothing should wait on it to answer.
pub fn down(s: &App) -> bool {
    link_of(s, crate::servers::runtime::active(s).as_str()) != Link::Online
}

/// Marks the header once trouble has lasted [`QUIET`], for every server whose link waits on
/// the clock; and steps the window's server's link with what its launch and health probe say
/// when that changes.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let now = *s.clock;
    let mut changed = probed(s, tx);
    let ticking: Vec<ServerId> = s.server_links.states.iter().filter(|(_, l)| l.ticks()).map(|(k, _)| k.clone()).collect();
    for server in ticking {
        changed |= step_link(s, &server, Event::Tick { now }, tx);
    }
    changed
}

/// What the launch and the health probe say of a server, for its link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe {
    Unknown,
    Answering,
    SignedOut,
    Incompatible,
    Unreachable,
}

/// The window's server's launch and health answers, into its link when they change. A probe
/// that finds nothing doesn't outvote a stream that's up.
fn probed(s: &mut App, tx: &mut Tx) -> bool {
    use crate::view::server_unavailable::{self as unavailable, Cover};
    let key = crate::servers::runtime::active(s).clone();
    let probe = match unavailable::cover(&key) {
        Some(Cover::SignedOut) => Probe::SignedOut,
        Some(Cover::Incompatible) => Probe::Incompatible,
        None if crate::launch::offline(s) || unavailable::unreachable(&key) => Probe::Unreachable,
        None if crate::servers::health::get(&key).is_some_and(|h| opencode_kit::servers::health::is_healthy(Some(&h))) => Probe::Answering,
        None => Probe::Unknown,
    };
    let server = key.as_str().to_owned();
    if s.server_links.heard.get(&server).copied().unwrap_or(Probe::Unknown) == probe {
        return false;
    }
    let now = *s.clock;
    let event = match probe {
        Probe::SignedOut => Event::Unauthorized,
        Probe::Incompatible => Event::Healthy { compatible: false },
        Probe::Answering => Event::Healthy { compatible: true },
        // Heard only once the stream is down too, so asked again each poll until then.
        Probe::Unreachable if matches!(link_of(s, &server), Link::Online | Link::Suspect { .. }) => return false,
        Probe::Unreachable => Event::Unreachable { now },
        Probe::Unknown => {
            s.server_links.heard.insert(server, probe);
            return false;
        }
    };
    s.server_links.heard.insert(server.clone(), probe);
    step_link(s, &server, event, tx)
}

/// `--stream-audit <file>` (the resilience harness's stream cuts): each poll where the open
/// session's newest reply shows other text than the last, one JSON line with it, so the harness
/// can check every text shown was a start of the final one.
pub fn audit(s: &App) {
    use std::io::Write;
    static FILE: std::sync::OnceLock<Option<std::sync::Mutex<(std::fs::File, String)>>> = std::sync::OnceLock::new();
    let Some(file) = FILE.get_or_init(|| crate::arg("--stream-audit").and_then(|p| std::fs::File::create(p).ok()).map(|f| std::sync::Mutex::new((f, String::new())))) else { return };
    let Route::Session { id, .. } = &*s.route else { return };
    let Some(Message::Assistant(a)) = s.transcripts.get(id).and_then(|t| t.messages.iter().rev().find(|m| matches!(m, Message::Assistant(_)))) else { return };
    let text: String = a.parts.iter().filter_map(|p| if let Part::Text { text } = p { Some(text.as_str()) } else { None }).collect();
    let mut f = file.lock().unwrap_or_else(|e| e.into_inner());
    if f.1 == text {
        return;
    }
    let line = serde_json::json!({ "t": *s.clock, "msg": a.id, "done": a.completed.is_some(), "text": text });
    let _ = writeln!(f.0, "{line}");
    f.1 = text;
}

/// After a reconnect, reads again what the lost link may have hidden: the pages of the open
/// session's subagents that were running, so their rows say where they are now, not where they
/// were (or "Starting").
pub fn catch_up(s: &mut App) {
    let Some(server) = server_of(s) else { return };
    s.server_links.catch_up.insert(server.clone(), CatchUp::AwaitingActive);
    s.server_links.refreshing.remove(&server);
    let Route::Session { id: open, .. } = &*s.route else { return };
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
    let Source::Live { requests: Some(r), .. } = &*s.source else { return };
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

    use super::{Event, Link, step_link};

    /// The open session, its stream down long enough to say so.
    fn reconnecting(s: &mut App) {
        crate::open_scene(s);
        s.finish_stream();
        let server = super::server_of(s).unwrap_or_default();
        let now = *s.clock;
        step_link(s, &server, Event::StreamDown { now: now - super::QUIET, error: None }, &mut rune::Tx::default());
        step_link(s, &server, Event::Tick { now }, &mut rune::Tx::default());
        debug_assert_eq!(s.server_links.state(&server), Link::Reopening { since: now - super::QUIET, shown: true });
    }

    const KEYS: &[Trigger] = &[
        replay(|s, _| {
            let server = super::server_of(s).unwrap_or_default();
            let now = *s.clock;
            step_link(s, &server, Event::StreamDown { now, error: None }, &mut rune::Tx::default());
        }),
        Trigger { key: 'b', label: "Stream back", run: |s, _| super::back(s) },
    ];

    rune_register::register!(crate::stories::STORY_LIST: Story = [Story::session("Session/Reconnecting", KEYS)]);
    rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("reconnecting", reconnecting)]);
}

