//! The Working row under a running turn: Desktop's spinner and its shimmering label.

use rune::{Easing, El, el, text};

use crate::App;
use crate::palette::Pal;
use crate::view::{UI, activity_ring};

/// The spinner and a shimmering "Working". Hidden, it is drawn still: its loops would keep
/// waking frames.
pub(crate) fn row(s: &App, p: &Pal, live: bool) -> El {
    let ring = if live { activity_ring(p) } else { el().key("ring").size(14.0, 14.0) };
    rune::row().h(28.0).gap(8.0).center_y().pb(12.0).child(ring).child(shimmer(s, p, "Working", live && !s.reduced_motion))
}

/// "Working" at 13 px Inter.
pub(crate) const WORKING_W: f32 = 52.0;

/// Desktop's `TextShimmer`: a soft band of the strong colour sweeps left to right across the
/// muted label every 1.2 s, from a gutter (spread + 1ch) before it to one after. The band is a
/// soft-edged window sliding over the label, with a copy inside sliding back so it stays put.
/// Its highlight is ±5.2ch of text-strong over text-weak (84 pt); both are Timeline knobs.
fn shimmer(s: &App, p: &Pal, label: &'static str, on: bool) -> El {
    let base = text(label).key("label").font(UI).color(p.muted);
    if !on {
        return rune::row().child(base);
    }
    let sheen = s.knobs.get(&crate::knobs::SHIMMER_WIDTH);
    let period = crate::knobs::secs(&s.knobs, &crate::knobs::SHIMMER_PERIOD);
    let gutter = sheen / 2.0 + 8.0;
    let (from, to) = ((-gutter - sheen / 2.0) / sheen, (WORKING_W + gutter - sheen / 2.0) / sheen);
    let sweep = |a: f32, b: f32| rune::LoopSpec::slide(period, [a, 0.0], [b, 0.0]).easing(Easing::LINEAR);
    let lit = el().key("lit").absolute(0.0, 0.0).w(sheen).h_full().looping(sweep(-from, -to))
        .child(text(label).font(UI).color(p.text).absolute(0.0, 0.0));
    rune::row().child(base).child(
        el().key("sheen").absolute(0.0, 0.0).w(sheen).h_full().clip_fade(sheen / 2.0, 0.0, sheen / 2.0, 0.0).looping(sweep(from, to)).child(lit),
    )
}
