//! The provider rail (p169, Kit's pick): Desktop's provider marks down the
//! model list's left side, as T3 Chat's picker has (Recent and Favorites first, then each provider). A
//! press jumps the list to that provider's heading and lights its first model; the headings stay.
//! The lit row's provider is lit on the rail.

use rune::{El, Icon, ScrollSpec, Transition, Tx, col, el, icon, row};
use rune_theme_opencode::provider_icons;

use super::{FAVORITES, PickerHost, RECENT};
use crate::palette::Pal;
use crate::popover::{self as pop, Look};

/// The rail's width: a 28 pt button and the list's inset each side.
pub const W: f32 = 28.0 + 2.0 * pop::LIST.inset;

/// Where the list starts: the rail and the hairline after it.
pub const LEAD: f32 = W + 0.5;

/// Recent's mark: a clock (Desktop's catalog has none; its rotate arrow read as "reset", p169).
static CLOCK: Icon = Icon {
    name: "clock",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="5.75" stroke="currentColor"/><path d="M8 4.75V8L10.25 9.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// Desktop's mark for a provider it has no icon for (`providers/custom-managed-provider.svg`,
/// `provider-group.tsx:23-47`), less its backdrop blur.
pub static CUSTOM: Icon = Icon {
    name: "custom-managed-provider",
    view: 16.0,
    body: r##"<defs><linearGradient id="cmp0" x1="7.99996" y1="1.33496" x2="7.99996" y2="14.665" gradientUnits="userSpaceOnUse"><stop stop-color="white"/><stop offset="1" stop-color="white" stop-opacity="0"/></linearGradient><linearGradient id="cmp1" x1="7.99988" y1="3.62598" x2="7.99988" y2="12.3737" gradientUnits="userSpaceOnUse"><stop stop-color="white" stop-opacity="0.8"/><stop offset="1" stop-color="white" stop-opacity="0.5"/></linearGradient><clipPath id="cmpc"><rect x="1.33496" y="1.33496" width="13.33" height="13.33" rx="1.66625"/></clipPath></defs><g clip-path="url(#cmpc)"><rect x="1.33496" y="1.33496" width="13.33" height="13.33" rx="1.66625" fill="#3A3A3A"/><rect x="1.33496" y="1.33496" width="13.33" height="13.33" rx="1.66625" fill="url(#cmp0)" fill-opacity="0.2"/><circle cx="7.99988" cy="7.99988" r="4.26977" stroke="#808080" stroke-width="0.208281"/><circle cx="7.99993" cy="7.99993" r="2.2786" stroke="#808080" stroke-width="0.208281"/><rect x="1.38703" y="7.94782" width="13.2259" height="0.104141" stroke="#808080" stroke-width="0.104141"/><rect x="1.38703" y="12.2176" width="13.2259" height="0.104141" stroke="#808080" stroke-width="0.104141"/><rect x="1.38703" y="3.67805" width="13.2259" height="0.104141" stroke="#808080" stroke-width="0.104141"/><rect x="7.94782" y="1.38703" width="0.104141" height="13.2259" stroke="#808080" stroke-width="0.104141"/><rect x="3.67805" y="1.38703" width="0.104141" height="13.2259" stroke="#808080" stroke-width="0.104141"/><rect x="12.2176" y="1.38703" width="0.104141" height="13.2259" stroke="#808080" stroke-width="0.104141"/><rect x="0.930816" y="1.00439" width="0.104141" height="19.8909" transform="rotate(-45 0.930816 1.00439)" stroke="#808080" stroke-width="0.104141"/><rect x="14.9958" y="0.930572" width="0.104141" height="19.8909" transform="rotate(45 14.9958 0.930572)" stroke="#808080" stroke-width="0.104141"/><path d="M8 3.73047C10.3541 3.73055 12.2695 5.64586 12.2695 8C12.2695 10.3541 10.3541 12.2695 8 12.2695C7.25389 12.2695 6.55544 12.071 5.94434 11.7324H5.94531L7.18457 10.1377C7.43838 10.2348 7.71265 10.291 8 10.291C9.26359 10.291 10.2909 9.26329 10.291 8C10.291 7.763 10.2451 7.54049 10.1807 7.33105L10.1367 7.18945L10.0186 7.27832L8.75879 8.22363H8.22656L7.80469 7.80566C7.80396 7.79142 7.80278 7.77391 7.80176 7.75391C7.79891 7.69808 7.79481 7.6227 7.79102 7.5459C7.78495 7.42322 7.77957 7.29979 7.77734 7.24023L8.72168 5.98145L8.81055 5.86328L8.66895 5.81934C8.45944 5.7548 8.23708 5.709 8 5.70898C6.73632 5.70898 5.70898 6.73661 5.70898 8C5.709 8.2865 5.76377 8.56025 5.86035 8.81348L4.2666 10.0547H4.26562C3.92743 9.4439 3.73049 8.74556 3.73047 8C3.73047 5.64581 5.64581 3.73047 8 3.73047Z" fill="url(#cmp1)" stroke="white" stroke-width="0.208281"/></g>"##,
};

/// One provider on the rail: its id, the name its button says, where the heading starts in the
/// list, and the first model under it.
pub struct Stop {
    pub group: String,
    pub label: String,
    pub y: f32,
    pub first: String,
}

/// Desktop's mark for the group a row `key` ("provider:model") sits in. A provider with no mark
/// of its own by id tries its id without a `console-` prefix, hyphens or suffix, then takes
/// Desktop's stand-in, its custom provider mark ([`CUSTOM`]).
pub fn mark(group: &str, key: &str) -> &'static Icon {
    if group == FAVORITES {
        return &rune_kit::glyphs::STAR_FILLED;
    }
    if group == RECENT {
        return &CLOCK;
    }
    let id = key.split(':').next().unwrap_or("");
    let bare = id.strip_prefix("console-").unwrap_or(id);
    [id, bare, &bare.replace('-', ""), bare.split('-').next().unwrap_or(bare)]
        .into_iter()
        // Desktop never draws its `synthetic` mark for a provider (`provider-group.tsx:33`).
        .filter(|id| *id != "synthetic")
        .find_map(provider_icons::named)
        .unwrap_or(&CUSTOM)
}

/// How big a group's mark draws: 16 pt, but Favorites' star at the size Desktop's own star had
/// (rune-kit's runs 12.36 of its 16, Desktop's 11.42).
fn size(group: &str) -> f32 {
    if group == FAVORITES { 16.0 * 11.42 / 12.36 } else { 16.0 }
}

/// The rail, as tall as the list beside it, `lit` being the lit row's group.
pub fn view<S: PickerHost>(p: &Pal, look: Look, stops: &[Stop], lit: Option<&str>) -> El {
    let mut rail = col().key("picker-rail").w(W).h_full().padding(look.inset).gap(look.row_gap).items_center()
        .scroll(ScrollSpec { stick_bottom: false });
    let mut pinned = false;
    for stop in stops {
        let saved = stop.group == FAVORITES || stop.group == RECENT;
        if pinned && !saved {
            rail = rail.child(el().key("rail-divider").w(16.0).h(0.5).my(2.0).fill(p.border_muted));
        }
        pinned = saved;
        rail = rail.child(button::<S>(p, look, stop, lit == Some(stop.group.as_str())));
    }
    row().key("picker-rail-edge").h_full().child(rail).child(el().w(0.5).h_full().fill(p.border_muted))
}

fn button<S: PickerHost>(p: &Pal, look: Look, stop: &Stop, lit: bool) -> El {
    let (first, y) = (stop.first.clone(), stop.y);
    row().key(format!("rail-{}", stop.group)).role("button").label(format!("Jump to {}", stop.label)).help(format!("Jump to {}", stop.label))
        .size(28.0, 28.0).center().transition(Transition::none()).radius(look.row_radius).fill(pop::lit(p, lit)).hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            pop::jump_highlight();
            s.pickers_mut().activate(first.clone());
            tx.scroll_to_y("picker-scroll", y);
        })
        .child(icon(mark(&stop.group, &stop.first), size(&stop.group)).color(p.icon_muted).transition(Transition::none()))
}
