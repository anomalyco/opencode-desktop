//! A subagent row while its agent starts, and how it comes alive. Until the call is written the
//! row reads "Delegating agent…"; until the child session runs, its spinner keeps a starting
//! pattern on the same 5×5 grid as the running radar. When the child starts, the words roll over
//! to the task and the starting dots crossfade into the radar, so the row wakes in place instead
//! of swapping. How it starts is a choice on ⌘⇧K (Subagent rows › Starting).
//!
//! The dots are wall-clock cycles, like the radar's, so every starting row is in step; the
//! Shimmer look is the Working sweep (`working::shimmer`), counted from the app clock's start.

use std::sync::Mutex;

use rune::{Color, Cycle, El, Spring, SpringPair, TextStyle, Transition, el, text};
use rune_knobs::{Group, Knob};

use crate::App;
use crate::palette::Pal;
use crate::view::{activity_ring_on, srgb_opacity};

pub const STARTING: Knob = Knob::choice("Starting", &["Ripple", "Breathe", "Shimmer"], 0)
    .about("How a subagent's row moves while starting")
    .hints(&["Dots ripple out from the centre", "The whole grid breathes slowly", "The words shimmer over a still grid"]);

rune_register::register!(crate::knobs::GROUP_LIST: Group = [Group::new("Subagent rows", "agent-card", &[STARTING])]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Ripple,
    Breathe,
    Shimmer,
}

pub fn look(s: &App) -> Look {
    match s.knobs.get(&STARTING).round() as usize {
        1 => Look::Breathe,
        2 => Look::Shimmer,
        _ => Look::Ripple,
    }
}

/// The handoff's crossfade: the radar fades up as the starting dots fade down on the same
/// spring, so the two grids' dots sit on each other and each one eases from its starting
/// brightness to the radar's, with no dip between.
const WAKE: SpringPair = SpringPair::new(Spring::new(0.7, 0.0), Spring::new(0.7, 0.0));

const WORDS: &str = "Delegating agent…";

/// The 14pt spinner over `on`: the starting dots while `starting`, else Desktop's radar. Either
/// fades in or out over the other, so the handoff is one grid waking up.
pub(crate) fn spinner(s: &App, p: &Pal, on: Color, starting: bool) -> El {
    let fade = Transition::opacity().springs(WAKE);
    let layer = |key: &'static str, child: El| el().key(key).absolute(0.0, 0.0).size(SIZE, SIZE).child(child).transition(fade);
    let shown = if starting { layer("boot", dots(p.icon_muted, on, look(s))) } else { layer("radar", activity_ring_on(p, on, SIZE)) };
    el().key("grid").size(SIZE, SIZE).child(shown)
}

/// "Delegating agent…" in the task's place, in `font`.
pub(crate) fn words(s: &App, p: &Pal, font: TextStyle) -> El {
    match look(s) {
        Look::Shimmer => crate::working::shimmer(s, p, WORDS, font, 0.0),
        _ => text(WORDS).font(font).color(p.text),
    }
}

const SIZE: f32 = 14.0;

/// The radar's resting dot, and its steady centre (`view::sidebar::radar`).
const DIM: f32 = 0.2;
const CENTRE: f32 = 0.2 + 0.8 * 0.55;

/// One dot of the grid at 16 units, its ring (0 the centre), and how the look lights it.
fn cycle_of(look: Look, ring: i32) -> Option<(f32, f32, fn(f32, i32) -> f32)> {
    match look {
        // A soft wave runs out from the centre to the edge, a ring at a time.
        Look::Ripple => Some((1.8, ring as f32 * 0.2, |x, ring| {
            let w = 0.45;
            let hump = if x < w { 0.5 - 0.5 * (std::f32::consts::TAU * x / w).cos() } else { 0.0 };
            let peak = if ring == 0 { 0.8 } else { 0.7 };
            DIM + (peak - DIM) * hump
        })),
        // The grid breathes together, slower than the radar turns.
        Look::Breathe => Some((2.4, 0.0, |x, ring| {
            let lung = 0.5 - 0.5 * (std::f32::consts::TAU * x).cos();
            let (low, high) = if ring == 0 { (0.35, 0.75) } else { (DIM, 0.5) };
            low + (high - low) * lung
        })),
        // Still: the radar at rest, its head not yet lit.
        Look::Shimmer => None,
    }
}

struct Dot {
    x: f32,
    y: f32,
    size: f32,
    still: f32,
    cycle: Option<Cycle>,
}

/// The starting grid for one colour, backdrop and look, built once each.
fn pattern(fg: Color, bg: Color, look: Look) -> &'static [Dot] {
    type Key = ([u32; 6], Look);
    static BUILT: Mutex<Vec<(Key, &'static [Dot])>> = Mutex::new(Vec::new());
    let key: Key = ([fg.r, fg.g, fg.b, bg.r, bg.g, bg.b].map(f32::to_bits), look);
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, dots)) = built.iter().find(|(k, _)| *k == key) {
        return dots;
    }
    let scale = SIZE / 16.0;
    let op = |a: f32| srgb_opacity(a, fg, bg);
    let dots: Vec<Dot> = (0..25)
        .map(|i: i32| {
            let (gx, gy) = (i % 5 - 2, i / 5 - 2);
            let ring = gx.abs().max(gy.abs());
            let (x, y) = ((8.0 + gx as f32 * 3.0 - 1.0) * scale, (8.0 + gy as f32 * 3.0 - 1.0) * scale);
            let rest = if ring == 0 { CENTRE } else { DIM };
            let cycle = cycle_of(look, ring).map(|(period, phase, level)| {
                const N: usize = 36;
                let stops: Vec<(f32, f32)> = (0..=N).map(|k| k as f32 / N as f32).map(|x| (x, op(level(x, ring)))).collect();
                Cycle { period, phase, stops: Box::leak(stops.into_boxed_slice()), still: op(rest), fps: 30.0 }
            });
            Dot { x, y, size: 2.0 * scale, still: op(rest), cycle }
        })
        .collect();
    let dots: &'static [Dot] = Box::leak(dots.into_boxed_slice());
    built.push((key, dots));
    dots
}

fn dots(fg: Color, bg: Color, look: Look) -> El {
    let mut r = el().key("dots").size(SIZE, SIZE);
    for (k, d) in pattern(fg, bg, look).iter().enumerate() {
        let dot = el().key(format!("d{k}")).absolute(d.x, d.y).size(d.size, d.size).radius(d.size * 0.24).fill(fg);
        r = r.child(match d.cycle {
            Some(c) => dot.cycle(c),
            None => dot.opacity(d.still),
        });
    }
    r
}
