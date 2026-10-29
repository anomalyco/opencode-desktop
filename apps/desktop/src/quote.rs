//! Quoting timeline text into the composer, as Desktop does (`session/timeline/quote.tsx`): a
//! Quote button over a settled timeline selection adds it to the composer as a card, where it
//! can carry a comment (`quote_cards`). It goes out as lines in the prompt's text and an entry
//! in `metadata.comments` (`opencode_kit::quote`).

use std::collections::HashSet;
use std::ops::Range;
use std::time::Duration;

use rune::{Cx, El, Icon, SelectionChange, TextEdit, TextStyle, Transition, Tx, icon, row, text};

use opencode_kit::model::Quote;

use crate::palette::Pal;
use crate::view::no_motion;
use crate::{App, Route};

/// A selection waits this long to settle before the button offers it.
const SETTLE: f64 = 0.15;
/// About 5k tokens: a longer selection says so and can't be quoted.
pub const MAX_LEN: usize = 20_000;
/// A selection under this many lines (and a half) is short: the button goes above it.
const SHORT_LINES: f32 = 3.0;
const GAP: f32 = 8.0;

const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const TOO_LONG: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);

pub static OUTLINE_QUOTE: Icon = Icon {
    name: "outline-quote",
    view: 16.0,
    body: r#"<path d="M9.5 8.49927C10.8333 8.49927 12.1667 8.49927 13.5 8.49927C13.5 9.74927 13.5 11.2493 13.5 12.4993C12.1667 12.4993 10.8333 12.4993 9.5 12.4993V7C9.5 5 10.5 3.5 13 3.5001" stroke="currentColor"/><path d="M2.5 8.49927C3.83333 8.49927 5.16667 8.49927 6.5 8.49927C6.5 9.74927 6.5 11.2493 6.5 12.4993C5.16667 12.4993 3.83333 12.4993 2.5 12.4993V7C2.5 5 3.5 3.5 6 3.5001" stroke="currentColor"/>"#,
};

pub static COMMENT: Icon = Icon {
    name: "comment",
    view: 16.0,
    body: r#"<path d="M13.5 2.5H2.5V13.5H4.5L5.5 11.5H13.5V2.5Z" stroke="currentColor" stroke-miterlimit="10" stroke-linecap="square"/>"#,
};

/// The quotes waiting in the composer, the comment editor, and the button's offer.
pub struct Quotes {
    /// Waiting in the composer for the next send, in the order they were added.
    pub items: Vec<Pending>,
    next: u64,
    /// The quote whose comment editor is open, and the comment being written.
    pub editing: Option<u64>,
    pub draft: TextEdit,
    offer: Option<Offer>,
    heard: Option<Heard>,
    settle: Settle,
    /// The selection Escape dismissed, which stays dismissed until it changes or clears.
    dismissed: Option<Ident>,
    /// Sent prompts whose quote cards were opened past the first five.
    pub more: HashSet<String>,
}

impl Default for Quotes {
    fn default() -> Self {
        Quotes { items: Vec::new(), next: 0, editing: None, draft: TextEdit::new(""), offer: None, heard: None, settle: Settle::Idle, dismissed: None, more: HashSet::new() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub key: u64,
    pub quote: Quote,
}

/// Which texts a selection covers and where, to tell one selection from another.
type Ident = Vec<(String, Range<usize>)>;

/// A settled selection in the timeline, read once it has rested `SETTLE`.
#[derive(Clone)]
struct Heard {
    session: String,
    text: String,
    backward: bool,
    ident: Ident,
}

/// What the button offers to quote.
struct Offer {
    session: String,
    text: String,
    truncated: bool,
    backward: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Settle {
    Idle,
    /// Heard since the last poll, which starts the wait.
    Heard,
    At(f64),
}

impl Quotes {
    /// The button is showing (or about to, once measured).
    pub fn offered(&self) -> bool {
        self.offer.is_some()
    }

    fn close(&mut self) {
        self.offer = None;
        self.heard = None;
        self.settle = Settle::Idle;
        self.dismissed = None;
    }

    /// Adds a quote to the composer; its key.
    pub fn push(&mut self, quote: Quote) -> u64 {
        self.next += 1;
        self.items.push(Pending { key: self.next, quote });
        self.next
    }

    pub fn remove(&mut self, key: u64) {
        self.items.retain(|q| q.key != key);
        if self.editing == Some(key) {
            self.editing = None;
        }
    }

    /// The quotes to send, leaving the composer without them.
    pub fn take(&mut self) -> Vec<Quote> {
        self.editing = None;
        std::mem::take(&mut self.items).into_iter().map(|q| q.quote).collect()
    }

    /// The quotes the next send carries.
    pub fn quotes(&self) -> Vec<Quote> {
        self.items.iter().map(|q| q.quote.clone()).collect()
    }

    pub fn any(&self) -> bool {
        !self.items.is_empty()
    }
}

/// The session the composer can take a quote into: not a subagent's (it has no composer), and
/// not in shell mode (a command takes no quotes).
fn available(s: &App) -> Option<&str> {
    let Route::Session(id) = &s.route else { return None };
    let child = s.session(id).is_some_and(|i| i.parent.is_some());
    (!child && !s.add.shell && !s.settings.open).then_some(id.as_str())
}

/// The timeline's `on_selection_change`: a new selection waits to settle; a cleared one closes
/// the button at once, and so does a press making one.
pub fn selection_changed(s: &mut App, change: &SelectionChange, tx: &mut Tx) {
    let q = &mut s.quote;
    let Some(sel) = change.selection() else {
        if q.offer.is_none() && q.heard.is_none() && q.dismissed.is_none() { tx.live_only(); }
        q.close();
        return;
    };
    if change.dragging() {
        if q.offer.is_none() { tx.live_only(); }
        q.offer = None;
        q.heard = None;
        q.settle = Settle::Idle;
        return;
    }
    let Some(session) = available(s).map(str::to_owned) else { return s.quote.close() };
    let ident = sel.texts().iter().filter(|t| t.is_selected()).map(|t| (t.path().to_owned(), t.range())).collect();
    s.quote.heard = Some(Heard { session, text: sel.text().to_owned(), backward: sel.backward(), ident });
    s.quote.settle = Settle::Heard;
    tx.wake_after(Duration::from_secs_f64(SETTLE));
}

/// The app's poll: offers a selection once it has settled, and closes what the route or a lost
/// focus leaves behind.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    let session = available(s).map(str::to_owned);
    if s.quote.offer.as_ref().is_some_and(|o| Some(&o.session) != session.as_ref()) {
        s.quote.close();
        changed = true;
    }
    changed |= crate::quote_cards::poll(s);
    match s.quote.settle {
        Settle::Heard => {
            s.quote.settle = Settle::At(now + SETTLE);
            tx.wake_after(Duration::from_secs_f64(SETTLE));
        }
        Settle::At(at) if now >= at - 1e-3 => {
            s.quote.settle = Settle::Idle;
            read(s);
            changed = true;
        }
        _ => {}
    }
    changed
}

fn read(s: &mut App) {
    let Some(heard) = s.quote.heard.clone() else { return };
    if s.quote.dismissed.as_ref() == Some(&heard.ident) {
        return;
    }
    s.quote.dismissed = None;
    let (text, truncated) = quote_text(&heard.text);
    if available(s) != Some(heard.session.as_str()) || text.is_empty() {
        return s.quote.close();
    }
    s.quote.offer = Some(Offer { session: heard.session, text, truncated, backward: heard.backward });
}

/// The selection as a quote (Desktop's `quoteText`): lines without trailing spaces, no more than
/// one blank line in a row, trimmed, and cut at `MAX_LEN` (true when cut).
pub fn quote_text(selected: &str) -> (String, bool) {
    let truncated = selected.chars().count() > MAX_LEN;
    let cut: String = if truncated { selected.chars().take(MAX_LEN).collect() } else { selected.to_owned() };
    let mut out = String::with_capacity(cut.len());
    let mut blanks = 0;
    for (i, line) in cut.split('\n').map(str::trim_end).enumerate() {
        blanks = if line.is_empty() { blanks + 1 } else { 0 };
        if blanks > 1 { continue; }
        if i > 0 { out.push('\n'); }
        out.push_str(line);
    }
    (out.trim().to_owned(), truncated)
}

/// Escape: hides the button, and the same selection stays dismissed until it changes.
pub fn dismiss(s: &mut App) -> bool {
    if s.quote.offer.is_none() {
        return false;
    }
    let ident = s.quote.heard.as_ref().map(|h| h.ident.clone());
    s.quote.close();
    s.quote.dismissed = ident;
    true
}

/// Typing in the composer leaves the timeline's selection behind, as a page's selection moves
/// into the field typed in.
pub fn typed(s: &mut App, tx: &mut Tx) {
    if s.quote.offer.is_some() || s.quote.heard.is_some() {
        s.quote.close();
        tx.clear_text_selection();
    }
}

/// The button's click: the selection becomes a quote in the composer. With Alt held its comment
/// editor opens; otherwise the prompt takes focus.
pub fn add(s: &mut App, comment: bool, tx: &mut Tx) {
    let Some(offer) = s.quote.offer.as_ref().filter(|o| !o.truncated) else { return };
    let quote = Quote { text: offer.text.clone(), comment: None };
    tx.clear_text_selection();
    // The card lands in the click, with nothing moving, as in Desktop.
    tx.instant();
    s.quote.close();
    let key = s.quote.push(quote);
    if comment {
        crate::quote_cards::open_editor(s, key);
    } else {
        s.composer.focused = true;
    }
}

/// The floating Quote button, over the settled selection: above a short or backward one, else
/// under it, whichever fits in the timeline's viewport, centred on its visible lines.
pub fn toolbar(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    let offer = s.quote.offer.as_ref()?;
    let z = s.zoom.max(1e-3);
    // Window points to the zoomed shell's: it's scaled about the window's centre.
    let view = cx.layout(&format!("scroll-{}", offer.session))?.map(|v| v / z);
    let (b_top, b_bottom, b_left, b_right) = (view[1], view[1] + view[3], view[0], view[0] + view[2]);
    let rects: Vec<[f32; 4]> = cx.selection_rects().into_iter().map(|r| r.map(|v| v / z)).collect();
    // Its own size, from the last layout. Unmeasured, it lays out unseen in the same pass the
    // selection's boxes first arrive in, so the next pass has both.
    let size = cx.layout_size("quote-toolbar").map(|[w, h]| (w / z, h / z));
    let unseen = || size.is_none().then(|| bar(p, offer.truncated).absolute(0.0, 0.0).opacity(0.0));
    let (Some(&first), Some(&last)) = (rects.first(), rects.last()) else { return unseen() };
    let (top, bottom) = (first[1], last[1] + last[3]);
    let visible: Vec<&[f32; 4]> = rects.iter().filter(|r| r[1] + r[3] > b_top.max(top) && r[1] < b_bottom.min(bottom)).collect();
    if visible.is_empty() {
        return unseen();
    }
    let left = visible.iter().map(|r| r[0]).fold(f32::INFINITY, f32::min);
    let right = visible.iter().map(|r| r[0] + r[2]).fold(f32::NEG_INFINITY, f32::max);
    let short = bottom - top < first[3] * (SHORT_LINES + 0.5);
    let (w, h) = size.unwrap_or((0.0, 0.0));
    let above = top - h - GAP;
    let below = bottom + GAP;
    let fits_above = above >= b_top + GAP && top <= b_bottom;
    let fits_below = below + h <= b_bottom - GAP && bottom >= b_top;
    let sides = if short || offer.backward { [(fits_above, above), (fits_below, below)] } else { [(fits_below, below), (fits_above, above)] };
    let y = sides.iter().find(|(fits, _)| *fits).map_or(b_bottom - h - GAP, |(_, y)| *y);
    let x = ((left + right - w) / 2.0).max(b_left + GAP).min(b_right - w - GAP);
    Some(bar(p, offer.truncated).absolute(x, y).opacity(if size.is_some() { 1.0 } else { 0.0 }))
}

fn bar(p: &Pal, truncated: bool) -> El {
    let mut button = row()
        .key("quote-add")
        .role("button")
        .label("Quote")
        .help("Quote and comment ⌥")
        .h(24.0)
        .px(9.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .child(icon(&OUTLINE_QUOTE, 16.0).color(p.text))
        .child(text("Quote").font(BUTTON).color(p.text));
    button = if truncated {
        button.opacity(0.5)
    } else {
        button.hover(p.hover).press(p.pressed).on_press_tx(|s: &mut App, tx: &mut Tx| add(s, tx.mods().alt, tx))
    };
    row()
        .key("quote-toolbar")
        .role("toolbar")
        .label("Quote selection")
        .padding(2.0)
        .gap(2.0)
        .center_y()
        .radius(8.0)
        .fill(if p.dark { p.layer1 } else { p.bg_base })
        // --v2-elevation-raised.
        .border(0.5, p.ring)
        .shadow(2.0, 4.0, p.shadow_far)
        .shadow(1.0, 2.0, p.shadow)
        // It comes, goes and moves at once.
        .transition(Transition::opacity().springs(no_motion()))
        .motion(no_motion().enter)
        .opacity_motion(rune::Anim::INSTANT)
        // Pressing it leaves the selection, and the prompt's focus, where they are.
        .keeps_text_selection()
        .children(truncated.then(|| text("Selection too long").font(TOO_LONG).color(p.muted).px(8.0)))
        .child(button)
}

#[cfg(test)]
#[path = "quote_tests.rs"]
mod tests;
