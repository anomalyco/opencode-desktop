//! How every popover opens and closes: the ⌘⇧K Popover knobs. [`super::place`] gives the surface
//! its entrance and exit from them, and the commits that open and close a popover go through
//! [`opening`] and [`closing`]. By default a popover grows out of the control that opened it
//! in 0.12 s and is gone in the frame it closes (Kit, t1150).

use rune::{Align, Side, Spring, SpringPair, Transition, Tx, Vec2};
use rune_knobs::{Group, Knob, Knobs, Preset};

pub const OPEN: Knob = Knob::choice("Popover open", &["Instant", "Fade", "Grow from trigger", "Slide from trigger"], 2)
    .label("Open")
    .about("How a popover appears")
    .hints(&["Whole in the frame it opens", "Fades in where it sits", "Grows out of its trigger, fading in", "Slides out from its trigger, fading in"]);
pub const CLOSE: Knob = Knob::choice("Popover close", &["Instant", "Fade", "Shrink"], 0)
    .label("Close")
    .about("How a popover goes away")
    .hints(&["Gone in the frame it closes", "Fades out where it sits", "Shrinks toward its trigger, fading out"]);
pub const DURATION: Knob = Knob::new("Popover duration", 0.05, 0.5, 0.12).step(0.01).label("Duration").about("Seconds to open; closing takes 80%");
pub const BOUNCE: Knob = Knob::new("Popover bounce", 0.0, 0.4, 0.0).step(0.01).label("Bounce").about("Spring bounce as it opens and closes");
pub const SCALE: Knob = Knob::new("Popover scale", 0.9, 1.0, 0.96).step(0.01).label("Grow from").about("Starting scale for Grow and Shrink");
pub const DISTANCE: Knob = Knob::new("Popover distance", 0.0, 16.0, 6.0).step(1.0).label("Slide distance").about("Points a slide travels from its trigger");

pub const SLIDE: Knob = Knob::new("Highlight slide", 0.0, 0.3, 0.1).step(0.01).label("Highlight slide").about("Seconds the lit row's fill takes to move");

pub const GROUP: Group = Group::new("Popover", "popover", &[OPEN, CLOSE, DURATION, BOUNCE, SCALE, DISTANCE, SLIDE])
    .default_label("Grow")
    .presets(&[
        Preset { name: "Instant", values: &[("Popover open", 0.0), ("Popover close", 0.0)] },
        Preset { name: "Fade", values: &[("Popover open", 1.0), ("Popover close", 1.0), ("Popover duration", 0.15)] },
        Preset { name: "Grow and shrink", values: &[("Popover open", 2.0), ("Popover close", 2.0), ("Popover duration", 0.2)] },
        Preset { name: "Slide", values: &[("Popover open", 3.0), ("Popover close", 1.0), ("Popover duration", 0.2)] },
    ]);

rune_register::register!(crate::knobs::GROUP_LIST: Group = [GROUP]);

/// Whether a popover plays an entrance.
pub fn opens(k: &Knobs) -> bool {
    k.get(&OPEN) as usize != 0
}

/// Whether a popover plays an exit.
pub fn closes(k: &Knobs) -> bool {
    k.get(&CLOSE) as usize != 0
}

/// A commit that opens a popover lands in the keystroke, unless it has an entrance to play (an
/// instant commit mounts it whole). Colours still change at once: the trigger's pressed fill
/// doesn't fade in beside it.
pub fn opening(k: &Knobs, tx: &mut Tx) {
    if opens(k) {
        tx.color_motion(Spring::NONE);
    } else {
        tx.instant();
    }
}

/// A trigger's press: opens or closes its popover with [`opening`] or [`closing`], as `toggle`
/// leaves it (true when open).
pub fn toggled(k: &Knobs, tx: &mut Tx, open: bool) {
    if open { opening(k, tx) } else { closing(k, tx) }
}

/// A commit that closes a popover lands in the keystroke, unless it has an exit to play (an
/// instant commit unmounts it at once).
pub fn closing(k: &Knobs, tx: &mut Tx) {
    if closes(k) {
        tx.color_motion(Spring::NONE);
    } else {
        tx.instant();
    }
}

/// The way from a popover on `side` of its trigger back toward it, and the point of the popover
/// nearest the trigger, in its unit coordinates.
fn toward_trigger(side: Side, align: Align) -> (Vec2, Vec2) {
    let cross = match align {
        Align::Start => 0.0,
        Align::Center => 0.5,
        Align::End => 1.0,
    };
    match side {
        Side::Top => (Vec2::new(0.0, 1.0), Vec2::new(cross, 1.0)),
        Side::Bottom => (Vec2::new(0.0, -1.0), Vec2::new(cross, 0.0)),
        Side::Left => (Vec2::new(1.0, 0.0), Vec2::new(1.0, cross)),
        Side::Right => (Vec2::new(-1.0, 0.0), Vec2::new(0.0, cross)),
    }
}

/// The surface's entrance and exit, for a popover on `side` of its trigger.
pub fn transition(k: &Knobs, side: Side, align: Align) -> Transition {
    let (way, anchor) = toward_trigger(side, align);
    let (scale, d) = (k.get(&SCALE), k.get(&DISTANCE));
    let open = match k.get(&OPEN) as usize {
        1 => Transition::opacity(),
        2 => Transition::scale_anchor(scale, anchor),
        3 => Transition::offset(way.x * d, way.y * d).combined(Transition::opacity()),
        _ => Transition::identity(),
    };
    let close = match k.get(&CLOSE) as usize {
        1 => Transition::opacity(),
        2 => Transition::scale_anchor(scale, anchor),
        _ => Transition::identity(),
    };
    let (secs, bounce) = (k.get(&DURATION), k.get(&BOUNCE));
    let enter = if opens(k) { Spring::new(secs, bounce) } else { Spring::NONE };
    let exit = if closes(k) { Spring::new(secs * 0.8, bounce) } else { Spring::NONE };
    Transition::asymmetric(open, close).springs(SpringPair::new(enter, exit))
}
