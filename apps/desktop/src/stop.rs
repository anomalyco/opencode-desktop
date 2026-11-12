//! Stop lands on the press. In the same frame the open session leaves `running`
//! and its unfinished steps stop counting as busy, so Stop turns back into Send, Working goes
//! and the composer takes the next prompt; then the server is asked to interrupt it, with
//! `resume`, as Desktop's composer asks (`session/composer/adapter.ts:56-60`; `composer/machine.ts`
//! `stop` → `interrupt`), so steers already waiting run next. Its execution end, already-idle
//! reply or idle snapshot confirms the stop. An interrupt that fails, or that the server hasn't
//! confirmed within [`CONFIRM_SECS`], puts the run back and says so. Each stop has its attempt,
//! which its interrupt's answer carries back (`Ticket`), so the answer to an earlier stop never
//! touches a later one.

use std::collections::HashMap;

use opencode_kit::client::{Request, Ticket, TicketKind, Write};
use opencode_kit::model::Message;
use rune::machine::{Attempt, Effects, Seq};
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
    seq: Seq,
}

/// One session's stop.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stop {
    /// Asked at `at` (wall milliseconds, `App::now_ms`, the same in every window), until the
    /// server confirms within `CONFIRM_SECS`; `attempt` is its interrupt's.
    Asked { at: i64, attempt: Attempt },
    /// The server confirmed; it holds while the run's last step is still open.
    Confirmed,
}

/// What happens to a session's stop.
enum Event {
    /// Stop pressed, at `now` (wall milliseconds).
    Pressed { now: i64 },
    /// The run ended, or the server said it was already idle.
    Confirmed,
    /// Its interrupt failed (`attempt`'s answer).
    Failed { attempt: Attempt },
    /// `now` is past its confirmation window.
    Due { now: i64 },
    /// A run started, or a prompt went to it.
    Forgotten,
    /// Its last step closed after the confirmation.
    Closed,
}

#[derive(Debug, PartialEq)]
enum Effect {
    /// Send the interrupt under this attempt.
    Interrupt(Attempt),
    /// The stop is undone: the run shows again; `failed` says the interrupt failed rather than
    /// went unconfirmed.
    Resume { failed: bool },
}

/// One session's stop machine: `None` is not stopping.
fn step(stop: &mut Option<Stop>, seq: &mut Seq, event: Event) -> Effects<Effect> {
    match (*stop, event) {
        (None, Event::Pressed { now }) => {
            let attempt = seq.next();
            *stop = Some(Stop::Asked { at: now, attempt });
            Effects::one(Effect::Interrupt(attempt))
        }
        (Some(Stop::Asked { .. }), Event::Confirmed) => {
            *stop = Some(Stop::Confirmed);
            Effects::none()
        }
        (Some(Stop::Asked { attempt, .. }), Event::Failed { attempt: a }) if attempt == a => {
            *stop = None;
            Effects::one(Effect::Resume { failed: true })
        }
        (Some(Stop::Asked { at, .. }), Event::Due { now }) if (now - at) as f64 / 1000.0 >= CONFIRM_SECS => {
            *stop = None;
            Effects::one(Effect::Resume { failed: false })
        }
        (Some(_), Event::Forgotten) | (Some(Stop::Confirmed), Event::Closed) => {
            *stop = None;
            Effects::none()
        }
        _ => Effects::none(),
    }
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

    fn step(&mut self, sid: &str, event: Event) -> Effects<Effect> {
        let mut stop = self.by.get(sid).copied();
        let effects = step(&mut stop, &mut self.seq, event);
        match stop {
            Some(stop) => self.by.insert(sid.to_owned(), stop),
            None => self.by.remove(sid),
        };
        effects
    }

    /// A run started in `sid`, or a prompt went to it: it's no longer being stopped.
    pub fn forget(&mut self, sid: &str) {
        self.step(sid, Event::Forgotten);
    }

    /// A fresh active list also confirms sessions that ended while the event stream was away.
    pub fn reconcile(&mut self, active: &[String]) {
        let ended: Vec<String> = self.by.keys().filter(|id| !active.contains(id)).cloned().collect();
        for id in ended {
            self.step(&id, Event::Confirmed);
        }
    }
}

impl App {
    /// Stops only the open session, as Desktop does. The server cancels its blocking work;
    /// detached background helpers keep running and can be stopped from their own pages.
    pub fn stop(&mut self) {
        let Route::Session { id: sid, .. } = (*self.route).clone() else { return };
        if !self.running.contains(&sid) || self.stops.has(&sid) {
            return;
        }
        // The fixture and the storybook answer as a server would, a little later.
        let fixture = self.stage.is_some() || matches!(*self.source, Source::Fixture);
        let now = *self.now_ms;
        for effect in self.stops.step(&sid, Event::Pressed { now: now }) {
            let Effect::Interrupt(attempt) = effect else { continue };
            let asked = if fixture {
                self.fixture_interrupt(&sid);
                true
            } else {
                let ticket = Ticket { kind: TicketKind::Stop, session: sid.clone(), attempt: attempt.0 };
                self.write(Write::Tracked { ticket, write: Box::new(Write::Interrupt(sid.clone())) })
            };
            if asked {
                self.running.remove(&sid);
            } else {
                self.stops.step(&sid, Event::Forgotten);
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
        let on_the_way = *self.clock + ON_THE_WAY_SECS;
        self.playback.retain(|(at, _, d)| *at <= on_the_way || d["sessionID"].as_str() != Some(sid));
        let at = *self.clock + FIXTURE_SECS;
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
    let Route::Session { id, .. } = &*s.route else { return false };
    s.composer.focused && (s.busy(id) || s.stopping(id))
}

/// Esc or ⌃G in the composer while the open session runs, as Desktop's: stops it. Again while
/// that stop settles, it does nothing more (Esc doesn't fall through and leave the composer).
pub fn key(s: &mut App, ev: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
    let Route::Session { id, .. } = (*s.route).clone() else { return false };
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
    s.stops.step(sid, Event::Confirmed);
}

/// Stop `attempt`'s interrupt failed (`Incoming::Settled`): when it's still the session's stop,
/// the run shows again and the toast says why.
pub fn failed(s: &mut App, sid: &str, attempt: u32, why: &str) {
    for effect in s.stops.step(sid, Event::Failed { attempt: Attempt(attempt) }) {
        if effect == (Effect::Resume { failed: true }) {
            s.running.insert(sid.to_owned());
            crate::windows::peer::touch(s);
            let why = why.split_once(": ").map_or(why, |(_, why)| why);
            crate::status_toast::fail(s, opencode_i18n::t!("rune.stop.failed"), why);
        }
    }
}

/// Each frame: a stop the server hasn't confirmed in time shows the run again and re-reads
/// what's running; a confirmed one ends once its last step closes. True when a run came back.
pub fn poll(s: &mut App, _clock: f64, tx: &mut rune::Tx) -> bool {
    if s.stops.by.is_empty() {
        return false;
    }
    let now = *s.now_ms;
    let ids: Vec<String> = s.stops.by.keys().cloned().collect();
    let mut late = false;
    for sid in ids {
        for effect in s.stops.step(&sid, Event::Due { now: now }) {
            if effect == (Effect::Resume { failed: false }) {
                late = true;
                s.running.insert(sid.clone());
                let why = if crate::reconnect::down(s) { opencode_i18n::t!("rune.stop.unreachable") } else { opencode_i18n::t!("rune.stop.unconfirmed") };
                crate::status_toast::fail(s, why, "");
            }
        }
        if !s.transcripts.get(&sid).is_some_and(|t| t.busy()) {
            s.stops.step(&sid, Event::Closed);
        }
    }
    if late {
        if let Source::Live { requests: Some(r), .. } = &*s.source {
            let _ = r.send(Request::Active);
        }
        crate::windows::peer::touch(s);
    }
    let left = |st: &Stop| match *st { Stop::Asked { at, .. } => Some((at - now) as f64 / 1000.0 + CONFIRM_SECS), Stop::Confirmed => None };
    if let Some(next) = s.stops.by.values().filter_map(left).reduce(f64::min) {
        tx.wake_after(std::time::Duration::from_secs_f64(next.max(0.0)));
    }
    late
}

