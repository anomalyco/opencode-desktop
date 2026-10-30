//! The matrix: each action, the circumstances it runs in, and what's allowed with a reason.

use rune::{Env, Key, KeyEvent, Mods, Runtime};
use serde_json::json;

use rune::motion_gate::{self, Allow, Job};
use super::{Circ, DT, Dock, Length, Prompt, Run, SID};
use crate::App;

/// Findings that are the design, matched on kind, element and cell (substrings, "" for any).
/// Every row needs its reason.
const ALLOW: &[Allow] = &[
    // (kind, element contains, cell contains, why)
    ("fade-snap", "turn-actions/#0/#0/working", "", "Working and Copy share a footprint and swap at once: a crossfade lays the copy icon's outline over the W (view::turn_actions)"),
    ("fade-snap", "turn-actions/#0/#0/copy", "", "Working and Copy share a footprint and swap at once: a crossfade lays the copy icon's outline over the W (view::turn_actions)"),
    ("jump", "turn-actions/#0/#0/working", "working-relabel", "new words take the Working label's place at once, as a new status does; the cell checks the shimmer's rhythm through the change (working::finish_pass)"),
    ("text-swap", "working/#1/label", "", "new words take the Working label's place at once, as a new status does (working::finish_pass)"),
    ("text-swap", "crumbs/", "switch-tab", "the header names the tab that opened at once, as the page under it changes at once"),
    ("pop-in", "window/activity-panel", "capsule-open-long", "the capsule's list opens and closes at once, as Desktop's menu does (subagents::panel)"),
    ("pop-out", "window/activity-panel", "capsule-open-long", "the capsule's list opens and closes at once, as Desktop's menu does (subagents::panel)"),
    ("pop-in", "window/activity-dismiss", "capsule-open-long", "an invisible layer under the open list that closes it on a click outside (view::app)"),
    ("pop-out", "window/activity-dismiss", "capsule-open-long", "an invisible layer under the open list that closes it on a click outside (view::app)"),
    ("reversal", "now/", "agents", "a subagent's result or error rolls up into its row as the pinned timeline slides down for the closing activity lane; against its row it travels one way"),
    ("reversal", "time-call_story_agent", "agents", "the card's final time rolls its digit up as the pinned timeline slides down for the closing activity lane; against its row it travels one way (odometer::odometer_end)"),
    ("jump", "working/#1/label", "working-relabel", "new words take the Working label's place at once, as a new status does; the cell checks the shimmer's rhythm through the change (working::finish_pass)"),
    ("jump", "c-msg_001a162cb6dbc013fixture00-0", "tool-expand", "a tool group opens and closes instantly, as a thought's disclosure does (turns::group)"),
    ("pop-in", "c-msg_001a162cb6dbc013fixture00-0/#0/steps", "tool-expand", "a tool group opens and closes instantly, as a thought's disclosure does (turns::group)"),
    ("pop-out", "c-msg_001a162cb6dbc013fixture00-0/#0/steps", "tool-expand", "a tool group opens and closes instantly, as a thought's disclosure does (turns::group)"),
    ("jump", "c-msg_001a162cb6dbc013fixture00-0", "send-during-reflow", "a tool group opens instantly (turns::group); the send that follows is what this cell checks"),
    ("pop-in", "c-msg_001a162cb6dbc013fixture00-0/#0/steps", "send-during-reflow", "a tool group opens instantly (turns::group); the send that follows is what this cell checks"),
    ("pop-in", "queue-list/queued-", "queue/", "a newly queued row takes over from the composer's words in the frame they clear (Tx::launch from \"prompt\") and rises as the queue's height springs (queue::panel); the gate doesn't pair the words that leave with the row that takes them"),
    ("pop-in", "queue-list/queued-", "queue-lands/", "a newly queued row takes over from the composer's words in the frame they clear (Tx::launch from \"prompt\"), as in the queue cells"),
    ("pop-out", "queue-list/queued-", "queue-lands/", "a delivered row hands its words to its bubble in the frame it clears (Tx::launch from queued-…/label, Arrivals::launch); the gate doesn't pair the row that leaves with the bubble that takes its words"),
    ("pop-out", "cmp-msg_gatecompaction-end/running", "compaction/", "the Compacting row and the divider that ends the compaction trade places at once, as Desktop's Switch does (compaction::view)"),
    ("pop-in", "mic/", "dictate", "the mic, the stop and its spinner swap at once on the press or the words landing, as the iOS composer's do (dictation::toolbar)"),
    ("pop-out", "mic/", "dictate", "the mic, the stop and its spinner swap at once on the press or the words landing, as the iOS composer's do (dictation::toolbar)"),
    ("pop-in", "timeline/u-", "scrolled-up", "read further up, a send flies nowhere: the timeline cuts to its end and the message is there (App::land)"),
    ("offset-jump", "timeline/t-", "send-from-up/", "read further up, a send cuts the timeline to its end (App::land)"),
    ("pop-in", "quote-toolbar", "", "the Quote button appears over a settled selection at once and goes at once, as Desktop's does (quote::toolbar)"),
    ("pop-out", "quote-toolbar", "", "the Quote button appears over a settled selection at once and goes at once, as Desktop's does (quote::toolbar)"),
    ("pop-in", "composer/attachments", "dropzone/", "a dropped file's chip lands at once and the composer grows to hold it, as a picked file's does; the overlay's fade is what this cell checks (dropzone::drop)"),
    ("jump", "dock/composer", "dropzone/", "the composer grows at once to hold a dropped file's chip, as for a picked file (dropzone::drop)"),
    ("jump", "session/dock", "dropzone/", "the composer grows at once to hold a dropped file's chip, as for a picked file (dropzone::drop)"),
    ("jump", "session/timeline-area", "dropzone/", "the timeline gives the composer the room for a dropped file's chip at once (dropzone::drop)"),
    ("fade-snap", "actions/copy", "fold/", "a prompt's Copy shows with its row's hover at once (view::user_bubble); scrolling to Show less puts the prompt under the pointer"),
    ("jerk", "editor-scroll/editor-pad", "long-prompt", "a 26-line paste grows the editor to its 180 pt cap from rest, and the send drops it back: a spring's first frames over a long way (view::composer). The fold and the flight are what these cells check"),
    ("offset-jump", "timeline/t-", "fold/scrolled-up/", "read further up, a send cuts the timeline to its end (App::land)"),
    ("jump", "timeline-area/scroll-", "dropzone/", "the timeline gives the composer the room for a dropped file's chip at once (dropzone::drop)"),
];

struct Action {
    name: &'static str,
    circs: fn() -> Vec<Circ>,
    run: fn(&mut Run),
}

/// Pinned and scrolled up on the long session, short and empty sessions, each wide and narrow.
fn places(base: Circ) -> Vec<Circ> {
    let mut v = Vec::new();
    for narrow in [false, true] {
        for scrolled in [false, true] {
            v.push(Circ { length: Length::Long, scrolled, narrow, ..base });
        }
        for length in [Length::Short, Length::Empty] {
            v.push(Circ { length, narrow, ..base });
        }
    }
    v
}

/// `places`, plus each prompt shape and the docks that can be open.
fn sends(base: Circ, docks: &[Dock]) -> Vec<Circ> {
    let mut v = places(base);
    for prompt in [Prompt::Lines, Prompt::Image, Prompt::Long] {
        for length in [Length::Long, Length::Empty] {
            v.push(Circ { prompt, length, ..base });
        }
    }
    for &dock in docks {
        for scrolled in [false, true] {
            v.push(Circ { dock, scrolled, ..base });
        }
    }
    v
}

fn streaming() -> Circ {
    Circ { unplayed: true, ..Circ::default() }
}

fn wide_and_narrow(base: Circ) -> Vec<Circ> {
    let mut v = Vec::new();
    for narrow in [false, true] {
        for scrolled in [false, true] {
            v.push(Circ { narrow, scrolled, ..base });
        }
    }
    v
}

const ACTIONS: &[Action] = &[
    Action { name: "send", circs: || sends(Circ::default(), &[Dock::Pending]), run: |r| {
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    // Sends on a live server's order: the inbox echoes the prompt as a steer while it flies,
    // the run takes it up, and each step's start re-reads the page (`served.rs`). The first
    // reply thinks before it answers; the second send's re-read brings that reply back as the
    // server stored it, under the rows it already has.
    Action { name: "send-live", circs: || sends(Circ::default(), &[]), run: |r| {
        r.type_prompt();
        r.enter(false);
        r.play_out(0.5);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    Action { name: "send-twice", circs: || places(Circ::default()), run: |r| {
        r.type_prompt();
        r.enter(false);
        r.play(0.15);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    // Read further up, a send flies nowhere: the timeline cuts to its end, the message there.
    Action { name: "send-from-up", circs: || vec![Circ { scrolled: true, ..Circ::default() }, Circ { scrolled: true, narrow: true, prompt: Prompt::Lines, ..Circ::default() }], run: |r| {
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
        assert!(r.rt.state.ends.at_end(SID), "the timeline shows its end after a send");
        let sent = r.rt.state.transcripts.get(SID).and_then(|t| t.messages.iter().rev().find(|m| matches!(m, opencode_kit::model::Message::User { .. }))).map(|m| m.id().to_owned());
        let bubble = sent.and_then(|id| r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("bubble") && e.path.split('/').any(|p| p == format!("u-{id}"))));
        assert!(bubble.is_some_and(|b| b.visible[3] > 0.0), "the sent message shows at the end");
    } },
    Action { name: "steer", circs: || sends(streaming(), &[Dock::Pending]), run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    // A steer on a live server's order while it flies: the inbox echoes it under the client's
    // id, the running reply streams a tool row above it, the run takes it up, and the next
    // step's start re-reads the page (`served.rs`). Zoomed too, as the flight draws under it.
    Action { name: "steer-live", circs: || {
        let mut v = places(streaming());
        v.push(Circ { zoomed: true, ..streaming() });
        v.push(Circ { zoomed: true, scrolled: true, ..streaming() });
        v
    }, run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(false);
        let Some(id) = r.rt.state.inboxes.get(SID).and_then(|i| i.items.last()).map(|i| i.id.clone()) else { return };
        let text = r.circ.body();
        let item = json!({ "type": "user", "delivery": "steer", "payload": { "text": text, "metadata": { "displayText": text } } });
        r.event("session.inbox.enqueued", json!({ "sessionID": SID, "inboxID": id, "item": item }));
        let running = r.rt.state.transcripts.get(SID)
            .and_then(|t| t.messages.iter().rev().find(|m| matches!(m, opencode_kit::model::Message::Assistant(_))))
            .map(|m| m.id().to_owned());
        let tool = |r: &mut Run, reply: &str, call: &str| {
            r.event("session.tool.input.started", json!({ "sessionID": SID, "assistantMessageID": reply, "id": call, "name": "read" }));
            r.play(0.03);
            r.event("session.tool.called", json!({ "sessionID": SID, "assistantMessageID": reply, "id": call, "input": { "filePath": "src/net/reconnect.ts" }, "executed": true }));
        };
        if let Some(running) = running {
            r.play(0.03);
            tool(r, &running, "call_steer_live_a");
        }
        r.play(0.03);
        r.event("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": id }));
        r.play(0.03);
        let model = json!({ "id": "claude-opus-5-5", "providerID": "anthropic", "variant": "high" });
        r.event("session.step.started", json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzsteerlive", "agent": "build", "model": model }));
        r.play(0.05);
        tool(r, "msg_zzzzsteerlive", "call_steer_live_b");
        r.play_out(1.0);
    } },
    Action { name: "queue", circs: || sends(streaming(), &[Dock::Pending]), run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(true);
        r.play_out(1.0);
    } },
    Action { name: "steer-lands", circs: || places(streaming()), run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(false);
        r.play(0.8);
        let Some(id) = r.rt.state.inboxes.get(SID).and_then(|i| i.items.last()).map(|i| i.id.clone()) else { return };
        r.event("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": id }));
        r.play_out(1.0);
    } },
    // A shell blocks the run: "Move to background ⌃B" fades in beside Working, and ⌃B (or a
    // steer) moves it, so the call returns and the hint fades out (`background.rs`).
    Action { name: "background-key", circs: || wide_and_narrow(Circ::default()), run: |r| {
        background_shell(r);
        r.act("ctrl-b", |rt| rt.key(KeyEvent { key: Key::Text("b".into()), mods: Mods { ctrl: true, ..Mods::default() } }));
        r.play_out(1.0);
    } },
    Action { name: "background-steer", circs: || wide_and_narrow(Circ::default()), run: |r| {
        background_shell(r);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.6);
    } },
    // Zoomed too: the delivery flight draws under the window's scale.
    Action { name: "queue-lands", circs: || {
        let mut v = places(streaming());
        v.push(Circ { dock: Dock::Pending, ..streaming() });
        v.push(Circ { zoomed: true, ..streaming() });
        v.push(Circ { zoomed: true, narrow: true, ..streaming() });
        v
    }, run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(true);
        r.play(0.8);
        // The oldest queued prompt: the one just queued, or the dock's own when it had one.
        let Some(id) = r.rt.state.inboxes.get(SID).and_then(|i| i.queued().next()).map(|i| i.id.clone()) else { return };
        r.event("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": id }));
        // A flight from the queue's left end to a right-aligned bubble crosses ~400 pt; its
        // spring's tail outlasts a 1 s tail.
        r.play_out(1.5);
    } },
    Action { name: "stream", circs: || {
        let mut v = places(streaming());
        v.push(Circ { dock: Dock::Question, ..streaming() });
        v.push(Circ { dock: Dock::Pending, ..streaming() });
        v
    }, run: |r| {
        r.start_stream();
        r.play_out(1.0);
    } },
    Action { name: "tool-expand", circs: || wide_and_narrow(Circ::default()), run: |r| {
        // The narrow window's pinned tail can hold no tool group: scroll one into view.
        for _ in 0..4 {
            if trigger(r, None).is_some() { break; }
            r.swipe_up(160.0, 0.6);
        }
        let Some(group) = tap_trigger(r, None) else { return };
        r.play(0.6);
        tap_trigger(r, Some(&group));
        r.play(0.6);
    } },
    // A long prompt lands folded; Read more opens it on the growth spring with its top held,
    // and Show less closes it again.
    Action { name: "fold", circs: || {
        let long = Circ { prompt: Prompt::Long, ..Circ::default() };
        let mut v = wide_and_narrow(long);
        v.push(Circ { length: Length::Empty, ..long });
        v
    }, run: |r| {
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
        let opened = |r: &Run| r.rt.state.expanded.iter().any(|k| k.starts_with("user-expanded:"));
        r.act("Read more", |rt| Run::tap(rt, "fold-toggle"));
        assert!(opened(r), "Read more opens the prompt");
        r.play(0.8);
        // Down to Show less at the end of the opened prompt, its top now above the timeline:
        // closing holds the toggle where it's pressed.
        r.swipe_up(-1200.0, 0.8);
        r.act("Show less", |rt| Run::tap(rt, "fold-toggle"));
        assert!(!opened(r), "Show less folds it again");
        r.play(0.8);
    } },
    Action { name: "send-during-reflow", circs: || wide_and_narrow(Circ::default()), run: |r| {
        r.type_prompt();
        if tap_trigger(r, None).is_none() { return; }
        r.step_n(2);
        r.enter(false);
        r.play_out(1.0);
    } },
    Action { name: "working", circs: || {
        let mut v = places(Circ::default());
        v.push(Circ { dock: Dock::Question, ..Circ::default() });
        v
    }, run: |r| {
        let m = json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzworking" });
        r.event("session.execution.started", m.clone());
        r.play(1.0);
        r.event("session.execution.succeeded", m);
        r.play(1.0);
    } },
    // The Working label changes mid-shimmer: the sweep keeps its rhythm, and once the run
    // ends nothing asks for frames.
    Action { name: "working-relabel", circs: || places(Circ::default()), run: |r| {
        let m = json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzworking" });
        r.event("session.execution.started", m.clone());
        r.play(0.6);
        r.act("relabel", |rt| rt.update(|s, _| {
            let now = s.clock;
            s.working.relabel("Running the tests", now);
        }));
        r.play(1.4);
        r.event("session.execution.succeeded", m);
        r.quiet();
        r.play(1.5);
    } },
    Action { name: "question", circs: || {
        let mut v = places(Circ::default());
        v.extend(wide_and_narrow(Circ { dock: Dock::Pending, ..Circ::default() }));
        v
    }, run: |r| {
        let q = r.rt.state.fixture["question"].clone();
        r.event("form.created", q);
        r.play(0.8);
        r.event("form.replied", json!({ "id": "frm_fx0001", "sessionID": SID }));
        r.play(0.8);
    } },
    // Into "Type your own answer" and out: a digit opens it, the words wrap and the row grows,
    // ↑ goes to the answer above, ↓ and a key come back, Esc leaves. All of it at once.
    Action { name: "question-custom", circs: || vec![Circ { dock: Dock::Question, ..Circ::default() }, Circ { dock: Dock::Question, narrow: true, ..Circ::default() }], run: |r| {
        let key = |r: &mut Run, what: &str, key: Key| r.act(what, |rt| rt.key(KeyEvent { key, mods: Mods::default() }));
        key(r, "4", Key::Text("4".into()));
        r.play(0.3);
        r.act("a long answer", |rt| rt.update(|s, _| {
            let t = s.typed_mut("frm_fx0001", "q0");
            *t = rune::TextEdit::new("Fake clock for the loop, but keep one slow end-to-end test on real timers so a regression shows up somewhere");
            t.focused = true;
        }));
        r.play(0.3);
        key(r, "↑", Key::Up);
        r.play(0.3);
        key(r, "↑", Key::Up);
        r.play(0.3);
        key(r, "↓", Key::Down);
        r.play(0.3);
        key(r, "!", Key::Text("!".into()));
        r.play(0.3);
        key(r, "esc", Key::Escape);
        r.play(0.5);
    } },
    // The agent asks: nothing joins the timeline while the dock asks, then the answer lands as
    // the call's row, each question over its answer (four questions), or as "Questions dismissed".
    Action { name: "question-answered", circs: || {
        let mut v = places(Circ::default());
        v.push(Circ { dock: Dock::Pending, ..Circ::default() });
        v
    }, run: |r| answer_question(r, 1) },
    Action { name: "question-dismissed", circs: || wide_and_narrow(Circ::default()), run: |r| answer_question(r, 4) },
    // The queued prompt is delivered while the question shows under it: the queue closes
    // into the question dock, then the question closes back to the composer.
    Action { name: "queue-lands-over-question", circs: || wide_and_narrow(Circ { dock: Dock::Pending, ..Circ::default() }), run: |r| {
        let q = r.rt.state.fixture["question"].clone();
        r.event("form.created", q);
        r.play(0.8);
        r.event("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": "msg_zzpending" }));
        r.play(1.5);
        r.event("form.replied", json!({ "id": "frm_fx0001", "sessionID": SID }));
        // The dock closing ~360 pt settles in about 0.75 s.
        r.play_out(1.5);
    } },
    Action { name: "revert", circs: || {
        let mut v = wide_and_narrow(Circ::default());
        v.push(Circ { length: Length::Short, ..Circ::default() });
        v
    }, run: |r| {
        let Some(to) = r.rt.state.transcripts.get(SID).and_then(|t| t.messages.iter().rev().find(|m| matches!(m, opencode_kit::model::Message::User { .. }))).map(|m| m.id().to_owned()) else { return };
        let staged = json!({ "sessionID": SID, "revert": { "messageID": to } });
        r.event("session.revert.staged", staged.clone());
        r.play(0.8);
        r.event("session.revert.cleared", json!({ "sessionID": SID }));
        r.play(0.8);
        r.event("session.revert.staged", staged);
        r.play(0.5);
        r.event("session.revert.committed", json!({ "sessionID": SID, "to": to }));
        r.play(0.8);
    } },
    // A compaction runs and ends: loud, its divider, summary and Compacting row, then the
    // divider with its usage; quiet, the divider's light while it runs and its label's ending.
    Action { name: "compaction", circs: || places(Circ::default()), run: |r| compaction(r, false, 1.6) },
    Action { name: "compaction-quiet", circs: || places(Circ::default()), run: |r| compaction(r, true, 1.6) },
    // A quiet one that ends inside its grace fades in once, as Compacted.
    Action { name: "compaction-quiet-grace", circs: || wide_and_narrow(Circ::default()), run: |r| compaction(r, true, 0.2) },
    Action { name: "switch-tab", circs: || wide_and_narrow(Circ::default()), run: |r| {
        let other = r.rt.state.tabs.iter().map(|t| t.id.clone()).find(|t| t != SID).expect("another tab");
        r.act("open another tab", |rt| rt.update(move |s, tx| s.open(&other, tx)));
        r.rec.loads.push(r.rec.frames - 1);
        r.play(1.0);
        r.act("open this tab", |rt| rt.update(|s, tx| s.open(SID, tx)));
        r.rec.loads.push(r.rec.frames - 1);
        r.play(1.0);
    } },
    // Thirty subagents: the capsule's list opens over the composer, scrolls inside its menu and
    // closes again.
    Action { name: "capsule-open-long", circs: || wide_and_narrow(Circ::default()), run: |r| {
        r.act("thirty agents run", |rt| rt.update(|s, _| crate::capsule_stories::run_thirty(s)));
        r.play(0.6);
        r.act("open the list", |rt| Run::tap(rt, "activity-chip"));
        r.play(0.6);
        r.act("scroll the list", |rt| {
            let (x, y) = rt.find("activity-scroll").expect("the list");
            rt.wheel(x, y, -120.0);
        });
        r.play(0.6);
        r.act("close it", |rt| Run::tap(rt, "activity-chip"));
        r.play_out(0.6);
    } },
    Action { name: "agents", circs: || wide_and_narrow(Circ::default()), run: agents },
    // Live numbers roll like an odometer (`odometer.rs`).
    Action { name: "odometer-9s", circs: || wide_and_narrow(Circ::default()), run: |r| super::odometers::time(r, 9) },
    Action { name: "odometer-59s", circs: || wide_and_narrow(Circ::default()), run: |r| super::odometers::time(r, 59) },
    Action { name: "odometer-usage", circs: || vec![Circ::default(), Circ { narrow: true, ..Circ::default() }], run: super::odometers::usage },
    Action { name: "odometer-questions", circs: || vec![Circ::default()], run: super::odometers::questions },
    Action { name: "odometer-queue", circs: || vec![Circ::default()], run: super::odometers::queue },
    Action { name: "agents-one", circs: || wide_and_narrow(Circ::default()), run: one_agent },
    Action { name: "agent-wakes", circs: || wide_and_narrow(Circ::default()), run: agent_wakes },
    // Dictation on the fake Hex: the toolbar's recording row comes and goes on the key, the
    // meter follows the voice, and the words land in the composer, which grows to hold them.
    Action { name: "dictate", circs: dictation_circs, run: |r| {
        r.type_prompt();
        r.act("⇧⌘M", |rt| rt.key(dictation_key()));
        r.play(1.0);
        r.act("⇧⌘M", |rt| rt.key(dictation_key()));
        r.play(1.6);
    } },
    // Send while it listens: the words land, then the prompt goes as any send does.
    Action { name: "dictate-send", circs: dictation_circs, run: |r| {
        r.type_prompt();
        r.act("⇧⌘M", |rt| rt.key(dictation_key()));
        r.play(0.6);
        r.act("Send", |rt| Run::tap(rt, "submit"));
        r.play(1.6);
    } },
    // A slow Hex: "Preparing dictation…" opens a line under the toolbar and closes it.
    Action { name: "dictate-slow", circs: dictation_circs, run: |r| {
        r.type_prompt();
        r.act("⇧⌘M", |rt| {
            rt.update(|s, _| s.dictation.slow_fake());
            rt.key(dictation_key());
        });
        r.play(1.8);
        r.act("Esc", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
        r.play(0.6);
    } },
    Action { name: "dictate-cancel", circs: dictation_circs, run: |r| {
        r.type_prompt();
        r.act("tap the mic", |rt| Run::tap(rt, "mic"));
        r.play(0.6);
        r.act("Esc", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
        r.play(0.6);
    } },
    // A call transcript pasted whole: the composer grows to its cap and the text scrolls inside
    // it, typing at the end keeps the caret in view, and the wheel over it scrolls the prompt.
    Action { name: "long-paste", circs: || wide_and_narrow(Circ::default()), run: |r| {
        r.act("tap the composer", |rt| Run::tap(rt, "prompt"));
        r.act("paste", |rt| rt.paste(crate::stories::long_paste()));
        r.play(1.0);
        r.act("type", |rt| rt.key(KeyEvent { key: Key::Text(" and one more line".into()), mods: Mods::default() }));
        r.play(0.6);
        r.act("wheel over it", |rt| {
            let (x, y) = rt.find("editor").expect("the editor");
            rt.wheel(x, y, 120.0);
        });
        r.play_out(0.6);
    } },
    Action { name: "jump-latest", circs: || vec![Circ::default(), Circ { narrow: true, ..Circ::default() }], run: |r| {
        r.swipe_up(1400.0, 1.0);
        if r.rt.find("jump-latest").is_none() { return; }
        r.act("tap jump-latest", |rt| Run::tap(rt, "jump-latest"));
        r.play(1.5);
    } },
    // A file dragged over the page: the frosted overlay fades in over 200 ms, out as the drag
    // leaves, in again, and out once the file lands in the composer (dropzone.rs).
    Action { name: "dropzone", circs: || wide_and_narrow(Circ::default()), run: |r| {
        // Its own file: the cells run in parallel, and each removes its file when it ends.
        let file = std::env::temp_dir().join(format!("oc-desktop-gate-drop-{}-{}.txt", std::process::id(), r.circ.name().replace('/', "-")));
        std::fs::write(&file, "notes").unwrap();
        let at = r.rt.find("panel").expect("the page");
        let drag = |rt: &mut Runtime<App>| rt.drag_files(vec![file.clone()], Some(at));
        r.act("drag a file over", &drag);
        r.play(0.5);
        r.act("drag it away", |rt| rt.drag_files_leave());
        r.play(0.5);
        r.act("drag it back", &drag);
        r.play(0.5);
        r.act("drop it", |rt| rt.drop_files(vec![file.clone()], Some(at)));
        r.play_out(0.6);
        std::fs::remove_file(&file).ok();
    } },
    // A drag selects a reply (its highlight is instant), then a prompt sends under it.
    Action { name: "select", circs: || wide_and_narrow(Circ::default()), run: |r| {
        let els = r.rt.elements();
        let words: Vec<_> = els.iter().filter(|e| !e.exiting && e.text.is_some() && e.path.contains("/t-msg_") && e.visible[3] >= 16.0 && e.visible[2] >= 40.0).collect();
        let (Some(first), Some(last)) = (words.first(), words.last()) else { panic!("no reply text on screen") };
        let (x0, y0) = (first.visible[0] + 1.0, first.visible[1] + 8.0);
        let (x1, y1) = (last.visible[0] + last.visible[2] - 1.0, last.visible[1] + last.visible[3] - 6.0);
        r.act("press on a reply", |rt| {
            rt.pointer_move(x0, y0);
            rt.pointer_down(x0, y0);
        });
        for i in 1..=6 {
            let t = i as f32 / 6.0;
            r.act("drag", |rt| rt.pointer_move(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t));
        }
        r.act("release", |rt| rt.pointer_up(x1, y1));
        assert!(r.rt.selected_text().is_some_and(|t| !t.is_empty()), "the drag selects the reply");
        r.play(0.4);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    Action { name: "browser-split", circs: || vec![Circ::default()], run: |r| {
        r.act("open browser", |rt| Run::tap(rt, "browser"));
        r.rec.loads.push(r.rec.frames - 1);
        r.quiet();
        r.play(0.6);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
        r.act("close browser", |rt| Run::tap(rt, "browser-close"));
        r.rec.loads.push(r.rec.frames - 1);
        r.quiet();
        r.play(0.6);
    } },
];

/// The run starts a step whose shell call blocks it, and the hint shows.
fn background_shell(r: &mut Run) {
    let m = json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzbackground" });
    r.event("session.execution.started", m.clone());
    r.event("session.step.started", json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzbackground", "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }));
    r.play(0.3);
    r.event("session.tool.called", json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzbackground", "id": "call_gate_shell", "name": "shell", "input": { "command": "cargo test" } }));
    r.play(1.0);
}

/// Background agents: two start a beat apart (the second joins the first's card) and step
/// along, then one finishes and the other fails.
fn agents(r: &mut Run) {
    use crate::agent_stories as st;
    r.act("an agent starts", |rt| rt.update(st::spawn));
    r.play(1.0);
    r.act("it steps", |rt| rt.update(st::advance));
    r.play(0.6);
    r.act("another starts", |rt| rt.update(st::spawn));
    r.play(1.0);
    r.act("both step", |rt| rt.update(st::advance));
    r.play(0.6);
    r.act("the first finishes", |rt| rt.update(st::finish_oldest));
    r.play(1.0);
    r.act("the second fails", |rt| rt.update(st::fail_newest));
    r.play(1.0);
}

/// One background agent on its own: a card with one row that starts, steps and finishes.
fn one_agent(r: &mut Run) {
    use crate::agent_stories as st;
    r.act("an agent starts", |rt| rt.update(st::spawn));
    r.play(1.0);
    r.act("it steps", |rt| rt.update(st::advance));
    r.play(0.6);
    r.act("it steps again", |rt| rt.update(st::advance));
    r.play(0.6);
    r.act("it finishes", |rt| rt.update(st::finish_oldest));
    r.play(1.0);
}

/// "Delegating agent…" wakes into a running row three times: the first call opens the card,
/// the next two join it while being written and wake in place there.
fn agent_wakes(r: &mut Run) {
    use crate::agent_stories as st;
    for _ in 0..3 {
        r.act("a call is written", |rt| rt.update(st::delegate));
        r.play(1.0);
        r.act("its agent starts", |rt| rt.update(st::spawn));
        r.play(0.8);
        r.act("it steps", |rt| rt.update(st::advance));
        r.play(0.6);
    }
}

fn dictation_circs() -> Vec<Circ> {
    let mut v = wide_and_narrow(Circ::default());
    v.push(Circ { prompt: Prompt::Lines, ..Circ::default() });
    v.push(Circ { length: Length::Empty, ..Circ::default() });
    v
}

fn dictation_key() -> KeyEvent {
    KeyEvent { key: Key::Command('m'), mods: Mods { cmd: true, shift: true, ..Mods::default() } }
}

/// The Question answers story's variant `v`, asked and then answered.
fn compaction(r: &mut Run, quiet: bool, runs: f64) {
    r.act("background compaction", |rt| rt.update(move |s, _| s.compactions.fallback = quiet));
    r.event("session.compaction.started", json!({ "sessionID": SID, "eventID": "evt_gatecompaction", "reason": "auto" }));
    r.play(runs / 2.0);
    r.event("session.compaction.delta", json!({ "sessionID": SID, "text": "## Goal\nKeep reconnects alive." }));
    r.play(runs / 2.0);
    let tokens = json!({ "input": 10_412, "output": 1_034, "reasoning": 96, "cache": { "read": 1_880, "write": 0 } });
    r.event("session.compaction.ended", json!({ "sessionID": SID, "reason": "auto", "text": "## Goal\nKeep reconnects alive.", "tokens": tokens }));
    r.quiet();
    r.play(1.5);
}

fn answer_question(r: &mut Run, v: usize) {
    use crate::question_stories as st;
    let send = |r: &mut Run, events: Vec<(&str, serde_json::Value)>| {
        for (ty, mut d) in events {
            d["sessionID"] = SID.into();
            r.event(ty, d);
        }
    };
    send(r, st::asked(v));
    r.play(0.8);
    send(r, st::answered(v));
    r.play_out(1.0);
}

/// Opening a session through the real gate: cold (ready at 100 ms), prefetched, slow.
fn open_runs() -> Vec<(String, Run)> {
    let mut v = Vec::new();
    for (at, name) in [(0, "cold"), (1, "prefetched"), (2, "slow")] {
        for narrow in [false, true] {
            let mut app = App::empty();
            crate::open_story::setup(&mut app, at);
            let size = if narrow { (720.0, 820.0) } else { (1280.0, 820.0) };
            let mut rt = Runtime::new(app, crate::view::app, size, Env::new(true, 1.0));
            rt.poll = Some(crate::poll);
            rt.on_key = Some(crate::on_key);
            rt.frame(0.0);
            let circ = Circ { narrow, ..Circ::default() };
            let mut run = Run { rt, rec: super::recording(size), circ };
            run.idle(0.5);
            run.step();
            v.push((format!("open-session/{name}/{}", if narrow { "narrow" } else { "wide" }), run));
        }
    }
    v
}

fn open(r: &mut Run) {
    let state = |s: &App| (s.route.clone(), s.opening.is_some(), s.transcripts.get("ses_open_story").map(|t| t.messages.len()));
    let mut seen = state(&r.rt.state);
    // A prefetched session swaps in on the frame of the keypress itself.
    r.act("open", |rt| rt.key(KeyEvent { key: Key::Text("o".into()), mods: Mods::default() }));
    for i in 0..=(1.5 / DT) as usize {
        if i > 0 {
            r.step_n(1);
        }
        let now = state(&r.rt.state);
        if now != seen {
            r.rec.loads.push(r.rec.frames - 1);
            seen = now;
        }
    }
}

/// A visible tool group's disclosure in the timeline (`path`'s, or the first): its path and
/// a point on it.
fn trigger(r: &Run, path: Option<&str>) -> Option<(String, f32, f32)> {
    let els = r.rt.elements();
    let t = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some("trigger") && e.path.contains("/c-") && e.visible[3] >= 12.0
        && path.is_none_or(|p| e.path == p))?;
    Some((t.path.clone(), t.rect[0] + 12.0, t.visible[1] + t.visible[3] / 2.0))
}

/// Taps [`trigger`]; returns the path tapped.
fn tap_trigger(r: &mut Run, path: Option<&str>) -> Option<String> {
    let (path, x, y) = trigger(r, path)?;
    r.act("tap a tool group", |rt| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    });
    Some(path)
}

impl Run {
    pub fn step_n(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }
}

/// Every cell: each action in each of its circumstances, then opening a session.
fn cells() -> Vec<(String, Job)> {
    let mut jobs: Vec<(String, Job)> = Vec::new();
    for a in ACTIONS {
        for c in (a.circs)() {
            let run = a.run;
            jobs.push((format!("{}/{}", a.name, c.name()), Box::new(move || {
                let mut r = Run::new(c);
                run(&mut r);
                r.rec
            })));
        }
    }
    for i in 0..6 {
        let (name, narrow) = (["cold", "prefetched", "slow"][i / 2], i % 2 == 1);
        jobs.push((format!("open-session/{name}/{}", if narrow { "narrow" } else { "wide" }), Box::new(move || {
            let (_, mut r) = open_runs().swap_remove(i);
            open(&mut r);
            r.rec
        })));
    }
    jobs
}

#[test]
fn every_message_animation_passes_the_motion_gate() {
    motion_gate::gate(cells(), ALLOW);
}
