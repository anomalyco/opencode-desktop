//! The storybook (⌘⇧D), on `rune-playground`: one story per component, grouped by type, each
//! alone on a plain stage with only its knobs (the values ⌘⇧K tunes) and its triggers beside it.
//! A story is a fresh fixture world; opening it sets the component up and starts its motion, and
//! R opens it again, so a replay runs the same every time. The app as it was waits beside the
//! storybook untouched and comes back on Esc or ⌘⇧D. Answers and sends in a story go nowhere.

use std::sync::{Arc, OnceLock};

use opencode_kit::model::{Attachment, Delivery, InboxItem, Message};
use rune::{El, Key, KeyEvent, TextEdit, Tx, col};
use rune_knobs::{Action, Group, Knob};
use rune_playground::{Look, Move};
use serde_json::{Value, json};

use crate::knobs::*;
use crate::palette::Pal;
use crate::turns::{self, Block};
use crate::{App, FIXTURE, Route, open_scene, open_story, view};

/// The open story, and the app as it was before the storybook opened.
pub struct Storybook {
    pub at: usize,
    real: Box<App>,
}

pub struct Story {
    group: &'static str,
    name: &'static str,
    knobs: &'static [Group],
    keys: &'static [Action<App>],
    /// Sets the story up in its fresh world and queues its motion with `App::play`.
    open: fn(&mut App, &mut Tx),
    view: fn(&App, &Pal) -> El,
}

impl rune_playground::Story<App> for Story {
    fn group(&self) -> &'static str {
        self.group
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn knobs(&self) -> &'static [Group] {
        self.knobs
    }
    fn keys(&self) -> &'static [Action<App>] {
        self.keys
    }
}

/// What the Composer story shows.
pub const COMPOSER: Knob = Knob::choice("Composer state", &["Empty", "Draft", "Queued", "Subagent running"], 0);
/// The Question story's form.
pub const QUESTION: Knob = Knob::choice("Question form", &["One", "Two", "Multi-select", "Long", "Typing"], 0);
/// What the Custom answer story's typed answer holds.
pub const CUSTOM_ANSWER: Knob = Knob::choice("Typed answer", &["Typing", "Long, wrapped", "Left the field", "Empty"], 0);

const COMPOSER_KNOBS: &[Group] = &[Group::new("Composer", "stage", &[COMPOSER, CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING])];
const QUESTION_KNOBS: &[Group] = &[Group::new("Question", "stage", &[QUESTION])];
const CUSTOM_KNOBS: &[Group] = &[Group::new("Custom answer", "stage", &[CUSTOM_ANSWER])];
const MORPH_KNOBS: &[Group] = &[QUESTION_DOCK];
const GLOW_KNOBS: &[Group] = &[Group::new("Arrival glow", "stage", &[GLOW_LENGTH, GLOW_RISE, GLOW_DECAY, GLOW_TAIL, GLOW_FADE, ROW_GROWTH])];
const ROW_KNOBS: &[Group] = &[Group::new("Rows", "stage", &[ROW_GROWTH])];
const WORKING_KNOBS: &[Group] = &[WORKING];
const ACTIONS_KNOBS: &[Group] = &[Group::new("Turn actions", "stage", &[ACTIONS_TAIL])];
const SUBAGENT_KNOBS: &[Group] = &[Group::new("Subagent", "stage", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING])];
const IMAGE_KNOBS: &[Group] = &[Group::new("Images", "stage", &[IMAGE_FLIGHT, ROW_GROWTH])];
const JUMP_KNOBS: &[Group] = &[Group::new("Jump to latest", "stage", &[JUMP_PLACE, JUMP_IN, JUMP_OUT, JUMP_RISE])];

pub(crate) const REPLAY: Action<App> = Action { cap: "R", label: "Replay", run: replay };

const MORPH_KEYS: &[Action<App>] = &[
    Action { cap: "R", label: "Replay the loop", run: replay },
    Action { cap: "O", label: "Open the dock", run: |s, tx| {
        s.play.clear();
        if !s.question_shown() {
            emit(s, "form.created", form(s, &[0]), tx);
        }
    } },
    Action { cap: "C", label: "Close it", run: |s, tx| {
        s.play.clear();
        if s.question_shown() {
            emit(s, "form.replied", replied(s), tx);
        }
    } },
];
const UNDER_QUEUE_KEYS: &[Action<App>] = &[
    REPLAY,
    Action { cap: "Q", label: "Queue one", run: queue },
    Action { cap: "O", label: "Open the dock", run: |s, tx| if !s.question_shown() { emit(s, "form.created", form(s, &[0]), tx) } },
    Action { cap: "C", label: "Close it", run: |s, tx| if s.question_shown() { emit(s, "form.replied", replied(s), tx) } },
    Action { cap: "D", label: "Deliver the oldest", run: deliver_queued },
];
const GLOW_KEYS: &[Action<App>] = &[REPLAY, Action { cap: "A", label: "Another arrives", run: arrive }];
const SEND_KEYS: &[Action<App>] = &[REPLAY, Action { cap: "S", label: "Send", run: send }];
const STREAM_KEYS: &[Action<App>] = &[REPLAY, Action { cap: "F", label: "Finish the stream", run: finish_stream }];
const TOOL_KEYS: &[Action<App>] = &[REPLAY, Action { cap: "T", label: "Fold or unfold the tools", run: fold_tools }];
const WORKING_KEYS: &[Action<App>] = &[
    Action { cap: "R", label: "Restart the run", run: replay },
    Action { cap: "L", label: "Change the label", run: relabel },
    Action { cap: "D", label: "End the run", run: finish },
];
const TURN_KEYS: &[Action<App>] = &[REPLAY, Action { cap: "W", label: "Start working", run: start }, Action { cap: "F", label: "Finish", run: finish }];
const STEER_KEYS: &[Action<App>] = &[
    REPLAY,
    Action { cap: "S", label: "Steer", run: steer },
    Action { cap: "D", label: "Deliver", run: deliver },
    Action { cap: "Q", label: "Queue one", run: |s, tx| prompt(s, QUEUED_TEXT, true, tx) },
    Action { cap: "F", label: "Finish the turn", run: finish },
];
const QUEUE_KEYS: &[Action<App>] = &[
    REPLAY,
    Action { cap: "Q", label: "Queue one", run: queue },
    Action { cap: "D", label: "Deliver the oldest", run: deliver_queued },
];
const SUBAGENT_KEYS: &[Action<App>] = &[REPLAY, Action { cap: "S", label: "Start the subagent", run: start_child }, Action { cap: "F", label: "Finish it", run: finish_child }];
const IMAGE_KEYS: &[Action<App>] = &[
    REPLAY,
    Action { cap: "O", label: "Open the first image", run: |s, _| view_image(s) },
    Action { cap: "C", label: "Close it", run: |s, _| s.attach.viewing = None },
];
const JUMP_KEYS: &[Action<App>] = &[
    REPLAY,
    Action { cap: "U", label: "Scroll up", run: scroll_up },
    Action { cap: "B", label: "Back to latest", run: |s, tx| tx.scroll_to_bottom(scroll_key(s)) },
];

pub static STORIES: &[Story] = &[
    Story { group: "Composer", name: "Composer", knobs: COMPOSER_KNOBS, keys: &[], open: composer_open, view: composer_view },
    Story { group: "Composer", name: "Queue dock", knobs: ROW_KNOBS, keys: QUEUE_KEYS, open: queue_open, view: session_view },
    Story { group: "Composer", name: "Dictation", knobs: &[], keys: crate::dictation::STORY_KEYS, open: crate::dictation::story_open, view: composer_view },
    Story { group: "Question dock", name: "Question", knobs: QUESTION_KNOBS, keys: &[], open: question_open, view: composer_view },
    Story { group: "Question dock", name: "Custom answer", knobs: CUSTOM_KNOBS, keys: &[], open: custom_open, view: composer_view },
    Story { group: "Question dock", name: "Morph", knobs: MORPH_KNOBS, keys: MORPH_KEYS, open: morph_open, view: session_view },
    Story { group: "Question dock", name: "Under a queue", knobs: MORPH_KNOBS, keys: UNDER_QUEUE_KEYS, open: under_queue_open, view: session_view },
    Story { group: "Sidebar", name: "Rename and menus", knobs: crate::session_menu_stories::KNOBS, keys: crate::session_menu_stories::KEYS, open: crate::session_menu_stories::open, view: crate::session_menu_stories::view },
    Story { group: "Header", name: "Session header", knobs: &[], keys: &[], open: |s, _| s.finish_stream(), view: header_view },
    Story { group: "Timeline", name: "Arrival glow", knobs: GLOW_KNOBS, keys: GLOW_KEYS, open: glow_open, view: session_view },
    Story { group: "Timeline", name: "Send", knobs: ROW_KNOBS, keys: SEND_KEYS, open: send_open, view: session_view },
    Story { group: "Timeline", name: "Streaming text", knobs: ROW_KNOBS, keys: STREAM_KEYS, open: stream_open, view: session_view },
    Story { group: "Timeline", name: "Tool rows", knobs: ROW_KNOBS, keys: TOOL_KEYS, open: tools_open, view: session_view },
    Story { group: "Timeline", name: "Selection", knobs: &[], keys: crate::selection_stories::KEYS, open: crate::selection_stories::open, view: session_view },
    Story { group: "Timeline", name: "Question answers", knobs: crate::question_stories::KNOBS, keys: crate::question_stories::KEYS, open: crate::question_stories::open, view: session_view },
    Story { group: "Timeline", name: "Working", knobs: WORKING_KNOBS, keys: WORKING_KEYS, open: working_open, view: session_view },
    Story { group: "Timeline", name: "Steer and Pending", knobs: ROW_KNOBS, keys: STEER_KEYS, open: steer_open, view: session_view },
    Story { group: "Timeline", name: "Subagent", knobs: SUBAGENT_KNOBS, keys: SUBAGENT_KEYS, open: subagent_open, view: session_view },
    Story { group: "Timeline", name: "Subagent rows", knobs: crate::agent_stories::KNOBS, keys: crate::agent_stories::KEYS, open: crate::agent_stories::open, view: session_view },
    Story { group: "Timeline", name: "Images", knobs: IMAGE_KNOBS, keys: IMAGE_KEYS, open: images_open, view: session_view },
    Story { group: "Timeline", name: "Jump to latest", knobs: JUMP_KNOBS, keys: JUMP_KEYS, open: jump_open, view: session_view },
    Story { group: "Timeline", name: "Turn actions", knobs: ACTIONS_KNOBS, keys: TURN_KEYS, open: actions_open, view: session_view },
    Story { group: "Navigation", name: "Back and forward", knobs: &[], keys: crate::history_story::KEYS, open: crate::history_story::open, view: history_view },
    Story { group: "Session opening", name: open_story::NAMES[0], knobs: &[], keys: open_story::KEYS, open: |s, tx| open_story::open(s, 0, tx), view: opening_view },
    Story { group: "Session opening", name: open_story::NAMES[1], knobs: &[], keys: open_story::KEYS, open: |s, tx| open_story::open(s, 1, tx), view: opening_view },
    Story { group: "Session opening", name: open_story::NAMES[2], knobs: &[], keys: open_story::KEYS, open: |s, tx| open_story::open(s, 2, tx), view: opening_view },
];

/// Seconds the dock and the composer each stay in the morph loop.
const BEAT: f64 = 1.6;
/// A subagent of the fixture's open session.
const CHILD: &str = "ses_fx0002aaaaaaaaaaaaaaaaaa";
const DRAFT: &str = "Also check that the sidebar recovers after the stream reconnects, and add a test for it";
const SEND_TEXT: &str = "Can you also check the reconnect jitter?";
const STEER_TEXT: &str = "Use the fake clock from the test utils, not a new one";
const STEER_AGAIN: &str = "And keep the retry budget as it is";
const QUEUED_TEXT: &str = "Then write up what changed in the PR description";
const ARRIVED: &str = "msg_zzzzstory0000";

// Opening and leaving.

/// Opens story `i` in a fresh fixture world, keeping the app as it was to come back to, and
/// starts its motion.
pub fn open(s: &mut App, i: usize, tx: &mut Tx) {
    let mut w = App::empty();
    w.load_fixture(FIXTURE);
    carry(s, &mut w);
    w.clock = s.clock;
    // Relative times read as the fixture has them, however long the app has run.
    w.base_ms -= (s.clock * 1000.0) as i64;
    w.now_ms = w.base_ms + (s.clock * 1000.0) as i64;
    w.waker = s.waker.clone();
    if let Some(waker) = w.waker.clone() {
        w.attach.set_waker(waker.clone());
        w.icons.set_waker(waker);
    }
    open_scene(&mut w);
    let mut old = std::mem::replace(s, w);
    let real = old.story.take().map_or_else(|| Box::new(old), |b| b.real);
    s.story = Some(Storybook { at: i, real });
    s.play.clear();
    (STORIES[i].open)(s, tx);
    tx.settle();
}

/// R: the open story from the start.
pub(crate) fn replay(s: &mut App, tx: &mut Tx) {
    if let Some(i) = s.story.as_ref().map(|b| b.at) {
        open(s, i, tx);
    }
}

/// A snapshot scene: the story called `name`, its motion starting at 0 s.
pub fn scene(s: &mut App, name: &str) {
    let i = rune_playground::find(STORIES, name).unwrap_or_else(|| panic!("no story {name}"));
    open(s, i, &mut Tx::default());
}

/// A snapshot scene on one of a story's variants.
pub fn scene_variant(s: &mut App, name: &str, knob: Knob, at: usize) {
    s.knobs.set(&knob, at as f32);
    scene(s, name);
}

/// Back to the app as it was, with the knobs and folds as the storybook left them.
pub fn close(s: &mut App, tx: &mut Tx) {
    let Some(b) = s.story.take() else { return };
    let mut real = *b.real;
    carry(s, &mut real);
    real.play.reset();
    *s = real;
    tx.settle();
}

/// What stays the same between the app and its stories.
fn carry(from: &mut App, to: &mut App) {
    to.knobs = std::mem::take(&mut from.knobs);
    to.play = std::mem::take(&mut from.play);
    to.zoom = from.zoom;
    to.sidebar_w = from.sidebar_w;
    to.sidebar_open = from.sidebar_open;
    to.edge = from.edge;
    to.reduced_motion = from.reduced_motion;
    to.debug = from.debug;
}

/// The triggers' queued steps, and a new variant setting its story up again. First thing in
/// `poll`, so the events a step plays land after it as arrivals do; true when one ran.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let at = s.story.as_ref().map(|b| b.at);
    let mut changed = false;
    if rune_playground::choices_changed(STORIES, at, &mut s.play, &s.knobs)
        && let Some(i) = at
    {
        open(s, i, tx);
        changed = true;
    }
    // The morph loop is for watching: the composer it lands on doesn't take the keys.
    if at.is_some_and(|i| STORIES[i].name == "Morph") {
        s.composer.focused = false;
    }
    rune_playground::poll(s, |s| &mut s.play, now, tx) || changed
}

/// ⌘⇧D opens and leaves the storybook; in it, `rune_playground::KEYS` and the story's triggers.
/// Typing in the composer or a question's answer keeps its keys.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let at = s.story.as_ref().map(|b| b.at);
    let toggle = matches!(ev.key, Key::Command('d' | 'D')) && ev.mods.shift;
    if at.is_some() && !toggle && (s.composer.focused || s.question_takes_text()) {
        return false;
    }
    match rune_playground::key(STORIES, at, &mut s.play, &mut s.knobs, ev) {
        Move::Pass => return false,
        Move::Run(step) => step(s, tx),
        Move::Open(i) => open(s, i, tx),
        Move::Leave => close(s, tx),
        Move::Done => tx.instant(),
    }
    true
}

// Drawing.

fn look(p: &Pal) -> Look {
    Look { text: p.text, muted: p.muted, faint: p.faint, selected: p.pressed, hover: p.hover, panel: p.bg_deep, line: p.ring, top: 0.0 }
}

fn knobs_style(p: &Pal) -> rune_knobs::Style {
    if p.dark {
        return rune_knobs::Style::dark();
    }
    rune_knobs::Style {
        bg: p.bg_base,
        inset: p.bg_deep,
        raised: p.pressed,
        fg: p.text,
        fg2: p.muted,
        fg3: p.faint,
        fill: p.layer1,
        fill_hover: p.hover,
        fill_selected: p.pressed,
        slider: p.faint.alpha(0.55),
        slider_hover: p.faint.alpha(0.7),
        slider_active: p.muted.alpha(0.8),
        handle: p.text,
        border: p.ring,
        rule: p.text.alpha(0.03),
        rim: p.ring,
        shadow: p.shadow,
        outline: p.text.alpha(0.55),
    }
}

/// The sidebar while the storybook is open: the stories under their groups, in place of the tabs.
pub fn list(s: &App, p: &Pal) -> Option<El> {
    let at = s.story.as_ref()?.at;
    Some(rune_playground::list(STORIES, Some(at), &s.play, look(p), |s: &mut App| &mut s.play, open))
}

/// The panel while the storybook is open: the story on its stage, its knobs and triggers beside.
pub fn page(s: &App, p: &Pal) -> El {
    let Some(st) = s.story.as_ref().and_then(|b| STORIES.get(b.at)) else { return rune_playground::empty() };
    let side = rune_playground::side(st, &s.knobs, &s.play, knobs_style(p), |s: &mut App| &mut s.knobs, look(p));
    rune_playground::page(rune_playground::stage((st.view)(s, p), false), side)
}

/// A component's frame on the stage: the panel's surface, as wide as the timeline's column.
fn frame(p: &Pal, key: &str) -> El {
    col().key(key).w_full().max_w(820.0).radius(10.0).fill(p.bg_base).border(0.5, p.ring).shadow(1.0, 2.0, p.shadow).clip_soft(0.0)
}

fn composer_view(s: &App, p: &Pal) -> El {
    frame(p, "story-composer").pt(16.0).child(view::story_composer(s, p))
}

fn header_view(s: &App, p: &Pal) -> El {
    frame(p, "story-header").child(view::story_header(s, p))
}

/// The timeline and what sits under it, in a window-sized frame.
fn session_view(s: &App, p: &Pal) -> El {
    frame(p, "story-session").h_full().max_h(680.0).child(view::story_session(s, p))
}

/// A session opening, the whole page with its header, before and after the click.
fn opening_view(s: &App, p: &Pal) -> El {
    frame(p, "story-opening").h_full().max_h(680.0).child(open_story::view(s, p))
}

/// The history strip over the page it is on.
fn history_view(s: &App, p: &Pal) -> El {
    frame(p, "story-history").h_full().max_h(680.0).child(crate::history_story::view(s, p))
}

// Setting up.

pub(crate) fn sid(s: &App) -> String {
    match &s.route {
        Route::Session(id) => id.clone(),
        _ => String::new(),
    }
}

fn scroll_key(s: &App) -> String {
    format!("scroll-{}", sid(s))
}

/// Plays a server event now, in this poll's playback, after the arrivals check.
pub(crate) fn emit(s: &mut App, ty: &str, mut data: Value, tx: &mut Tx) {
    if data["sessionID"].is_null() {
        data["sessionID"] = sid(s).into();
    }
    let at = s.clock;
    let i = s.playback.iter().position(|e| e.0 > at).unwrap_or(s.playback.len());
    s.playback.insert(i, (at, ty.into(), data));
    tx.wake_after(std::time::Duration::ZERO);
}

fn composer_open(s: &mut App, _: &mut Tx) {
    s.finish_stream();
    match s.knobs.get(&COMPOSER) as usize {
        1 => s.composer = TextEdit::new(DRAFT),
        2 => pending(s),
        3 => {
            s.running.insert(CHILD.into());
        }
        _ => {}
    }
}

/// The open session still running, with a steer and a queued prompt waiting for it.
fn pending(s: &mut App) {
    let id = sid(s);
    s.running.insert(id.clone());
    let inbox = s.inboxes.entry(id).or_default();
    let item = |id: &str, text: &str, delivery| InboxItem { id: id.into(), text: text.into(), delivery, files: vec![] };
    inbox.upsert(item("msg_story_steer", STEER_TEXT, Delivery::Steer));
    inbox.upsert(item("msg_story_queue", QUEUED_TEXT, Delivery::Queue));
}

/// The fixture's question form with only the questions in `keep`.
fn form(s: &App, keep: &[usize]) -> Value {
    let mut q = s.fixture["question"].clone();
    let fields = q["fields"].as_array().cloned().unwrap_or_default();
    q["fields"] = keep.iter().filter_map(|i| fields.get(*i).cloned()).collect();
    q
}

fn replied(s: &App) -> Value {
    let q = &s.fixture["question"];
    json!({ "id": q["id"], "sessionID": q["sessionID"] })
}

fn question_open(s: &mut App, _: &mut Tx) {
    s.finish_stream();
    let q = match s.knobs.get(&QUESTION) as usize {
        1 => form(s, &[0, 1]),
        2 => form(s, &[1]),
        3 => long(s),
        _ => form(s, &[0]),
    };
    s.apply_event("form.created", &q);
    if s.knobs.get(&QUESTION) as usize == 4 {
        s.question_type("Fake clock, but keep one slow test on real timers");
    }
}

/// The two-question form on its last question, with its typed answer as the knob says.
fn custom_open(s: &mut App, _: &mut Tx) {
    s.finish_stream();
    let q = form(s, &[0, 1]);
    s.apply_event("form.created", &q);
    s.question_last();
    let text = match s.knobs.get(&CUSTOM_ANSWER) as usize {
        1 | 2 => "Fake clock for the loop, but keep one slow end-to-end test on real timers so a regression in the scheduler still shows up somewhere",
        3 => "",
        _ => "All of them",
    };
    s.question_type(text);
    if matches!(s.knobs.get(&CUSTOM_ANSWER) as usize, 2 | 3) {
        s.question_leave_field();
    }
}

fn long(s: &App) -> Value {
    let mut q = form(s, &[0]);
    q["fields"][0]["title"] = "Backoff test and where its clock comes from".into();
    q["fields"][0]["description"] = "The reconnect loop sleeps between attempts, doubling each time up to 30 seconds with some jitter. A test that waits on real timers would take minutes and flake on a busy machine, so how should the backoff test control time, and should it also cover the jitter staying in range?".into();
    q["fields"][0]["options"][0]["description"] = "Inject a clock into the reconnect loop and step it by hand, so each attempt's delay is checked exactly and the test runs in milliseconds whatever the machine is doing.".into();
    q["fields"][0]["options"][2]["description"] = "Pull the delay sequence out as a pure function of the attempt number and a seeded random source, and test only that, leaving the loop itself untested.".into();
    q
}

/// The dock opens half a second in, then closes and opens every beat.
fn morph_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.play.after(0.5, ask, tx);
}

fn ask(s: &mut App, tx: &mut Tx) {
    emit(s, "form.created", form(s, &[0]), tx);
    s.play.after(BEAT, answer, tx);
}

fn answer(s: &mut App, tx: &mut Tx) {
    emit(s, "form.replied", replied(s), tx);
    s.play.after(BEAT, ask, tx);
}

/// A prompt waits in the queue while the session works, then the question opens under it:
/// the queue and the dock share one outline.
fn under_queue_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id);
    s.play.after(0.2, queue, tx);
    s.play.after(1.0, |s, tx| emit(s, "form.created", form(s, &[0]), tx), tx);
}

/// A prompt from another client lands and glows.
fn glow_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.play.after(0.4, arrive, tx);
}

fn arrive(s: &mut App, tx: &mut Tx) {
    s.sends += 1;
    let text = ["Sent from my phone: does this cover the laptop waking on battery too?", "And on a flaky hotel network?", "One more: what about a VPN dropping?"];
    let n = s.sends as usize;
    emit(s, "session.prompted", json!({ "messageID": format!("{ARRIVED}{n}"), "text": text[(n - 1) % text.len()] }), tx);
}

/// A draft in the composer, sent; the fixture's reply plays under it.
fn send_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.composer = TextEdit::new(SEND_TEXT);
    s.play.after(0.6, send, tx);
}

fn send(s: &mut App, tx: &mut Tx) {
    prompt(s, SEND_TEXT, false, tx);
}

/// Sends `text` as typed in the composer: a steer or, with `queue`, a queued prompt while the
/// session works.
fn prompt(s: &mut App, text: &str, queue: bool, tx: &mut Tx) {
    s.composer = TextEdit::new(text);
    s.submit(queue, tx);
}

/// The fixture's streaming turn, from its first event.
fn stream_open(s: &mut App, _: &mut Tx) {
    let id = sid(s);
    s.start_stream_if_due(&id);
}

fn finish_stream(s: &mut App, _: &mut Tx) {
    while let Some((_, ty, data)) = s.playback.pop_front() {
        s.apply_event(&ty, &data);
    }
}

/// A send whose reply calls two tools; the group they fold into opens and closes after.
fn tools_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.play.after(0.4, send, tx);
    s.play.after(4.6, fold_tools, tx);
    s.play.after(6.2, fold_tools, tx);
}

/// Opens or closes the last group of tools in the timeline.
fn fold_tools(s: &mut App, _: &mut Tx) {
    let Some(t) = s.transcripts.get(&sid(s)) else { return };
    let mut steps = Vec::new();
    for m in &t.messages {
        match m {
            Message::Assistant(a) => steps.push(a),
            _ => steps.clear(),
        }
    }
    let key = turns::fold(&steps, false).into_iter().rev().find_map(|b| match b {
        Block::Group { key, .. } => Some(key),
        _ => None,
    });
    if let Some(k) = key
        && !s.expanded.remove(&k)
    {
        s.expanded.insert(k);
    }
}

/// The finished session starts working: Working joins the end with its shimmer.
fn working_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.play.after(0.4, start, tx);
}

fn start(s: &mut App, tx: &mut Tx) {
    emit(s, "session.execution.started", json!({}), tx);
}

fn finish(s: &mut App, tx: &mut Tx) {
    emit(s, "session.execution.succeeded", json!({}), tx);
}

/// Labels of different widths, for a label that changes mid-sweep.
const LABELS: &[&str] = &["Working", "Thinking", "Running the tests", "Searching the codebase"];

fn relabel(s: &mut App, _: &mut Tx) {
    let at = LABELS.iter().position(|l| *l == s.working.label()).unwrap_or(0);
    let now = s.clock;
    s.working.relabel(LABELS[(at + 1) % LABELS.len()], now);
}

/// Working, until the turn finishes and its actions take its place.
fn actions_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id);
    s.play.after(1.0, finish, tx);
}

/// A steer sent while the session works rises as Pending, and lands when it's delivered.
fn steer_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id);
    s.composer = TextEdit::new(STEER_TEXT);
    s.play.after(0.5, |s, tx| prompt(s, STEER_TEXT, false, tx), tx);
    s.play.after(2.3, deliver, tx);
}

fn steer(s: &mut App, tx: &mut Tx) {
    let steering = s.inbox_for(&sid(s)).map_or(0, |i| i.steering().count());
    prompt(s, if steering == 0 { STEER_TEXT } else { STEER_AGAIN }, false, tx);
}

/// The server delivers the oldest steer.
fn deliver(s: &mut App, tx: &mut Tx) {
    let Some(id) = s.inbox_for(&sid(s)).and_then(|i| i.steering().next()).map(|i| i.id.clone()) else { return };
    emit(s, "session.inbox.delivered", json!({ "inboxID": id }), tx);
}

/// Two prompts queued while the session works open the dock out of the composer's top edge;
/// delivered one by one, they leave it and it closes back behind the composer.
fn queue_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id);
    s.play.after(0.4, queue, tx);
    s.play.after(1.2, queue, tx);
    s.play.after(2.4, deliver_queued, tx);
    s.play.after(3.2, deliver_queued, tx);
}

fn queue(s: &mut App, tx: &mut Tx) {
    let queued = s.inbox_for(&sid(s)).map_or(0, |i| i.queued().count());
    prompt(s, [QUEUED_TEXT, STEER_AGAIN][queued % 2], true, tx);
}

/// The server delivers the oldest queued prompt.
fn deliver_queued(s: &mut App, tx: &mut Tx) {
    let Some(id) = s.inbox_for(&sid(s)).and_then(|i| i.queued().next()).map(|i| i.id.clone()) else { return };
    emit(s, "session.inbox.delivered", json!({ "inboxID": id }), tx);
}

/// The review subagent runs, with the capsule over the composer, and finishes.
fn subagent_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.running.remove(CHILD);
    s.play.after(0.4, start_child, tx);
    s.play.after(2.8, finish_child, tx);
}

fn start_child(s: &mut App, tx: &mut Tx) {
    emit(s, "session.execution.started", json!({ "sessionID": CHILD }), tx);
}

fn finish_child(s: &mut App, tx: &mut Tx) {
    emit(s, "session.execution.succeeded", json!({ "sessionID": CHILD }), tx);
}

/// Two screenshots, made once so their thumbnails keep their identity across replays.
fn pictures() -> &'static [Attachment] {
    static PICS: OnceLock<Vec<Attachment>> = OnceLock::new();
    PICS.get_or_init(|| {
        let shot = |name: &str, (w, h): (u32, u32), a: [f32; 3], b: [f32; 3]| {
            let img = image::RgbaImage::from_fn(w, h, |x, y| {
                let t = (x as f32 / w as f32 * 0.7 + y as f32 / h as f32 * 0.3).clamp(0.0, 1.0);
                let c = |i: usize| ((a[i] + (b[i] - a[i]) * t) * 255.0) as u8;
                image::Rgba([c(0), c(1), c(2), 255])
            });
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).expect("encode a PNG");
            Attachment { mime: "image/png".into(), name: name.into(), bytes: Arc::from(out.into_inner()), media: None }
        };
        vec![
            shot("sidebar.png", (640, 400), [0.22, 0.36, 0.86], [0.62, 0.32, 0.78]),
            shot("reconnect.png", (480, 480), [0.95, 0.55, 0.25], [0.86, 0.24, 0.42]),
        ]
    })
}

/// A prompt with two images arrives; the first opens in the viewer and closes.
fn images_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.play.after(0.4, |s, tx| {
        let id = format!("{ARRIVED}img");
        opencode_kit::model::set_user_files(&id, pictures().to_vec());
        emit(s, "session.prompted", json!({ "messageID": id, "text": "Here's the sidebar after a wake, and the reconnect log" }), tx);
    }, tx);
    s.play.after(1.6, |s, _| view_image(s), tx);
    s.play.after(3.0, |s, _| s.attach.viewing = None, tx);
}

fn view_image(s: &mut App) {
    let files = opencode_kit::model::user_files(&format!("{ARRIVED}img"));
    if let Some(f) = files.first() {
        crate::attachments::view_image(s, "images", f);
    }
}

/// The timeline glides up past a screen, Jump to latest shows, and takes it back down.
fn jump_open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.play.after(0.4, scroll_up, tx);
    s.play.after(2.0, |s, tx| tx.scroll_to_bottom(scroll_key(s)), tx);
}

fn scroll_up(s: &mut App, tx: &mut Tx) {
    tx.scroll_to(scroll_key(s), 0.0, 0.0);
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use opencode_kit::client::Server;
    use rune::{Env, Key, KeyEvent, Mods, Runtime, TextEdit};

    use super::{STORIES, open};
    use crate::{App, FIXTURE, Source, open_scene};

    const DT: f64 = 1.0 / 120.0;

    fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
        rt.key(KeyEvent { key, mods });
        rt.frame(DT);
    }

    fn letter(rt: &mut Runtime<App>, c: &str) {
        press(rt, Key::Text(c.into()), Mods::default());
    }

    fn run(rt: &mut Runtime<App>, secs: f64) {
        for _ in 0..(secs * 120.0) as usize {
            rt.frame(DT);
        }
    }

    /// ⌘⇧D, then polling: the real app never polls here, so it doesn't save its state or
    /// reach the server.
    fn book(rt: &mut Runtime<App>) {
        press(rt, Key::Command('d'), Mods { cmd: true, shift: true, ..Mods::default() });
        rt.poll = Some(crate::poll);
    }

    fn at(rt: &Runtime<App>) -> Option<usize> {
        rt.state.story.as_ref().map(|b| b.at)
    }

    fn story(name: &str) -> usize {
        rune_playground::find(STORIES, name).unwrap()
    }

    fn real() -> (Runtime<App>, std::sync::mpsc::Receiver<opencode_kit::client::Write>) {
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
        (rt, writes)
    }

    #[test]
    fn the_storybook_leaves_the_app_as_it_was_and_sends_nothing() {
        let (mut rt, writes) = real();
        book(&mut rt);
        assert_eq!(at(&rt), Some(0));
        assert!(matches!(rt.state.source, Source::Fixture), "a story is its own fixture world");
        assert!(rt.find("book-group-0").is_some() && rt.find("story-0").is_some() && rt.find("tabs").is_none(), "stories in place of the tabs");
        assert!(rt.find("book-side").is_some() && rt.find("story-composer").is_some(), "the composer alone, its knobs beside it");
        assert!(rt.find("timeline").is_none(), "nothing else on the stage");

        // ↓ walks the list; the steer story sends, steers and delivers, and nothing goes out.
        press(&mut rt, Key::Down, Mods::default());
        assert_eq!(rt.state.play.panel, false);
        press(&mut rt, Key::Down, Mods::default());
        assert_eq!(at(&rt), Some(story("Dictation")), "to the next story down");
        let steer = story("Steer and Pending");
        open(&mut rt.state, steer, &mut rune::Tx::default());
        run(&mut rt, 1.0);
        let id = super::sid(&rt.state);
        assert_eq!(rt.state.inbox_for(&id).map(|i| i.steering().count()), Some(1), "the steer waits as Pending");
        run(&mut rt, 1.6);
        assert!(rt.state.inbox_for(&id).is_none(), "delivered");
        let last = rt.state.transcripts[&id].messages.last().unwrap();
        assert!(matches!(last, opencode_kit::model::Message::User { text, .. } if text == super::STEER_TEXT));

        rt.poll = None;
        press(&mut rt, Key::Escape, Mods::default());
        assert!(rt.state.story.is_none());
        assert!(matches!(rt.state.source, Source::Live { .. }));
        assert_eq!(rt.state.composer.text, "a real draft");
        assert!(writes.try_recv().is_err(), "nothing reached the server");
    }

    /// The queue dock story fills the dock from the composer and empties it into the timeline.
    #[test]
    fn the_queue_dock_story_fills_and_empties_the_dock() {
        let (mut rt, _) = real();
        book(&mut rt);
        open(&mut rt.state, story("Queue dock"), &mut rune::Tx::default());
        let id = super::sid(&rt.state);
        let queued = |rt: &Runtime<App>| rt.state.inbox_for(&id).map_or(0, |i| i.queued().count());
        let dock = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("queue")).map_or(0.0, |e| e.rect[3]);
        run(&mut rt, 2.0);
        assert_eq!(queued(&rt), 2, "two prompts wait in the dock");
        assert!(dock(&rt) > 40.0, "the dock is open");
        run(&mut rt, 2.5);
        assert_eq!(queued(&rt), 0, "both delivered");
        assert_eq!(dock(&rt), 0.0, "the dock closed behind the composer");
        let last = rt.state.transcripts[&id].messages.last().unwrap();
        assert!(matches!(last, opencode_kit::model::Message::User { text, .. } if text == super::STEER_AGAIN));
        letter(&mut rt, "q");
        run(&mut rt, 0.1);
        assert_eq!(queued(&rt), 1, "Q queues another");
        letter(&mut rt, "d");
        run(&mut rt, 0.1);
        assert_eq!(queued(&rt), 0, "D delivers it");
    }

    /// R plays a story from the start the same way every time: the glow lands at the same
    /// moment of each replay.
    #[test]
    fn replays_run_the_same_every_time() {
        let (mut rt, _) = real();
        book(&mut rt);
        open(&mut rt.state, story("Arrival glow"), &mut rune::Tx::default());
        let mut lit = Vec::new();
        for _ in 0..3 {
            rt.key(KeyEvent { key: Key::Text("r".into()), mods: Mods::default() });
            let mut first = None;
            for f in 0..120 {
                rt.frame(DT);
                if first.is_none() && !rt.state.glows.is_empty() {
                    first = Some(f);
                }
            }
            lit.push(first.expect("it glowed"));
        }
        assert!(lit.iter().all(|f| *f == lit[0]), "the glow lands at the same frame of each replay: {lit:?}");
        assert!(rt.find("story-session").is_some());
    }

    /// Every story opens, plays its motion and replays without a hitch; each digit flips the
    /// variant of a story that has one.
    #[test]
    fn every_story_plays_and_replays() {
        let (mut rt, _) = real();
        book(&mut rt);
        for i in 0..STORIES.len() {
            open(&mut rt.state, i, &mut rune::Tx::default());
            assert!(rt.find("book-side").is_some());
            run(&mut rt, 3.0);
            letter(&mut rt, "r");
            run(&mut rt, 1.0);
            assert_eq!(at(&rt), Some(i), "{}", STORIES[i].name);
        }
        open(&mut rt.state, story("Question"), &mut rune::Tx::default());
        rt.frame(DT);
        letter(&mut rt, "2");
        rt.frame(DT);
        let fields = rt.state.pending.iter().find_map(|p| match p {
            opencode_kit::model::Pending::Form { fields, .. } => Some(fields.len()),
            _ => None,
        });
        assert_eq!(fields, Some(2), "2 shows the two-question form");
    }
}
