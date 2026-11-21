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
/// `[data-component="tag"]` (`badge.css:1-39`): 16 tall, 4 in, 2 round, 0.5 border, 11 at 530.
pub fn tag(p: &Pal, label: &str, danger: bool) -> El {
    let (fill, border, ink) = if danger {
        (p.danger_bg, p.danger_border, p.danger)
    } else {
        (p.layer2, p.border, p.muted)
    };
    row()
        .key(format!("tag-{label}"))
        .h(16.0)
        .px(4.0)
        .center()
        .radius(2.0)
        .border(0.5, border)
        .fill(fill)
        .child(text(label.to_owned()).font(TAG).color(ink))
}

const TAG: rune::TextStyle = rune::TextStyle::new(11.0, 530.0).lh(11.0).tracking(0.05);
