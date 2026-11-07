//! What the popovers and palettes share: the list popover, the highlight that follows a query,
//! arrow-key stepping, and the server's `{ data: [..] }` lists.

use std::rc::Rc;

use opencode_kit::model::SessionInfo;
use rune::{Color, Cx, El, Tx, col, row, text};
use serde_json::Value;

use crate::App;
use crate::palette::Pal;
use crate::view::UI;

/// Desktop's composer suggestions (`composer/editor/editor.tsx:72-82`): 28 pt rows in 6 pt of
/// padding, at most 320 pt tall less some of the timeline kept in view.
const ROW_H: f32 = 28.0;
const PAD: f32 = 6.0;
const MAX_H: f32 = 320.0;
/// How much of the next row shows under the last whole one.
const PEEK: f32 = 18.0;
/// How much of the timeline above stays in view, at most a quarter of it.
const RESERVE: f32 = 80.0;
/// The empty state's line (`editor.tsx:1376`: `px-2 py-1` on a 24 pt line).
const EMPTY_H: f32 = 32.0;

/// The timeline's height over the composer, which the popover keeps some of in view
/// (`editor.tsx:1271-1279`); none without one.
pub fn boundary(cx: &mut Cx<App>) -> Option<f32> {
    cx.layout_size("timeline-area").map(|[_, h]| h).filter(|h| *h > 0.0)
}

/// `composerSuggestionMaxHeight` (`editor.tsx:1412-1423`): whole rows and a peek of the next,
/// under the top padding, inside what the timeline (`boundary` tall) can spare.
fn max_height(boundary: f32) -> f32 {
    let limit = MAX_H.min(boundary - RESERVE.min(boundary / 4.0));
    if limit < PAD + ROW_H + PEEK {
        return limit;
    }
    PAD + ((limit - PAD - PEEK) / ROW_H).floor() * ROW_H + PEEK
}

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
    /// The timeline's height above the composer, which the popover leaves some of in view;
    /// none where there is no timeline (a new session).
    pub boundary: Option<f32>,
}

pub struct Row {
    /// What the app knows the row by: handed to `hover` and `pick`.
    pub id: String,
    pub key: String,
    pub label: String,
    pub lead: Option<El>,
    pub description: String,
    /// Drawn at the row's end, muted: a command's keys as Desktop writes them ("Ctrl+P").
    pub trail: Option<String>,
}

/// The popover over the composer, 8 points up and as wide (`editor.tsx:1281-1289`): rows of an
/// optional lead, a label and a muted description (`:1378-1404`). It appears and goes at once,
/// and so does its highlight.
pub fn popup(
    p: &Pal,
    spec: Spec,
    rows: Vec<Row>,
    active: Option<&str>,
    hover: impl Fn(&mut App, &str) + 'static,
    pick: impl Fn(&mut App, &str, &mut Tx) + 'static,
) -> El {
    let (hover, pick) = (Rc::new(hover), Rc::new(pick));
    let empty = rows.is_empty();
    let count = rows.len();
    let mut list = col().key(format!("{}-list", spec.key)).w_full().padding(PAD);
    for r in rows {
        let (id, id_hover) = (r.id.clone(), r.id.clone());
        let (hover, pick) = (hover.clone(), pick.clone());
        let mut words = row().key("words").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().children(r.lead);
        words = words.child(text(r.label.clone()).key("label").font(UI).color(p.text));
        if !r.description.is_empty() {
            words = words.child(text(r.description).key("description").font(UI).color(p.muted).truncate());
        }
        let trail = r.trail.map(|keys| text(keys).key("keys").font(UI).color(p.muted));
        list = list.child(
            row()
                .key(r.key)
                .role("option")
                .label(r.label)
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .gap(8.0)
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
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    s.composer.focused = true;
                    pick(s, &id, tx);
                })
                .child(words)
                .children(trail),
        );
    }
    let scroll = format!("{}-scroll", spec.key);
    let body = if empty {
        col().key(scroll).w_full().padding(PAD)
            .child(row().key("empty").h(EMPTY_H).px(8.0).center_y().child(text(spec.empty).font(UI).color(p.muted)))
    } else {
        // Rows scroll under the padding with no fade and no bar (`no-scrollbar`), cut where the
        // height ends as Desktop's are.
        let h = (count as f32 * ROW_H + 2.0 * PAD).min(max_height(spec.boundary.unwrap_or(2.0 * MAX_H)));
        col().key(scroll).w_full().h(h).scroll(rune::ScrollSpec { stick_bottom: false }).scroll_margin(PAD).child(list)
    };
    // `rounded-xl bg-v2-background-bg-base p-1.5 shadow-[var(--v2-elevation-raised)]`: 10 pt
    // corners and the raised ring and drops, no border of its own.
    p.raised.into_iter().fold(col(), El::box_shadow)
        .key(spec.key)
        .role("listbox")
        .label(spec.label)
        .above("composer", 8.0)
        .w(spec.width)
        .radius(10.0)
        .fill(p.bg_base)
        .clip_soft(0.0)
        .transition(rune::Transition::none())
        .on_press(|_: &mut App| {})
        .child(body)
}
