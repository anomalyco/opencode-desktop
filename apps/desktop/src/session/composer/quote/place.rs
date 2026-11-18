//! Where the quote's comment editor stands as its text scrolls (Kit, t1564): beside its text,
//! scrolling with it, out under the session header or the composer when the text goes, never
//! pinned to an edge. Each frame it lands where it's placed; going to the text's other side,
//! which the scroll doesn't carry it through, glides there on a spring.

use rune::machine::Effects;
use rune::{Live, Spring};
use rune_knobs::{Group, Knob};

/// Where the editor stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// By its card in the composer: an editor opened from the card.
    Card,
    /// Over its text.
    Above,
    /// Under its text.
    Below,
    /// Over the composer, centred: no pointer to stand by the text for.
    Pinned,
}

/// The editor's position, `[left, top]` in its band's points, as last placed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Placing {
    /// Just opened: its first place lands.
    #[default]
    Unplaced,
    /// Landing where it's placed each frame.
    Riding { place: Place, at: [f32; 2] },
    /// On its spring to `at`, which keeps being placed as the timeline scrolls, until it rests.
    Gliding { place: Place, at: [f32; 2] },
}

impl Placing {
    pub fn place(self) -> Option<Place> {
        match self {
            Placing::Unplaced => None,
            Placing::Riding { place, .. } | Placing::Gliding { place, .. } => Some(place),
        }
    }

    pub fn at(self) -> Option<[f32; 2]> {
        match self {
            Placing::Unplaced => None,
            Placing::Riding { at, .. } | Placing::Gliding { at, .. } => Some(at),
        }
    }
}

pub enum Event {
    /// This frame's place.
    Placed { place: Place, at: [f32; 2] },
    /// The glide's spring came to rest.
    Rested,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// Put it there at once.
    Land([f32; 2]),
    /// Move it there on the spring.
    Glide([f32; 2]),
}

pub fn step(state: &mut Placing, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (Placing::Riding { place: was, .. }, Event::Placed { place, at }) if crosses(was, place) => {
            *state = Placing::Gliding { place, at };
            Effects::one(Effect::Glide(at))
        }
        (Placing::Gliding { .. }, Event::Placed { place, at }) => {
            *state = Placing::Gliding { place, at };
            Effects::one(Effect::Glide(at))
        }
        (_, Event::Placed { place, at }) => {
            *state = Placing::Riding { place, at };
            Effects::one(Effect::Land(at))
        }
        (Placing::Gliding { place, at }, Event::Rested) => {
            *state = Placing::Riding { place, at };
            Effects::none()
        }
        (_, Event::Rested) => Effects::none(),
    }
}

/// Going to the text's other side: the one move the scroll doesn't carry it through.
fn crosses(was: Place, now: Place) -> bool {
    matches!((was, now), (Place::Above, Place::Below) | (Place::Below, Place::Above))
}

pub const GLIDE: Knob = Knob::new("Comment glide", 0.1, 0.8, 0.32).step(0.01).label("Changes side in").about("Seconds the comment takes to change side");
pub const BOUNCE: Knob = Knob::new("Comment bounce", 0.0, 0.5, 0.0).step(0.01).label("Bounce").about("How far it overshoots its new side");

pub const GROUP: Group = Group::new("Quote comment", "quote-comment", &[GLIDE, BOUNCE]).under("Timeline");

rune_register::register!(crate::knobs::GROUP_LIST: Group = [GROUP]);

pub fn spring(knobs: &rune_knobs::Knobs) -> Spring {
    Spring::new(knobs.get(&GLIDE), knobs.get(&BOUNCE))
}

thread_local! {
    /// Where the open editor is drawn: one is open at a time.
    pub static AT: Live<[f32; 2]> = Live::new([0.0, 0.0]);
}
