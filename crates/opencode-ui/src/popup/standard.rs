//! `menu-v2`'s standard look (`appearance="standard"`, `menu.css`), as Desktop's app menu takes
//! it (`windows-menu.tsx`, `.desktop-app-menu`): a 4 pt padding inside a one-pixel ring, the v1
//! raised surface and `shadow-md`, 6 pt corners, rows 4 pt by 8 pt around a 13 pt label on 19.5 in
//! `text-strong`, lit in `surface-raised-base-hover`, a shortcut or a submenu's chevron at the
//! end, and one-pixel rules 4 pt above and below.

use rune::{El, Icon, Shadow, TextStyle, Tx, col, el, icon, row, text};
use crate::palette::Pal;

use super::keys::key;
use super::menu::{Entry, Menu, row_key};
use super::state::{choose, choose_sub, focus_lit, hover, hover_sub, moved, open_sub};
use super::view::takes_focus;
use super::{PopupHost, SubOpen};

const PAD: f32 = 4.0;
const RADIUS: f32 = 6.0;
const ROW_RADIUS: f32 = 4.0;
const ROW_X: f32 = 8.0;
const ROW_H: f32 = 4.0 + 19.5 + 4.0;
const GAP: f32 = 8.0;
/// A submenu's narrowest (`min-width: 8rem`).
const SUB_MIN_W: f32 = 128.0;
const LABEL: TextStyle = TextStyle::new(13.0, 530.0).lh(19.5);
const SHORTCUT: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0).tracking(0.05);

/// `menu-v2-item-chevron`.
static CHEVRON: Icon = Icon {
    name: "menu-v2-chevron",
    view: 16.0,
    body: r#"<path d="M6 4L10 8L6 12V4Z" fill="currentColor"/>"#,
};

/// The menu, row `active` lit, and the row whose submenu `sub` is open.
pub(super) fn root<S: PopupHost + 'static>(p: &Pal, menu: &Menu, active: Option<usize>, sub: Option<SubOpen>, hair: f32) -> El {
    let rows = menu.entries.iter().enumerate().map(|(n, entry)| {
        let lit = active == Some(n) || sub.is_some_and(|o| o.at == n);
        match entry {
            Entry::Item(i) => item(p, &row_key(&i.id), &i.title, i.shortcut.as_deref(), i.enabled, lit)
                .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| enter(s, inside, 0, tx, |s, tx| hover(s, n, tx)))
                .on_press_tx(move |s: &mut S, tx: &mut Tx| choose(s, n, tx)),
            Entry::Sub(sub) if sub.enabled => item(p, &row_key(&sub.id), &sub.title, None, true, lit)
                .child(icon(&CHEVRON, 16.0).key("chevron").color(p.standard.icon).mirror_in_rtl())
                .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| enter(s, inside, 0, tx, |s, tx| hover(s, n, tx)))
                .on_press_tx(move |s: &mut S, tx: &mut Tx| open_sub(s, n, false, tx)),
            Entry::Sub(sub) => item(p, &row_key(&sub.id), &sub.title, None, false, false)
                .child(icon(&CHEVRON, 16.0).key("chevron").color(p.standard.icon).mirror_in_rtl()),
            Entry::Separator => rule(p, n, hair),
            Entry::Group(title) => heading(p, n, title),
        }
    });
    // It holds focus and takes the keys from there, as Kobalte's menus do (`menu-v2-content`).
    surface::<S>(p, "popup-menu", menu, hair)
        .w(menu.min_w)
        .on_pointer_move_local(|s: &mut S, e: rune::PointerEvent, tx: &mut Tx| moved(s, [e.window_x, e.window_y], tx))
        .focusable(true)
        .tab_index(-1)
        .no_focus_ring()
        .on_key(|s: &mut S, ev, tx: &mut Tx| key(s, ev, tx))
        .children(rows.collect::<Vec<_>>())
}

/// A submenu, row `active` lit.
pub(super) fn sub<S: PopupHost + 'static>(p: &Pal, menu: &Menu, active: Option<usize>, hair: f32) -> El {
    let rows = menu.entries.iter().enumerate().map(|(m, entry)| match entry {
        Entry::Item(i) => item(p, &row_key(&i.id), &i.title, i.shortcut.as_deref(), i.enabled, active == Some(m))
            .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| enter(s, inside, 1, tx, |s, tx| hover_sub(s, m, tx)))
            .on_press_tx(move |s: &mut S, tx: &mut Tx| choose_sub(s, m, tx)),
        Entry::Separator => rule(p, m, hair),
        Entry::Group(title) => heading(p, m, title),
        // One level deep, as Desktop's app menu is.
        Entry::Sub(_) => el(),
    });
    let w = (2.0 * (PAD + hair) + menu.entries.iter().map(row_w).fold(0.0, f32::max)).max(SUB_MIN_W);
    surface::<S>(p, "popup-submenu", menu, hair).w(w).on_pointer_move_local(|s: &mut S, e: rune::PointerEvent, tx: &mut Tx| moved(s, [e.window_x, e.window_y], tx)).children(rows.collect::<Vec<_>>())
}

/// A row's width with nothing to squeeze it: its label, its shortcut 8 pt after, and its padding.
/// A submenu takes its widest (`width: max-content`).
fn row_w(entry: &Entry) -> f32 {
    let w = |s: &str, style| rune::text::measure(s, &[], style, None).0;
    2.0 * ROW_X
        + match entry {
            Entry::Item(i) => w(&i.title, LABEL) + i.shortcut.as_deref().map_or(0.0, |k| GAP + w(k, SHORTCUT)),
            Entry::Group(title) => w(title, LABEL),
            Entry::Sub(sub) => w(&sub.title, LABEL) + GAP + 16.0,
            Entry::Separator => 0.0,
        }
}

/// The pointer entering a row of the menu `level` deep lights it at once (`on_hover`'s commit
/// would spring the fill), and focus moves onto it without a ring.
fn enter<S: PopupHost + 'static>(s: &mut S, inside: bool, level: usize, tx: &mut Tx, f: impl FnOnce(&mut S, &mut Tx)) {
    if inside {
        tx.instant();
        f(s, tx);
        focus_lit(s, level, false, tx);
    }
}

fn surface<S: 'static>(p: &Pal, key: &str, menu: &Menu, hair: f32) -> El {
    let shadow = |y, blur, spread, light, dark| Shadow {
        y,
        blur,
        spread,
        color: rune::Color::BLACK.alpha(if p.dark { dark } else { light }),
        ..Shadow::default()
    };
    col()
        .key(key.to_owned())
        .role("menu")
        .label(menu.label.clone())
        .padding(PAD + hair)
        .radius(RADIUS)
        .border(hair, p.border_muted)
        // Desktop clips its background to the padding box. The translucent stroke sees
        // the surface under the menu, not another layer of the menu's own opaque fill.
        .child(el().pin(Some(hair), Some(hair), Some(hair), Some(hair)).radius(RADIUS - hair).fill(p.standard.surface))
        // `shadow-md`.
        .box_shadow(shadow(6.0, 12.0, -2.0, 0.075, 0.1))
        .box_shadow(shadow(4.0, 8.0, -2.0, 0.075, 0.15))
        .box_shadow(shadow(1.0, 2.0, 0.0, 0.1, 0.15))
        .transition(rune::Transition::none())
        .motion(rune::Spring::NONE)
        .instant_changes()
        .on_press(|_: &mut S| {})
}

/// A row: its label, and a shortcut at its end. A disabled one is faint and neither lights nor
/// picks.
fn item(p: &Pal, key: &str, title: &str, shortcut: Option<&str>, enabled: bool, lit: bool) -> El {
    let r = row()
        .key(key.to_owned())
        .role("menuitem")
        .label(title.to_owned())
        .h(ROW_H)
        .px(ROW_X)
        .gap(GAP)
        .center_y()
        .radius(ROW_RADIUS)
        .fill(if lit && enabled { p.standard.hover } else { rune::Color::TRANSPARENT })
        .child(text(title.to_owned()).key("label").font(LABEL).color(if enabled { p.standard.text } else { p.standard.weak }).grow(1.0))
        .children(shortcut.map(|keys| text(keys.to_owned()).key("shortcut").dir(rune::Direction::Ltr).font(SHORTCUT).color(p.faint)));
    if enabled { takes_focus(r) } else { r.disabled(true) }
}

/// A group's title.
fn heading(p: &Pal, n: usize, title: &str) -> El {
    row()
        .key(format!("popup-heading-{n}"))
        .h(ROW_H)
        .px(ROW_X)
        .center_y()
        .child(text(title.to_owned()).font(LABEL).color(p.standard.weak))
}

/// A one-pixel rule across the whole surface, 4 pt above and below.
fn rule(p: &Pal, n: usize, hair: f32) -> El {
    col().key(format!("popup-sep-{n}")).h(1.0).my(PAD).mx(-PAD).child(el().w_full().h(hair).fill(p.border_muted))
}
