//! The one popover every menu and picker is built from: its surface, where it sits, its search
//! row, group headers, option rows, dividers, keys and dismissal. Extend it here (and in its
//! story) when a menu needs something new; never draw a local copy beside the feature.
//!
//! One surface for all of them: layer 1, `--v2-elevation-floating` (two drops and a 0.5 pt ring),
//! placed at whole points as Kobalte places it, and corners of the
//! inset plus a row's corners, so the highlight sits concentric in it. Two looks set the
//! metrics, both Desktop's: [`MENU`] (`menu-v2`, a short list of actions or choices; 6 pt
//! corners) and [`LIST`] (the model dialog's `search-list`, a long list with a search field;
//! 12 pt). In each, the query, the group headers and the row labels share one left edge.
//!
//! The rules here are meant to outlive the module (audit 2026-10-30): the lit row, by pointer
//! or keys, is `p.row_active` ([`lit`]); a group's title is [`group_header`]; a shortcut at a
//! row's end is [`hint`]. App-wide rows, fields and keycaps should take them from here.

pub mod motion;
pub mod nav;
mod slide;

use std::ops::Range;

use rune::{Align, Color, El, Side, Span, TextEdit, TextStyle, Transition, Tx, col, el, icon, rich, row, text};
use rune_kit::ScrollBar as _;
use rune_knobs::Knobs;
use rune_theme_opencode::icons as ic;

use crate::palette::{Elevate as _, Pal};
use crate::style::{UI, UI_MED};

pub use nav::{Highlight, HighlightEvent, Lit, Nav, cycle, nav, nav_step};
pub use slide::jump as jump_highlight;

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
    /// Above a group's title, below the group before it.
    pub head: f32,
}

/// Desktop's `menu-v2`: a 2 pt inset, 6 pt corners, 28 pt rows with 4 pt corners, 12 pt before
/// the label and the check at the end.
pub const MENU: Look = Look { inset: 2.0, row_radius: 4.0, row_h: 28.0, row_gap: 0.0, pad_l: 12.0, pad_r: 6.0, gap: 8.0, head: 16.0 };

/// Desktop's model dialog (`select-dialog.css`, #64216): a 6 pt inset, 32 pt rows 2 pt apart
/// with 6 pt corners, 8 pt before a row's check slot and 3 after its end, and a group's title
/// 16 pt below what's above it.
pub const LIST: Look = Look { inset: 6.0, row_radius: 6.0, row_h: 32.0, row_gap: 2.0, pad_l: 8.0, pad_r: 3.0, gap: 6.0, head: 16.0 };

/// The search row's height: the field keeps Desktop's 28 pt.
pub const SEARCH_H: f32 = 28.0;

/// A row's 16 pt check or icon.
pub const SLOT: f32 = 16.0;

/// A group's title: Desktop's list header, 12 pt on 18, faint.
pub const HEADER: TextStyle = TextStyle::new(12.0, 440.0).lh(18.0);
/// A shortcut at a row's end: Desktop's `menu-v2` shortcut, 11 pt at 530, faint.
pub const HINT: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0);
const BADGE: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0);

impl Look {
    /// From a row's highlight edge to its label.
    pub fn label_x(&self) -> f32 {
        self.pad_l
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

/// The surface: fill, Desktop's floating elevation (`menu-v2`'s `box-shadow`) and corners. It takes presses on its own
/// empty space, so they don't reach the dismiss layer under it.
pub fn surface<S: 'static>(p: &Pal, look: Look, key: impl Into<String>, label: impl Into<String>) -> El {
    col()
        .key(key)
        .role("menu")
        .label(label)
        .radius(look.radius())
        .fill(p.layer1)
        .elevate(p.floating)
        .transition(Transition::none())
        // Its height and rows follow a filter in the keystroke, and what it holds comes and goes
        // with it, in commits that play its entrance or exit.
        .motion(rune::Spring::NONE)
        .instant_changes()
        .on_press(|_: &mut S| {})
}

/// Puts `surface` on `side` of the element keyed `anchor`, `gap` away. It flips when it doesn't
/// fit, stays 8 pt inside the window, and is at most as tall as the room it has (its scroll
/// shrinks to fit). It opens and closes as the Popover knobs say ([`motion`]).
pub fn place(k: &Knobs, surface: El, anchor: &str, side: Side, align: Align, gap: f32) -> El {
    surface.transition(motion::transition(k, side, align)).anchored_fit(anchor.to_owned(), side, align, gap).anchored_round()
}

/// A window-sized layer under the popover (mount it just before): a press outside closes it
/// (`close`), in the same frame unless the popover has an exit to play.
pub fn scrim<S: 'static>(k: &Knobs, key: impl Into<String>, close: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let closes = motion::closes(k);
    el().key(key)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .focusable(false)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            if closes {
                tx.color_motion(rune::Spring::NONE);
            } else {
                tx.instant();
            }
            close(s, tx);
        })
}

/// [`scrim`] reading the popover's knobs from host state `S` on press.
pub fn scrim_with<S: 'static>(
    key: impl Into<String>,
    knobs: impl Fn(&S) -> &Knobs + 'static,
    close: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
    el().key(key)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .focusable(false)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            motion::closing(knobs(s), tx);
            close(s, tx);
        })
}

/// The search field's text input, bare (the row around it is [`search_row`]); add `.on_edit`
/// and `.on_input_key` to it. It replaces the model picker's field rather than adding a style.
pub fn search_field(p: &Pal, key: &str, edit: &TextEdit, placeholder: &str) -> El {
    crate::style::field(edit, UI)
        .key(key.to_owned())
        .single_line()
        .truncate_fade()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .animate_edit_layout()
        .color(p.text)
        .placeholder(placeholder.to_owned(), p.faint)
        .caret(p.text, p.selection).selection_style(p.field_selection)
}

/// The model dialog's search row and the hairline under it (`select-dialog.css:49-96`): 6 pt
/// in, a 28 pt field whose glass sits 8 pt into a 24 pt slot and whose query starts 6 pt after
/// it, where the model names do; `trailing` ([`search_button`]s) 8 pt after the query and 4 pt
/// apart, the last 2 pt from the field's end (`text-input.css` `text-input-v2-trailing`). The
/// hairline runs across the whole surface, 6 pt under the field.
pub fn search_row(p: &Pal, key: &str, field: El, trailing: Vec<El>) -> El {
    let value = row().key("search-value").grow(1.0).shrink().min_w(0.0).h_full().gap(6.0).center_y()
        .child(row().key("search-glass").w(24.0).pl(8.0).h_full().center_y().child(icon(&ic::MAGNIFYING_GLASS, SLOT).color(p.icon_muted)))
        .child(field);
    let field = row().key("search").h(SEARCH_H).w_full().pr(2.0).gap(8.0).center_y()
        .child(value)
        .children((!trailing.is_empty()).then(|| row().key("search-trailing").gap(4.0).center_y().children(trailing)));
    col().key(key.to_owned()).w_full().pt(LIST.inset).gap(LIST.inset)
        .child(row().w_full().px(LIST.inset).child(field))
        // A 1 pt box drawn at half its height (`transform: scaleY(0.5)`), `border-base`
        // (`select-dialog.css:54-58`).
        .child(rune::hairline(p.border).key("search-rule").w_full())
}

/// A 24 pt `ghost-muted` icon button at the search row's end (`select-dialog.tsx:1102-1116`,
/// `icon.tsx:59`): a muted 16 pt icon on the hover and pressed overlays, its name in a tooltip.
pub fn search_button<S: 'static>(p: &Pal, key: &str, glyph: &'static rune::Icon, label: &str, press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    row().key(key.to_owned()).role("button").label(label.to_owned()).help(label.to_owned())
        .size(24.0, 24.0).center().radius(6.0).cursor(rune::Cursor::Pointer).hover(p.hover).press(p.pressed)
        .on_press_tx(press)
        .child(icon(glyph, SLOT).color(p.icon_muted))
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

/// A row's shortcut at its end.
pub fn hint(p: &Pal, keys: &str) -> El {
    text(keys.to_owned()).key("hint").font(HINT).color(p.faint)
}

/// One option row. Its parts go in order: lead (an icon), label, what follows the label (a faint
/// provider, badges), the check when it's the current choice, then the trailing end (a button, a
/// shortcut).
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
    /// In a [`List`]: its sliding highlight lights the row, not the row's own fill.
    pub slides: bool,
    /// A destructive action (Delete): its label in Desktop's critical ink.
    pub danger: bool,
    /// Shown but not offered: a faint label that neither lights nor picks.
    pub disabled: bool,
    /// The stretch of the label a search matched, in the accent (`search-list.css:170-173`).
    pub mark: Option<Range<usize>>,
    /// Medium weight (530, `UI_MED`), as the session location card's project title uses (`session-identity-header.tsx:278`).
    pub medium: bool,
    /// Optional byte offset in `label` where the non-shrinking tail starts (`session-identity-header.tsx:404-407`),
    /// laying out `label[..at]` with `min-w-0 truncate` beside `label[at..]` in LTR order.
    pub middle_split: Option<usize>,
}

impl Opt {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Opt {
        Opt { key: key.into(), label: label.into(), role: "menuitemradio", active: false, checked: false, lead: None, after: vec![], trailing: vec![], slides: false, danger: false, disabled: false, mark: None, medium: false, middle_split: None }
    }
    pub fn mark(mut self, r: Option<Range<usize>>) -> Opt { self.mark = r; self }
    pub fn medium(mut self, on: bool) -> Opt { self.medium = on; self }
    pub fn middle_split(mut self, at: Option<usize>) -> Opt { self.middle_split = at; self }
    pub fn danger(mut self, on: bool) -> Opt { self.danger = on; self }
    pub fn disabled(mut self, on: bool) -> Opt { self.disabled = on; self }
    pub fn role(mut self, r: &'static str) -> Opt { self.role = r; self }
    pub fn active(mut self, on: bool) -> Opt { self.active = on; self }
    pub fn slides(mut self) -> Opt { self.slides = true; self }
    pub fn checked(mut self, on: bool) -> Opt { self.checked = on; self }
    pub fn lead(mut self, e: El) -> Opt { self.lead = Some(e); self }
    pub fn after(mut self, e: El) -> Opt { self.after.push(e); self }
    pub fn trailing(mut self, e: El) -> Opt { self.trailing.push(e); self }
}

/// An option row: the highlight follows the pointer at once (`hover`) and a press picks it. A
/// disabled one does neither.
pub fn option<S: 'static>(p: &Pal, look: Look, o: Opt, hover: impl Fn(&mut S) + 'static, pick: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    option_tx(p, look, o, move |s: &mut S, _: &mut Tx| hover(s), pick)
}

/// [`option`] whose `hover` gets the change's [`Tx`]: a menu that moves focus to the row the
/// pointer lights, as Kobalte's menu items take focus on hover.
pub fn option_tx<S: 'static>(p: &Pal, look: Look, o: Opt, hover: impl Fn(&mut S, &mut Tx) + 'static, pick: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let slides = o.slides;
    let r = row()
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
        .fill(lit(p, o.active && !o.slides && !o.disabled));
    let r = if o.disabled {
        // `[data-component="menu-v2-item"][data-disabled]` (`menu.css:157-160`): fades the whole row to 0.5.
        r.disabled(true).opacity(0.5)
    } else {
        r.cursor(rune::Cursor::Pointer)
            .press(p.pressed)
            // Not `on_hover`: its commit would animate the highlight's fill from row to row.
            .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
                if inside {
                    tx.instant();
                    if slides { slide::jump(); }
                    hover(s, tx);
                }
            })
            .on_press_tx(pick)
    };
    let ink = if o.danger { p.danger } else { p.text };
    let font = if o.medium { UI_MED } else { UI };
    let words = if let Some(at) = o.middle_split.filter(|&at| at > 0 && at < o.label.len() && o.label.is_char_boundary(at)) {
        // `session-identity-header.tsx:404-407` (`4d4a95a3239c`): `<bdi dir="ltr" class="flex min-w-0">` holding
        // `<span class="min-w-0 truncate">{parent}</span><span class="max-w-full shrink-0 truncate">{last}</span>`.
        // `El::truncate()` sets `flex_shrink = 1.0`, so leave `tail` un-shrunk (`flex_shrink = 0.0`, single line)
        // inside a clipped `split` row so `head` absorbs truncation first while `tail` stays whole.
        let (head, tail) = o.label.split_at(at);
        let split = row()
            .key("label")
            .dir(rune::Direction::Ltr)
            .shrink()
            .min_w(0.0)
            .clip()
            .center_y()
            .child(text(head.to_owned()).key("head").font(font).color(ink).shrink().min_w(0.0).truncate())
            .child(text(tail.to_owned()).key("tail").font(font).color(ink));
        row().key("words").grow(1.0).shrink().min_w(0.0).gap(look.gap).center_y().child(split).children(o.after)
    } else {
        let label = match o.mark.filter(|r| r.end <= o.label.len() && o.label.is_char_boundary(r.start) && o.label.is_char_boundary(r.end)) {
            Some(r) => rich(o.label, vec![Span::new(r.start, r.end).color(p.accent)]),
            None => text(o.label),
        };
        row().key("words").grow(1.0).shrink().min_w(0.0).gap(look.gap).center_y()
            .child(label.key("label").font(font).color(ink).shrink().truncate_fade())
            .children(o.after)
    };
    r.children(o.lead)
        .child(words)
        .children(o.checked.then(|| icon(&ic::CHECK_SMALL, SLOT).key("check").color(p.icon)))
        .children(o.trailing)
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
    key: String,
    rows: Vec<El>,
    h: f32,
    /// Where the lit row starts, for the sliding highlight.
    lit: Option<f32>,
    under: Option<El>,
    /// The model dialog's list ([`List::stuck_headers`]).
    stuck: Option<Stuck>,
    /// Above the first row or header: the look's inset, or none under the model dialog's
    /// search, whose first header starts right under the hairline.
    top: f32,
}

/// What [`List::stuck_headers`] paints with.
struct Stuck {
    fill: Color,
    lit: Color,
    bar: rune_kit::ScrollBarStyle,
}

impl List {
    pub fn new(key: &str, look: Look) -> List {
        List { look, key: key.to_owned(), rows: Vec::new(), h: 2.0 * look.inset, lit: None, under: None, stuck: None, top: look.inset }
    }

    fn push(&mut self, e: El, h: f32) {
        self.h += h + if self.rows.is_empty() { 0.0 } else { self.look.row_gap };
        self.rows.push(e);
    }

    /// A group's title, the look's `head` below the search row's hairline, or that plus the row
    /// gap below the group above.
    pub fn header(&mut self, p: &Pal, key: impl Into<String>, label: &str) {
        let top = if self.rows.is_empty() { (self.look.head - self.top).max(0.0) } else { self.look.head };
        let e = match &self.stuck {
            // Across the list's inset so rows scroll under an opaque band, its words where the
            // model names start (34 in, 6 from the end); the label fades out as the next one
            // comes up, gone just as it would touch it (`select-dialog.css:121-175`,
            // `select-dialog.tsx:785-818`).
            // Until it sticks it fills only behind its label (`background-clip: content-box`), so
            // sliding up over the stuck one its padding never covers that label as it fades.
            Some(stuck) => {
                let inset = self.look.inset;
                let band = rune::Shadow { inset: true, spread: 1.0e4, color: stuck.fill, ..Default::default() };
                row().key(key).transition(Transition::none()).h(top + HEADER_H).mx(-inset).pl(inset + self.look.pad_l + SLOT + self.look.gap).pr(inset).pt(top).pb(2.0)
                    .stuck_shadow(band).sticky_top(0.0).stuck_fade(top + HEADER_H - 2.0 - self.look.head, 16.0)
                    .child(row().key("label").grow(1.0).shrink().min_w(0.0).h_full().center_y().fill(stuck.fill)
                        .child(text(label.to_owned()).font(HEADER).color(p.faint).shrink().truncate()))
                    // `::after`: the home page's 16 pt eased fade under the stuck header, faded in by
                    // how far rows have scrolled beneath it, whole at 16 (`select-dialog.tsx:817`).
                    .child(rune::el().key("shade").absolute(0.0, top + HEADER_H).w_full().h(16.0)
                        .linear_gradient(std::f32::consts::PI, crate::style::eased_fade(stuck.fill)).stuck_reveal(16.0))
            }
            None => group_header(p, self.look, key, label, top),
        };
        self.push(e, top + HEADER_H);
    }

    /// The model dialog's list (`select-dialog.css:98-187`): no room above its first header,
    /// every header sticking for the whole list on `bg-base`, the next sliding over it, the rows
    /// fading in a 16 point band under the stuck one by how far they've scrolled beneath it and
    /// over 24 points at the list's bottom, the lit row on the hover overlay in both schemes (`select-dialog.css:221-224`), and
    /// `MenuScroll`'s thumb.
    pub fn stuck_headers(mut self, p: &Pal) -> Self {
        self.h -= self.top;
        self.top = 0.0;
        self.stuck = Some(Stuck { fill: p.bg_base, lit: p.hover, bar: rune_kit::ScrollBarStyle::menu_scroll(p.border_muted, p.q.border_strong) });
        self
    }

    pub fn row(&mut self, e: El) {
        let h = self.look.row_h;
        self.push(e, h);
    }

    /// An option row made with [`Opt::slides`]; when `lit`, the highlight goes under it.
    pub fn option(&mut self, e: El, lit: bool) {
        if lit {
            let gap = if self.rows.is_empty() { 0.0 } else { self.look.row_gap };
            self.lit = Some(self.h - self.look.inset + gap);
        }
        self.row(e);
    }

    /// Puts the highlight under the lit row, sliding there from the last one lit under the
    /// same `query`.
    pub fn slide(&mut self, k: &Knobs, p: &Pal, query: &str) {
        let fill = self.stuck.as_ref().map_or(p.row_active, |s| s.lit);
        self.under = self.lit.map(|y| slide::highlight(k, fill, self.look, y, query));
    }

    /// Anything else (an empty state), `h` tall.
    pub fn other(&mut self, e: El, h: f32) {
        self.push(e, h);
    }

    /// Its height with every row in, before the scroll caps it.
    pub fn height(&self) -> f32 {
        self.h
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The scroll keyed `key`, at most `max_h` tall.
    pub fn build(self, key: &str, max_h: f32) -> El {
        let list = col().key(self.key).w_full().padding(self.look.inset).pt(self.top).gap(self.look.row_gap).children(self.under).children(self.rows);
        // The list's 24 point linear mask at its bottom (`search-list.css`'s `--search-list-edge`);
        // under a stuck header its gradient fades the rows instead.
        let fade = if self.stuck.is_some() {
            rune::ScrollFade::strength(24.0).linear().end_only()
        } else {
            rune::ScrollFade::strength(8.0)
        };
        let viewport = col().w_full().h(self.h.min(max_h)).shrink().min_h(0.0).scroll_fade(fade);
        match self.stuck {
            // The arrow keys keep the lit row clear of the stuck header (`scroll-padding-top: 36px`).
            // The model list's thumb (its own style), else no bar as the menus' lists here have none.
            Some(stuck) => crate::scroller::scroller(key).hidden(viewport.scroll_margin(36.0), list).scroll_bar_styled(stuck.bar),
            None => crate::scroller::scroller(key).hidden(viewport, list),
        }
    }
}
