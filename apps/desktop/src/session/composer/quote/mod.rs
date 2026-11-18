//! Quoting timeline text into the composer, as Desktop does (`session/timeline/quote.tsx`): a
//! Quote button over a settled timeline selection adds it to the composer as a card, where it
//! can carry a comment (`quote_cards`). It goes out as lines in the prompt's text and an entry
//! in `metadata.comments` (`opencode_kit::quote`).

use std::collections::HashSet;

use rune::{Cx, El, SelectionChange, TextEdit, TextStyle, Transition, Tx, icon, row, text};

use opencode_kit::model::{PromptComment, Quote};

use crate::palette::Pal;
use crate::{App, Route};

/// About 5k tokens: a longer selection says so and can't be quoted.
pub const MAX_LEN: usize = 20_000;
/// A selection under this many lines (and a half) is short: the button goes above it.
const SHORT_LINES: f32 = 3.0;
const GAP: f32 = 8.0;

const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const TOO_LONG: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);

pub use session_view::user_parts::OUTLINE_QUOTE;

pub mod cards;
pub mod comment;
pub mod editor;
pub mod machine;
pub mod place;
mod card_stories;
mod stories;
pub use editor::send_result;
use machine::Ident;

/// The quotes waiting in the composer, the comment editor, and the toolbar's machine.
#[derive(Default)]
pub struct Quotes {
    /// Waiting in the composer for the next send, in the order they were added.
    pub items: Vec<Pending>,
    next: u64,
    /// The quote whose comment editor is open, and the comment being written.
    pub comment: machine::Comment,
    pub toolbar: machine::Toolbar,
    seq: rune::machine::Seq,
    /// The timeline's selection as it last changed, which the toolbar reads once it settles
    /// and a right-click there copies or quotes.
    selection: Option<Heard>,
    /// Sent prompts whose quote cards were opened past the first five.
    pub more: HashSet<String>,
    /// Takes an edit that lands after the comment editor closed: the field's `on_edit` needs
    /// somewhere to write.
    pub(crate) spare: TextEdit,
    pub(crate) comment_seq: rune::machine::Seq,
    /// The editor's height when it opened, which its bottom edge keeps as the draft grows.
    pub(crate) opening: std::cell::Cell<f32>,
    /// Where the editor stands as its text scrolls.
    pub(crate) placing: std::cell::Cell<place::Placing>,
    /// Where the offered selection stands, for the editor that replaces the toolbar.
    pub(crate) placed: std::cell::RefCell<Option<comment::Anchor>>,
    /// The comment field's `@` file search.
    pub(crate) mention_fetch: rune::Latest<(rune::machine::Attempt, Option<Vec<String>>)>,
}

/// A comment waiting in the composer: a quote, or a comment on lines of a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub key: u64,
    pub comment: PromptComment,
    /// Where in the timeline a quote was quoted from, lit while its comment editor is open.
    pub source: Vec<rune::TextMark>,
}

impl Pending {
    pub fn quote(&self) -> Option<&Quote> {
        self.comment.quote()
    }
}

/// A timeline selection as heard.
#[derive(Clone)]
struct Heard {
    session: String,
    text: String,
    backward: bool,
    ident: Ident,
    /// The selected bytes of each text, to light once quoted.
    source: Vec<rune::TextMark>,
}

impl Quotes {
    /// The toolbar is showing (or about to, once measured).
    pub fn offered(&self) -> bool {
        self.offer().is_some()
    }

    fn offer(&self) -> Option<&machine::Offer> {
        match &self.toolbar.phase {
            machine::Phase::Shown(offer) => Some(offer),
            _ => None,
        }
    }

    fn step(&mut self, event: machine::Event<'_>, tx: &mut Tx) {
        for effect in machine::step(&mut self.seq, &mut self.toolbar, event) {
            match effect {
                machine::Effect::WakeAt(due) => tx.wake_at(due),
                machine::Effect::Read => {}
            }
        }
    }

    /// Adds a comment to the composer; its key.
    pub fn push(&mut self, comment: impl Into<PromptComment>) -> u64 {
        self.push_from(comment.into(), Vec::new())
    }

    /// [`Self::push`] for a quote taken from the timeline at `source`.
    pub fn push_from(&mut self, comment: PromptComment, source: Vec<rune::TextMark>) -> u64 {
        self.next += 1;
        self.items.push(Pending { key: self.next, comment, source });
        self.next
    }

    pub fn remove(&mut self, key: u64) {
        self.items.retain(|q| q.key != key);
        // An editor whose quote is gone closes (`retained`).
        if self.comment.key() == Some(key) {
            self.comment = machine::Comment::Closed;
        }
    }

    /// The comments to send, leaving the composer without them.
    pub fn take(&mut self) -> Vec<PromptComment> {
        self.comment = machine::Comment::Closed;
        std::mem::take(&mut self.items).into_iter().map(|q| q.comment).collect()
    }

    /// The comments the next send carries.
    pub fn quotes(&self) -> Vec<PromptComment> {
        self.items.iter().map(|q| q.comment.clone()).collect()
    }

    pub fn any(&self) -> bool {
        !self.items.is_empty()
    }

    /// Changes when the cards change: one added or removed, or a comment saved (its editor
    /// closes), for the draft's saved copy (`restore::draft_doc`).
    pub fn revision(&self) -> (usize, u64, Option<u64>) {
        (self.items.len(), self.next, self.comment.key())
    }
}

/// The session the composer can take a quote into: not a subagent's (it has no composer), and
/// not in shell mode (a command takes no quotes).
fn available(s: &App) -> Option<&str> {
    let Route::Session { id, .. } = &*s.route else { return None };
    let child = s.session(id).is_some_and(|i| i.parent.is_some());
    (!child && !s.add.shell() && !s.settings.open).then_some(id.as_str())
}

/// The timeline's `on_selection_change`: a new selection waits to settle; a cleared one closes
/// the toolbar at once, and so does a press making one.
pub fn selection_changed(s: &mut App, change: &SelectionChange, tx: &mut Tx) {
    let now = tx.clock();
    let q = &mut *s.quote;
    let idle = q.toolbar == machine::Toolbar::default();
    let Some(sel) = change.selection() else {
        if idle && q.selection.is_none() { tx.live_only(); }
        q.selection = None;
        return q.step(machine::Event::Changed { cleared: true, dragging: false, now }, tx);
    };
    if change.dragging() {
        if !q.offered() && !matches!(q.toolbar.phase, machine::Phase::Dismissed(_)) { tx.live_only(); }
        q.selection = None;
        return q.step(machine::Event::Changed { cleared: false, dragging: true, now }, tx);
    }
    let Some(session) = available(s).map(str::to_owned) else {
        s.quote.selection = None;
        return s.quote.step(machine::Event::Reset, tx);
    };
    let ident = sel.texts().iter().filter(|t| t.is_selected()).map(|t| (t.path().to_owned(), t.range())).collect();
    let source = sel.texts().iter().filter(|t| t.is_selected()).map(|t| t.mark()).collect();
    s.quote.selection = Some(Heard { session, text: sel.text().to_owned(), backward: sel.backward(), ident, source });
    s.quote.step(machine::Event::Changed { cleared: false, dragging: false, now }, tx);
}

/// The app's poll: reads a selection once it has settled, and closes what the route or a lost
/// focus leaves behind.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    // Every poll comes here: the session is compared in place, and copied only for a read.
    let session = s.quote.toolbar.settle.and_then(|_| available(s).map(str::to_owned));
    if s.quote.offer().is_some_and(|o| Some(o.session.as_str()) != available(s)) {
        s.quote.step(machine::Event::Reset, tx);
        changed = true;
    }
    changed |= editor::poll(s, tx);
    if s.quote.toolbar.settle.is_some_and(|w| now + 1e-3 >= w.due) {
        let q = &mut *s.quote;
        let read = machine::step(&mut q.seq, &mut q.toolbar, machine::Event::Due { now });
        if read.into_iter().any(|e| e == machine::Effect::Read) {
            read_selection(s, session.as_deref(), tx);
            changed = true;
        }
    }
    changed
}

/// The settled selection becomes what the toolbar offers (`quote.read`, then `quote.measured`).
fn read_selection(s: &mut App, session: Option<&str>, tx: &mut Tx) {
    let Some(heard) = s.quote.selection.clone() else {
        return s.quote.step(machine::Event::Read { offer: None, ident: &Vec::new() }, tx);
    };
    let (text, truncated) = quote_text(&heard.text);
    let offer = (session == Some(heard.session.as_str()) && !text.is_empty())
        .then(|| machine::Offer { session: heard.session.clone(), text, truncated, backward: heard.backward });
    s.quote.step(machine::Event::Read { offer, ident: &heard.ident }, tx);
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

/// Escape: hides the toolbar, and the same selection stays dismissed until it changes.
pub fn dismiss(s: &mut App, tx: &mut Tx) -> bool {
    if !s.quote.offered() {
        return false;
    }
    let ident = s.quote.selection.as_ref().map(|h| h.ident.clone());
    s.quote.step(machine::Event::Escape { ident }, tx);
    true
}

/// The timeline text the open comment editor's quote came from, lit in the timeline's
/// selection colour while the editor is open and the composer can take quotes
/// (`createTimelineQuoteHighlight`, `selectQuoteHighlightKey`).
pub fn marks(s: &App) -> Vec<rune::TextMark> {
    let key = s.quote.comment.key().filter(|_| available(s).is_some());
    key.and_then(|k| s.quote.items.iter().find(|q| q.key == k)).map(|q| q.source.clone()).unwrap_or_default()
}

/// Typing in the composer leaves the timeline's selection behind, as a page's selection moves
/// into the field typed in.
pub fn typed(s: &mut App, tx: &mut Tx) {
    if s.quote.offered() || s.quote.selection.is_some() {
        s.quote.selection = None;
        s.quote.step(machine::Event::Reset, tx);
        tx.clear_text_selection();
    }
}

/// The toolbar's click: the selection becomes a quote in the composer and its comment editor
/// opens where the toolbar stood; with Alt held it skips the comment and the prompt takes focus
/// (`quote.tsx:343-395`).
pub fn add(s: &mut App, comment: bool, tx: &mut Tx) {
    let Some(offer) = s.quote.offer().filter(|o| !o.truncated) else { return };
    let text = offer.text.clone();
    add_text(s, text, comment, tx);
}

/// The timeline's selection when it lies in message `message` (a selected text's path names
/// it), for a right-click there to copy or quote.
pub fn selected_in(s: &App, message: &str) -> Option<String> {
    let heard = s.quote.selection.as_ref()?;
    heard.ident.iter().any(|(path, _)| path.contains(message)).then(|| heard.text.clone()).filter(|t| !t.trim().is_empty())
}

/// Whether `selected` can go into the composer as a quote now: there's one to take it, and it
/// isn't too long.
pub fn can_quote(s: &App, selected: &str) -> bool {
    available(s).is_some() && !quote_text(selected).1
}

/// `selected` becomes a quote in the composer, as the button's click makes one.
pub fn quote_selected(s: &mut App, selected: &str, tx: &mut Tx) {
    let (text, truncated) = quote_text(selected);
    if available(s).is_none() || truncated || text.is_empty() {
        return;
    }
    add_text(s, text, false, tx);
}

fn add_text(s: &mut App, text: String, comment: bool, tx: &mut Tx) {
    let quote = Quote { text, comment: None };
    tx.clear_text_selection();
    // The card lands in the click, with nothing moving, as in Desktop.
    tx.instant();
    let source = s.quote.selection.take().map(|h| h.source).unwrap_or_default();
    s.quote.step(machine::Event::Reset, tx);
    let key = s.quote.push_from(quote.into(), source);
    if comment {
        let anchor = s.quote.placed.borrow_mut().take();
        editor::open(s, key, anchor);
    } else if crate::auto_focus_composer(tx) {
        s.composer.focused = true;
    }
}

/// The floating Quote button, over the settled selection: above a short or backward one, else
/// under it, whichever fits in the timeline's viewport, centred on its visible lines.
pub fn toolbar(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    let offer = s.quote.offer()?;
    // Rune reads rects in layout points, before the zoomed shell's `El::scale`, and the shell
    // sits centred in the window-sized root: take off its corner to get the shell's points.
    let [ox, oy] = cx.layout_with("zoomed", |r| [r[0], r[1]]).unwrap_or([0.0, 0.0]);
    let shell = |r: [f32; 4]| [r[0] - ox, r[1] - oy, r[2], r[3]];
    // The selection's boxes first, so a pass before the timeline's layout still re-runs when
    // they arrive.
    let rects: Vec<[f32; 4]> = cx.selection_rects().into_iter().map(shell).collect();
    let view = shell(cx.window_layout(&format!("scroll-{}", offer.session))?);
    let (b_top, b_bottom, b_left, b_right) = (view[1], view[1] + view[3], view[0], view[0] + view[2]);
    // Its own size, from the last layout. Unmeasured, it lays out unseen in the same pass the
    // selection's boxes first arrive in, so the next pass has both.
    let size = cx.layout_size("quote-toolbar").map(|[w, h]| (w, h));
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
    // Where the quoted lines stand from their first text, for an editor that replaces the bar.
    let first_path = s.quote.selection.as_ref().and_then(|h| h.ident.first().map(|(path, _)| path.rsplit('/').next().unwrap_or(path).to_owned()));
    if let Some(path) = first_path
        && let Some(origin) = cx.window_layout(&path).map(shell)
    {
        let rects = rects.iter().map(|r| [r[0] - origin[0], r[1] - origin[1], r[2], r[3]]).collect();
        *s.quote.placed.borrow_mut() = Some(comment::Anchor { session: offer.session.clone(), path, rects, short, backward: offer.backward });
    }
    let x = ((left + right - w) / 2.0).max(b_left + GAP).min(b_right - w - GAP);
    let (x, y) = if cx.hover_none() { ((b_left + b_right - w) / 2.0, b_bottom - h - GAP) } else { (x, y) };
    Some(bar(p, offer.truncated).absolute(x, y).opacity(if size.is_some() { 1.0 } else { 0.0 }))
}

fn bar(p: &Pal, truncated: bool) -> El {
    bar_with(p, truncated, |s: &mut App, tx: &mut Tx| add(s, !tx.mods().alt, tx))
}

/// The Quote toolbar with `on` as its button's press: the timeline's, or a terminal's.
pub(crate) fn bar_with(p: &Pal, truncated: bool, on: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    let mut button = row()
        .key("quote-add")
        .role("button")
        .label_static(opencode_i18n::t!("session.quote.add"))
        .help(opencode_i18n::t!("session.quote.addWithoutComment")).help_keys([crate::keymap::modifier_label(rune::ChordMod::Alt)])
        .h(24.0)
        .px(9.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .child(icon(&OUTLINE_QUOTE, 16.0).color(p.text))
        .child(text(opencode_i18n::t!("session.quote.add")).font(BUTTON).color(p.text));
    button = if truncated {
        button.opacity(0.5)
    } else {
        button.hover(p.hover).press(p.pressed).on_press_tx(on)
    };
    row()
        .key("quote-toolbar")
        .role("toolbar")
        .label_static(opencode_i18n::t!("session.quote.actions"))
        .padding(2.0)
        .gap(2.0)
        .center_y()
        .radius(8.0)
        .fill(crate::view::composer_fill(p))
        // --v2-elevation-raised.
        .box_shadow(Pal::ring_out(p.ring))
        .shadow(2.0, 4.0, p.shadow_far)
        .shadow(1.0, 2.0, p.shadow)
        // It comes, goes and moves at once.
        .transition(Transition::none())
        .motion(rune::Spring::NONE)
        .opacity_motion(rune::Anim::INSTANT)
        // Pressing it leaves the selection, and the prompt's focus, where they are.
        .keeps_text_selection()
        .children(truncated.then(|| text(opencode_i18n::t!("session.quote.tooLong")).font(TOO_LONG).color(p.muted).px(8.0)))
        .child(button)
}
