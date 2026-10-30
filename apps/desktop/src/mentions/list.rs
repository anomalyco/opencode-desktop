//! What the popovers and palettes share: the list popover, the highlight that follows a query,
//! arrow-key stepping, and the server's `{ data: [..] }` lists.

use std::rc::Rc;

use opencode_kit::model::SessionInfo;
use rune::{Color, El, Tx, col, row, text};
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
            Some(old) if s.updated >= old.updated => *old = s,
            Some(_) => {}
            None => into.push(s),
        }
    }
}

pub use crate::popover::{Highlight, cycle, nav_step};

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
    popover
}
