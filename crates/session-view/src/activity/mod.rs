//! The timeline as Desktop's New timeline draws it (Settings › Experimental › New timeline,
//! `settings.general.newTimeline`, `session-ui/src/timeline-next`), which is Rune's only
//! timeline: what each call is ([`rules`]), work that runs on as events ([`events`],
//! [`event_row`]), and a group named by what the agent did ([`label`]).

pub mod event_row;
pub mod events;
pub mod label;
pub mod rules;

use rune::{El, row, text};

use crate::palette::Pal;

/// Desktop's `Badge` after a row's words: "Failed", "Exit 1", "Stopped". `danger` in the
/// danger state's colours, else neutral.
pub fn tag(p: &Pal, label: &str, danger: bool) -> El {
    let (fill, ink) = if danger { (p.danger_bg, p.danger) } else { (p.layer2, p.muted) };
    row().key(format!("tag-{label}")).h(18.0).px(6.0).center().radius(4.0).fill(fill)
        .child(text(label.to_owned()).font(TAG).color(ink))
}

const TAG: rune::TextStyle = rune::TextStyle::new(12.0, 530.0).lh(16.0);
