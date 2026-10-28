//! The storybook's Odometer story (Components › Odometer): every live number the app rolls,
//! each in the font and words it has where it's used, ticking once a second. T ticks once, F
//! ticks fast, S switches session (the numbers change at once, with no roll), R replays.

use std::cell::Cell;

use opencode_kit::model;
use rune::{El, TextStyle, Tx, col, row, text};
use rune_knobs::Action;

use crate::App;
use crate::odometer::{odometer, odometer_end};
use crate::palette::Pal;
use crate::stories::replay;
use crate::subagents::elapsed;
use crate::view::{UI, UI_SMALL};

pub const KEYS: &[Action<App>] = &[
    Action { cap: "R", label: "Replay", run: replay },
    Action { cap: "T", label: "Tick once", run: tick },
    Action { cap: "F", label: "Tick fast", run: fast },
    Action { cap: "S", label: "Switch session", run: switch },
];

thread_local! {
    /// Ticks since the story opened, and which session it shows.
    static TICK: Cell<i64> = const { Cell::new(0) };
    static SESSION: Cell<i64> = const { Cell::new(0) };
}

/// The numbers start just short of their turns (9s → 10s, 59s → 1m 00s, $0.99 → $1.00) and tick
/// once a second.
pub fn open(s: &mut App, tx: &mut Tx) {
    TICK.set(0);
    SESSION.set(0);
    for k in 1..=40 {
        s.play.after(0.6 + (k - 1) as f64, tick, tx);
    }
}

fn tick(_: &mut App, _: &mut Tx) {
    TICK.set(TICK.get() + 1);
}

/// Twelve ticks a tenth of a second apart, so rolls land on rolls still moving.
fn fast(s: &mut App, tx: &mut Tx) {
    for k in 0..12 {
        s.play.after(k as f64 * 0.1, tick, tx);
    }
}

/// Another session's numbers: they show at once, since each session's odometers are their own.
fn switch(_: &mut App, _: &mut Tx) {
    SESSION.set(SESSION.get() + 1);
    TICK.set(TICK.get() + 37);
}

pub fn view(_: &App, p: &Pal) -> El {
    let n = TICK.get();
    let sid = SESSION.get();
    let secs = |at: i64| (at + n) * 1000;
    let tokens = (54_400 + n * 1_730) as u64;
    let cents = 92 + n * 3;
    let agents = 1 + n / 3;
    let queued = 1 + n / 4;
    let question = n % 4 + 1;
    let tools = 3 + n / 2;
    let place = |name: &str, el: El| {
        row().key(format!("place-{name}")).w_full().h(32.0).gap(16.0).center_y().child(text(name.to_owned()).font(UI).color(p.faint).w(180.0)).child(el)
    };
    let time = |at: i64| {
        let ms = secs(at);
        odometer_end(format!("story-time-{at}-{sid}"), elapsed(ms), (ms / 1000) as f64, UI_SMALL.lh(16.0), p.faint)
    };
    col()
        .key("story-odometer")
        .w_full()
        .max_w(520.0)
        .px(24.0)
        .py(20.0)
        .gap(4.0)
        .radius(10.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .child(place("Subagent card time", time(7)))
        .child(place("Subagent card time", time(56)))
        .child(place("Header context", odometer_end(format!("story-ctx-{sid}"), model::tokens_short(tokens), tokens as f64, UI_SMALL, p.muted)))
        .child(place("Header cost", odometer_end(format!("story-cost-{sid}"), format!("${}.{:02}", cents / 100, cents % 100), cents as f64, UI_SMALL, p.faint)))
        .child(place("Subagents capsule", odometer(format!("story-chip-{sid}"), format!("{agents} subagent{}", if agents == 1 { "" } else { "s" }), agents as f64, UI, p.text)))
        .child(place("Queue", odometer(format!("story-queue-{sid}"), format!("{queued} QUEUED"), queued as f64, TextStyle::new(11.0, 530.0).lh(14.0).tracking(0.05), p.muted)))
        .child(place("Question dock", odometer(format!("story-q-{sid}"), format!("{question} of 4 questions"), question as f64, TextStyle::new(13.0, 440.0).lh(20.0), p.muted)))
        .child(place("Tool group", row().gap(4.0).child(text("Used").font(UI).color(p.muted)).child(odometer(format!("story-tools-{sid}"), tools.to_string(), tools as f64, UI, p.text))))
}
