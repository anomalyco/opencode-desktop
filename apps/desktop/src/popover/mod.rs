//! The one popover every menu and picker is built from: its surface, where it sits, its search
//! row, group headers, option rows, dividers, keys and dismissal. Extend it here (and in its
//! story) when a menu needs something new; never draw a local copy beside the feature.
//!
//! One surface for all of them: layer 1, a 0.5 pt ring, `shadow(4, 12)`, and corners of the
//! inset plus a row's corners, so the highlight sits concentric in it. Two looks set the
//! metrics, both Desktop's: [`MENU`] (`menu-v2`, a short list of actions or choices; 6 pt
//! corners) and [`LIST`] (the model dialog's `search-list`, a long list with a search field;
//! 12 pt). In each, the search icon, the query, the group headers and the row labels share one
//! left edge.
//!
//! The rules here are meant to outlive the module (audit 2026-10-30): the lit row, by pointer
//! or keys, is `p.row_active` ([`lit`]); a group's title is [`group_header`]; a shortcut at a
//! row's end is [`hint`]. App-wide rows, fields and keycaps should take them from here.

pub mod nav;

use rune::{Align, Color, El, Icon, ScrollSpec, Side, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, text, text_input};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;
use crate::view::UI;

pub use nav::{Highlight, Nav, cycle, nav, nav_step};

/// A popover's metrics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// From the surface's edge to a row's highlight.
    pub inset: f32,
    pub row_radius: f32,
    pub row_h: f32,
    /// Between rows.
    pub row_gap: f32,
    /// A row's padding before its first slot and after its last.
    pub pad_l: f32,
    pub pad_r: f32,
    /// Between a row's slots.
    pub gap: f32,
    /// Rows start with a 16 pt slot for the check (or an icon), so labels line up with the query.
    pub lead: bool,
}

/// Desktop's `menu-v2`: a 2 pt inset, 6 pt corners, 28 pt rows with 4 pt corners, 12 pt before
/// the label and the check at the end.
pub const MENU: Look = Look { inset: 2.0, row_radius: 4.0, row_h: 28.0, row_gap: 0.0, pad_l: 12.0, pad_r: 6.0, gap: 8.0, lead: false };

/// Desktop's model dialog (`select-dialog.css`): a 6 pt inset, 12 pt corners, 28 pt rows 4 pt
/// apart with 6 pt corners, and the check in a 16 pt slot before the label.
pub const LIST: Look = Look { inset: 6.0, row_radius: 6.0, row_h: 28.0, row_gap: 4.0, pad_l: 6.0, pad_r: 3.0, gap: 6.0, lead: true };

/// The lead slot: a 16 pt check or icon.
pub const SLOT: f32 = 16.0;

/// A group's title: Desktop's list header, 12 pt on 18, faint.
pub const HEADER: TextStyle = TextStyle::new(12.0, 440.0).lh(18.0);
/// A shortcut at a row's end: Desktop's `menu-v2` shortcut, 11 pt at 530, faint.
pub const HINT: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0);
const BADGE: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0);

impl Look {
    /// From a row's highlight edge to its label.
    pub fn label_x(&self) -> f32 {
        self.pad_l + if self.lead { SLOT + self.gap } else { 0.0 }
    }

    /// The surface's corners: concentric with a row's highlight.
    pub fn radius(&self) -> f32 {
        self.inset + self.row_radius
    }
}

/// The lit row's fill, whether the pointer or the arrow keys lit it.
pub fn lit(p: &Pal, on: bool) -> Color {
    if on { p.row_active } else { Color::TRANSPARENT }
}

/// The surface: fill, a 0.5 pt ring, the drop shadow and corners. It takes presses on its own
/// empty space, so they don't reach the dismiss layer under it.
pub fn surface(p: &Pal, look: Look, key: impl Into<String>, label: impl Into<String>) -> El {
    col()
        .key(key)
        .role("menu")
        .label(label)
        .radius(look.radius())
        .fill(p.layer1)
        .border(0.5, p.ring)
        .shadow(4.0, 12.0, p.shadow)
        .transition(Transition::none())
        // Its height and rows follow a filter in the keystroke.
        .motion(rune::Spring::NONE)
        .on_press(|_: &mut App| {})
}

/// Puts `surface` on `side` of the element keyed `anchor`, `gap` away. It flips when it doesn't
/// fit, stays 8 pt inside the window, and is at most as tall as the room it has (its scroll
/// shrinks to fit).
pub fn place(surface: El, anchor: &str, side: Side, align: Align, gap: f32) -> El {
    surface.anchored_fit(anchor.to_owned(), side, align, gap)
}

/// A window-sized layer under the popover (mount it just before): a press outside closes it
/// (`close`), in the same frame.
pub fn scrim(key: impl Into<String>, close: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    el().key(key)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            close(s, tx);
        })
}

/// The search field's text input, bare (the row around it is [`search_row`]); add `.on_edit`
/// and `.on_input_key` to it. It replaces the model picker's field rather than adding a style.
pub fn search_field(p: &Pal, key: &str, edit: &TextEdit, placeholder: &str) -> El {
    text_input(edit, UI)
        .key(key.to_owned())
        .single_line()
        .truncate_fade()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h(20.0)
        .animate_edit_layout()
        .placeholder(placeholder.to_owned(), p.faint)
        .caret(p.text, p.selection)
}

/// The search row and the hairline under it, across the whole surface: the glass sits in the
/// rows' lead slot, so the query starts where their labels do. Use it with [`LIST`].
pub fn search_row(p: &Pal, look: Look, key: &str, field: El) -> El {
    col().key(key.to_owned()).w_full().pt(look.inset).gap(look.inset)
        .child(row().key("search").h(look.row_h).w_full().pl(look.inset + look.pad_l).pr(look.inset).gap(look.gap).center_y()
            .child(icon(&ic::MAGNIFYING_GLASS, SLOT).color(p.icon_muted))
            .child(field))
        .child(divider(p, "search-divider"))
}

/// A full-width hairline.
pub fn divider(p: &Pal, key: &str) -> El {
    el().key(key.to_owned()).h(0.5).w_full().fill(p.border_muted)
}

/// A small label after a row's name (Desktop's `Badge`: "Free", "Latest").
pub fn badge(p: &Pal, label: &str) -> El {
    row().key(format!("badge-{label}")).h(16.0).px(4.0).center().radius(2.0).fill(p.layer2).border(0.5, p.border)
        .child(text(label.to_owned()).font(BADGE).color(p.muted))
}

static STAR: Icon = Icon {
    name: "star",
    view: 16.0,
    body: r#"<path d="M8 2.6L9.59 6.42L13.71 6.75L10.57 9.43L11.53 13.45L8 11.3L4.47 13.45L5.43 9.43L2.29 6.75L6.41 6.42Z" stroke="currentColor" stroke-linejoin="round"/>"#,
};

static STAR_FILLED: Icon = Icon {
    name: "star-filled",
    view: 16.0,
    body: r#"<path d="M8 2.6L9.59 6.42L13.71 6.75L10.57 9.43L11.53 13.45L8 11.3L4.47 13.45L5.43 9.43L2.29 6.75L6.41 6.42Z" fill="currentColor" stroke="currentColor" stroke-linejoin="round"/>"#,
};

/// A row's star at its end: filled when `on`. A press toggles it and leaves the row unpicked.
pub fn star(p: &Pal, on: bool, toggle: impl Fn(&mut App) + 'static) -> El {
    row().key("star").role("button").label(if on { "Unfavorite" } else { "Favorite" })
        .help(if on { "Unfavorite ⌘F" } else { "Favorite ⌘F" })
        .size(20.0, 20.0).center().radius(4.0).hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            toggle(s);
        })
        .child(icon(if on { &STAR_FILLED } else { &STAR }, 14.0).color(if on { p.icon } else { p.icon_muted }))
}

/// A row's shortcut at its end.
pub fn hint(p: &Pal, keys: &str) -> El {
    text(keys.to_owned()).key("hint").font(HINT).color(p.faint)
}

/// One option row. Its parts go in order: lead (the check slot, or an icon), label, what follows
/// the label (a faint provider, badges), then the trailing end (a button, a shortcut, the check
/// when the look has no lead slot).
pub struct Opt {
    pub key: String,
    pub label: String,
    pub role: &'static str,
    /// Highlighted, by the pointer or the arrow keys.
    pub active: bool,
    /// The current choice.
    pub checked: bool,
    pub lead: Option<El>,
    pub after: Vec<El>,
    pub trailing: Vec<El>,
}

impl Opt {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Opt {
        Opt { key: key.into(), label: label.into(), role: "menuitemradio", active: false, checked: false, lead: None, after: vec![], trailing: vec![] }
    }
    pub fn role(mut self, r: &'static str) -> Opt { self.role = r; self }
    pub fn active(mut self, on: bool) -> Opt { self.active = on; self }
    pub fn checked(mut self, on: bool) -> Opt { self.checked = on; self }
    pub fn lead(mut self, e: El) -> Opt { self.lead = Some(e); self }
    pub fn after(mut self, e: El) -> Opt { self.after.push(e); self }
    pub fn trailing(mut self, e: El) -> Opt { self.trailing.push(e); self }
}

/// An option row: the highlight follows the pointer at once (`hover`) and a press picks it.
pub fn option(p: &Pal, look: Look, o: Opt, hover: impl Fn(&mut App) + 'static, pick: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    let mut r = row()
        .key(o.key)
        .role(o.role)
        // Rows come and go with a filter in the keystroke, as Desktop's list does.
        .transition(Transition::none())
        .label(o.label.clone())
        .h(look.row_h)
        .w_full()
        .pl(look.pad_l)
        .pr(look.pad_r)
        .gap(look.gap)
        .center_y()
        .radius(look.row_radius)
        .fill(lit(p, o.active))
        .press(p.pressed)
        // Not `on_hover`: its commit would animate the highlight's fill from row to row.
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside {
                tx.instant();
                hover(s);
            }
        })
        .on_press_tx(pick);
    let check = || icon(&ic::CHECK_SMALL, SLOT).key("check").color(p.icon);
    if look.lead {
        let slot = row().key("lead").size(SLOT, SLOT).center();
        r = r.child(match (o.checked, o.lead) {
            (true, _) => slot.child(check()),
            (false, Some(l)) => slot.child(l),
            (false, None) => slot,
        });
    } else if let Some(l) = o.lead {
        r = r.child(l);
    }
    let words = row().key("words").grow(1.0).shrink().min_w(0.0).gap(look.gap).center_y()
        .child(text(o.label).font(UI).color(p.text).shrink().truncate_fade())
        .children(o.after);
    r = r.child(words).children(o.trailing);
    if o.checked && !look.lead {
        r = r.child(check());
    }
    r
}

/// A group header's height below the room above it.
pub const HEADER_H: f32 = 20.0;

/// A group's title, faint, on the row labels' edge, `top` below what's above it.
pub fn group_header(p: &Pal, look: Look, key: impl Into<String>, label: &str, top: f32) -> El {
    row().key(key).transition(Transition::none()).h(top + HEADER_H).w_full().pl(look.label_x()).pr(look.pad_r).pt(top).pb(2.0)
        .child(text(label.to_owned()).font(HEADER).color(p.faint).shrink().truncate_fade())
}

/// The rows, with group headers between them, in a scroll that shrinks to the room the popover
/// has. It counts its own height as rows go in.
pub struct List {
    look: Look,
    el: El,
    h: f32,
    n: usize,
}

impl List {
    pub fn new(key: &str, look: Look) -> List {
        List { look, el: col().key(key.to_owned()).w_full().padding(look.inset).gap(look.row_gap), h: 2.0 * look.inset, n: 0 }
    }

    fn push(&mut self, e: El, h: f32) {
        self.el = std::mem::replace(&mut self.el, el()).child(e);
        self.h += h + if self.n > 0 { self.look.row_gap } else { 0.0 };
        self.n += 1;
    }

    /// A group's title, 16 pt below what's above it.
    pub fn header(&mut self, p: &Pal, key: impl Into<String>, label: &str) {
        let top = 16.0 - if self.n == 0 { self.look.inset } else { self.look.row_gap };
        let e = group_header(p, self.look, key, label, top);
        self.push(e, top + HEADER_H);
    }

    pub fn row(&mut self, e: El) {
        let h = self.look.row_h;
        self.push(e, h);
    }

    /// Anything else (an empty state), `h` tall.
    pub fn other(&mut self, e: El, h: f32) {
        self.push(e, h);
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The scroll keyed `key`, at most `max_h` tall.
    pub fn build(self, key: &str, max_h: f32) -> El {
        col().key(key.to_owned()).w_full().h(self.h.min(max_h)).shrink().min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(8.0)
            .child(self.el)
    }
}

/// The empty state's height in a list ("No models found").
pub const EMPTY_H: f32 = 36.0;

/// "No models found": faint, on the labels' edge.
pub fn empty(p: &Pal, look: Look, key: &str, label: &str) -> El {
    row().key(key.to_owned()).transition(Transition::none()).h(EMPTY_H).w_full().pl(look.label_x()).center_y().child(text(label.to_owned()).font(UI).color(p.faint))
}

#[cfg(test)]
mod tests;
