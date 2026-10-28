//! The matrix: each action, the circumstances it runs in, and what's allowed with a reason.

use rune::{Env, Key, KeyEvent, Mods, Runtime};
use serde_json::json;

use super::check::{Finding, Recording};
use super::{Circ, DT, Dock, Length, Prompt, Run, SID};
use crate::App;

/// Findings that are the design, matched on kind, element and cell (substrings, "" for any).
/// Every row needs its reason.
const ALLOW: &[(&str, &str, &str, &str)] = &[
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
    ("pop-in", "quote-toolbar", "", "the Quote button appears over a settled selection at once and goes at once, as Desktop's does (quote::toolbar)"),
    ("pop-out", "quote-toolbar", "", "the Quote button appears over a settled selection at once and goes at once, as Desktop's does (quote::toolbar)"),
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
    for prompt in [Prompt::Lines, Prompt::Image] {
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
    Action { name: "steer", circs: || sends(streaming(), &[Dock::Pending]), run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(false);
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
    Action { name: "queue-lands", circs: || {
        let mut v = places(streaming());
        v.push(Circ { dock: Dock::Pending, ..streaming() });
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
    Action { name: "jump-latest", circs: || vec![Circ::default(), Circ { narrow: true, ..Circ::default() }], run: |r| {
        r.swipe_up(1400.0, 1.0);
        if r.rt.find("jump-latest").is_none() { return; }
        r.act("tap jump-latest", |rt| Run::tap(rt, "jump-latest"));
        r.play(1.5);
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
];

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
            let mut run = Run { rt, rec: Recording::new(size, 1.0 / DT), circ };
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

/// `MOTION_GATE_DUMP=<cell>|<path part>[|from|to]` writes those tracks' frames to
/// `MOTION_GATE_OUT`.dump (or stderr).
fn dump(cell: &str, rec: &Recording) {
    sheet(cell, rec);
    let Ok(spec) = std::env::var("MOTION_GATE_DUMP") else { return };
    let mut p = spec.split('|');
    let (Some(c), Some(path)) = (p.next(), p.next()) else { return };
    if cell != c { return; }
    let from = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let to = p.next().and_then(|x| x.parse().ok()).unwrap_or(usize::MAX);
    let text = super::check::dump(rec, path, from, to);
    match std::env::var("MOTION_GATE_OUT") {
        Ok(out) => std::fs::write(format!("{out}.dump"), text).unwrap(),
        Err(_) => eprint!("{text}"),
    }
}

/// `MOTION_GATE_SHEET=<cell>|<frame>[|<path part>]` writes a contact strip of the frames
/// around `frame` to `MOTION_GATE_OUT`.sheet.png, the path part's outlines in red.
fn sheet(cell: &str, rec: &Recording) {
    let Ok(spec) = std::env::var("MOTION_GATE_SHEET") else { return };
    let mut p = spec.split('|');
    let (Some(c), Some(Ok(at))) = (p.next(), p.next().map(str::parse)) else { return };
    if cell != c { return; }
    let out = std::env::var("MOTION_GATE_OUT").unwrap_or_else(|_| std::env::temp_dir().join("motion-gate").display().to_string());
    super::check::sheet(rec, at, p.next().unwrap_or("")).save(format!("{out}.sheet.png")).unwrap();
}

pub struct Cell {
    pub name: String,
    pub findings: Vec<Finding>,
    pub frames: usize,
    pub skipped: bool,
}

fn allowed(f: &Finding, cell: &str) -> Option<&'static str> {
    ALLOW.iter().find(|a| f.kind == a.0 && f.element.contains(a.1) && cell.contains(a.2)).map(|a| a.3)
}

/// Every cell, run in parallel; `filter` keeps cells whose name contains it.
pub fn run_all(filter: &str) -> Vec<Cell> {
    type Job = Box<dyn FnOnce() -> Cell + Send>;
    let mut jobs: Vec<Job> = Vec::new();
    for a in ACTIONS {
        for c in (a.circs)() {
            let name = format!("{}/{}", a.name, c.name());
            if !name.contains(filter) { continue; }
            let run = a.run;
            jobs.push(Box::new(move || {
                let mut r = Run::new(c);
                run(&mut r);
                let frames = r.rec.frames;
                dump(&name, &r.rec);
                Cell { name, skipped: frames < 10, frames, findings: r.finish() }
            }));
        }
    }
    for i in 0..6 {
        let (name, narrow) = (["cold", "prefetched", "slow"][i / 2], i % 2 == 1);
        let full = format!("open-session/{name}/{}", if narrow { "narrow" } else { "wide" });
        if !full.contains(filter) { continue; }
        jobs.push(Box::new(move || {
            let (name, mut r) = open_runs().swap_remove(i);
            open(&mut r);
            let frames = r.rec.frames;
            dump(&name, &r.rec);
            Cell { name, frames, skipped: false, findings: r.finish() }
        }));
    }
    let jobs = std::sync::Mutex::new(jobs.into_iter().enumerate().collect::<Vec<_>>());
    let out = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(12);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(scope, || loop {
                let Some((i, job)) = jobs.lock().unwrap().pop() else { break };
                let cell = job();
                out.lock().unwrap().push((i, cell));
            }).unwrap();
        }
    });
    let mut cells = out.into_inner().unwrap();
    cells.sort_by_key(|c| c.0);
    cells.into_iter().map(|c| c.1).collect()
}

#[test]
fn every_message_animation_passes_the_motion_gate() {
    let filter = std::env::var("MOTION_GATE").unwrap_or_default();
    let started = std::time::Instant::now();
    let cells = run_all(&filter);
    let mut report = String::new();
    let mut failing = 0;
    let mut allowed_n = 0;
    let list_allowed = std::env::var_os("MOTION_GATE_ALLOWED").is_some();
    for c in &cells {
        if c.skipped {
            failing += 1;
            report += &format!("SKIP {} (its setup didn't apply)\n", c.name);
        }
        for f in &c.findings {
            match allowed(f, &c.name) {
                Some(why) => {
                    allowed_n += 1;
                    if list_allowed {
                        report += &format!("allowed  {}  in {}  ({why})\n", f, c.name);
                    }
                }
                None => {
                    failing += 1;
                    report += &format!("{}  in {}\n", f, c.name);
                }
            }
        }
    }
    let frames: usize = cells.iter().map(|c| c.frames).sum();
    let summary = format!("motion gate: {} cells, {frames} frames, {failing} findings ({allowed_n} allowed) in {:.1} s\n", cells.len(), started.elapsed().as_secs_f32());
    let mut kinds: Vec<&str> = cells.iter().flat_map(|c| c.findings.iter().filter(|f| allowed(f, &c.name).is_none()).map(|f| f.kind)).collect();
    kinds.sort();
    kinds.dedup();
    for r in super::check::RULES.iter().filter(|r| kinds.contains(&r.kind)) {
        report += &format!("  {}: {} ({})\n", r.kind, r.means, r.threshold);
    }
    report = summary.clone() + &report;
    if let Ok(path) = std::env::var("MOTION_GATE_OUT") {
        std::fs::write(path, &report).unwrap();
    }
    assert!(failing == 0, "{report}");
    eprint!("{summary}");
}
