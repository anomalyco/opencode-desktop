//! The provider rail (p169, Kit's pick): Desktop's provider marks down the
//! model list's left side, as T3 Chat's picker has (Favorites first, then each provider). A
//! press jumps the list to that provider's heading and lights its first model; the headings stay.
//! The lit row's provider is lit on the rail.

use rune::{El, Icon, ScrollSpec, Transition, Tx, col, el, icon, row};

use super::provider_icons;
use crate::App;
use crate::palette::Pal;
use crate::pickers::{FAVORITES, RECENT};
use crate::popover::{self as pop, Look};

/// The rail's width: a 28 pt button and the list's inset each side.
pub const W: f32 = 28.0 + 2.0 * pop::LIST.inset;

/// One provider on the rail: its heading's name, where the heading starts in the list, and the
/// first model under it.
pub struct Stop {
    pub group: String,
    pub y: f32,
    pub first: String,
}

/// Desktop's mark for the group a row `key` ("provider:model") sits in. A provider with no mark
/// of its own by id tries its id without a `console-` prefix, hyphens or suffix, then takes
/// Desktop's stand-in.
pub fn mark(group: &str, key: &str) -> &'static Icon {
    if group == FAVORITES {
        return &rune_kit::glyphs::STAR_FILLED;
    }
    if group == RECENT {
        return &provider_icons::RECENT;
    }
    let id = key.split(':').next().unwrap_or("");
    let bare = id.strip_prefix("console-").unwrap_or(id);
    [id, bare, &bare.replace('-', ""), bare.split('-').next().unwrap_or(bare)]
        .into_iter()
        .find_map(provider_icons::named)
        .unwrap_or(provider_icons::FALLBACK)
}

/// The rail, as tall as the list beside it, `lit` being the lit row's group.
pub fn view(p: &Pal, look: Look, stops: &[Stop], lit: Option<&str>) -> El {
    let mut rail = col().key("picker-rail").w(W).h_full().padding(look.inset).gap(look.row_gap).items_center()
        .scroll(ScrollSpec { stick_bottom: false });
    let mut pinned = false;
    for stop in stops {
        let saved = stop.group == FAVORITES || stop.group == RECENT;
        if pinned && !saved {
            rail = rail.child(el().key("rail-divider").w(16.0).h(0.5).my(2.0).fill(p.border_muted));
        }
        pinned = saved;
        rail = rail.child(button(p, look, stop, lit == Some(stop.group.as_str())));
    }
    row().key("picker-rail-edge").h_full().child(rail).child(el().w(0.5).h_full().fill(p.border_muted))
}

fn button(p: &Pal, look: Look, stop: &Stop, lit: bool) -> El {
    let (first, y) = (stop.first.clone(), stop.y);
    row().key(format!("rail-{}", stop.group)).role("button").label(format!("Jump to {}", stop.group)).help(format!("Jump to {}", stop.group))
        .size(28.0, 28.0).center().transition(Transition::none()).radius(look.row_radius).fill(pop::lit(p, lit)).hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            pop::jump_highlight();
            s.pickers.activate(first.clone());
            tx.scroll_to_y("picker-scroll", y);
        })
        .child(icon(mark(&stop.group, &stop.first), 16.0).color(p.icon_muted).transition(Transition::none()))
}
