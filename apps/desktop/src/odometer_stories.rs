//! The storybook's Odometer story (Components › Odometer): every live number the app rolls,
//! each in the font and words it has where it's used, ticking once a second. T ticks once, F
//! ticks fast, S switches session (the numbers change at once, with no roll), R replays.
//! Header › Ticking is the same numbers in place: a subagent card's time and the header's context
//! and cost.

use std::cell::Cell;

use opencode_kit::model::{self, Message};
use rune::{El, Play, TextStyle, col, row, text};

use crate::{App, Route};
use crate::odometer::{odometer, odometer_end};
use crate::palette::Pal;
use crate::stories::{Trigger, replay};
use crate::subagents::elapsed;
use crate::view::{UI, UI_SMALL};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 't', label: "Tick once", run: tick },
    Trigger { key: 'f', label: "Tick fast", run: fast },
    Trigger { key: 's', label: "Switch session", run: switch },
];

pub const TICKING_KEYS: &[Trigger] = &[replay(ticking)];

thread_local! {
    /// Ticks since the story opened, and which session it shows.
    static TICK: Cell<i64> = const { Cell::new(0) };
    static SESSION: Cell<i64> = const { Cell::new(0) };
}

/// The numbers start just short of their turns (9s → 10s, 59s → 1m 00s, $0.99 → $1.00) and tick
/// once a second.
fn open(_: &mut App, tx: &mut Play<App>) {
    TICK.set(0);
    SESSION.set(0);
    for k in 1..=40 {
        tx.after(0.6 + (k - 1) as f64, tick);
    }
}

fn tick(_: &mut App, _: &mut Play<App>) {
    TICK.set(TICK.get() + 1);
}

/// Twelve ticks a tenth of a second apart, so rolls land on rolls still moving.
fn fast(_: &mut App, tx: &mut Play<App>) {
    for k in 0..12 {
        tx.after(k as f64 * 0.1, tick);
    }
}

/// Another session's numbers: they show at once, since each session's odometers are their own.
fn switch(_: &mut App, _: &mut Play<App>) {
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

/// A subagent 7.5 s into its run, so its card's time turns 9s → 10s, and the header's context and
/// cost rising as steps finish ($1.99 → $2.00 among them).
fn ticking(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.2, |s, tx| {
        crate::agent_stories::spawn(s, tx);
        let now = s.now_ms;
        let parent = s.fixture["open"].as_str().map(str::to_owned);
        if let Some(c) = s.sessions.iter_mut().rev().find(|x| x.parent.is_some() && x.parent == parent) {
            c.created = now - 7_500;
        }
    });
    for k in 0..4 {
        tx.after(0.9 + k as f64, |s, _| {
            let Route::Session(id) = s.route.clone() else { return };
            let total = s.sessions.iter().find(|x| x.id == id).map_or(0.0, |i| i.cost);
            // To $1.97, then up 1¢ a step.
            let add = if total < 1.9 { 1.97 - total } else { 0.01 };
            if let Some(i) = s.sessions.iter_mut().find(|x| x.id == id) {
                i.cost += add;
            }
            let last = s.transcripts.get_mut(&id).and_then(|t| t.messages.iter_mut().rev().find_map(|m| match m {
                Message::Assistant(a) if a.tokens.context() > 0 => Some(a),
                _ => None,
            }));
            if let Some(a) = last {
                a.tokens.input += 2_300;
            }
        });
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story { path: "Components/Odometer", scene: |s| crate::stories::stage(s, view), knobs: &[], keys: KEYS, variants: &[] },
    crate::stories::Story { path: "Header/Ticking", scene: |s| crate::stories::watch(s, crate::stories::ticking_view), knobs: &[], keys: TICKING_KEYS, variants: &[] },
]);
