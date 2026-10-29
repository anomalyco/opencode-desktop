//! The storybook (⌘⇧D), on Rune's `App::playground`: one story per component, grouped by type,
//! each alone on a plain stage with only its knobs (the values ⌘⇧K tunes) and its triggers
//! beside it. A story is a scene: a fresh fixture world, with its R step setting the component
//! up and starting its motion, so a replay runs the same every time. The app as it was waits
//! beside the playground untouched and comes back on Esc or ⌘⇧D, with the knobs as the stories
//! left them. Answers and sends in a story go nowhere.

use std::sync::{Arc, OnceLock};

use opencode_kit::model::{Attachment, Delivery, InboxItem, Message};
use rune::{Cx, El, Play, Playground, Step, TextEdit, Tx, col};
use rune_knobs::Group;
use serde_json::{Value, json};

use crate::knobs::*;
use crate::palette::{Pal, pal};
use crate::turns::{self, Block};
use crate::{App, FIXTURE, Route, open_scene, open_story, view};

/// What a story's scene draws, and whether it only plays (its composer never takes the keys).
#[derive(Clone, Copy)]
pub struct Stage {
    view: fn(&App, &Pal) -> El,
    watch: bool,
}

impl Stage {
    pub fn watch(&self) -> bool {
        self.watch
    }
}

/// A trigger beside a story: its letter, what it does, and its step.
pub struct Trigger {
    pub key: char,
    pub label: &'static str,
    pub run: Step<App>,
}

/// The R trigger: sets the story up in its fresh world and starts its motion; it runs when the
/// story is chosen and on each replay.
pub const fn replay(run: Step<App>) -> Trigger {
    Trigger { key: 'r', label: "Replay", run }
}

pub struct Story {
    /// "Group/Name".
    path: &'static str,
    /// Sets up its stage in the fresh world, with [`stage`].
    scene: fn(&mut App),
    /// The knobs beside it, which the keyboard reaches with → from the list.
    knobs: &'static [Group],
    keys: &'static [Trigger],
    /// Each variant's name and the value of `App::variant` it sets (1 to 9 pick them).
    variants: &'static [(&'static str, usize)],
}

/// What the Composer story shows.
const COMPOSER_VARIANTS: &[(&str, usize)] = &[("Empty", 0), ("Draft", 1), ("Queued", 2), ("Subagent running", 3), ("Long paste", 4)];
/// The Question story's form.
const QUESTION_VARIANTS: &[(&str, usize)] = &[("One", 0), ("Two", 1), ("Multi-select", 2), ("Long", 3), ("Typing", 4)];
/// What the Custom answer story's typed answer holds.
const CUSTOM_VARIANTS: &[(&str, usize)] = &[("Typing", 0), ("Long, wrapped", 1), ("Left the field", 2), ("Empty", 3)];

const COMPOSER_KNOBS: &[Group] = &[Group::new("Composer", "stage", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING])];
const MORPH_KNOBS: &[Group] = &[QUESTION_DOCK];
const GLOW_KNOBS: &[Group] = &[Group::new("Arrival glow", "stage", &[GLOW_LENGTH, GLOW_RISE, GLOW_DECAY, GLOW_TAIL, GLOW_FADE, ROW_GROWTH])];
pub(crate) const ROW_KNOBS: &[Group] = &[Group::new("Rows", "stage", &[ROW_GROWTH])];
const WORKING_KNOBS: &[Group] = &[WORKING];
const ACTIONS_KNOBS: &[Group] = &[Group::new("Turn actions", "stage", &[ACTIONS_TAIL])];
const SUBAGENT_KNOBS: &[Group] = &[Group::new("Subagent", "stage", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING])];
const IMAGE_KNOBS: &[Group] = &[Group::new("Images", "stage", &[IMAGE_FLIGHT, ROW_GROWTH])];
const JUMP_KNOBS: &[Group] = &[Group::new("Jump to latest", "stage", &[JUMP_PLACE, JUMP_IN, JUMP_OUT, JUMP_RISE])];

const MORPH_KEYS: &[Trigger] = &[
    Trigger { key: 'r', label: "Replay the loop", run: morph_open },
    Trigger { key: 'o', label: "Open the dock", run: |s, tx| {
        tx.clear();
        if !s.question_shown() {
            emit(s, "form.created", form(s, &[0]), tx);
        }
    } },
    Trigger { key: 'c', label: "Close it", run: |s, tx| {
        tx.clear();
        if s.question_shown() {
            emit(s, "form.replied", replied(s), tx);
        }
    } },
];
const UNDER_QUEUE_KEYS: &[Trigger] = &[
    replay(under_queue_open),
    Trigger { key: 'q', label: "Queue one", run: queue },
    Trigger { key: 'o', label: "Open the dock", run: |s, tx| if !s.question_shown() { emit(s, "form.created", form(s, &[0]), tx) } },
    Trigger { key: 'c', label: "Close it", run: |s, tx| if s.question_shown() { emit(s, "form.replied", replied(s), tx) } },
    Trigger { key: 'd', label: "Deliver the oldest", run: deliver_queued },
];
const COMPOSER_KEYS: &[Trigger] = &[replay(composer_open)];
const QUESTION_KEYS: &[Trigger] = &[replay(question_open)];
const CUSTOM_KEYS: &[Trigger] = &[replay(custom_open)];
const HEADER_KEYS: &[Trigger] = &[replay(|s, _| s.finish_stream())];
const GLOW_KEYS: &[Trigger] = &[replay(glow_open), Trigger { key: 'a', label: "Another arrives", run: arrive }];
const SEND_KEYS: &[Trigger] = &[replay(send_open), Trigger { key: 's', label: "Send", run: send }];
const STREAM_KEYS: &[Trigger] = &[replay(stream_open), Trigger { key: 'f', label: "Finish the stream", run: finish_stream }];
const TOOL_KEYS: &[Trigger] = &[replay(tools_open), Trigger { key: 't', label: "Fold or unfold the tools", run: fold_tools }];
const WORKING_KEYS: &[Trigger] = &[
    Trigger { key: 'r', label: "Restart the run", run: working_open },
    Trigger { key: 'l', label: "Change the label", run: relabel },
    Trigger { key: 'd', label: "End the run", run: finish },
];
const TURN_KEYS: &[Trigger] = &[replay(actions_open), Trigger { key: 'w', label: "Start working", run: start }, Trigger { key: 'f', label: "Finish", run: finish }];
const STEER_KEYS: &[Trigger] = &[
    replay(steer_open),
    Trigger { key: 's', label: "Steer", run: steer },
    Trigger { key: 'd', label: "Deliver", run: deliver },
    Trigger { key: 'q', label: "Queue one", run: |s, tx| prompt(s, QUEUED_TEXT, true, tx) },
    Trigger { key: 'f', label: "Finish the turn", run: finish },
];
const QUEUE_KEYS: &[Trigger] = &[
    replay(queue_open),
    Trigger { key: 'q', label: "Queue one", run: queue },
    Trigger { key: 'd', label: "Deliver the oldest", run: deliver_queued },
];
const SUBAGENT_KEYS: &[Trigger] = &[replay(subagent_open), Trigger { key: 's', label: "Start the subagent", run: start_child }, Trigger { key: 'f', label: "Finish it", run: finish_child }];
const IMAGE_KEYS: &[Trigger] = &[
    replay(images_open),
    Trigger { key: 'o', label: "Open the first image", run: |s, _| view_image(s) },
    Trigger { key: 'c', label: "Close it", run: |s, _| s.attach.viewing = None },
];
const JUMP_KEYS: &[Trigger] = &[
    replay(jump_open),
    Trigger { key: 'u', label: "Scroll up", run: scroll_up },
    Trigger { key: 'b', label: "Back to latest", run: |s, tx| tx.scroll_to_bottom(scroll_key(s)) },
];

pub static STORIES: &[Story] = &[
    Story { path: "Composer/Composer", scene: |s| stage(s, composer_view), knobs: COMPOSER_KNOBS, keys: COMPOSER_KEYS, variants: COMPOSER_VARIANTS },
    Story { path: "Composer/Queue dock", scene: |s| stage(s, session_view), knobs: ROW_KNOBS, keys: QUEUE_KEYS, variants: &[] },
    Story { path: "Composer/Dictation", scene: |s| stage(s, composer_view), knobs: &[], keys: crate::dictation::STORY_KEYS, variants: &[] },
    Story { path: "Composer/History", scene: |s| stage(s, composer_view), knobs: &[], keys: crate::prompt_history_story::KEYS, variants: &[] },
    Story { path: "Composer/Mentions", scene: |s| stage(s, session_view), knobs: &[], keys: crate::mention_stories::KEYS, variants: &[] },
    Story { path: "Composer/Subagents capsule, 30 running", scene: |s| stage(s, session_view), knobs: &[], keys: crate::capsule_stories::KEYS, variants: &[] },
    Story { path: "Question dock/Question", scene: |s| stage(s, composer_view), knobs: &[], keys: QUESTION_KEYS, variants: QUESTION_VARIANTS },
    Story { path: "Question dock/Custom answer", scene: |s| stage(s, composer_view), knobs: &[], keys: CUSTOM_KEYS, variants: CUSTOM_VARIANTS },
    Story { path: "Question dock/Morph", scene: |s| watch(s, session_view), knobs: MORPH_KNOBS, keys: MORPH_KEYS, variants: &[] },
    Story { path: "Question dock/Under a queue", scene: |s| stage(s, session_view), knobs: MORPH_KNOBS, keys: UNDER_QUEUE_KEYS, variants: &[] },
    Story { path: "Sidebar/Rename and menus", scene: |s| stage(s, crate::session_menu_stories::view), knobs: &[], keys: crate::session_menu_stories::KEYS, variants: crate::session_menu_stories::VARIANTS },
    Story { path: "Components/Odometer", scene: |s| stage(s, crate::odometer_stories::view), knobs: &[], keys: crate::odometer_stories::KEYS, variants: &[] },
    Story { path: "Header/Session header", scene: |s| stage(s, header_view), knobs: &[], keys: HEADER_KEYS, variants: &[] },
    Story { path: "Header/Ticking", scene: |s| watch(s, ticking_view), knobs: &[], keys: crate::odometer_stories::TICKING_KEYS, variants: &[] },
    Story { path: "Timeline/Arrival glow", scene: |s| stage(s, session_view), knobs: GLOW_KNOBS, keys: GLOW_KEYS, variants: &[] },
    Story { path: "Timeline/Send", scene: |s| stage(s, session_view), knobs: ROW_KNOBS, keys: SEND_KEYS, variants: &[] },
    Story { path: "Timeline/Long prompt", scene: |s| stage(s, session_view), knobs: ROW_KNOBS, keys: crate::fold_stories::KEYS, variants: crate::fold_stories::VARIANTS },
    Story { path: "Timeline/Streaming text", scene: |s| stage(s, session_view), knobs: ROW_KNOBS, keys: STREAM_KEYS, variants: &[] },
    Story { path: "Timeline/Tool rows", scene: |s| stage(s, session_view), knobs: ROW_KNOBS, keys: TOOL_KEYS, variants: &[] },
    Story { path: "Timeline/Selection", scene: |s| stage(s, session_view), knobs: &[], keys: crate::selection_stories::KEYS, variants: &[] },
    Story { path: "Timeline/Quote", scene: |s| stage(s, session_view), knobs: &[], keys: crate::quote_stories::KEYS, variants: &[] },
    Story { path: "Timeline/Question answers", scene: |s| stage(s, session_view), knobs: ROW_KNOBS, keys: crate::question_stories::KEYS, variants: crate::question_stories::VARIANTS },
    Story { path: "Timeline/Compaction", scene: |s| stage(s, session_view), knobs: ROW_KNOBS, keys: crate::compaction_stories::KEYS, variants: crate::compaction_stories::VARIANTS },
    Story { path: "Timeline/Working", scene: |s| stage(s, session_view), knobs: WORKING_KNOBS, keys: WORKING_KEYS, variants: &[] },
    Story { path: "Timeline/Steer and Pending", scene: |s| stage(s, session_view), knobs: ROW_KNOBS, keys: STEER_KEYS, variants: &[] },
    Story { path: "Timeline/Move to background", scene: |s| stage(s, session_view), knobs: &[], keys: crate::background_stories::KEYS, variants: &[] },
    Story { path: "Timeline/Subagent", scene: |s| stage(s, session_view), knobs: SUBAGENT_KNOBS, keys: SUBAGENT_KEYS, variants: &[] },
    Story { path: "Timeline/Subagent rows", scene: |s| stage(s, session_view), knobs: &[], keys: crate::agent_stories::KEYS, variants: crate::agent_stories::VARIANTS },
    Story { path: "Timeline/Images", scene: |s| stage(s, session_view), knobs: IMAGE_KNOBS, keys: IMAGE_KEYS, variants: &[] },
    Story { path: "Timeline/Jump to latest", scene: |s| stage(s, session_view), knobs: JUMP_KNOBS, keys: JUMP_KEYS, variants: &[] },
    Story { path: "Timeline/Frosted surfaces", scene: |s| watch(s, crate::frosted_stories::view), knobs: &[], keys: crate::frosted_stories::KEYS, variants: &[] },
    Story { path: "Timeline/Turn actions", scene: |s| stage(s, session_view), knobs: ACTIONS_KNOBS, keys: TURN_KEYS, variants: &[] },
    Story { path: "Navigation/Back and forward", scene: |s| stage(s, history_view), knobs: &[], keys: crate::history_story::KEYS, variants: &[] },
    Story { path: open_story::PATHS[0], scene: |s| stage(s, opening_view), knobs: &[], keys: open_story::KEYS[0], variants: open_story::VARIANTS },
    Story { path: open_story::PATHS[1], scene: |s| stage(s, opening_view), knobs: &[], keys: open_story::KEYS[1], variants: open_story::VARIANTS },
    Story { path: open_story::PATHS[2], scene: |s| stage(s, opening_view), knobs: &[], keys: open_story::KEYS[2], variants: open_story::VARIANTS },
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

// The playground.

/// The window's storybook: each story a scene, its triggers, knobs and variants.
pub fn playground() -> Playground<App> {
    let mut pg = Playground::<App>::new()
        .scenes(|name| name.contains('/'))
        .fresh(fresh)
        .keep(keep)
        .typing(|s| !s.stage.is_some_and(|st| st.watch) && (s.composer.focused || s.question_takes_text()))
        .folds(load_folds, save_folds);
    for st in STORIES {
        for t in st.keys {
            pg = pg.trigger(st.path, t.key, t.label, t.run);
        }
        for &(name, v) in st.variants {
            pg = pg.variant(st.path, name, move |s: &mut App| s.variant = v);
        }
        if !st.knobs.is_empty() {
            let knobs = st.knobs;
            pg = pg.side_keys(st.path, move |s, active, cx| side(s, knobs, active, cx), rune_knobs::side_key(knobs, |s: &mut App| &mut s.knobs));
        }
    }
    pg
}

/// `<data dir>/rune-opencode/storybook.json`: the groups folded in the list.
fn folds_file() -> Option<std::path::PathBuf> {
    crate::restore::data_dir().map(|d| d.join("storybook.json"))
}

/// Headless runs and tests neither read nor write them, so a capture's list never depends on
/// what was folded at the desk.
fn load_folds() -> Vec<String> {
    if crate::headless() || cfg!(test) {
        return Vec::new();
    }
    folds_file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|mut v| serde_json::from_value(v["folded"].take()).ok()).unwrap_or_default()
}

fn save_folds(folded: &[String]) {
    if crate::headless() || cfg!(test) {
        return;
    }
    if let Some(f) = folds_file() {
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        let _ = std::fs::write(f, json!({ "folded": folded }).to_string());
    }
}

/// The app's scenes, then a scene per story under its path.
pub fn scenes(app: &'static [(&'static str, fn(&mut App))]) -> &'static [(&'static str, fn(&mut App))] {
    let all: Vec<_> = app.iter().copied().chain(STORIES.iter().map(|st| (st.path, st.scene))).collect();
    Box::leak(all.into_boxed_slice())
}

/// A story's world: the fixture, opened on its session, with what stays the same between the
/// app and its stories.
fn fresh(real: &App) -> App {
    let mut w = App::empty();
    w.load_fixture(FIXTURE);
    w.knobs = real.knobs.clone();
    w.zoom = real.zoom;
    w.sidebar_w = real.sidebar_w;
    w.sidebar_open = real.sidebar_open;
    w.edge = real.edge;
    w.reduced_motion = real.reduced_motion;
    w.debug = real.debug;
    w.settings.scheme = real.settings.scheme;
    w.clock = real.clock;
    // Relative times read as the fixture has them, however long the app has run.
    w.base_ms -= (real.clock * 1000.0) as i64;
    w.now_ms = w.base_ms + (real.clock * 1000.0) as i64;
    w.waker = real.waker.clone();
    if let Some(waker) = w.waker.clone() {
        w.attach.set_waker(waker.clone());
        w.icons.set_waker(waker);
    }
    open_scene(&mut w);
    w
}

/// What the stories tuned stays in the app.
fn keep(stage: &App, real: &mut App) {
    real.knobs = stage.knobs.clone();
}

/// Sets up a story's stage: `view` alone on it.
fn stage(s: &mut App, view: fn(&App, &Pal) -> El) {
    s.stage = Some(Stage { view, watch: false });
}

/// [`stage`] for a story that only plays.
fn watch(s: &mut App, view: fn(&App, &Pal) -> El) {
    s.stage = Some(Stage { view, watch: true });
}

// Drawing.

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

/// The story's knobs, beside its stage; `active` with the keyboard on them.
fn side(s: &App, knobs: &'static [Group], active: bool, cx: &mut Cx<App>) -> El {
    let p = pal(s.dark(cx));
    rune_knobs::inline(&s.knobs, knobs, knobs_style(&p), |s: &mut App| &mut s.knobs, active)
}

/// The stage: the story's component alone on the window's ground.
pub fn page(s: &App, p: &Pal, st: Stage) -> El {
    col().key("story-stage").w_full().h_full().fill(p.bg_deep).padding(24.0).center().child((st.view)(s, p))
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

/// The header over the timeline, for numbers that tick in both.
fn ticking_view(s: &App, p: &Pal) -> El {
    frame(p, "story-ticking").h_full().max_h(680.0).child(view::story_header(s, p)).child(view::story_session(s, p))
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

fn composer_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    match s.variant {
        1 => s.composer = TextEdit::new(DRAFT),
        2 => pending(s),
        3 => {
            s.running.insert(CHILD.into());
        }
        // A call transcript pasted whole: the editor stops at its cap and shows the end.
        4 => {
            s.composer = TextEdit::new(long_paste());
            tx.scroll_to_bottom_instant("editor");
        }
        _ => {}
    }
}

/// Three hundred lines of a call transcript.
pub(crate) fn long_paste() -> String {
    (1..=300).map(|i| format!("[{:02}:{:02}] {}: so the composer grows with every line we paste, line {i} of the call transcript",
        i / 60, i % 60, if i % 2 == 0 { "Kit" } else { "Dax" })).collect::<Vec<_>>().join("\n")
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

fn question_open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let q = match s.variant {
        1 => form(s, &[0, 1]),
        2 => form(s, &[1]),
        3 => long(s),
        _ => form(s, &[0]),
    };
    s.apply_event("form.created", &q);
    if s.variant == 4 {
        s.question_type("Fake clock, but keep one slow test on real timers");
    }
}

/// The two-question form on its last question, with its typed answer as its variant says.
fn custom_open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let q = form(s, &[0, 1]);
    s.apply_event("form.created", &q);
    s.question_last();
    let text = match s.variant {
        1 | 2 => "Fake clock for the loop, but keep one slow end-to-end test on real timers so a regression in the scheduler still shows up somewhere",
        3 => "",
        _ => "All of them",
    };
    s.question_type(text);
    if matches!(s.variant, 2 | 3) {
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
fn morph_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.5, ask);
}

fn ask(s: &mut App, tx: &mut Play<App>) {
    emit(s, "form.created", form(s, &[0]), tx);
    tx.after(BEAT, answer);
}

fn answer(s: &mut App, tx: &mut Play<App>) {
    emit(s, "form.replied", replied(s), tx);
    tx.after(BEAT, ask);
}

/// A prompt waits in the queue while the session works, then the question opens under it:
/// the queue and the dock share one outline.
fn under_queue_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id);
    tx.after(0.2, queue);
    tx.after(1.0, |s, tx| emit(s, "form.created", form(s, &[0]), tx));
}

/// A prompt from another client lands and glows.
fn glow_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.4, arrive);
}

fn arrive(s: &mut App, tx: &mut Play<App>) {
    s.sends += 1;
    let text = ["Sent from my phone: does this cover the laptop waking on battery too?", "And on a flaky hotel network?", "One more: what about a VPN dropping?"];
    let n = s.sends as usize;
    emit(s, "session.prompted", json!({ "messageID": format!("{ARRIVED}{n}"), "text": text[(n - 1) % text.len()] }), tx);
}

/// A draft in the composer, sent; the fixture's reply plays under it.
fn send_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.composer = TextEdit::new(SEND_TEXT);
    tx.after(0.6, send);
}

fn send(s: &mut App, tx: &mut Play<App>) {
    prompt(s, SEND_TEXT, false, tx);
}

/// Sends `text` as typed in the composer: a steer or, with `queue`, a queued prompt while the
/// session works.
fn prompt(s: &mut App, text: &str, queue: bool, tx: &mut Tx) {
    s.composer = TextEdit::new(text);
    s.submit(queue, tx);
}

/// The fixture's streaming turn, from its first event.
fn stream_open(s: &mut App, _: &mut Play<App>) {
    let id = sid(s);
    s.start_stream_if_due(&id);
}

fn finish_stream(s: &mut App, _: &mut Play<App>) {
    while let Some((_, ty, data)) = s.playback.pop_front() {
        s.apply_event(&ty, &data);
    }
}

/// A send whose reply calls two tools; the group they fold into opens and closes after.
fn tools_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.4, send);
    tx.after(4.6, fold_tools);
    tx.after(6.2, fold_tools);
}

/// Opens or closes the last group of tools in the timeline.
fn fold_tools(s: &mut App, _: &mut Play<App>) {
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
fn working_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.4, start);
}

fn start(s: &mut App, tx: &mut Play<App>) {
    emit(s, "session.execution.started", json!({}), tx);
}

fn finish(s: &mut App, tx: &mut Play<App>) {
    emit(s, "session.execution.succeeded", json!({}), tx);
}

/// Labels of different widths, for a label that changes mid-sweep.
const LABELS: &[&str] = &["Working", "Thinking", "Running the tests", "Searching the codebase"];

fn relabel(s: &mut App, _: &mut Play<App>) {
    let at = LABELS.iter().position(|l| *l == s.working.label()).unwrap_or(0);
    let now = s.clock;
    s.working.relabel(LABELS[(at + 1) % LABELS.len()], now);
}

/// Working, until the turn finishes and its actions take its place.
fn actions_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id);
    tx.after(1.0, finish);
}

/// A steer sent while the session works shows in place as Pending, and lands when it's delivered.
fn steer_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id);
    s.composer = TextEdit::new(STEER_TEXT);
    tx.after(0.5, |s, tx| prompt(s, STEER_TEXT, false, tx));
    tx.after(2.3, deliver);
}

fn steer(s: &mut App, tx: &mut Play<App>) {
    let steering = s.inbox_for(&sid(s)).map_or(0, |i| i.steering().count());
    prompt(s, if steering == 0 { STEER_TEXT } else { STEER_AGAIN }, false, tx);
}

/// The server delivers the oldest steer.
fn deliver(s: &mut App, tx: &mut Play<App>) {
    let Some(id) = s.inbox_for(&sid(s)).and_then(|i| i.steering().next()).map(|i| i.id.clone()) else { return };
    emit(s, "session.inbox.delivered", json!({ "inboxID": id }), tx);
}

/// Two prompts queued while the session works open the dock out of the composer's top edge;
/// delivered one by one, they leave it and it closes back behind the composer.
fn queue_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id);
    tx.after(0.4, queue);
    tx.after(1.2, queue);
    tx.after(2.4, deliver_queued);
    tx.after(3.2, deliver_queued);
}

fn queue(s: &mut App, tx: &mut Play<App>) {
    let queued = s.inbox_for(&sid(s)).map_or(0, |i| i.queued().count());
    prompt(s, [QUEUED_TEXT, STEER_AGAIN][queued % 2], true, tx);
}

/// The server delivers the oldest queued prompt.
fn deliver_queued(s: &mut App, tx: &mut Play<App>) {
    let Some(id) = s.inbox_for(&sid(s)).and_then(|i| i.queued().next()).map(|i| i.id.clone()) else { return };
    emit(s, "session.inbox.delivered", json!({ "inboxID": id }), tx);
}

/// The review subagent runs, with the capsule over the composer, and finishes.
fn subagent_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.running.remove(CHILD);
    tx.after(0.4, start_child);
    tx.after(2.8, finish_child);
}

fn start_child(s: &mut App, tx: &mut Play<App>) {
    emit(s, "session.execution.started", json!({ "sessionID": CHILD }), tx);
}

fn finish_child(s: &mut App, tx: &mut Play<App>) {
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
fn images_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.4, |s, tx| {
        let id = format!("{ARRIVED}img");
        opencode_kit::model::set_user_files(&id, pictures().to_vec());
        emit(s, "session.prompted", json!({ "messageID": id, "text": "Here's the sidebar after a wake, and the reconnect log" }), tx);
    });
    tx.after(1.6, |s, _| view_image(s));
    tx.after(3.0, |s, _| s.attach.viewing = None);
}

fn view_image(s: &mut App) {
    let files = opencode_kit::model::user_files(&format!("{ARRIVED}img"));
    if let Some(f) = files.first() {
        crate::attachments::view_image(s, "images", f);
    }
}

/// The timeline glides up past a screen, Jump to latest shows, and takes it back down.
fn jump_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.4, scroll_up);
    tx.after(2.0, |s, tx| tx.scroll_to_bottom(scroll_key(s)));
}

fn scroll_up(s: &mut App, tx: &mut Play<App>) {
    tx.scroll_to(scroll_key(s), 0.0, 0.0);
}

/// The app with its storybook, headless and closed, on an empty real state that never reaches
/// a server: open a story with `Runtime::open_playground`.
#[cfg(test)]
pub(crate) fn storybook() -> rune::Runtime<App> {
    let spec = rune::AppSpec { setup: None, ..crate::spec(App::empty) };
    let mut rt = crate::rune_app(spec).headless_runtime((1280.0, 820.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

/// The storybook open on `path`, on its variant `variant` if given.
#[cfg(test)]
pub(crate) fn story(path: &str, variant: Option<&str>) -> rune::Runtime<App> {
    let mut rt = storybook();
    rt.open_playground(path, variant);
    rt
}

/// The open story's world.
#[cfg(test)]
pub(crate) fn world(rt: &rune::Runtime<App>) -> &App {
    rt.playground_state::<App>().expect("a story is open")
}

#[cfg(test)]
#[path = "stories_tests.rs"]
mod tests;
