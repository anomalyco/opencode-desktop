//! Deterministic receipts: two timed-out sends, then a failed first and an admitted second.
use rune::{Play, TextEdit};
use opencode_kit::model::Message;
use crate::{App, Route, prompt_history::Entry};
use crate::stories::{Story, Trigger, replay};
use super::Sent;

pub(crate) const FIRST: &str = "msg_zzreceipt1";
pub(crate) const SECOND: &str = "msg_zzreceipt2";

pub(crate) fn setup(s: &mut App) {
    crate::scenes::busy_scene(s);
    s.fixture_run.playback.clear();
    let Route::Session { id: sid, .. } = (*s.route).clone() else { return };
    for (id, body) in [(FIRST, "Why does it run on that machine?"), (SECOND, "Let's start with the ideas first.")] {
        if let Some(t) = s.transcripts.get_mut(&sid) {
            t.messages.push(Message::User { id: id.into(), text: body.into() });
        }
        let prompt = opencode_kit::client::Prompt { id: Some(id.into()), text: body.into(), ..Default::default() };
        let sent = Sent::new(&sid, id, Entry::of(&TextEdit::new(body)), vec![], vec![], false).with_prompt(&prompt);
        s.unsent.note(sent);
        s.send_failed(id, &opencode_kit::wire_error::WireError::new("POST", "/prompt", opencode_kit::wire_error::WireKind::TimedOut));
    }
}

pub(crate) fn confirmed(s: &mut App) {
    let Route::Session { id: sid, .. } = (*s.route).clone() else { return };
    s.judge_sends_by_page(&sid, &[Message::User { id: SECOND.into(), text: "Let's start with the ideas first.".into() }]);
    // The timed-out first is unconfirmed until its receipt check: the server never got it.
    s.step_send(FIRST, super::machine::Event::Checked(super::machine::Found::Missing));
    s.dialogs.toast = Default::default();
}

const LONG: &str = "Well, what are the demos in order? It's also the performance improvements, right? \
You should get the best performance in the server and on those algorithms, with profiling information \
and instrumentation, and the slides should walk through how we got these features. Similarly, we need \
to show off the effect service where we can swap deterministic time. Maybe some visual in-app \
representation of this too, like the paper I gave you earlier about draggable handles and the rest.\n\n\
There's lots of cool stuff in there we could borrow. Show the timeline scrubbing, the replay of a \
recorded session, how a test pins the clock and steps it frame by frame, and why that makes the \
flaky cases go away. Then end on the numbers: cold start, the first frame, and memory over an hour \
of streaming, each beside last month's build so the improvement is obvious without explaining it.\n\n\
And keep each slide to one idea, with the code it came from a click away rather than pasted in.";

/// Kit's report: a long steer folded to Read more, still waiting on a receipt, with another
/// steer queued under it in the same run.
pub(crate) fn queued_long(s: &mut App) {
    crate::scenes::busy_scene(s);
    s.settings.fold_user = true;
    let Route::Session { id: sid, .. } = (*s.route).clone() else { return };
    for (id, body) in [(FIRST, LONG), (SECOND, "Wait, I don't see the show source thing in the slide. Is it built into what I've been running?")] {
        s.inboxes.entry(sid.clone()).or_default().upsert(opencode_kit::model::InboxItem { id: id.into(), text: body.into(), delivery: opencode_kit::model::Delivery::Steer, files: vec![] });
    }
    let prompt = opencode_kit::client::Prompt { id: Some(FIRST.into()), text: LONG.into(), ..Default::default() };
    s.unsent.note(Sent::new(&sid, FIRST, Entry::of(&TextEdit::new(LONG)), vec![], vec![], true).with_prompt(&prompt));
    s.send_failed(FIRST, &opencode_kit::wire_error::WireError::new("POST", "/prompt", opencode_kit::wire_error::WireKind::TimedOut));
}

fn open(s: &mut App, _: &mut Play<App>) { setup(s); }

fn offline(s:&mut App) {
    crate::open_scene(s); s.finish_stream();
    let Route::Session { id: session, .. }=(*s.route).clone() else { return };
    let body="Run the tests before you merge.";
    let mut sent=Sent::new(&session,FIRST,Entry::of(&TextEdit::new(body)),vec![],vec![],false);
    sent.phase=super::Phase::Waiting { why: "waiting for connection".into() }; sent.journaled=true;
    s.unsent.note(sent); s.show_outbox();
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("send-unconfirmed", setup),
    ("outbox-offline", offline),
    ("outbox-rejected", |s| { offline(s); s.send_failed(FIRST, &opencode_kit::wire_error::WireError::new("POST", "/prompt", opencode_kit::wire_error::WireKind::Status { status: 403, tag: None, message: None })); }),
    ("send-failed", |s| { setup(s); confirmed(s); }),
    ("send-queued-long", queued_long),
]);
rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::session("Timeline/Send receipts", &[
        replay(open),
        Trigger { key: 'f', label: "Receive confirmations", run: |s, tx| { tx.instant(); confirmed(s); } },
    ]),
    Story::session("Timeline/Offline send", &[replay(|s,_| offline(s))]),
]);
