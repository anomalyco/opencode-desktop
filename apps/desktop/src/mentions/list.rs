//! What the popovers and palettes share: the list popover, the highlight that follows a query,
//! arrow-key stepping, and the server's `{ data: [..] }` lists.

use std::rc::Rc;

use opencode_kit::model::SessionInfo;
use rune::{Color, El, KeyEvent, Tx, col, row, text};
use serde_json::Value;

use crate::App;
use crate::palette::Pal;
use crate::view::UI;

/// `composerSuggestionMaxHeight` at full height: 8 px of padding, whole 28 px rows, and an
/// 18 px peek of the next.
const ROW_H: f32 = 28.0;
const PAD: f32 = 6.0;
const MAX_H: f32 = 306.0;

/// The array under a response's `data`, or the value itself when it is one (the fixture's).
pub fn data_list(v: &Value) -> impl Iterator<Item = &Value> {
    v.get("data").unwrap_or(v).as_array().into_iter().flatten()
}

/// Merges `incoming` sessions into `into` in place, replacing matching ids.
pub fn upsert_sessions(into: &mut Vec<SessionInfo>, incoming: Vec<SessionInfo>) {
    for s in incoming {
        match into.iter_mut().find(|x| x.id == s.id) {
            Some(old) => *old = s,
            None => into.push(s),
        }
    }
}

/// The highlighted row and the query it was picked under: it holds only while the query is
/// unchanged and the row is still listed, else the first row is highlighted.
#[derive(Default)]
pub struct Highlight(Option<(String, String)>);

impl Highlight {
    pub fn resolve<'a>(&self, query: &str, keys: impl Iterator<Item = &'a str> + Clone) -> Option<String> {
        let picked = self.0.as_ref().filter(|(_, q)| q == query).map(|(k, _)| k.as_str());
        picked.filter(|k| keys.clone().any(|x| x == *k)).map(str::to_owned).or_else(|| keys.clone().next().map(str::to_owned))
    }

    pub fn set(&mut self, key: String, query: String) {
        self.0 = Some((key, query));
    }

    pub fn clear(&mut self) {
        self.0 = None;
    }
}

/// Up, Down, Ctrl-N and Ctrl-P as a step of ±1. `ctrl_alt` lets Ctrl-N/P through with Alt held.
pub fn nav_step(ev: &KeyEvent, ctrl_alt: bool) -> Option<i64> {
    let ctrl = |c: &str| {
        ev.mods.ctrl && !ev.mods.cmd && (ctrl_alt || !ev.mods.alt) && matches!(&ev.key, rune::Key::Text(t) if t.eq_ignore_ascii_case(c))
    };
    let plain = !ev.mods.alt && !ev.mods.cmd && !ev.mods.shift;
    match &ev.key {
        rune::Key::Down if plain => Some(1),
        rune::Key::Up if plain => Some(-1),
        _ if ctrl("n") => Some(1),
        _ if ctrl("p") => Some(-1),
        _ => None,
    }
}

/// The index `step` from `at` (the first when none), wrapping around `len` items.
pub fn cycle(len: usize, at: Option<usize>, step: i64) -> usize {
    (at.unwrap_or(0) as i64 + step).rem_euclid(len as i64) as usize
}

/// Moves the highlight `step` rows from `active`, scrolling to it; returns the new key.
pub fn step_key<'a>(keys: impl Iterator<Item = &'a str> + Clone, active: Option<&str>, step: i64, tx: &mut Tx, scroll: &str, row_prefix: &str) -> Option<String> {
    let len = keys.clone().count();
    if len == 0 {
        return None;
    }
    let at = keys.clone().position(|k| Some(k) == active);
    let key = keys.clone().nth(cycle(len, at, step))?.to_owned();
    tx.instant();
    tx.scroll_into_view_in(scroll, format!("{row_prefix}{key}"));
    Some(key)
}

/// One popover's keys and wording.
pub struct Spec {
    /// The popover's key; its list and scroll are `<key>-list` and `<key>-scroll`.
    pub key: &'static str,
    pub label: &'static str,
    pub empty: &'static str,
    pub width: f32,
}

pub struct Row {
    /// What the app knows the row by: handed to `hover` and `pick`.
    pub id: String,
    pub key: String,
    pub label: String,
    pub lead: Option<El>,
    pub description: String,
}

/// The popover over the composer, 8 points up: rows of an optional lead, a label and a muted
/// description. It appears and goes at once, and so does its highlight.
pub fn popup(
    zoom: f32,
    p: &Pal,
    spec: Spec,
    rows: Vec<Row>,
    active: Option<&str>,
    hover: impl Fn(&mut App, &str) + 'static,
    pick: impl Fn(&mut App, &str) + 'static,
) -> El {
    let (hover, pick) = (Rc::new(hover), Rc::new(pick));
    let empty = rows.is_empty();
    let count = rows.len();
    let mut list = col().key(format!("{}-list", spec.key)).w_full().padding(PAD);
    for r in rows {
        let (id, id_hover) = (r.id.clone(), r.id.clone());
        let (hover, pick) = (hover.clone(), pick.clone());
        let mut words = row().key("words").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().children(r.lead);
        words = words.child(text(r.label.clone()).font(UI).color(p.text));
        if !r.description.is_empty() {
            words = words.child(text(r.description).font(UI).color(p.muted).truncate_fade());
        }
        list = list.child(
            row()
                .key(r.key)
                .role("option")
                .label(r.label)
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .center_y()
                .radius(6.0)
                .fill(if active == Some(id.as_str()) { p.hover } else { Color::TRANSPARENT })
                // Not `on_hover`: its commit would animate the highlight's fill from row to row.
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                    if inside {
                        tx.instant();
                        hover(s, &id_hover);
                    }
                })
                .on_press(move |s: &mut App| {
                    pick(s, &id);
                    s.composer.focused = true;
                })
                .child(words),
        );
    }
    let scroll = format!("{}-scroll", spec.key);
    let body = if empty {
        col().key(scroll).w_full().padding(PAD).child(text(spec.empty).font(UI).color(p.muted).px(8.0).py(4.0))
    } else {
        // Edge to edge: rows scroll under the popover's border, fading only while hidden there.
        let h = (count as f32 * ROW_H + 2.0 * PAD).min(MAX_H);
        col().key(scroll).w_full().h(h).scroll(rune::ScrollSpec { stick_bottom: false }).scroll_edge_fade(16.0).scroll_margin(PAD).child(list)
    };
    let popover = col()
        .key(spec.key)
        .role("listbox")
        .label(spec.label)
        .above("composer", 8.0)
        .w(spec.width)
        .radius(12.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(4.0, 12.0, p.shadow)
        .clip_soft(0.0)
        .transition(rune::Transition::none())
        .on_press(|_: &mut App| {})
        .child(body);
    crate::view::follow_zoom(popover, zoom, true)
}
