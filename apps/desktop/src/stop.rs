//! Stop lands on the press. In the same frame the family's running sessions leave `running`
//! and their unfinished steps stop counting as busy, so Stop turns back into Send, Working goes
//! and the composer takes the next prompt; then the server is asked to interrupt each. Its
//! `session.execution.*` end confirms the stop. An interrupt that fails, or that the server
//! hasn't confirmed within [`CONFIRM_SECS`], puts the run back and says so.

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

#[derive(Default)]
pub struct Stops {
    /// Sessions stopped and when (app clock), until the server confirms and their last step closes.
    by: HashMap<String, Stop>,
}

struct Stop {
    at: f64,
    confirmed: bool,
}

impl Stops {
    /// Stopped, and its last step may still be open: it doesn't count as busy.
    pub fn has(&self, sid: &str) -> bool {
        self.by.contains_key(sid)
    }

    /// Stopped and not confirmed yet: a resync mustn't bring it back.
    pub fn pending(&self, sid: &str) -> bool {
        self.by.get(sid).is_some_and(|s| !s.confirmed)
    }

    /// A run started in `sid`, or a prompt went to it: it's no longer being stopped.
    pub fn forget(&mut self, sid: &str) {
        self.by.remove(sid);
    }
}

impl App {
    /// Stops every running session in the open session's family.
    pub fn stop(&mut self) {
        let Route::Session(id) = self.route.clone() else { return };
        let root = self.root_of(&id).to_owned();
        let running: Vec<String> = self.running.iter().filter(|s| self.root_of(s) == root).cloned().collect();
        // The fixture and the storybook answer as a server would, a little later.
        let fixture = self.stage.is_some() || matches!(self.source, Source::Fixture);
        for sid in running {
            let asked = if fixture {
                self.fixture_interrupt(&sid);
                true
            } else {
                self.write(Write::Interrupt(sid.clone()))
            };
            if asked {
                self.running.remove(&sid);
                self.stops.by.insert(sid, Stop { at: self.clock, confirmed: false });
            }
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

    /// Whether a stop pressed in `id`'s family is still waiting on the server.
    pub fn stopping_family(&self, id: &str) -> bool {
        let root = self.root_of(id);
        self.stops.by.iter().any(|(s, st)| !st.confirmed && self.root_of(s) == root)
    }
}

/// Esc or ⌃G in the composer while the open session runs, as Desktop's: stops it. Again while
/// that stop settles, it does nothing more (Esc doesn't fall through and leave the composer).
pub fn key(s: &mut App, ev: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
    let m = ev.mods;
    let esc = ev.key == rune::Key::Escape && !m.cmd && !m.ctrl && !m.alt && !m.shift;
    let ctrl_g = matches!(&ev.key, rune::Key::Text(t) if t.eq_ignore_ascii_case("g")) && m.ctrl && !m.cmd && !m.alt && !m.shift;
    let Route::Session(id) = s.route.clone() else { return false };
    if !(esc || ctrl_g) || !s.composer.focused {
        return false;
    }
    if s.busy(&id) {
        press(s, tx);
        return true;
    }
    s.stopping_family(&id)
}

/// A press of Stop: everything it changes lands in this frame, colours too.
pub fn press(s: &mut App, tx: &mut rune::Tx) {
    tx.instant();
    s.stop();
}

/// The server ended `sid`'s run.
pub fn confirmed(s: &mut App, sid: &str) {
    if let Some(st) = s.stops.by.get_mut(sid) {
        st.confirmed = true;
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
    }
    crate::status_toast::fail(s, "Couldn't stop the session", why);
    true
}

/// Each frame: a stop the server hasn't confirmed in time shows the run again and re-reads
/// what's running; a confirmed one ends once its last step closes. True when a run came back.
pub fn poll(s: &mut App, now: f64, tx: &mut rune::Tx) -> bool {
    if s.stops.by.is_empty() {
        return false;
    }
    let late: Vec<String> = s.stops.by.iter().filter(|(_, st)| !st.confirmed && now - st.at >= CONFIRM_SECS).map(|(id, _)| id.clone()).collect();
    for sid in &late {
        s.stops.by.remove(sid);
        s.running.insert(sid.clone());
        crate::status_toast::fail(s, "The server didn't confirm the stop", "");
    }
    if !late.is_empty()
        && let Source::Live { requests: Some(r), .. } = &s.source
    {
        let _ = r.send(Request::Active);
    }
    let App { stops, transcripts, .. } = s;
    stops.by.retain(|id, st| !st.confirmed || transcripts.get(id).is_some_and(|t| t.busy()));
    if let Some(next) = stops.by.values().filter(|st| !st.confirmed).map(|st| st.at + CONFIRM_SECS - now).reduce(f64::min) {
        tx.wake_after(std::time::Duration::from_secs_f64(next.max(0.0)));
    }
    !late.is_empty()
}

#[cfg(test)]
#[path = "stop_tests.rs"]
mod tests;
