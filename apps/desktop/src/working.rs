//! The Working row under a running turn: Desktop's spinner and its label, in the shimmer style
//! the Working knobs pick (⌘⇧K, Working).
//!
//! Every style is loops the renderer runs by itself, so nothing lays out again per frame. Their
//! rhythm is keyed to the run rather than to when a node mounted: [`sync`] notes when each
//! session's run started, and a loop that mounts mid-run either waits for its place (a slide's
//! `delay`) or reads the clock against the run's start (a `Cycle`'s `phase`). A new label keeps
//! the same nodes, so their loops carry on at the same phase (`LoopTrack::retime`).

use std::collections::HashMap;

use rune::color::{hex, srgb_decode, srgb_encode};
use rune::{Color, Cycle, Easing, El, LoopSpec, el, text};

use crate::App;
use crate::knobs::{
    WORKING, WORKING_BAND, WORKING_DIRECTION, WORKING_INTENSITY, WORKING_PACE, WORKING_PAUSE, WORKING_PERIOD, WORKING_SOFTNESS, WORKING_SPEED,
    WORKING_STYLE, WORKING_UNIT, secs,
};
use crate::palette::Pal;
use crate::view::{UI, activity_ring};

pub const STYLES: [&str; 6] = ["Desktop", "Smooth sweep", "Breath", "Wave", "Glint", "Dots"];
const DESKTOP: usize = 0;
const SMOOTH: usize = 1;
const BREATH: usize = 2;
const WAVE: usize = 3;
const GLINT: usize = 4;
const DOTS: usize = 5;

#[derive(Default)]
pub struct Working {
    /// When each session's current run started, on the app clock.
    since: HashMap<String, f64>,
    /// A label in place of "Working" (the storybook's L).
    label: Option<&'static str>,
    /// The label before the last change and when it changed, until the sweep's pass then
    /// under way ends: that pass carries on from where it was ([`finish_pass`]).
    relabeled: Option<(&'static str, f64)>,
    /// The style whose preset the knobs last loaded.
    pub style: usize,
}

impl Working {
    pub fn label(&self) -> &'static str {
        self.label.unwrap_or("Working")
    }
    /// Shows `label` from `now` on (app clock).
    pub fn relabel(&mut self, label: &'static str, now: f64) {
        if self.relabeled.is_none() {
            self.relabeled = Some((self.label(), now));
        }
        self.label = Some(label);
    }
}

/// After the frame's events: starts the clock of each run that began and drops those that
/// ended, ends a pass a new label interrupted once it's over, and loads the preset of a newly
/// picked style. True when what's drawn changed.
pub fn sync(s: &mut App) -> bool {
    let App { working, transcripts, running, clock, .. } = s;
    let live = |id: &str| running.contains(id) || transcripts.get(id).is_some_and(|t| t.busy());
    let runs = working.since.len();
    working.since.retain(|id, _| live(id));
    let mut changed = working.since.len() != runs;
    for id in transcripts.keys().chain(running.iter()) {
        if !working.since.contains_key(id) && live(id) {
            working.since.insert(id.clone(), *clock);
            changed = true;
        }
    }
    if let Some((old, at)) = s.working.relabeled {
        let since = match &s.route {
            crate::Route::Session(id) => s.working.since.get(id).copied(),
            _ => None,
        };
        let pass = |t: f64, since: f64, b: Band| ((t - since) / b.period).floor();
        if !since.zip(band(s, true, width(old))).is_some_and(|(since, b)| pass(s.clock, since, b) == pass(at, since, b)) {
            s.working.relabeled = None;
            changed = true;
        }
    }
    let style = s.knobs.get(&WORKING_STYLE).round() as usize;
    if style == s.working.style {
        return changed;
    }
    s.working.style = style;
    let preset = if style == DESKTOP { "Default" } else { STYLES[style] };
    if s.knobs.drifted(&WORKING, preset) {
        s.knobs.apply(&WORKING, preset);
    }
    true
}

/// The knob value for `--working-style <name>` ("glint", "smooth-sweep"), if it names a style.
pub fn style_arg() -> Option<f32> {
    let name = crate::arg("--working-style")?.replace(['-', '_'], " ");
    STYLES.iter().position(|s| s.eq_ignore_ascii_case(&name)).map(|i| i as f32)
}

/// The spinner and the label. Hidden, it is drawn still: its loops would keep waking frames.
pub(crate) fn row(s: &App, p: &Pal, session: &str, live: bool) -> El {
    let ring = if live { activity_ring(p) } else { el().key("ring").size(14.0, 14.0) };
    rune::row().h(28.0).gap(8.0).center_y().pb(12.0).child(ring).child(label(s, p, session, live && !s.reduced_motion))
}

/// A label's width at the row's 13 px Inter.
pub(crate) fn width(label: &str) -> f32 {
    rune::text::measure(label, &[], UI, None).0
}

fn style(s: &App) -> usize {
    s.knobs.get(&WORKING_STYLE).round() as usize
}

fn label(s: &App, p: &Pal, session: &str, on: bool) -> El {
    let words = s.working.label();
    let style = style(s);
    let (weak, strong) = colors(p);
    let base = text(words).key("label").font(UI).color(weak);
    let w = width(words);
    if !on {
        let still = rune::row().child(base);
        return if style == DOTS { still.child(text("...").key("dots").font(UI).color(weak)) } else { still };
    }
    let run = Run { since: s.working.since.get(session).copied().unwrap_or(s.clock), now: s.clock };
    let k = &s.knobs;
    let lit = if style == DESKTOP { strong } else { weak.mix(strong, k.get(&WORKING_INTENSITY)) };
    let period = secs(k, &WORKING_PERIOD);
    if let Some(mut b) = band(s, p.dark, w) {
        let mut shown = words;
        if let Some((old, at)) = s.working.relabeled
            && let Some(was) = band(s, p.dark, width(old))
            && ((s.clock - run.since) / was.period).floor() == ((at - run.since) / was.period).floor()
        {
            let f = ((at - run.since) / was.period).rem_euclid(1.0) as f32;
            let here = was.x0 + f * (was.x1 - was.x0);
            let past = if was.x1 >= was.x0 { here >= width(old) } else { here + was.width <= 0.0 };
            if past {
                // Out of sight past the old words: the pass ends unseen, over them.
                (b, shown) = (was, old);
            } else {
                b = finish_pass(was, w, f);
            }
        }
        return sweep(run, shown, base, lit, b);
    }
    match style {
        BREATH => {
            let c = Cycle { period: period as f32, phase: run.phase(period, 0.0), stops: BREATHE, still: 0.0, fps: 120.0 };
            rune::row().child(base).child(text(words).key("lit").font(UI).color(lit).absolute(0.0, 0.0).cycle(c))
        }
        WAVE => wave(run, words, base, lit, k.get(&WORKING_BAND).max(4.0), period, k.get(&WORKING_DIRECTION) > 0.5),
        _ => {
            let mut r = rune::row().child(base);
            let dot = width(".");
            for (i, &stops) in DOT_STOPS.iter().enumerate() {
                let c = Cycle { period: period as f32, phase: run.phase(period, 0.0), stops, still: 1.0, fps: 120.0 };
                r = r.child(text(".").key(format!("dot{i}")).font(UI).color(lit).absolute(w + dot * i as f32, 0.0).cycle(c));
            }
            r
        }
    }
}

/// Desktop's gutter: its spread (5.2ch at 13 px Inter) and 1ch more.
const DESKTOP_GUTTER: f32 = 50.0;

/// Where a run is: when it started and the app clock now.
#[derive(Clone, Copy)]
struct Run {
    since: f64,
    now: f64,
}

impl Run {
    /// The delay that starts a loop mounting now in step with the run, `offset` periods in at
    /// its start. A loop that mounts mid-pass waits for the next.
    fn delay(self, period: f64, offset: f64) -> f64 {
        let d = (-((self.now - self.since) / period + offset)).rem_euclid(1.0) * period;
        if d > period - 1e-6 { 0.0 } else { d }
    }
    /// A `Cycle`'s phase that reads the clock from the run's start, `offset` periods in.
    fn phase(self, period: f64, offset: f64) -> f32 {
        (self.since / period - offset).rem_euclid(1.0) as f32
    }
}

/// A sweep: a band `width` wide whose left edge goes from `x0` to `x1` once a `period`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Band {
    width: f32,
    /// The soft part of each edge.
    soft: f32,
    x0: f32,
    x1: f32,
    period: f64,
}

/// The sweep over a label `w` wide, or None for the styles that don't sweep.
fn band(s: &App, dark: bool, w: f32) -> Option<Band> {
    let k = &s.knobs;
    let (width, soft, gutter, travel, period, rtl) = match style(s) {
        DESKTOP => {
            // Desktop's band is ±5.2ch, a straight ramp blended in sRGB. Ours is a smoothstep
            // ramp blended in linear light, so the half-width that looks the same is narrower
            // on dark and wider on light. It travels from a gutter of spread + 1ch before the
            // label to one after it every 1.2 s, whatever the label's width.
            let half = if dark { 40.0 } else { 55.0 };
            (2.0 * half, half, DESKTOP_GUTTER, w + 2.0 * DESKTOP_GUTTER, 1.2, false)
        }
        SMOOTH | GLINT => {
            // The band leaves the label entirely before it comes round; a pause is more travel
            // past the end, out of sight.
            let band = (if k.get(&WORKING_UNIT) > 0.5 { w * k.get(&WORKING_BAND) / 100.0 } else { k.get(&WORKING_BAND) }).max(4.0);
            let span = w + band;
            let pass = if k.get(&WORKING_PACE) > 0.5 { f64::from(span / k.get(&WORKING_SPEED)) } else { secs(k, &WORKING_PERIOD) };
            let total = pass + secs(k, &WORKING_PAUSE);
            let soft = k.get(&WORKING_SOFTNESS) * band / 2.0;
            (band, soft, band / 2.0, span * (total / pass) as f32, total, k.get(&WORKING_DIRECTION) > 0.5)
        }
        _ => return None,
    };
    let x0 = -gutter - width / 2.0;
    let x1 = x0 + travel;
    let (x0, x1) = if rtl { (w - x0 - width, w - x1 - width) } else { (x0, x1) };
    Some(Band { width, soft, x0, x1, period })
}

/// The rest of a pass a new label `w` wide interrupted `at` of the way through: the band
/// carries on from where it was, at its old size, speed and period. Only if that wouldn't
/// clear the new words before the pass ends does it speed up, just enough to: it never jumps,
/// and the next pass is the new label's, in the run's rhythm.
fn finish_pass(was: Band, w: f32, at: f32) -> Band {
    let here = was.x0 + at * (was.x1 - was.x0);
    let speed = was.x1 - was.x0;
    let left = (1.0 - at).max(1e-3);
    let slope = if speed >= 0.0 { speed.max((w - here) / left) } else { speed.min((-was.width - here) / left) };
    Band { x0: here - at * slope, x1: here + (1.0 - at) * slope, ..was }
}

/// A soft-edged window of the lit `words` sliding over the weak label: the window travels, and
/// the lit copy inside it slides back so it stays on the label.
fn sweep(run: Run, words: &'static str, base: El, lit: Color, b: Band) -> El {
    let w = b.width;
    let delay = run.delay(b.period, 0.0);
    let spec = |a: f32, c: f32| LoopSpec { delay, ..LoopSpec::slide(b.period, [a, 0.0], [c, 0.0]).easing(Easing::LINEAR) };
    let copy = el().key("lit").absolute(0.0, 0.0).w(w).h_full().looping(spec(-b.x0 / w, -b.x1 / w))
        .child(text(words).font(UI).color(lit).absolute(0.0, 0.0));
    rune::row().child(base).child(
        el().key("sheen").absolute(0.0, 0.0).w(w).h_full().clip_fade(b.soft, 0.0, b.soft, 0.0).looping(spec(b.x0 / w, b.x1 / w)).child(copy),
    )
}

/// Each glyph brightens and dims in turn, a little after the one before: a crest that rolls
/// along the label once a period, `length` points from one crest to the next. A glyph's place
/// in the wave hangs on its own position, not the label's width, so new words keep the wave.
fn wave(run: Run, words: &'static str, base: El, lit: Color, length: f32, period: f64, rtl: bool) -> El {
    let mut r = rune::row().child(base);
    for (i, (at, ch)) in words.char_indices().enumerate() {
        if ch.is_whitespace() {
            continue;
        }
        // Where the label sets this glyph, kerning with the one before included.
        let glyph = &words[at..at + ch.len_utf8()];
        let advance = width(glyph);
        let x = width(&words[..at + ch.len_utf8()]) - advance;
        let centre = x + advance / 2.0;
        let lag = f64::from(if rtl { -centre } else { centre } / length);
        let c = Cycle { period: period as f32, phase: run.phase(period, -lag), stops: CREST, still: 0.0, fps: 120.0 };
        r = r.child(text(glyph).key(format!("g{i}")).font(UI).color(lit).absolute(x, 0.0).cycle(c));
    }
    r
}

/// An ease-in-out rise and fall over the whole period.
const BREATHE: &[(f32, f32)] = &[
    (0.0, 0.0), (0.1, 0.104), (0.2, 0.352), (0.3, 0.648), (0.4, 0.896), (0.5, 1.0),
    (0.6, 0.896), (0.7, 0.648), (0.8, 0.352), (0.9, 0.104), (1.0, 0.0),
];

/// A raised-cosine crest over the first half of the period, then rest.
const CREST: &[(f32, f32)] = &[
    (0.0, 0.0), (0.05, 0.095), (0.1, 0.345), (0.15, 0.655), (0.2, 0.905), (0.25, 1.0),
    (0.3, 0.905), (0.35, 0.655), (0.4, 0.345), (0.45, 0.095), (0.5, 0.0), (1.0, 0.0),
];

/// Three dots that come on one after another and go out together.
const DOT_STOPS: [&[(f32, f32)]; 3] = [
    &[(0.0, 0.0), (0.05, 0.0), (0.15, 1.0), (0.8, 1.0), (0.9, 0.0), (1.0, 0.0)],
    &[(0.0, 0.0), (0.3, 0.0), (0.4, 1.0), (0.8, 1.0), (0.9, 0.0), (1.0, 0.0)],
    &[(0.0, 0.0), (0.55, 0.0), (0.65, 1.0), (0.8, 1.0), (0.9, 0.0), (1.0, 0.0)],
];

/// Desktop's `--text-weak` and `--text-strong` (white at 42.2% and 93.6% on dark), each solved
/// to the opaque colour a browser blends over the page in sRGB.
fn colors(p: &Pal) -> (Color, Color) {
    if !p.dark {
        return (hex("#8f8f8f").unwrap(), hex("#171717").unwrap());
    }
    let bg = p.bg_base;
    let over = |a: f32| {
        let c = |b: f32| srgb_decode(srgb_encode(b) + (1.0 - srgb_encode(b)) * a);
        Color { r: c(bg.r), g: c(bg.g), b: c(bg.b), a: 1.0 }
    };
    (over(0.422), over(0.936))
}

#[cfg(test)]
#[path = "working_tests.rs"]
mod tests;
