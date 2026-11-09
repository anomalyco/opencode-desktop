//! Stop lands on the press. In the same frame the open session leaves `running`
//! and its unfinished steps stop counting as busy, so Stop turns back into Send, Working goes
//! and the composer takes the next prompt; then the server is asked to interrupt it. Its
//! execution end, already-idle reply or idle snapshot confirms the stop. An interrupt that fails,
//! or that the server hasn't confirmed within [`CONFIRM_SECS`], puts the run back and says so.

use std::collections::HashMap;

use opencode_kit::client::{Request, Write};
use opencode_kit::model::Message;
use serde_json::json;

use crate::{App, Route, Source};

/// How long the server has to confirm a stop before the run shows again.
pub const CONFIRM_SECS: f64 = 5.0;
/// How long the fixture's server takes to confirm one.
pub const FIXTURE_SECS: f64 = 1.2;
/// How much of the fixture's stream was already on its way when the stop went out.
const ON_THE_WAY_SECS: f64 = 0.3;

/// The sessions being stopped, in the server's copy (`windows::peer`): a stop one window
/// pressed holds in all of them.
#[derive(Default)]
pub struct Stops {
    /// Sessions stopped, until the server confirms and their last step closes.
    by: HashMap<String, Stop>,
}

/// One session's stop.
#[derive(Clone, Copy, PartialEq)]
enum Stop {
    /// Asked at `at` (wall milliseconds, `App::now_ms`, the same in every window), until the
    /// server confirms within `CONFIRM_SECS`.
    Asked { at: i64 },
    /// The server confirmed; it holds while the run's last step is still open.
    Confirmed,
}

impl Stops {
    /// Stopped, and its last step may still be open: it doesn't count as busy.
    pub fn has(&self, sid: &str) -> bool {
        self.by.contains_key(sid)
    }

    /// Stopped and not confirmed yet: a resync mustn't bring it back.
    pub fn pending(&self, sid: &str) -> bool {
        matches!(self.by.get(sid), Some(Stop::Asked { .. }))
    }

    /// A run started in `sid`, or a prompt went to it: it's no longer being stopped.
    pub fn forget(&mut self, sid: &str) {
        self.by.remove(sid);
    }

    /// A fresh active list also confirms sessions that ended while the event stream was away.
    pub fn reconcile(&mut self, active: &[String]) {
        for (id, stop) in &mut self.by {
            if !active.contains(id) {
                *stop = Stop::Confirmed;
            }
        }
    }
}

impl App {
    /// Stops only the open session, as Desktop does. The server cancels its blocking work;
    /// detached background helpers keep running and can be stopped from their own pages.
    pub fn stop(&mut self) {
        let Route::Session(sid) = self.route.clone() else { return };
        if !self.running.contains(&sid) {
            return;
        }
        // The fixture and the storybook answer as a server would, a little later.
        let fixture = self.stage.is_some() || matches!(self.source, Source::Fixture);
        let asked = if fixture {
            self.fixture_interrupt(&sid);
            true
        } else {
            self.write(Write::Interrupt(sid.clone()))
        };
        if asked {
            self.running.remove(&sid);
            self.stops.by.insert(sid, Stop::Asked { at: self.now_ms });
        }
        if fixture {
            self.wake_at_playback();
        }
    }

    /// The fixture's server interrupting `sid`, as a live one does: what's already on its way
    /// lands, the rest of the run never comes, then its open step fails as interrupted and the
    /// run ends.
    fn fixture_interrupt(&mut self, sid: &str) {
        let on_the_way = self.clock + ON_THE_WAY_SECS;
        self.playback.retain(|(at, _, d)| *at <= on_the_way || d["sessionID"].as_str() != Some(sid));
        let at = self.clock + FIXTURE_SECS;
        let open = self.transcripts.get(sid).and_then(|t| match t.messages.last() {
            Some(Message::Assistant(a)) if a.completed.is_none() => Some(a.id.clone()),
            _ => None,
        });
        if let Some(step) = open {
            let error = json!({ "type": "aborted", "message": "Step interrupted" });
            self.schedule(at, "session.step.failed", json!({ "sessionID": sid, "assistantMessageID": step, "error": error }));
        }
        self.schedule(at, "session.execution.interrupted", json!({ "sessionID": sid, "reason": "user" }));
    }

    /// Whether this session's stop is still waiting on the server.
    pub fn stopping(&self, id: &str) -> bool {
        self.stops.pending(id)
    }
}

/// Whether Escape stops the open session's run: the composer has focus while it runs or its stop
/// settles (the escape stack's `Run` layer).
pub fn escape_stops(s: &App) -> bool {
    let Route::Session(id) = &s.route else { return false };
    s.composer.focused && (s.busy(id) || s.stopping(id))
}

/// Esc or ⌃G in the composer while the open session runs, as Desktop's: stops it. Again while
/// that stop settles, it does nothing more (Esc doesn't fall through and leave the composer).
pub fn key(s: &mut App, ev: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
    let Route::Session(id) = s.route.clone() else { return false };
    if !session_view::composer::is_stop_shortcut(ev) || !s.composer.focused {
        return false;
    }
    if s.busy(&id) {
        press(s, tx);
        return true;
    }
    s.stopping(&id)
}

/// A press of Stop: everything it changes starts in this frame. Stop's square shrinks into
/// Send's arrow, Working goes, and Interrupted rises into its place ([`interrupted`]).
pub fn press(s: &mut App, _tx: &mut rune::Tx) {
    s.stop();
}

/// The server ended `sid`'s run.
pub fn confirmed(s: &mut App, sid: &str) {
    if let Some(st) = s.stops.by.get_mut(sid) {
        *st = Stop::Confirmed;
    }
}

/// A write's error: when it's an interrupt's, the run shows again and the toast says why. True
/// when it was one.
pub fn failed(s: &mut App, error: &str) -> bool {
    let (path, why) = error.split_once(": ").unwrap_or((error, ""));
    let Some(sid) = path.strip_prefix("POST /api/session/").and_then(|p| p.strip_suffix("/interrupt")) else { return false };
    if s.stops.pending(sid) {
        s.stops.by.remove(sid);
        s.running.insert(sid.to_owned());
        crate::status_toast::fail(s, opencode_i18n::t!("rune.stop.failed"), why);
    }
    true
}

/// Each frame: a stop the server hasn't confirmed in time shows the run again and re-reads
/// what's running; a confirmed one ends once its last step closes. True when a run came back.
pub fn poll(s: &mut App, _clock: f64, tx: &mut rune::Tx) -> bool {
    if s.stops.by.is_empty() {
        return false;
    }
    let now = s.now_ms;
    let left = |st: &Stop| match *st { Stop::Asked { at } => Some((at - now) as f64 / 1000.0 + CONFIRM_SECS), Stop::Confirmed => None };
    let late: Vec<String> = s.stops.by.iter().filter(|(_, st)| left(st).is_some_and(|l| l <= 0.0)).map(|(id, _)| id.clone()).collect();
    for sid in &late {
        s.stops.by.remove(sid);
        s.running.insert(sid.clone());
        let why = if crate::reconnect::down(s) { opencode_i18n::t!("rune.stop.unreachable") } else { opencode_i18n::t!("rune.stop.unconfirmed") };
        crate::status_toast::fail(s, why, "");
    }
    if !late.is_empty()
        && let Source::Live { requests: Some(r), .. } = &s.source
    {
        let _ = r.send(Request::Active);
    }
    if !late.is_empty() {
        crate::windows::peer::touch(s);
    }
    let App { stops, transcripts, .. } = s;
    stops.by.retain(|id, st| *st != Stop::Confirmed || transcripts.get(id).is_some_and(|t| t.busy()));
    if let Some(next) = stops.by.values().filter_map(left).reduce(f64::min) {
        tx.wake_after(std::time::Duration::from_secs_f64(next.max(0.0)));
    }
    !late.is_empty()
}

#[cfg(test)]
#[path = "stop_tests.rs"]
mod tests;
