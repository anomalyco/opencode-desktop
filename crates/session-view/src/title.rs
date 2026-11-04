//! A session's title changing under you (the server's rename a few seconds into a turn): the
//! old title blurs out while the new one fades in and the row's width springs (Kit picked the
//! crossfade over swapping at once, t1137).

use rune::{Color, El, Spring, SpringPair, TextStyle, Transition, text};
use rune_knobs::{Knob, Knobs};

pub const TITLE_CHANGE: Knob = Knob::choice("Title change", &["Instant", "Crossfade"], 1)
    .about("How a renamed session's title changes")
    .hints(&["Swaps at once while the width opens", "Old title fades out, new fades in"]);
/// The crossfade, as a spring's duration.
pub const TITLE_SPEED: Knob = Knob::new("Title speed", 0.15, 0.6, 0.35).step(0.01).about("Seconds a title change takes");

/// `title`, keyed under `key`, changing by the Title change knob.
pub fn title(k: &Knobs, key: &str, title: String, font: TextStyle, color: Color) -> El {
    let spring = Spring::new(k.get(&TITLE_SPEED), 0.0);
    match k.get(&TITLE_CHANGE) as usize {
        1 => {
            // Keyed by the words, so a new title is a new node: the old one leaves in place while
            // the row's width springs, and the new one comes in over it.
            let tr = Transition::blur_replace().springs(SpringPair::new(spring, Spring::new(spring.duration * 0.8, 0.0)));
            text(title.clone()).key(format!("{key}:{title}")).font(font).color(color).truncate_fade().transition(tr)
        }
        _ => text(title).font(font).color(color).truncate_fade(),
    }
}
