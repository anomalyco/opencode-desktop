//! Plugin popovers (`contrib::POPOVER`): one open at a time, placed beside the element its
//! plugin named and closed by a press outside it, Esc, the plugin, or a reload. The plugin draws
//! the content; the host places it with the app's popover motion, as every menu opens.

use opencode_plugin::abi::{ALIGN_CENTER, ALIGN_END, SIDE_LEFT, SIDE_RIGHT, SIDE_TOP, SURFACE_POPOVER, contrib as kinds};
use rune::{Align, El, Key, KeyEvent, Side, Tx};

use super::contrib::active;
use super::render::{self, Props};
use crate::App;
use crate::palette::Pal;

/// The open popover: its plugin's slot, its id and the element it sits beside.
pub struct Open {
    pub slot: usize,
    pub id: String,
    pub anchor: String,
}

/// Opens slot `i`'s popover `id` beside `anchor`, or closes it when it's the one open.
pub fn toggle(app: &mut App, i: usize, id: &str, anchor: &str, tx: &mut Tx) {
    if app.plugins.popover.as_ref().is_some_and(|o| o.slot == i && o.id == id) {
        return close(app, tx);
    }
    if !active(&app.plugins, kinds::POPOVER).any(|(s, c)| s == i && c.id == id) {
        return;
    }
    tx.instant();
    app.plugins.popover = Some(Open { slot: i, id: id.to_owned(), anchor: anchor.to_owned() });
    crate::popover::motion::opening(&app.knobs, tx);
}

pub fn close(app: &mut App, tx: &mut Tx) {
    if app.plugins.popover.take().is_some() {
        tx.instant();
        crate::popover::motion::closing(&app.knobs, tx);
    }
}

/// Esc closes the open popover.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if app.plugins.popover.is_none() || ev.key != Key::Escape {
        return false;
    }
    close(app, tx);
    true
}

/// The open popover and the scrim under it that closes it, for the window's top layer.
pub fn view(app: &App, p: &Pal) -> Vec<El> {
    let Some(open) = &app.plugins.popover else { return vec![] };
    let Some((i, c)) = active(&app.plugins, kinds::POPOVER).find(|(s, c)| *s == open.slot && c.id == open.id) else { return vec![] };
    let Some(render) = c.render.as_ref() else { return vec![] };
    let session = app.active_session_id().unwrap_or_default();
    let directory = app.session(session).map_or("", |s| s.directory.as_str());
    let props = Props { session, directory, ..Props::new(SURFACE_POPOVER, &c.id) };
    let Ok(Some(content)) = render::surface(app, i, render, &props, p) else { return vec![] };
    let side = match c.flags & 0xff {
        SIDE_TOP => Side::Top,
        SIDE_LEFT => Side::Left,
        SIDE_RIGHT => Side::Right,
        _ => Side::Bottom,
    };
    let align = match (c.flags >> 8) & 0xff {
        ALIGN_CENTER => Align::Center,
        ALIGN_END => Align::End,
        _ => Align::Start,
    };
    let scrim = crate::popover::scrim::<App>("plugin-popover-scrim", close);
    let content = rune::col().key(format!("plugin-popover-{}", c.key)).child(content);
    vec![scrim, crate::popover::place(&app.knobs, content, &open.anchor, side, align, c.order as f32)]
}
