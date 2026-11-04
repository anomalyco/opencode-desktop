//! A subagent row while its agent starts, and how it comes alive. Until the call is written the
//! row reads "Delegating agent…"; until the child session runs, its spinner ripples: a soft wave
//! runs out from the centre of the same 5×5 grid as the running radar, a ring at a time. When
//! the child starts, the words give way to the task and the ripple crossfades into the radar, so
//! the row wakes in place instead of swapping.
//!
//! The dots are wall-clock cycles, like the radar's, so every starting row is in step.

use std::sync::OnceLock;

use rune::{Color, Cycle, El, Spring, SpringPair, TextStyle, Transition, el, text};

use crate::palette::Pal;
use crate::style::activity_ring_sized;

/// The handoff's crossfade: the radar fades up as the starting dots fade down on the same
/// spring, so the two grids' dots sit on each other and each one eases from its starting
/// brightness to the radar's, with no dip between.
const WAKE: SpringPair = SpringPair::new(Spring::new(0.7, 0.0), Spring::new(0.7, 0.0));

const WORDS: &str = "Delegating agent…";

/// The 14pt spinner: the starting dots while `starting`, else Desktop's radar. Either fades in
/// or out over the other, so the handoff is one grid waking up.
pub fn spinner(p: &Pal, starting: bool) -> El {
    let fade = Transition::opacity().springs(WAKE);
    let layer = |key: &'static str, child: El| el().key(key).absolute(0.0, 0.0).size(SIZE, SIZE).child(child).transition(fade);
    let shown = if starting { layer("boot", dots(p.icon_muted)) } else { layer("radar", activity_ring_sized(p, SIZE)) };
    el().key("grid").size(SIZE, SIZE).child(shown)
}

/// "Delegating agent…" in the task's place, in `font`.
pub fn words(p: &Pal, font: TextStyle) -> El {
    text(WORDS).font(font).color(p.text)
}

const SIZE: f32 = 14.0;

/// The radar's resting dot, and its steady centre (`view::sidebar::radar`).
const DIM: f32 = 0.2;
const CENTRE: f32 = 0.2 + 0.8 * 0.55;

/// How bright a dot in `ring` (0 the centre) is `x` of the way through the ripple's loop: a
/// soft hump as the wave passes, then rest.
fn ripple(x: f32, ring: i32) -> f32 {
    const WIDTH: f32 = 0.45;
    let hump = if x < WIDTH { 0.5 - 0.5 * (std::f32::consts::TAU * x / WIDTH).cos() } else { 0.0 };
    let peak = if ring == 0 { 0.8 } else { 0.7 };
    DIM + (peak - DIM) * hump
}

/// The ripple's loop, and how far behind the centre each ring runs (fractions of the loop).
const PERIOD: f32 = 1.8;
const RING_LAG: f32 = 0.2;

struct Dot {
    x: f32,
    y: f32,
    size: f32,
    cycle: Cycle,
}

/// The starting grid, built once.
fn pattern() -> &'static [Dot] {
    static DOTS: OnceLock<Vec<Dot>> = OnceLock::new();
    DOTS.get_or_init(|| {
        let scale = SIZE / 16.0;
        (0..25)
            .map(|i: i32| {
                let (gx, gy) = (i % 5 - 2, i / 5 - 2);
                let ring = gx.abs().max(gy.abs());
                let (x, y) = ((8.0 + gx as f32 * 3.0 - 1.0) * scale, (8.0 + gy as f32 * 3.0 - 1.0) * scale);
                let rest = if ring == 0 { CENTRE } else { DIM };
                const N: usize = 36;
                let stops: Vec<(f32, f32)> = (0..=N).map(|k| k as f32 / N as f32).map(|x| (x, ripple(x, ring))).collect();
                let cycle = Cycle { period: PERIOD, phase: ring as f32 * RING_LAG, stops: Box::leak(stops.into_boxed_slice()), still: rest, fps: 30.0 };
                Dot { x, y, size: 2.0 * scale, cycle }
            })
            .collect()
    })
}

fn dots(fg: Color) -> El {
    let mut r = el().key("dots").size(SIZE, SIZE);
    for (k, d) in pattern().iter().enumerate() {
        r = r.child(el().key(format!("d{k}")).absolute(d.x, d.y).size(d.size, d.size).radius(d.size * 0.24).fill(fg).cycle(d.cycle));
    }
    r
}
