//! What the popovers and palettes share: the list popover, the highlight that follows a query,
//! arrow-key stepping, and the server's `{ data: [..] }` lists.

use std::rc::Rc;

use opencode_kit::model::SessionInfo;
use rune::{Color, Cx, El, Tx, col, row, text};
use serde_json::Value;

use crate::App;
use crate::palette::{Elevate as _, Pal};
use crate::view::UI;

/// `composer-suggestions` inherits the page's 13/24 regular face with normal letter spacing.
/// The compact menu label (13/16 with negative tracking) pulls long descriptions to the left.
const WORDS: rune::TextStyle = rune::TextStyle::new(13.0, 440.0).lh(24.0);

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
/// `COMPOSER_SUGGESTION_SEARCH_HEIGHT`.
const SEARCH_H: f32 = 28.0;

/// The timeline's height over the composer, which the popover keeps some of in view
/// (`editor.tsx:1271-1279`); none without one.
pub fn boundary(cx: &mut Cx<App>) -> Option<f32> {
    cx.layout_size("timeline-area").map(|[_, h]| h).filter(|h| *h > 0.0)
}

/// `composerSuggestionMaxHeight` (`editor.tsx:1412-1423`): whole rows and a peek of the next,
/// under the top padding and a `search` field, inside what the timeline (`boundary` tall) can
/// spare. The history list pads both edges and shows only whole rows.
pub fn max_height(boundary: f32, search: bool, history: bool) -> f32 {
    let limit = MAX_H.min(boundary - RESERVE.min(boundary / 4.0));
    let chrome = PAD * if history { 2.0 } else { 1.0 } + if search { SEARCH_H } else { 0.0 };
    let peek = if history { 0.0 } else { PEEK };
    if limit < chrome + ROW_H + peek {
        return limit;
    }
    chrome + ((limit - chrome - peek) / ROW_H).floor() * ROW_H + peek
}

/// The search field over the commands menu and the history list (`editor.tsx:1293, 1360`):
/// `px-2 py-1` round a 13/20 line in `text-base`, its placeholder `text-faint`, no border.
pub fn suggest_search(p: &Pal, edit: &rune::TextEdit, label: &str, placeholder: &str) -> El {
    row().key("suggest-search").h(SEARCH_H).w_full().px(8.0).center_y().child(
        crate::popover::search_field(p, "suggest-search-input", edit, placeholder)
            .label(label.to_owned())
            .spellcheck(false)
            .on_edit(|s: &mut App| &mut s.suggest.search)
            .on_input_change(|s: &mut App, tx: &mut Tx| s.suggest_query(tx))
            .on_input_key(|s: &mut App, ev, tx| s.suggest_key(ev, tx)),
    )
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

pub use crate::popover::{Highlight, cycle};

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
    /// The commands menu's own search field, over its rows and scrolling with them.
    pub search: Option<El>,
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
    hover: impl Fn(&mut App, &str, &mut Tx) + 'static,
    pick: impl Fn(&mut App, &str, &mut Tx) + 'static,
) -> El {
    let (hover, pick) = (Rc::new(hover), Rc::new(pick));
    let empty = rows.is_empty();
    let count = rows.len();
    let searching = spec.search.is_some();
    let mut list = col().key(format!("{}-list", spec.key)).w_full().padding(PAD).children(spec.search);
    for r in rows {
        let (id, id_hover, id_press) = (r.id.clone(), r.id.clone(), r.id.clone());
        let (hover, press) = (hover.clone(), pick.clone());
        let mut words = row().key("words").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().children(r.lead);
        words = words.child(text(r.label.clone()).key("label").font(WORDS).color(p.text));
        if !r.description.is_empty() {
            words = words.child(text(r.description).key("description").font(WORDS).color(p.muted).truncate());
        }
        let trail = r.trail.map(|keys| text(keys).key("keys").font(WORDS).color(p.muted));
        list = list.child(
            row()
                .key(r.key)
                .role("option")
                .label(r.label)
                .focus_selected(active == Some(id.as_str()))
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .gap(8.0)
                .center_y()
                .radius(6.0)
                .fill(if active == Some(id.as_str()) { p.hover } else { Color::TRANSPARENT })
                // The pointer moving over a row highlights it (`onPointerMove` with movement), so
                // a list opening under a resting pointer keeps its first row lit. Not `on_hover`:
                // its commit would animate the highlight's fill from row to row.
                .on_pointer_move_local(move |s: &mut App, _, tx: &mut Tx| hover(s, &id_hover, tx))
                // A row picks on the click, a release over the row it went down on, as Desktop's
                // row button does (`editor.tsx:1388`, `onClick`); VoiceOver's press too.
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    s.composer.focused = true;
                    press(s, &id_press, tx);
                })
                .child(words)
                .children(trail),
        );
    }
    let scroll = format!("{}-scroll", spec.key);
    let body = if empty {
        list.child(row().key("empty").h(EMPTY_H).px(8.0).center_y().child(text(spec.empty).font(UI).color(p.muted))).key(scroll)
    } else {
        // Rows scroll under the padding with no fade and no bar (`no-scrollbar`), cut where the
        // height ends as Desktop's are.
        let search = if searching { SEARCH_H } else { 0.0 };
        let h = (search + count as f32 * ROW_H + 2.0 * PAD).min(max_height(spec.boundary.unwrap_or(2.0 * MAX_H), searching, false));
        session_view::scroller::scroller(scroll).hidden(col().w_full().h(h).scroll_margin(PAD), list)
    };
    // `rounded-xl bg-v2-background-bg-base p-1.5 shadow-[var(--v2-elevation-raised)]`: 10 pt
    // corners and the raised ring and drops, no border of its own.
    col()
        .elevate(p.raised)
        .key(spec.key)
        .role("listbox")
        .label(spec.label)
        .above("composer", 8.0)
        .w(spec.width)
        .radius(10.0)
        .fill(p.bg_base)
        .clip_soft(0.0)
        .transition(rune::Transition::none())
        // A press anywhere on it leaves the composer focused: a blur would close the popover
        // under the pointer before its row's press landed (Kit's Scrawl, Nov 8).
        .preserve_focus_on_press(true)
        .on_press(|_: &mut App| {})
        .child(body)
}
