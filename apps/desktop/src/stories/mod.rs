//! The storybook (⌘⇧D), on Rune's `App::playground`: one story per component, grouped by type,
//! each alone on a plain stage with only its knobs (the values ⌘⇧K tunes) and its triggers
//! beside it. A story is a scene: a fresh fixture world, with its R step setting the component
//! up and starting its motion, so a replay runs the same every time. The app as it was waits
//! beside the playground untouched and comes back on Esc or ⌘⇧D, with the knobs as the stories
//! left them. Answers and sends in a story go nowhere.

#[cfg(feature = "stories")]
pub mod composer;
pub mod demo;
pub mod demo_script;
pub mod fixture_ids;
pub mod open;
pub mod scenes;
#[cfg(feature = "stories")]
pub mod shell;
#[cfg(feature = "stories")]
pub mod timeline;

use self::fixture_ids::CHILD;
use self::scenes::open_scene;
use std::sync::{Arc, OnceLock};

use opencode_kit::model::{Attachment, Delivery, InboxItem, Message};
use rune::{Cx, El, Play, Playground, Step, TextEdit, Tx, col};
use rune_knobs::Group;
use rune_register::{collect, register, slice};
use serde_json::{Value, json};

use crate::knobs::*;
use crate::palette::{Pal, pal};
use crate::turns::{self, Block};
use crate::{App, FIXTURE, Route, view};

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

#[derive(Clone, Copy)]
pub struct Story {
    /// "Group/Name".
    pub(crate) path: &'static str,
    /// Sets up its stage in the fresh world, with [`stage`].
    pub(crate) scene: fn(&mut App),
    /// The knobs beside it, which the keyboard reaches with → from the list.
    pub(crate) knobs: &'static [Group],
    pub(crate) keys: &'static [Trigger],
    /// Each variant's name and the value of `App::variant` it sets (1 to 9 pick them).
    pub(crate) variants: &'static [(&'static str, usize)],
}

impl Story {
    /// A story on the session stage.
    pub(crate) const fn session(path: &'static str, keys: &'static [Trigger]) -> Story {
        Story { path, scene: |s| stage(s, session_view), knobs: &[], keys, variants: &[] }
    }

    /// A story on the composer stage.
    pub(crate) const fn composer(path: &'static str, keys: &'static [Trigger]) -> Story {
        Story { path, scene: |s| stage(s, composer_view), knobs: &[], keys, variants: &[] }
    }

    pub(crate) const fn knobs(self, knobs: &'static [Group]) -> Story {
        Story { knobs, ..self }
    }

    pub(crate) const fn variants(self, variants: &'static [(&'static str, usize)]) -> Story {
        Story { variants, ..self }
    }
}

/// What the Composer story shows.
const COMPOSER_VARIANTS: &[(&str, usize)] = &[("Empty", 0), ("Draft", 1), ("Queued", 2), ("Subagent running", 3), ("Long paste", 4)];

const COMPOSER_KNOBS: &[Group] = &[Group::new("Composer", "stage", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING])];
pub(crate) const ROW_KNOBS: &[Group] = &[Group::new("Rows", "stage", &[ROW_GROWTH])];
const WORKING_KNOBS: &[Group] = &[WORKING];
const ACTIONS_KNOBS: &[Group] = &[Group::new("Turn actions", "stage", &[ACTIONS_TAIL])];
const SUBAGENT_KNOBS: &[Group] = &[Group::new("Subagent", "stage", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING])];
const IMAGE_KNOBS: &[Group] = &[Group::new("Images", "stage", &[IMAGE_FLIGHT, ROW_GROWTH])];
const JUMP_KNOBS: &[Group] = &[Group::new("Jump to latest", "stage", &[JUMP_PLACE, JUMP_IN, JUMP_OUT, JUMP_RISE])];

const COMPOSER_KEYS: &[Trigger] = &[replay(composer_open)];
const HEADER_KEYS: &[Trigger] = &[replay(|s, _| s.finish_stream())];
const ARRIVAL_KEYS: &[Trigger] = &[replay(arrival_open), Trigger { key: 'a', label: "Another arrives", run: arrive }];
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
    Trigger { key: 'o', label: "Open the first image", run: view_image },
    Trigger { key: 'z', label: "Zoom in", run: |s, tx| s.composer_ext.attach.zoom.zoom_in(tx) },
    Trigger { key: 'e', label: "Escape", run: |_, tx| tx.press(rune::Key::Escape) },
];
const JUMP_KEYS: &[Trigger] = &[
    replay(jump_open),
    Trigger { key: 'u', label: "Scroll up", run: scroll_up },
    Trigger { key: 'b', label: "Back to latest", run: |s, tx| tx.scroll_to_bottom(scroll_key(s)) },
];

slice!(pub STORY_LIST: Story);

register!(STORY_LIST: Story = [
    Story::composer("Composer/Composer", COMPOSER_KEYS).knobs(COMPOSER_KNOBS).variants(COMPOSER_VARIANTS),
    Story::session("Composer/Queue dock", QUEUE_KEYS).knobs(ROW_KNOBS),
    Story { path: "Header/Session header", scene: |s| stage(s, header_view), knobs: &[], keys: HEADER_KEYS, variants: &[] },
    Story::session("Timeline/Remote prompt", ARRIVAL_KEYS),
    Story::session("Timeline/Send", SEND_KEYS).knobs(ROW_KNOBS),
    Story::session("Timeline/Streaming text", STREAM_KEYS).knobs(ROW_KNOBS),
    Story::session("Timeline/Tool rows", TOOL_KEYS).knobs(ROW_KNOBS),
    Story::session("Timeline/Working", WORKING_KEYS).knobs(WORKING_KNOBS),
    Story::session("Timeline/Steer and Pending", STEER_KEYS).knobs(ROW_KNOBS),
    Story::session("Timeline/Subagent", SUBAGENT_KEYS).knobs(SUBAGENT_KNOBS),
    Story::session("Timeline/Images", IMAGE_KEYS).knobs(IMAGE_KNOBS),
    Story::session("Timeline/Jump to latest", JUMP_KEYS).knobs(JUMP_KNOBS),
    Story::session("Timeline/Turn actions", TURN_KEYS).knobs(ACTIONS_KNOBS),
    Story::session("Timeline/Copy tick", COPY_KEYS).knobs(&[crate::copied::GROUP]),
]);

/// The storybook's order, which module paths don't give: a story's group first, then where it sits
/// in the group. A story not named here goes after these, by module path.
const ORDER: &[&str] = &[
    "Composer/Composer",
    "Composer/Grows a line",
    "Composer/Queue dock",
    "Composer/Dictation",
    "Composer/History",
    "Composer/Pickers",
    "Pickers/Model picker",
    "Composer/Mentions",
    "Composer/Subagents capsule, 30 running",
    "Composer/Todo pill",
    "Composer/New session",
    "Question dock/Question",
    "Question dock/Custom answer",
    "Question dock/Morph",
    "Question dock/Under a queue",
    "Sidebar/Rename and menus",
    "Sidebar/Project icons",
    "Sidebar/Peek",
    "Home/Session search",
    "Home/No server",
    "Components/Odometer",
    "Components/Toast",
    "Header/Session header",
    "Header/Ticking",
    "Header/Title change",
    "Header/Narrow window",
    "Timeline/Remote prompt",
    "Timeline/Send",
    "Timeline/Long prompt",
    "Timeline/Streaming text",
    "Timeline/Generative UI",
    "Timeline/Diagram streaming",
    "Timeline/Diagram full size",
    "Timeline/Tool rows",
    "Timeline/Long tool group",
    "Timeline/Tool row fold",
    "Timeline/Selection",
    "Timeline/Find",
    "Timeline/Quote",
    "Timeline/Rewind",
    "Timeline/Fork",
    "Timeline/Question answers",
    "Timeline/Compaction",
    "Timeline/Working",
    "Timeline/Streaming line growth",
    "Timeline/Stop",
    "Timeline/Steer and Pending",
    "Timeline/Sends behind waiting steers",
    "Timeline/Move to background",
    "Timeline/Subagent",
    "Timeline/Subagent rows",
    "Timeline/Steered subagent",
    "Timeline/Subagent starting",
    "Timeline/Images",
    "Timeline/Jump to latest",
    "Timeline/Fade into composer",
    "Timeline/Scroll bar",
    "Timeline/Frosted surfaces",
    "Timeline/Turn actions",
    "Timeline/Copy tick",
    "Timeline/Errors",
    "Timeline/Retry",
    "Timeline/Notices",
    "Navigation/Back and forward",
    "Side panel/Side region",
    "Browser/Pane",
    "Browser/Engine install",
    "Terminal/Pane",
    "Review/Changes",
    "Review/Inline diff",
    "Files/Pane",
    "Files/Quick open (⌘P)",
    "Files/Markdown file",
    "Projects/Open project",
    "Projects/Project switcher",
    "Dev/Rewind",
];

/// Every story, in the storybook's order.
pub fn stories() -> &'static [Story] {
    static ALL: OnceLock<Vec<Story>> = OnceLock::new();
    ALL.get_or_init(|| ordered(collect(&STORY_LIST), ORDER, |st| st.path))
}

/// `items` with those named in `order` first, in that order; the rest keep their place after them.
pub(crate) fn ordered<T>(mut items: Vec<T>, order: &[&str], name: impl Fn(&T) -> &str) -> Vec<T> {
    items.sort_by_key(|i| order.iter().position(|n| *n == name(i)).unwrap_or(usize::MAX));
    items
}

const COPY_KEYS: &[Trigger] = &[replay(copy_last)];

/// Copies the session's last reply, as pressing its Copy does.
fn copy_last(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    *s.timeline_fx.copied = crate::copied::Copied::default();
    let id = sid(s);
    let last = s
        .transcripts
        .get(&id)
        .and_then(|t| t.messages.iter().rev().find(|m| matches!(m, Message::Assistant(_))))
        .map(|m| m.id().to_owned());
    if let Some(m) = last {
        tx.after(0.4, move |s, tx| s.mark_copied(format!("copy-{m}"), tx));
    }
}

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
        .typing(|s| {
            !s.fixture_run.stage.is_some_and(|st| st.watch)
                && (s.composer.focused
                    || s.question_takes_text()
                    || crate::plugin_host::field_focused(s)
                    || s.dialogs.project_switcher.search.focused
                    || s.search.focused
                    || s.dialogs.switcher.search.focused
                    || s.dialogs.fork.search.focused
                    || s.find_focused())
        })
        .folds(load_folds, save_folds)
        .widths(load_widths, save_widths);
    for st in stories() {
        for t in st.keys {
            pg = pg.trigger(st.path, t.key, t.label, t.run);
        }
        for &(name, v) in st.variants {
            pg = pg.variant(st.path, name, move |s: &mut App| *s.fixture_run.variant = v);
        }
        if !st.knobs.is_empty() {
            let knobs = st.knobs;
            pg = pg.side_keys(st.path, move |s, active, cx| side(s, knobs, active, cx), rune_knobs::side_key(knobs, |s: &mut App| &mut s.diag.knobs));
        }
    }
    pg
}

/// The groups folded in the list, kept in the app's database (`rune.dev` `storybook`, in the
/// boot packet). Headless runs and tests neither read nor write them, so a capture's
/// list never depends on what was folded at the desk.
fn load_folds() -> Vec<String> {
    if crate::headless() {
        return Vec::new();
    }
    crate::storage::json(crate::storage::DEV, "storybook").and_then(|mut v| serde_json::from_value(v["folded"].take()).ok()).unwrap_or_default()
}

/// The story list's and side column's widths, beside the folds (Rune keeps them in a file of its
/// own unless the app does). Zero leaves Rune's defaults.
fn load_widths() -> (f32, f32) {
    let v = if crate::headless() { None } else { crate::storage::json(crate::storage::DEV, "storybook.widths") };
    let w = |k: &str| v.as_ref().and_then(|v| v[k].as_f64()).map_or(0.0, |w| w as f32);
    (w("list"), w("side"))
}

fn save_widths(list: f32, side: f32) {
    if !crate::headless() {
        crate::storage::set_json(crate::storage::DEV, "storybook.widths", &json!({ "list": list, "side": side }));
    }
}

fn save_folds(folded: &[String]) {
    if !crate::headless() {
        crate::storage::set_json(crate::storage::DEV, "storybook", &json!({ "folded": folded }));
    }
}

/// The app's scenes, then a scene per story under its path.
pub fn scenes() -> &'static [(&'static str, fn(&mut App))] {
    let all: Vec<_> = self::scenes::all().into_iter().chain(stories().iter().map(|st| (st.path, st.scene))).collect();
    Box::leak(all.into_boxed_slice())
}

/// A story's world: the fixture, opened on its session, with what stays the same between the
/// app and its stories.
fn fresh(real: &App) -> App {
    let mut w = App::empty();
    w.load_fixture(FIXTURE);
    w.diag.knobs = real.diag.knobs.clone();
    *w.zoom = *real.zoom;
    *w.sidebar_w = *real.sidebar_w;
    *w.sidebar_open = *real.sidebar_open;
    *w.appearance_state.reduced_motion = *real.appearance_state.reduced_motion;
    *w.diag.debug = *real.diag.debug;
    w.settings.scheme = real.settings.scheme;
    w.settings.theme = real.settings.theme.clone();
    (w.settings.ui_font, w.settings.code_font) = (real.settings.ui_font, real.settings.code_font);
    w.settings.text_renderer = real.settings.text_renderer;
    *w.clock = *real.clock;
    // Relative times read as the fixture has them, however long the app has run.
    *w.fixture_run.base_ms -= (*real.clock * 1000.0) as i64;
    *w.now_ms = *w.fixture_run.base_ms + (*real.clock * 1000.0) as i64;
    w.waker = real.waker.clone();
    serve_pictures(&w);
    if let Some(waker) = (*w.waker).clone() {
        w.composer_ext.attach.set_waker(waker.clone());
        w.appearance_state.icons.set_waker(waker.clone());
        w.composer_ext.mentions.set_waker(waker.clone());
        w.dialogs.switcher.set_waker(waker.clone());
        w.composer_ext.add.slash.set_waker(waker);
    }
    open_scene(&mut w);
    w
}

/// What the stories tuned stays in the app.
fn keep(stage: &App, real: &mut App) {
    real.diag.knobs = stage.diag.knobs.clone();
}

/// Sets up a story's stage: `view` alone on it.
pub(crate) fn stage(s: &mut App, view: fn(&App, &Pal) -> El) {
    *s.fixture_run.stage = Some(Stage { view, watch: false });
}

/// [`stage`] for a story that only plays.
pub(crate) fn watch(s: &mut App, view: fn(&App, &Pal) -> El) {
    *s.fixture_run.stage = Some(Stage { view, watch: true });
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
    rune_knobs::inline(&s.diag.knobs, knobs, knobs_style(&p), |s: &mut App| &mut s.diag.knobs, active)
}

/// The stage: the story's component alone on the window's ground.
pub fn page(s: &App, p: &Pal, st: Stage) -> El {
    col().key("story-stage").w_full().h_full().fill(p.bg_deep).padding(24.0).center().child((st.view)(s, p))
}

/// A component's frame on the stage: the panel's surface, as wide as the timeline's column.
pub(crate) fn frame(p: &Pal, key: &str) -> El {
    col().key(key).w_full().max_w(820.0).radius(10.0).fill(p.bg_base).box_shadow(Pal::ring_out(p.ring)).shadow(1.0, 2.0, p.shadow).clip_soft(0.0)
}

pub(crate) fn composer_view(s: &App, p: &Pal) -> El {
    frame(p, "story-composer").pt(16.0).child(view::story_composer(s, p))
}

pub(crate) fn header_view(s: &App, p: &Pal) -> El {
    frame(p, "story-header").child(view::story_header(s, p))
}

/// The header over the timeline, for numbers that tick in both.
#[cfg(feature = "stories")]
pub(crate) fn ticking_view(s: &App, p: &Pal) -> El {
    frame(p, "story-ticking").h_full().max_h(680.0).child(view::story_header(s, p)).child(view::story_session(s, p))
}

/// The timeline and what sits under it, in a window-sized frame.
pub(crate) fn session_view(s: &App, p: &Pal) -> El {
    // With the drawn menus' layer, so a story can show one (a right-click's).
    frame(p, "story-session").h_full().max_h(680.0).child(view::story_session(s, p)).children(crate::popup::view(s, p))
}

/// A session opening, the whole page with its header, before and after the click.
pub(crate) fn opening_view(s: &App, p: &Pal) -> El {
    frame(p, "story-opening").h_full().max_h(680.0).child(self::open::view(s, p))
}

/// The history strip over the page it is on.
#[cfg(feature = "stories")]
pub(crate) fn history_view(s: &App, p: &Pal) -> El {
    frame(p, "story-history").h_full().max_h(680.0).child(self::shell::history::view(s, p))
}

// Setting up.

pub(crate) fn sid(s: &App) -> String {
    match &*s.route {
        Route::Session { id, .. } => id.clone(),
        _ => String::new(),
    }
}

pub(crate) fn scroll_key(s: &App) -> String {
    format!("scroll-{}", sid(s))
}

/// Plays a server event in the next poll.
pub(crate) fn emit(s: &mut App, ty: &str, mut data: Value, tx: &mut Tx) {
    if data["sessionID"].is_null() {
        data["sessionID"] = sid(s).into();
    }
    s.schedule(*s.clock, ty, data);
    tx.wake_after(std::time::Duration::ZERO);
}

fn composer_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    match *s.fixture_run.variant {
        1 => *s.composer = TextEdit::new(DRAFT),
        2 => pending(s),
        3 => {
            s.running.insert(CHILD.into());
        }
        // A call transcript pasted whole: the editor stops at its cap and shows the end.
        4 => {
            *s.composer = TextEdit::new(long_paste());
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

/// A prompt from another client lands without an entrance.
fn arrival_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.4, arrive);
}

fn arrive(s: &mut App, tx: &mut Play<App>) {
    *s.queue.sends += 1;
    let text = ["Sent from my phone: does this cover the laptop waking on battery too?", "And on a flaky hotel network?", "One more: what about a VPN dropping?"];
    let n = *s.queue.sends as usize;
    emit(s, "session.prompted", json!({ "messageID": format!("{ARRIVED}{n}"), "text": text[(n - 1) % text.len()] }), tx);
}

/// A draft in the composer, sent; the fixture's reply plays under it.
fn send_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    *s.composer = TextEdit::new(SEND_TEXT);
    tx.after(0.6, send);
}

fn send(s: &mut App, tx: &mut Play<App>) {
    prompt(s, SEND_TEXT, false, tx);
}

/// Sends `text` as typed in the composer: a steer or, with `queue`, a queued prompt while the
/// session works.
fn prompt(s: &mut App, text: &str, queue: bool, tx: &mut Tx) {
    *s.composer = TextEdit::new(text);
    s.submit(queue, tx);
}

/// The fixture's streaming turn, from its first event.
pub(crate) fn stream_open(s: &mut App, _: &mut Play<App>) {
    let id = sid(s);
    s.start_stream_if_due(&id);
}

fn finish_stream(s: &mut App, _: &mut Play<App>) {
    while let Some((_, ty, data)) = s.fixture_run.playback.pop_front() {
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
    if let Some(k) = key {
        s.toggle_expanded(&k);
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
    let at = LABELS.iter().position(|l| *l == s.timeline_fx.working.label()).unwrap_or(0);
    let now = *s.clock;
    s.timeline_fx.working.relabel(LABELS[(at + 1) % LABELS.len()], now);
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
    *s.composer = TextEdit::new(STEER_TEXT);
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

pub(crate) fn queue(s: &mut App, tx: &mut Play<App>) {
    let queued = s.inbox_for(&sid(s)).map_or(0, |i| i.queued().count());
    prompt(s, [QUEUED_TEXT, STEER_AGAIN][queued % 2], true, tx);
}

/// The server delivers the oldest queued prompt.
pub(crate) fn deliver_queued(s: &mut App, tx: &mut Play<App>) {
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
        let shot = |name: &str, size, a, b| Attachment { mime: "image/png".into(), name: name.into(), bytes: Arc::from(gradient(size, a, b)), media: None };
        vec![
            shot("sidebar.png", (640, 400), [0.22, 0.36, 0.86], [0.62, 0.32, 0.78]),
            shot("reconnect.png", (480, 480), [0.95, 0.55, 0.25], [0.86, 0.24, 0.42]),
        ]
    })
}

/// A reply's markdown picture reads from the storybook, never a server or the web.
pub(crate) fn serve_pictures(w: &App) {
    w.composer_ext.attach.markdown.set_fetch(
        Arc::new(|_: &str, path: &str| if path == REPLY_PICTURE { Ok(reply_picture().to_vec()) } else { Err("not in the storybook".into()) }),
        Arc::new(|_: &str| Err("offline".into())),
    );
}

/// A finished reply that shows the storybook's picture inline, at the end of the open session.
pub(crate) fn add_picture_reply(s: &mut App) {
    let text = format!("Fixed: the first heartbeat after a wake now waits for the socket. Here it is after the fix:\n\n![The sidebar after the fix](/story/{REPLY_PICTURE})\n");
    let reply = opencode_kit::model::Assistant { id: format!("{ARRIVED}imgreply"), completed: Some(1), parts: vec![opencode_kit::model::Part::Text { text }], ..Default::default() };
    let session = sid(s);
    if let Some(t) = s.transcripts.get_mut(&session) { t.messages.push(Message::Assistant(reply)); }
}

/// The file the story's reply shows inline, and its bytes.
const REPLY_PICTURE: &str = "after.png";
fn reply_picture() -> &'static [u8] {
    static PIC: OnceLock<Vec<u8>> = OnceLock::new();
    PIC.get_or_init(|| gradient((1200, 760), [0.16, 0.62, 0.52], [0.18, 0.30, 0.70]))
}

/// A `w`×`h` PNG shading from `a` to `b`.
fn gradient((w, h): (u32, u32), a: [f32; 3], b: [f32; 3]) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        let t = (x as f32 / w as f32 * 0.7 + y as f32 / h as f32 * 0.3).clamp(0.0, 1.0);
        let c = |i: usize| ((a[i] + (b[i] - a[i]) * t) * 255.0) as u8;
        image::Rgba([c(0), c(1), c(2), 255])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).expect("encode a PNG");
    out.into_inner()
}

/// A prompt with two images and a quote arrives, both above its words, and a reply shows its
/// own picture inline; the first image flies into the viewer and zooms in, Escape fits it, and a
/// second Escape while it still glides flies it home.
fn images_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.4, |s, tx| {
        let id = format!("{ARRIVED}img");
        opencode_kit::model::set_user_files(&id, pictures().to_vec());
        let quote = "One call for you: the sidebar reconnects on wake, but it drops the first heartbeat after the socket comes back";
        opencode_kit::model::set_user_comments(&id, vec![opencode_kit::model::Quote { text: quote.into(), comment: None }.into()]);
        emit(s, "session.prompted", json!({ "messageID": id, "text": "Here's the sidebar after a wake, and the reconnect log" }), tx);
    });
    tx.after(0.7, |s, tx| {
        add_picture_reply(s);
        tx.animated();
    });
    tx.after(1.6, view_image);
    tx.after(2.6, |s, tx| s.composer_ext.attach.zoom.zoom_in(tx));
    tx.after(3.6, |_, tx| tx.press(rune::Key::Escape));
    tx.after(3.8, |_, tx| tx.press(rune::Key::Escape));
}

/// Opens the first picture of the story's prompt, as a press on its thumbnail does.
fn view_image(s: &mut App, tx: &mut Play<App>) {
    let id = format!("{ARRIVED}img");
    if let Some(f) = opencode_kit::model::user_files(&id).first() {
        crate::attachments::view_image(s, &format!("user-attachments:{id}"), 0, f, tx);
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
