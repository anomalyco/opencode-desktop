//! The storybook (⌘⇧D): fixed example states of the composer and the question dock, so their
//! look and the morph between them can be checked and tuned with knobs (⌘⇧K) without waiting
//! for a real session to get there. Each story is a fresh fixture world; the app as it was waits
//! beside it untouched and comes back on Esc or ⌘⇧D. Answers and sends in a story go nowhere.

use opencode_kit::model::{Delivery, InboxItem, Message};
use rune::{Color, El, Key, KeyEvent, TextEdit, Tx, col, row, spacer, text};
use serde_json::{Value, json};

use crate::palette::Pal;
use crate::view::{UI, UI_SMALL};
use crate::{App, FIXTURE, Route, open_scene};

pub struct Story {
    pub name: &'static str,
    build: fn(&mut App),
}

/// The open story, and the app as it was before the storybook opened.
pub struct Storybook {
    pub at: usize,
    real: Box<App>,
}

/// The morph story: the dock opens, stays a beat, closes, and again.
pub const MORPH: usize = 0;
/// Seconds the dock and the composer each stay in the morph story.
const BEAT: f64 = 1.6;
/// A subagent of the fixture's open session.
const CHILD: &str = "ses_fx0002aaaaaaaaaaaaaaaaaa";

pub const ALL: &[Story] = &[
    Story { name: "Morph loop", build: |s| schedule(s, 0.5) },
    Story { name: "Question, one", build: |s| ask(s, form(s, &[0])) },
    Story { name: "Questions, two", build: |s| ask(s, form(s, &[0, 1])) },
    Story { name: "Question, multi-select", build: |s| ask(s, form(s, &[1])) },
    Story { name: "Question, long", build: long },
    Story { name: "Question, typing", build: typing },
    Story { name: "Composer", build: |_| {} },
    Story { name: "Composer, draft", build: |s| s.composer = TextEdit::new("Also check that the sidebar recovers after the stream reconnects, and add a test for it") },
    Story { name: "Composer, pending", build: pending },
    Story { name: "Composer, subagent", build: |s| { s.running.insert(CHILD.into()); } },
];

/// Opens story `i`, keeping the app as it was to come back to.
pub fn open(s: &mut App, i: usize, tx: &mut Tx) {
    let mut w = App::empty();
    w.load_fixture(FIXTURE);
    carry(s, &mut w);
    w.clock = s.clock;
    // Relative times read as the fixture has them, however long the app has run.
    w.base_ms -= (s.clock * 1000.0) as i64;
    w.waker = s.waker.clone();
    if let Some(waker) = w.waker.clone() {
        w.attach.set_waker(waker.clone());
        w.icons.set_waker(waker);
    }
    open_scene(&mut w);
    w.finish_stream();
    (ALL[i].build)(&mut w);
    let mut old = std::mem::replace(s, w);
    let real = old.story.take().map_or_else(|| Box::new(old), |b| b.real);
    s.story = Some(Storybook { at: i, real });
    tx.settle();
}

/// A snapshot scene: the story called `name`.
pub fn scene(s: &mut App, name: &str) {
    let i = ALL.iter().position(|st| st.name == name).unwrap_or_else(|| panic!("no story {name}"));
    open(s, i, &mut Tx::default());
}

/// Back to the app as it was, with the knobs as the storybook left them.
pub fn close(s: &mut App, tx: &mut Tx) {
    let Some(b) = s.story.take() else { return };
    let mut real = *b.real;
    carry(s, &mut real);
    *s = real;
    tx.settle();
}

/// What stays the same between the app and its stories.
fn carry(from: &mut App, to: &mut App) {
    to.knobs = std::mem::take(&mut from.knobs);
    to.zoom = from.zoom;
    to.sidebar_w = from.sidebar_w;
    to.sidebar_open = from.sidebar_open;
    to.edge = from.edge;
    to.reduced_motion = from.reduced_motion;
    to.debug = from.debug;
}

/// The fixture's question form with only the questions in `keep`.
fn form(s: &App, keep: &[usize]) -> Value {
    let mut q = s.fixture["question"].clone();
    let fields = q["fields"].as_array().cloned().unwrap_or_default();
    q["fields"] = keep.iter().filter_map(|i| fields.get(*i).cloned()).collect();
    q
}

fn ask(s: &mut App, q: Value) {
    s.apply_event("form.created", &q);
}

fn long(s: &mut App) {
    let mut q = form(s, &[0]);
    q["fields"][0]["title"] = "Backoff test and where its clock comes from".into();
    q["fields"][0]["description"] = "The reconnect loop sleeps between attempts, doubling each time up to 30 seconds with some jitter. A test that waits on real timers would take minutes and flake on a busy machine, so how should the backoff test control time, and should it also cover the jitter staying in range?".into();
    q["fields"][0]["options"][0]["description"] = "Inject a clock into the reconnect loop and step it by hand, so each attempt's delay is checked exactly and the test runs in milliseconds whatever the machine is doing.".into();
    q["fields"][0]["options"][2]["description"] = "Pull the delay sequence out as a pure function of the attempt number and a seeded random source, and test only that, leaving the loop itself untested.".into();
    ask(s, q);
}

fn typing(s: &mut App) {
    ask(s, form(s, &[0]));
    s.question_type("Fake clock, but keep one slow test on real timers");
}

/// The open session still running, with a steer and a queued prompt waiting for it.
fn pending(s: &mut App) {
    let Route::Session(id) = s.route.clone() else { return };
    if let Some(Message::Assistant(a)) = s.transcripts.get_mut(&id).and_then(|t| t.messages.last_mut()) {
        a.completed = None;
    }
    s.running.insert(id.clone());
    let inbox = s.inboxes.entry(id).or_default();
    let item = |id: &str, text: &str, delivery| InboxItem { id: id.into(), text: text.into(), delivery, files: vec![] };
    inbox.upsert(item("msg_story_steer", "Use the fake clock from the test utils, not a new one", Delivery::Steer));
    inbox.upsert(item("msg_story_queue", "Then write up what changed in the PR description", Delivery::Queue));
}

/// The morph story's next round: the question at `delay` seconds from now, answered a beat later.
fn schedule(s: &mut App, delay: f64) {
    let q = form(s, &[0]);
    let replied = json!({ "id": q["id"], "sessionID": q["sessionID"] });
    let at = s.clock + delay;
    s.playback.push_back((at, "form.created".into(), q));
    s.playback.push_back((at + BEAT, "form.replied".into(), replied));
    s.wake_at_playback();
}

/// Keeps the morph story going: the next round once the last one has played.
/// The loop is for watching, so the composer it lands on doesn't take the keys.
pub fn poll(s: &mut App) {
    if s.story.as_ref().is_none_or(|b| b.at != MORPH) {
        return;
    }
    s.composer.focused = false;
    if s.playback.is_empty() {
        schedule(s, BEAT);
    }
}

/// Plays the morph story's round again now: back to the composer first if the dock is up.
fn replay(s: &mut App, tx: &mut Tx) {
    s.playback.clear();
    let mut delay = 0.0;
    if s.question_shown() {
        let q = form(s, &[0]);
        s.apply_event("form.replied", &json!({ "id": q["id"], "sessionID": q["sessionID"] }));
        s.morph(tx);
        delay = BEAT / 2.0;
    }
    schedule(s, delay);
}

/// ⌘⇧D opens and closes the storybook. In it, J and K (and ↑ ↓ when the dock isn't up) move
/// between stories, and Space or R replays the morph story. Typing keeps its keys.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = ev.mods;
    if let Key::Command(c) = ev.key
        && c.eq_ignore_ascii_case(&'d')
        && m.shift
        && !m.alt
        && !m.ctrl
    {
        if s.story.is_some() { close(s, tx) } else { open(s, MORPH, tx) }
        return true;
    }
    let Some(at) = s.story.as_ref().map(|b| b.at) else { return false };
    if m.cmd || m.ctrl || m.alt || s.composer.focused || s.question_takes_text() {
        return false;
    }
    let n = ALL.len();
    let step = match &ev.key {
        Key::Text(t) if at == MORPH && (t == " " || t.eq_ignore_ascii_case("r")) => {
            replay(s, tx);
            return true;
        }
        Key::Text(t) if t == "j" => 1,
        Key::Text(t) if t == "k" => n - 1,
        Key::Down if !s.question_shown() => 1,
        Key::Up if !s.question_shown() => n - 1,
        _ => return false,
    };
    open(s, (at + step) % n, tx);
    true
}

/// The storybook's sidebar, in place of the tabs: one row per story.
pub fn list(s: &App, p: &Pal) -> Option<El> {
    let at = s.story.as_ref()?.at;
    let mut c = col().key("stories").w_full().grow(1.0).gap(4.0);
    c = c.child(row().key("stories-title").h(28.0).pl(6.0).center_y().child(text("Storybook").font(UI).color(p.faint)));
    for (i, st) in ALL.iter().enumerate() {
        let on = at == i;
        c = c.child(
            row()
                .key(format!("story-{i}"))
                .role("button")
                .label(st.name)
                .h(28.0)
                .w_full()
                .px(8.0)
                .center_y()
                .radius(6.0)
                .fill(if on { p.pressed } else { Color::TRANSPARENT })
                .hover(p.hover)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| open(s, i, tx))
                .child(text(st.name).font(UI).color(if on { p.text } else { p.muted }).truncate()),
        );
    }
    let hint = if at == MORPH { "Space replays · J K moves · Esc leaves" } else { "J K moves · Esc leaves · ⌘⇧K knobs" };
    Some(c.child(spacer()).child(text(hint).font(UI_SMALL).color(p.faint).px(6.0)))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use opencode_kit::client::Server;
    use rune::{Env, Key, KeyEvent, Mods, Runtime, TextEdit};

    use crate::{App, FIXTURE, Source, open_scene};

    fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
        rt.key(KeyEvent { key, mods });
        rt.frame(1.0 / 120.0);
    }

    #[test]
    fn the_storybook_leaves_the_app_as_it_was_and_sends_nothing() {
        let mut app = App::empty();
        app.load_fixture(FIXTURE);
        open_scene(&mut app);
        app.finish_stream();
        app.composer = TextEdit::new("a real draft");
        let (w, writes) = channel();
        app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        rt.frame(0.0);

        press(&mut rt, Key::Command('d'), Mods { cmd: true, shift: true, ..Mods::default() });
        assert_eq!(rt.state.story.as_ref().map(|b| b.at), Some(super::MORPH));
        assert!(matches!(rt.state.source, Source::Fixture), "a story is its own fixture world");
        assert!(rt.find("story-0").is_some() && rt.find("tabs").is_none(), "stories in place of the tabs");

        // The morph story opens and closes the dock on its own, again and again.
        rt.poll = Some(crate::poll);
        let mut flips = 0;
        let mut shown = rt.state.question_shown();
        for _ in 0..120 * 7 {
            rt.frame(1.0 / 120.0);
            if rt.state.question_shown() != shown {
                shown = !shown;
                flips += 1;
            }
        }
        assert!(flips >= 3, "{flips} flips");

        // J moves to the next story; answering it closes the dock and sends nothing.
        press(&mut rt, Key::Text("j".into()), Mods::default());
        assert_eq!(rt.state.story.as_ref().map(|b| b.at), Some(1));
        assert!(rt.state.question_shown());
        press(&mut rt, Key::Enter, Mods::default());
        assert!(!rt.state.question_shown(), "answered");
        rt.poll = None;

        // The composer has the keyboard back: Esc leaves it, then Esc leaves the storybook.
        assert!(rt.state.composer.focused);
        press(&mut rt, Key::Escape, Mods::default());
        assert!(!rt.state.composer.focused && rt.state.story.is_some());
        press(&mut rt, Key::Escape, Mods::default());
        assert!(rt.state.story.is_none());
        assert!(matches!(rt.state.source, Source::Live { .. }));
        assert_eq!(rt.state.composer.text, "a real draft");
        assert!(writes.try_recv().is_err(), "nothing reached the server");
    }
}
