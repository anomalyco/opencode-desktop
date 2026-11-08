use rune::{Key, KeyEvent, Mods, Runtime, TextEdit};

use super::{stories, story, storybook, world};
use crate::{App, FIXTURE, Source, open_scene};

const DT: f64 = 1.0 / 120.0;

fn letter(rt: &mut Runtime<App>, c: &str) {
    rt.key(KeyEvent { key: Key::Text(c.into()), mods: Mods::default() });
    rt.frame(DT);
}

fn run(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs * 120.0) as usize {
        rt.frame(DT);
    }
}

#[test]
fn the_storybook_leaves_the_app_as_it_was_and_sends_nothing() {
    let mut rt = storybook();
    let app = &mut rt.state;
    app.load_fixture(FIXTURE);
    open_scene(app);
    app.finish_stream();
    app.composer = TextEdit::new("a real draft");
    let writes = app.fake_writes();
    rt.frame(DT);

    // The steer story sends, steers and delivers in its own world, and nothing goes out.
    rt.open_playground("Timeline/Steer and Pending", None);
    assert!(matches!(world(&rt).source, Source::Fixture), "a story is its own fixture world");
    assert!(rt.playground_element("story-session").is_some(), "the session alone on the stage");
    assert!(rt.playground_element("tabs").is_none(), "nothing else on the stage");
    run(&mut rt, 1.0);
    let id = super::sid(world(&rt));
    assert_eq!(world(&rt).inbox_for(&id).map(|i| i.steering().count()), Some(1), "the steer waits as Pending");
    run(&mut rt, 1.6);
    assert!(world(&rt).inbox_for(&id).is_none(), "delivered");
    let last = world(&rt).transcripts[&id].messages.last().unwrap();
    assert!(matches!(last, opencode_kit::model::Message::User { text, .. } if text == super::STEER_TEXT));

    rt.update(|_, tx| tx.close_playground());
    rt.frame(DT);
    assert!(rt.playground_state::<App>().is_none());
    assert!(rt.state.stage.is_none());
    assert!(matches!(rt.state.source, Source::Live { .. }));
    assert_eq!(rt.state.composer.text, "a real draft");
    assert!(writes.try_recv().is_err(), "nothing reached the server");
}

/// The queue dock story fills the dock from the composer and empties it into the timeline.
#[test]
fn the_queue_dock_story_fills_and_empties_the_dock() {
    let mut rt = story("Composer/Queue dock", None);
    let id = super::sid(world(&rt));
    let queued = |rt: &Runtime<App>| world(rt).inbox_for(&id).map_or(0, |i| i.queued().count());
    let dock = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("queue")).map_or(0.0, |e| e.rect[3]);
    run(&mut rt, 2.0);
    assert_eq!(queued(&rt), 2, "two prompts wait in the dock");
    assert!(dock(&rt) > 40.0, "the dock is open");
    run(&mut rt, 2.5);
    assert_eq!(queued(&rt), 0, "both delivered");
    assert_eq!(dock(&rt), 0.0, "the dock closed behind the composer");
    let last = world(&rt).transcripts[&id].messages.last().unwrap();
    assert!(matches!(last, opencode_kit::model::Message::User { text, .. } if text == super::STEER_AGAIN));
    rt.playground_trigger('q');
    run(&mut rt, 0.1);
    assert_eq!(queued(&rt), 1, "Q queues another");
    rt.playground_trigger('d');
    run(&mut rt, 0.1);
    assert_eq!(queued(&rt), 0, "D delivers it");
}

/// R plays a story from the start the same way every time: the glow lands at the same
/// moment of each replay.
#[test]
fn replays_run_the_same_every_time() {
    let mut rt = story("Timeline/Arrival glow", None);
    let mut lit = Vec::new();
    for _ in 0..3 {
        rt.playground_trigger('r');
        let mut first = None;
        for f in 0..120 {
            rt.frame(DT);
            if first.is_none() && !world(&rt).glows.is_empty() {
                first = Some(f);
            }
        }
        lit.push(first.expect("it glowed"));
    }
    assert!(lit.iter().all(|f| *f == lit[0]), "the glow lands at the same frame of each replay: {lit:?}");
    assert!(rt.playground_element("story-session").is_some());
}

/// Every story opens, plays its motion and replays without a hitch, on each of its variants. A
/// story without a stage plays over the whole window.
#[test]
fn every_story_plays_and_replays() {
    let mut rt = storybook();
    let shown = |rt: &Runtime<App>| rt.playground_element(if world(rt).stage.is_some() { "story-stage" } else { "window" }).is_some();
    for st in stories() {
        let variants: Vec<Option<&str>> = if st.variants.is_empty() { vec![None] } else { st.variants.iter().map(|(n, _)| Some(*n)).collect() };
        for v in variants {
            rt.open_playground(st.path, v);
            assert!(shown(&rt), "{} {v:?}", st.path);
            run(&mut rt, 3.0);
            rt.playground_trigger('r');
            run(&mut rt, 1.0);
            assert!(shown(&rt), "{} {v:?}", st.path);
        }
    }
}

/// From the side column, a digit picks the story's variant: 2 shows the Question story's two-question form.
#[test]
fn a_digit_picks_the_variant() {
    let mut rt = story("Question dock/Question", None);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(DT);
    letter(&mut rt, "2");
    rt.frame(DT);
    let fields = world(&rt).pending.iter().find_map(|p| match p {
        opencode_kit::model::Pending::Form { fields, .. } => Some(fields.len()),
        _ => None,
    });
    assert_eq!(fields, Some(2));
}

/// The morph loop opens and closes the dock until C takes over: it closes and stays closed.
#[test]
fn a_trigger_stops_the_morph_loop() {
    let mut rt = story("Question dock/Morph", None);
    run(&mut rt, 1.0);
    assert!(world(&rt).question_shown(), "the loop opened the dock");
    rt.playground_trigger('c');
    run(&mut rt, 4.0);
    assert!(!world(&rt).question_shown(), "closed, and the loop's next open is gone");
}

/// The Odometer story ticks its numbers once a second, and S shows another session's at once.
#[test]
fn the_odometer_story_ticks_and_switches_session_at_once() {
    let mut rt = story("Components/Odometer", None);
    let label = |rt: &Runtime<App>, key: &str| rt.playground_element(key).and_then(|e| e.label);
    run(&mut rt, 0.1);
    assert_eq!(label(&rt, "story-cost-0").as_deref(), Some("$0.92"));
    run(&mut rt, 3.0);
    assert_eq!(label(&rt, "story-cost-0").as_deref(), Some("$1.01"), "it rose past $1.00");
    rt.playground_trigger('s');
    rt.frame(DT);
    assert!(label(&rt, "story-cost-0").is_none() && label(&rt, "story-cost-1").is_some(), "another session's number mounts fresh");
}

/// → from the list puts the keyboard on the story's knobs: → and ← step the first, ↓ picks the
/// next, and after Esc the arrows move in the list again. The app's own knobs wait for the close.
#[test]
fn arrows_reach_the_side_columns_knobs() {
    use crate::knobs::{GLOW_LENGTH, GLOW_RISE};
    let mut rt = story("Timeline/Arrival glow", None);
    let press = |rt: &mut Runtime<App>, key: Key| {
        rt.key(KeyEvent { key, mods: Mods::default() });
        rt.frame(DT);
    };
    let knob = |rt: &Runtime<App>, d| world(rt).knobs.get(d);
    let (length, rise) = (knob(&rt, &GLOW_LENGTH), knob(&rt, &GLOW_RISE));
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Right);
    assert_ne!(knob(&rt, &GLOW_LENGTH), length, "→ steps the first knob");
    press(&mut rt, Key::Left);
    assert_eq!(knob(&rt, &GLOW_LENGTH), length, "← steps it back rather than leaving");
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Right);
    assert_ne!(knob(&rt, &GLOW_RISE), rise, "↓ picks the next knob");
    assert_eq!(rt.state.knobs.get(&GLOW_RISE), rise, "the app's knobs wait for the close");
    press(&mut rt, Key::Escape);
    let tuned = knob(&rt, &GLOW_RISE);
    press(&mut rt, Key::Right);
    assert_eq!(knob(&rt, &GLOW_RISE), tuned, "after Esc the keyboard is back in the list");
}

/// The layout switch story opens Settings over the window in top tabs; S and T switch.
#[test]
fn the_layout_switch_story_switches_both_ways() {
    use crate::layout_state::TabLayout;
    let mut rt = story("Settings/Layout switch", None);
    run(&mut rt, 0.5);
    assert_eq!(world(&rt).tab_layout(), TabLayout::Horizontal);
    assert!(rt.playground_element("tab-strip").is_some(), "top tabs in the titlebar");
    rt.playground_trigger('s');
    run(&mut rt, 1.0);
    assert_eq!(world(&rt).tab_layout(), TabLayout::Vertical);
    assert!(rt.playground_element("sidebar-inner").is_some(), "side tabs in the sidebar");
    rt.playground_trigger('t');
    run(&mut rt, 1.0);
    assert_eq!(world(&rt).tab_layout(), TabLayout::Horizontal);
}

/// Stories, scenes and knob groups register from their own modules; the lists come out in the
/// order the central lists had.
#[test]
fn registered_lists_keep_their_order() {
    let paths: Vec<_> = stories().iter().map(|s| s.path).collect();
    assert_eq!(paths, STORY_PATHS);
    let groups: Vec<_> = crate::knobs::groups().iter().map(|g| g.name).collect();
    assert_eq!(groups, ["Global", "Composer", "Question dock", "Timeline", "Tool groups", "Working", "Terminal", "Header", "Dividers", "Copy", "Popover", "Stream", "Todo pill", "Project icons", "Experiments"]);
    let scenes: Vec<_> = super::scenes().iter().map(|s| s.0).collect();
    let all: Vec<_> = APP_SCENES.iter().chain(STORY_PATHS).copied().collect();
    assert_eq!(scenes, all);
}

const APP_SCENES: &[&str] = &[
    "home",
    "no-server",
    "session",
    "streaming",
    "busy",
    "busy-deliver",
    "queue",
    "queue-3",
    "queue-question",
    "queue-question-3",
    "queue-question-morph",
    "draft",
    "settings",
    "settings-updates",
    "settings-sidebar-closed",
    "question",
    "question-morph",
    "question-long",
    "sidebar-closed",
    "status",
    "tab-badges",
    "demo",
    "reply-image",
    "long-summary",
    "diagram-wide",
    "genui",
    "genui-stream",
    "diagram-stream",
    "permission",
    "reconnecting",
    "review-collapsed",
    "send-unconfirmed",
    "send-failed",
    "send-queued-long",
    "shell-fold",
    "short-group",
    "stop-background",
    "subagent",
    "no-server-remote",
];

const STORY_PATHS: &[&str] = &[
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
    "Home/Session search",
    "Home/No server",
    "Components/Odometer",
    "Components/Toast",
    "Header/Session header",
    "Header/Ticking",
    "Header/Title change",
    "Header/Narrow window",
    "Timeline/Arrival glow",
    "Timeline/Send",
    "Timeline/Long prompt",
    "Timeline/Streaming text",
    "Timeline/Generative UI",
    "Timeline/Diagram streaming",
    "Timeline/Diagram full size",
    "Timeline/Tool rows",
    "Timeline/Long tool group",
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
    "Settings/Updates",
    "Dev/Rewind",
    "Settings/Appearance",
    "Composer/Auto-accept",
    "Composer/Commands",
    "Timeline/Compacted summary",
    "Files/Link tips",
    "Timeline/Session mention",
    "Popover/Menus",
    "Session opening/Cold · ready at 100 ms",
    "Session opening/Prefetched · ready now",
    "Session opening/Slow · 150 ms fallback",
    "Session opening/Subagent · ready now",
    "Requests/Permission",
    "Popover/Open and close",
    "Question dock/Long question",
    "Session/Reconnecting",
    "Timeline/Send receipts",
    "Settings/Page",
    "Settings/Layout switch",
    "Composer/Background helpers",
    "Stream/Text speed",
    "Composer/Subagent reply",
    "Timeline/Open tool step",
];
