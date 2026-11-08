//! A lost link after the app has connected: the event stream dropped, went silent past
//! [`STREAM_IDLE`], or a read or write failed on the way (a timeout, a refusal). Everything stays
//! on screen; once it has lasted [`QUIET`] a calm "Reconnecting…" toast says so, and it leaves
//! the moment the server answers again (`Incoming::Reconnected` also resyncs). The stream thread
//! in `opencode_kit::client` retries on its own with backoff.

use std::collections::HashSet;
use std::time::Duration;

use opencode_kit::client::Request;
use opencode_kit::model::{self, Message, Part, ToolStatus};
use rune::Tx;

use crate::{App, Route, Source};

/// A drop shorter than this says nothing: the stream thread's first retry is 1 s away.
pub const QUIET: f64 = 2.0;
pub const RECONNECTING: &str = "Reconnecting…";
/// The event stream is reopened after this long with nothing on it. The server sends a
/// heartbeat every 15 s, so a tunnel that accepts but never answers is caught here.
pub const STREAM_IDLE: Duration = Duration::from_secs(25);

#[derive(Default)]
pub struct Link {
    /// When the event stream was seen to drop (`App::clock`), while it is down.
    down_at: Option<f64>,
    /// When a read or write failed for want of a link, while the stream hasn't said either way.
    suspect_at: Option<f64>,
    /// The "Reconnecting…" toast, while it shows.
    toast: Option<u64>,
    /// Subagents whose pages were asked for again after a reconnect: their step logs give way to
    /// the fresh page when it lands.
    refreshing: HashSet<String>,
    /// Set by a reconnect until the next list of running sessions, which may name subagents that
    /// started while the link was down.
    catching_up: bool,
}

/// A failure of the link itself, not the server's answer (a 5xx is a busy server, not a drop).
pub fn link_error(e: &str) -> bool {
    !e.contains("http status") && (opencode_kit::plain_error::is_transient(e) || opencode_kit::plain_error::failed_for_sure(e))
}

/// Notes a worker line (`Incoming::Status`). True when it was the event stream's own
/// (`"events: …"`), which it claims; another call that failed on the link only marks it suspect.
pub fn dropped(s: &mut App, e: &str, tx: &mut Tx) -> bool {
    let link = &mut s.launch.link;
    if e.starts_with("events:") {
        if link.down_at.is_none() {
            link.down_at = Some(s.clock);
            tx.wake_after(Duration::from_secs_f64(QUIET));
        }
        s.status = e.to_owned();
        return true;
    }
    if link_error(e) && link.down_at.is_none() && link.suspect_at.is_none() {
        link.suspect_at = Some(s.clock);
        tx.wake_after(Duration::from_secs_f64(QUIET));
    }
    false
}

/// The stream is back.
pub fn back(s: &mut App) {
    s.launch.link.down_at = None;
    heard(s);
}

/// The server answered something: a suspect link is fine.
pub fn heard(s: &mut App) {
    let link = &mut s.launch.link;
    link.suspect_at = None;
    if link.down_at.is_none()
        && let Some(id) = link.toast.take()
    {
        crate::status_toast::dismiss_id(s, id);
    }
}

/// The link is down or in doubt.
pub fn down(s: &App) -> bool {
    s.launch.link.down_at.is_some() || s.launch.link.suspect_at.is_some()
}

/// Shows the toast once the trouble has lasted [`QUIET`]. A suspect link whose stream outlives
/// [`STREAM_IDLE`] without dropping is alive after all.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let link = &mut s.launch.link;
    if let (None, Some(at)) = (link.down_at, link.suspect_at)
        && s.clock - at >= STREAM_IDLE.as_secs_f64() + QUIET
    {
        tx.instant();
        heard(s);
        return true;
    }
    let Some(at) = link.down_at.or(link.suspect_at) else { return false };
    if s.unsent.waiting_for_link() {
        // Each affected bubble already states what's waiting. Don't also announce it.
        if let Some(id)=link.toast.take() { crate::status_toast::dismiss_id(s,id);return true; }
        return false;
    }
    if link.toast.is_some() {
        return false;
    }
    let left = at + QUIET - s.clock;
    if left > 1e-3 {
        tx.wake_after(Duration::from_secs_f64(left));
        return false;
    }
    tx.instant();
    s.launch.link.toast = Some(crate::status_toast::waiting(s, RECONNECTING));
    if s.launch.link.down_at.is_none() {
        tx.wake_after(STREAM_IDLE);
    }
    true
}

/// After a reconnect, reads again what the lost link may have hidden: the pages of the open
/// session's subagents that were running, so their rows say where they are now, not where they
/// were (or "Starting").
pub fn catch_up(s: &mut App) {
    s.launch.link.catching_up = true;
    s.launch.link.refreshing.clear();
    let Route::Session(open) = &s.route else { return };
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
        refresh(s, id);
    }
}

fn refresh(s: &mut App, id: String) {
    let Source::Live { requests: Some(r), .. } = &s.source else { return };
    if s.launch.link.refreshing.insert(id.clone()) {
        let _ = r.send(Request::Session(id));
    }
}

/// The server's running sessions: families we don't know are looked up, and right after a
/// reconnect each running subagent's page is read too.
pub fn active(s: &mut App, ids: &[String]) {
    let catching_up = std::mem::take(&mut s.launch.link.catching_up);
    let mut missing: Vec<String> = ids.iter().filter_map(|id| s.missing_ancestor(id)).collect();
    missing.sort();
    missing.dedup();
    for id in missing {
        s.fetch_info(&id);
    }
    if catching_up {
        let kids: Vec<String> = ids.iter().filter(|id| s.session(id).is_none_or(|i| i.parent.is_some())).cloned().collect();
        for id in kids {
            refresh(s, id);
        }
    }
}

/// A page landed: a subagent read again after a reconnect shows its latest step from it.
pub fn page_in(s: &mut App, id: &str) {
    if s.launch.link.refreshing.remove(id) {
        s.forget_agent_steps(id);
    }
}

mod story {
    use crate::App;
    use crate::stories::{Story, Trigger, replay};

    /// The open session, its stream down long enough to say so.
    fn reconnecting(s: &mut App) {
        crate::open_scene(s);
        s.finish_stream();
        s.launch.link.down_at = Some(s.clock - super::QUIET);
    }

    const KEYS: &[Trigger] = &[
        replay(|s, _| s.launch.link.down_at = Some(s.clock)),
        Trigger { key: 'b', label: "Stream back", run: |s, _| super::back(s) },
    ];

    rune_register::register!(crate::stories::STORY_LIST: Story = [Story::session("Session/Reconnecting", KEYS)]);
    rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("reconnecting", reconnecting)]);
}

#[cfg(test)]
#[path = "reconnect_tests.rs"]
mod tests;
