//! A stop shows in the frame of the press, whatever the server does next: slow, silent, or
//! refusing the interrupt.
use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::client::{Incoming, Write};
use rune::{KeyEvent, Mods, Runtime};
use serde_json::json;

use super::*;
use rune::Key;

const DT: f64 = 1.0 / 120.0;

struct Live {
    rt: Runtime<App>,
    sid: String,
    writes: Receiver<Write>,
    /// What the server sends back, as the write thread and the event stream would.
    server: Sender<Incoming>,
}

/// The fixture's open session mid-run on a live source: its writes land in a channel and nothing
/// answers them until a test says so.
fn live() -> Live {
    let mut app = App::fixture_session();
    crate::scenes::busy_scene(&mut app);
    let Route::Session { id: sid, .. } = app.route.clone() else { panic!("no open session") };
    app.running.retain(|id| *id == sid);
    let writes = app.fake_writes();
    let (server, inbox) = channel();
    app.inbox = Some(inbox);
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    let mut l = Live { rt, sid, writes, server };
    tap(&mut l, "prompt");
    l.rt.frame(DT);
    assert!(l.rt.state.composer.focused);
    assert_eq!(submit(&l), "Stop", "running, the button stops");
    assert!(working(&l) > 0.999, "and Working shows");
    l.rt.frame(DT);
    l
}

/// A write as the server sees it, its ticket set aside.
fn untracked(w: Write) -> Write {
    match w {
        Write::Tracked { write, .. } => *write,
        w => w,
    }
}

/// The ticket of the session's first stop.
fn stop_ticket(l: &Live) -> opencode_kit::client::Ticket {
    opencode_kit::client::Ticket { kind: opencode_kit::client::TicketKind::Stop, session: l.sid.clone(), attempt: 1 }
}

fn submit(l: &Live) -> String {
    l.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("submit")).and_then(|e| e.label).expect("the submit button")
}

fn working(l: &Live) -> f32 {
    l.rt.elements().into_iter().filter(|e| !e.exiting && e.key.as_deref() == Some("working")).map(|e| e.opacity).fold(0.0, f32::max)
}

fn tap(l: &mut Live, key: &str) {
    let r = l.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).rect;
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    l.rt.pointer_move(x, y);
    l.rt.pointer_down(x, y);
    l.rt.pointer_up(x, y);
}

fn esc(l: &mut Live) {
    l.rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    l.rt.frame(DT);
}

fn play(l: &mut Live, secs: f64) {
    for _ in 0..(secs / DT).round() as usize {
        l.rt.frame(DT);
    }
}

fn event(l: &mut Live, ty: &str, data: serde_json::Value) {
    let mut data = data;
    data["sessionID"] = l.sid.clone().into();
    l.server.send(Incoming::Event { ty: ty.into(), data }).unwrap();
    l.rt.frame(DT);
}

fn shown(l: &Live, key: &str) -> Option<rune::ElementInfo> {
    l.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key))
}

/// Stopped as of this frame: the button sends, the square is on its way out and Interrupted on
/// its way in, and Working goes.
fn stopped(l: &Live) {
    assert_eq!(submit(l), "Send", "Stop turned back into Send");
    assert!(shown(l, "stop-glyph").is_none() && shown(l, "send-glyph").is_some(), "the arrow took the square's place");
    assert!(shown(l, "interrupted").is_some(), "the turn says Interrupted");
    assert!(working(l) < 0.999, "Working is going");
    assert!(!l.rt.state.busy(&l.sid), "Enter sends a new prompt rather than steering");
}

fn running(l: &Live) {
    assert_eq!(submit(l), "Stop");
    assert!(shown(l, "interrupted").is_none());
    assert!(working(l) > 0.999);
}

#[test]
fn the_stop_square_shrinks_into_the_arrow_as_interrupted_rises_into_workings_place() {
    let mut l = live();
    let working_y = l.rt.elements().into_iter().find(|e| e.key.as_deref() == Some("working")).unwrap().rect[1];
    tap(&mut l, "submit");
    l.rt.frame(DT);
    play(&mut l, 0.05);
    let leaving = l.rt.elements().into_iter().find(|e| e.exiting && e.key.as_deref() == Some("stop-glyph")).expect("the square leaves");
    assert!(leaving.scale < 0.95 && leaving.opacity < 0.95, "shrinking as it fades: {} {}", leaving.scale, leaving.opacity);
    let arrow = shown(&l, "send-glyph").unwrap();
    assert!(arrow.scale < 0.95 && arrow.scale >= 0.5, "the arrow grows in: {}", arrow.scale);
    let rising = shown(&l, "interrupted").unwrap();
    assert!(rising.opacity < 0.95, "Interrupted fades in: {}", rising.opacity);
    play(&mut l, 0.4);
    assert!(!l.rt.elements().iter().any(|e| e.key.as_deref() == Some("stop-glyph")), "the square is gone");
    let (arrow, label) = (shown(&l, "send-glyph").unwrap(), shown(&l, "interrupted").unwrap());
    assert!(arrow.scale > 0.999 && label.opacity > 0.999 && working(&l) < 0.001);
    assert!(rising.rect[1] > label.rect[1] + 0.5, "it rose: {} to {}", rising.rect[1], label.rect[1]);
    assert!(label.rect[1] < working_y + 1.0, "it sits beside Copy, a little above Working's line");
    assert!(l.rt.elements().iter().all(|e| e.text.as_deref() != Some("Step interrupted")), "no red error under the turn");
}

#[test]
fn a_stop_shows_in_the_frame_of_the_press_while_the_server_takes_its_time() {
    let mut l = live();
    esc(&mut l);
    stopped(&l);
    assert!(matches!(l.writes.try_recv().map(untracked), Ok(Write::Interrupt(s)) if s == l.sid), "the interrupt went out");

    // Words already on their way still land, and Working stays gone.
    let reply = match l.rt.state.transcripts[&l.sid].messages.last() {
        Some(opencode_kit::model::Message::Assistant(a)) => a.id.clone(),
        _ => panic!("no running step"),
    };
    event(&mut l, "session.text.delta", json!({ "assistantMessageID": reply, "delta": " late words" }));
    play(&mut l, 2.0);
    stopped(&l);

    // A second press asks for nothing more and doesn't fall through to another Esc.
    esc(&mut l);
    assert!(l.writes.try_recv().is_err(), "one interrupt");
    assert!(l.rt.state.composer.focused);

    // A resync that still lists the run doesn't bring it back while the stop is pending.
    l.server.send(Incoming::Active(vec![l.sid.clone()])).unwrap();
    l.rt.frame(DT);
    stopped(&l);

    // The server confirms; the step closes after it.
    event(&mut l, "session.execution.interrupted", json!({ "reason": "user" }));
    stopped(&l);
    event(&mut l, "session.step.failed", json!({ "assistantMessageID": reply, "error": { "type": "aborted", "message": "Step interrupted" } }));
    play(&mut l, CONFIRM_SECS + 1.0);
    stopped(&l);
    assert!(l.rt.state.stops.by.is_empty(), "the stop is over");
    assert!(l.rt.state.status.is_empty() || !l.rt.state.status.contains("stop"), "no toast: {}", l.rt.state.status);
}

#[test]
fn the_composer_stop_button_lands_on_the_press_too() {
    let mut l = live();
    tap(&mut l, "submit");
    l.rt.frame(DT);
    stopped(&l);
    assert!(matches!(l.writes.try_recv().map(untracked), Ok(Write::Interrupt(_))));
    tap(&mut l, "submit");
    l.rt.frame(DT);
    assert!(l.writes.try_recv().is_err(), "a second press sends nothing");
    stopped(&l);
}

#[test]
fn an_interrupt_that_fails_puts_the_run_back_and_says_so() {
    let mut l = live();
    esc(&mut l);
    stopped(&l);
    let error = format!("POST /api/session/{}/interrupt: http status: 500", l.sid);
    l.server.send(Incoming::Settled { ticket: stop_ticket(&l), result: Err(error) }).unwrap();
    l.rt.frame(DT);
    running(&l);
    assert_eq!(l.rt.state.status, "Couldn't stop the session: http status: 500");
    assert!(l.rt.state.toast.current().is_some_and(|t| t.persistent), "a toast that waits for Kit");
    // ⌃G stops it again, as Esc does.
    l.rt.key(KeyEvent { key: Key::Text("g".into()), mods: Mods { ctrl: true, ..Mods::default() } });
    l.rt.frame(DT);
    stopped(&l);
    assert!(l.rt.state.composer.focused);
}

#[test]
fn a_stop_the_server_never_confirms_comes_back_after_a_few_seconds() {
    let mut l = live();
    esc(&mut l);
    play(&mut l, CONFIRM_SECS - 0.1);
    stopped(&l);
    play(&mut l, 0.2);
    running(&l);
    assert_eq!(l.rt.state.status, "The server didn't confirm the stop");
}

#[test]
fn a_stop_lost_on_a_dead_link_says_the_link() {
    let mut l = live();
    l.server.send(Incoming::Status("events: timeout: per-read".into())).unwrap();
    l.rt.frame(DT);
    esc(&mut l);
    play(&mut l, CONFIRM_SECS + 0.2);
    running(&l);
    assert_eq!(l.rt.state.status, "Can't reach OpenCode to stop the session");
}

#[test]
fn a_prompt_sent_while_the_stop_settles_shows_working_for_itself() {
    let mut l = live();
    esc(&mut l);
    stopped(&l);
    l.rt.update(|s, tx| {
        s.composer = rune::TextEdit::new("try the other way");
        s.send(tx);
    });
    l.rt.frame(DT);
    assert!(matches!(l.writes.try_recv().map(untracked), Ok(Write::Interrupt(_))));
    assert!(matches!(l.writes.try_recv(), Ok(Write::Send { .. })), "sent as a new prompt");
    assert!(working(&l) > 0.999, "the new prompt's Working shows");
}

#[test]
fn the_fixture_ends_its_stream_as_a_server_would() {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::scenes::open_scene(&mut app);
    let Route::Session { id: sid, .. } = app.route.clone() else { panic!("no open session") };
    app.start_stream_if_due(&sid);
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    for _ in 0..(2.0 / DT) as usize {
        rt.frame(DT);
    }
    assert!(rt.state.busy(&sid), "streaming");
    rt.update(|s, _| s.stop());
    rt.frame(DT);
    assert!(!rt.state.busy(&sid));
    for _ in 0..(10.0 / DT) as usize {
        rt.frame(DT);
    }
    assert!(!rt.state.busy(&sid), "the rest of the run never came");
    assert!(rt.state.stops.by.is_empty(), "the step closed as interrupted");
    let Some(opencode_kit::model::Message::Assistant(a)) = rt.state.transcripts[&sid].messages.last() else { panic!() };
    assert_eq!(a.error.as_deref(), Some("Step interrupted"));
    assert!(a.failure.as_ref().is_some_and(|f| crate::view::interrupted(&f.kind)), "its tail says Interrupted rather than an error");
}

#[test]
fn idle_parent_with_background_children_shows_send() {
    let mut l = live();
    l.rt.update(|s, _| {
        crate::stop_stories::background(s);
    });
    l.rt.frame(DT);
    assert_eq!(l.rt.state.running_subagents(&l.sid).len(), 6);
    assert_eq!(submit(&l), "Send", "background helpers must not turn the idle parent's Send into Stop");
    esc(&mut l);
    assert!(l.writes.try_recv().is_err());
    assert_eq!(l.rt.state.running_subagents(&l.sid).len(), 6);
}

#[test]
fn stop_on_an_idle_parent_never_interrupts_background_children() {
    let mut l = live();
    let child = crate::fixture_ids::CHILD;
    l.rt.update(|s, _| {
        s.finish_stream();
        s.running.clear();
        s.running.insert(child.into());
        s.stop();
    });
    assert!(l.writes.try_recv().is_err(), "an idle session needs no interrupt");
    assert!(l.rt.state.stops.by.is_empty());
    assert!(l.rt.state.running.contains(child));
}

#[test]
fn stop_interrupts_only_the_open_session_as_desktop_does() {
    let mut l = live();
    let child = crate::fixture_ids::CHILD;
    l.rt.update(|s, _| { s.running.insert(child.into()); });
    esc(&mut l);
    let interrupts: Vec<_> = l.writes.try_iter().filter_map(|w| match untracked(w) { Write::Interrupt(id) => Some(id), _ => None }).collect();
    assert_eq!(interrupts, vec![l.sid.clone()]);
    assert!(l.rt.state.running.contains(child));
    assert_eq!(submit(&l), "Send");
}

#[test]
fn an_already_idle_interrupt_reply_confirms_without_an_execution_end() {
    let mut l = live();
    esc(&mut l);
    event(&mut l, "session.interrupt.acknowledged", json!({ "interrupted": false }));
    assert!(!l.rt.state.stops.pending(&l.sid));
    play(&mut l, CONFIRM_SECS + 1.0);
    assert!(!l.rt.state.status.contains("confirm the stop"));
    assert_eq!(submit(&l), "Send");
}

#[test]
fn an_accepted_interrupt_waits_for_execution_end_and_ignores_late_write_failure() {
    let mut l = live();
    esc(&mut l);
    event(&mut l, "session.interrupt.acknowledged", json!({ "interrupted": true }));
    assert!(l.rt.state.stops.pending(&l.sid));
    event(&mut l, "session.execution.interrupted", json!({ "reason": "user" }));
    l.server.send(Incoming::Settled { ticket: stop_ticket(&l), result: Err(format!("POST /api/session/{}/interrupt: timed out", l.sid)) }).unwrap();
    l.rt.frame(DT);
    assert!(!l.rt.state.status.contains("stop"));
    assert_eq!(submit(&l), "Send");
}

#[test]
fn stop_on_a_child_interrupts_that_child_not_its_parent() {
    let mut l = live();
    let child = crate::fixture_ids::CHILD;
    l.rt.update(|s, _| {
        s.route = Route::here(child.into());
        s.running.insert(child.into());
        s.stop();
    });
    assert!(matches!(l.writes.try_recv().map(untracked), Ok(Write::Interrupt(id)) if id == child));
    assert!(l.writes.try_recv().is_err());
    assert!(l.rt.state.running.contains(&l.sid));
}

#[test]
fn an_idle_active_snapshot_confirms_a_stop_without_an_execution_end() {
    let mut l = live();
    esc(&mut l);
    l.server.send(Incoming::Active(vec![])).unwrap();
    l.rt.frame(DT);
    assert!(!l.rt.state.stops.pending(&l.sid), "already idle is confirmation, even without an execution-end event");
    play(&mut l, CONFIRM_SECS + 1.0);
    assert!(!l.rt.state.status.contains("confirm the stop"));
    assert_eq!(submit(&l), "Send");
}

fn enter(l: &mut Live, cmd: bool, repeat: bool) {
    let ev = KeyEvent { key: Key::Enter, mods: Mods { cmd, ..Mods::default() } };
    if repeat { l.rt.repeat_key(ev) } else { l.rt.key(ev) }
    l.rt.frame(DT);
}

fn interrupts(l: &Live) -> Vec<String> {
    l.writes.try_iter().filter_map(|w| match untracked(w) { Write::Interrupt(id) => Some(id), _ => None }).collect()
}

#[test]
fn enter_in_an_empty_composer_stops_the_run_as_desktop_does() {
    let mut l = live();
    enter(&mut l, false, false);
    stopped(&l);
    assert_eq!(interrupts(&l), vec![l.sid.clone()]);
    assert!(l.rt.state.composer.focused);
}

#[test]
fn whitespace_is_empty_and_cmd_enter_stops_too() {
    let mut l = live();
    l.rt.update(|s, _| s.composer.insert("   \n "));
    l.rt.frame(DT);
    assert_eq!(submit(&l), "Stop");
    enter(&mut l, true, false);
    stopped(&l);
    assert_eq!(interrupts(&l), vec![l.sid.clone()]);
}

#[test]
fn a_held_enter_never_stops() {
    let mut l = live();
    enter(&mut l, false, true);
    enter(&mut l, true, true);
    running(&l);
    assert!(interrupts(&l).is_empty());
}

#[test]
fn enter_with_words_still_steers_and_its_repeats_never_stop() {
    let mut l = live();
    l.rt.update(|s, _| s.composer.insert("try the other way"));
    l.rt.frame(DT);
    enter(&mut l, false, false);
    assert!(l.rt.state.composer.text.is_empty(), "steered");
    assert!(l.rt.state.busy(&l.sid));
    enter(&mut l, false, true);
    assert!(l.rt.state.busy(&l.sid), "the held key's repeats don't stop the run it steered");
    let writes: Vec<_> = l.writes.try_iter().map(untracked).collect();
    assert!(writes.iter().any(|w| !matches!(w, Write::Interrupt(_))), "the steer went out");
    assert!(!writes.iter().any(|w| matches!(w, Write::Interrupt(_))));
}

#[test]
fn enter_on_an_idle_session_does_nothing_even_with_background_children() {
    let mut l = live();
    l.rt.update(|s, _| crate::stop_stories::background(s));
    l.rt.frame(DT);
    assert_eq!(submit(&l), "Send");
    l.rt.update(|s, _| s.composer.focused = true);
    enter(&mut l, false, false);
    enter(&mut l, true, false);
    assert!(l.writes.try_recv().is_err(), "nothing sent, nothing interrupted");
    assert_eq!(l.rt.state.running_subagents(&l.sid).len(), 6);
}
