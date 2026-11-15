//! Find in a session (⌘F): Desktop's `TimelineSearchBar` and `createTimelineSearchController`
//! (`session/timeline/search-bar.tsx`, `search-controller.ts`).
//!
//! Matches are indexed across the whole visible transcript (mounted or not), cached per message
//! so streaming replies only re-scan the changed part, and hashed per row so `rune::memo` only
//! rebuilds mounted rows whose highlights or active match actually changed.

use crate::pickers::PickerHost as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

use std::sync::Arc;

use opencode_kit::model::{Message, Part, Transcript};
use rune::{El, Key, KeyEvent, MenuItem, TextEdit, TextHighlight, TextStyle, Transition, Tx, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::keymap::Bind;
use crate::palette::Pal;
use crate::transcript_index::{Index, KEY_BUDGET, Kind, Results, RowHit, Source, retire};
use crate::view::{INSTANT, UI};
use crate::{App, Route};

const COUNT: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0).features("tnum");

/// Find's state: Desktop's `TimelineSearchState` (`session/timeline/search-machine.ts:30-33`).
/// Its query is the bar's field (`input`), its matches the index [`Find`] keeps; `seeking` has no
/// counterpart, as Rune scrolls to a row that isn't built yet in the frame it asks.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Closed,
    /// Open on session `session`'s matches; `active` is the lit one's place in them (0 with none).
    Open { session: String, active: usize },
}

/// `TimelineSearchEvent`, less the seek's.
#[derive(Clone, Debug, PartialEq)]
pub enum Event<'a> {
    /// `open`: Find opens on `session`, or focuses and selects its query when it is open.
    Open { session: &'a str },
    Close,
    /// `query.changed`: the first match lights.
    QueryChanged,
    /// `move`: the lit match steps by `delta` of `count`, wrapping.
    Move { delta: i32, count: usize },
    /// `session.changed`: another session's timeline shows; Find stays open on its matches.
    SessionChanged { session: &'a str },
    /// `matches.synced`: the matches changed; `kept` is where the lit one is among them now.
    MatchesSynced { kept: Option<usize> },
    /// `disposed`: no session's timeline shows (Home), so Find closes.
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    FocusInput,
    BlurInput,
    /// Bring the lit match into view.
    Reveal,
}

/// `transitionTimelineSearch`.
pub fn step(state: &mut State, event: Event) -> rune::machine::Effects<Effect> {
    use rune::machine::Effects;
    match (&mut *state, event) {
        (State::Closed, Event::Open { session }) => {
            *state = State::Open { session: session.to_owned(), active: 0 };
            Effects::one(Effect::FocusInput)
        }
        (State::Closed, _) => Effects::none(),
        // Already open: focus and select its query again.
        (State::Open { .. }, Event::Open { .. }) => Effects::one(Effect::FocusInput),
        (State::Open { .. }, Event::Close) => {
            *state = State::Closed;
            Effects::one(Effect::BlurInput)
        }
        (State::Open { .. }, Event::Left) => {
            *state = State::Closed;
            Effects::none()
        }
        (State::Open { active, .. }, Event::QueryChanged) => {
            *active = 0;
            Effects::one(Effect::Reveal)
        }
        (State::Open { active, .. }, Event::Move { delta, count }) => {
            if delta == 0 || count == 0 {
                return Effects::none();
            }
            *active = (*active as i64 + i64::from(delta)).rem_euclid(count as i64) as usize;
            Effects::one(Effect::Reveal)
        }
        (State::Open { session, active }, Event::SessionChanged { session: next }) => {
            if session != next {
                *session = next.to_owned();
                *active = 0;
            }
            Effects::none()
        }
        // The lit match keeps its place by identity, so a list that shrinks and grows again
        // never brings an old index back.
        (State::Open { active, .. }, Event::MatchesSynced { kept }) => {
            *active = kept.unwrap_or(0);
            Effects::none()
        }
    }
}

/// What the matches are: none; those of the rows on screen while the worker matches the rest
/// (`reveal`: the first match in the transcript lights and comes into view when they land); or
/// every row's, for the transcript of shape `shape`.
#[derive(Default)]
enum Answer {
    #[default]
    None,
    Partial { query: String, results: Arc<Results>, reveal: bool },
    Complete { query: String, shape: u64, results: Arc<Results> },
}

/// What a copy of the transcript is checked against: its session, version (with whether it
/// streams and whether outlines read as words), how many messages show, and the Slack chips'
/// words its prompts show (`slack`).
type CopyKey = (String, u64, usize, u64);

/// Rows copied a frame (`find_shape`): ~4 ms at 100,000 messages.
const COPY_BUDGET: usize = 16_384;

/// Find's copy of the open transcript's rows: none under way; under way, `done` messages read
/// and hashed as it goes; or done.
#[derive(Default)]
enum Copying {
    #[default]
    Idle,
    Running { next: Source, done: usize, hash: DefaultHasher },
    Done(Source),
}

/// Find's index worker: idle, or making the index of transcript shape `shape`. It has its own
/// worker, so keys arriving faster than a large index builds never restart the build.
#[derive(Default, PartialEq)]
enum Indexing {
    #[default]
    Idle,
    Running { shape: u64 },
}

/// Where a new query's lit match goes: the first in the session (official's `queryChanged`), or,
/// for Find opened on a search result, the first in that message once every match is known.
#[derive(Clone, Default, PartialEq)]
enum Target {
    #[default]
    First,
    Message(String),
}

/// Find's match worker: idle, or matching `query` in the index of shape `shape`.
#[derive(Default, PartialEq)]
enum Matching {
    #[default]
    Idle,
    Running { shape: u64, query: String },
}

#[derive(Default)]
pub struct Find {
    pub state: State,
    pub input: TextEdit,

    /// The open session's transcript as last checked, and its shape.
    checked: Option<(CopyKey, u64)>,
    copying: Copying,
    /// Its rows, copied when its shape changes while Find is open, and their index.
    source: Option<Arc<Source>>,
    index: Option<Arc<Index>>,
    answer: Answer,
    indexing: Indexing,
    indexer: rune::Latest<Arc<Index>>,
    matching: Matching,
    matcher: rune::Latest<(u64, String, Arc<Results>)>,
    target: Target,
}

impl Find {
    pub fn set_waker(&mut self, waker: rune::Waker) {
        self.indexer.set_waker(waker.clone());
        self.matcher.set_waker(waker);
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, State::Open { .. })
    }

    /// A closed Find holds nothing: its index is made again (on the worker) when it opens.
    fn release(&mut self) {
        retire((std::mem::take(&mut self.answer), self.source.take(), self.index.take()));
        self.checked = None;
        self.copying = Copying::Idle;
        self.indexing = Indexing::Idle;
        self.matching = Matching::Idle;
        self.indexer.cancel();
        self.matcher.cancel();
        self.target = Target::First;
    }

    pub fn query(&self) -> &str {
        self.input.text.trim()
    }

    fn results(&self) -> Option<&Results> {
        match &self.answer {
            Answer::None => None,
            Answer::Partial { results, .. } | Answer::Complete { results, .. } => Some(results).filter(|_| self.is_open()).map(|r| &**r),
        }
    }

    /// The query the matches are for.
    fn answered(&self) -> Option<&str> {
        match &self.answer {
            Answer::None => None,
            Answer::Partial { query, .. } | Answer::Complete { query, .. } => Some(query),
        }
    }

    pub fn count(&self) -> usize {
        self.results().map_or(0, |r| r.total)
    }

    /// The worker is still matching the rows off screen: the count isn't known yet.
    pub fn counting(&self) -> bool {
        matches!(self.answer, Answer::Partial { .. })
    }

    pub fn active_index(&self) -> usize {
        let active = match self.state {
            State::Open { active, .. } => active,
            State::Closed => 0,
        };
        active.min(self.count().saturating_sub(1))
    }

    /// The lit match: its row and its place in the row.
    fn lit(&self) -> Option<(&RowHit, usize)> {
        self.results()?.locate(self.active_index())
    }

    /// The lit match's place in `row_key`'s row, when it is there.
    fn active_in(&self, row_key: &str) -> Option<usize> {
        self.lit().filter(|(h, _)| &*h.key == row_key).map(|(_, local)| local)
    }

    /// Hash included in `RowSpec::build`'s `rune::memo` key: `0` when Find is closed or `row_key`
    /// has no matches, so non-matching rows never rebuild on keystrokes or stepping.
    pub fn row_version(&self, row_key: &str) -> u64 {
        let Some(hit) = self.results().and_then(|r| r.get(row_key)) else { return 0 };
        match self.active_in(row_key) {
            Some(local) => row_hash(&hit.ranges, Some(local)),
            None => hit.version,
        }
    }

    /// Highlights for a user bubble (`text(shown)`).
    pub fn text_highlights(&self, p: &Pal, row_key: &str) -> Vec<TextHighlight> {
        let Some(row) = self.results().and_then(|r| r.get(row_key)) else { return Vec::new() };
        let active = self.active_in(row_key);
        row.ranges
            .iter()
            .enumerate()
            .filter_map(|(i, rs)| Some(text_mark(p, rs.first()?.clone(), i, active == Some(i))))
            .collect()
    }

    /// Highlights for an assistant `Markdown` block (`Part::Text`).
    pub fn markdown_highlights<'a>(&'a self, p: &Pal, row_key: &str) -> Vec<(&'a [Range<usize>], rune_kit::Highlight)> {
        let Some(row) = self.results().and_then(|r| r.get(row_key)) else { return Vec::new() };
        let active = self.active_in(row_key);
        row.ranges.iter().enumerate().map(|(i, rs)| (rs.as_slice(), markdown_mark(p, i, active == Some(i)))).collect()
    }

    /// A row's matches (source byte ranges of its text) and which of them is active, for a
    /// view that paints them itself (`genui`).
    pub fn row_hits(&self, row_key: &str) -> Option<(&[Vec<Range<usize>>], Option<usize>)> {
        let hit = self.results()?.get(row_key)?;
        Some((hit.ranges.as_slice(), self.active_in(row_key)))
    }
}

pub(crate) use session_view::style::{ACTIVE_KEY, markdown_mark, text_mark};

fn str_sig(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut h = DefaultHasher::new();
    b.len().hash(&mut h);
    b[..b.len().min(32)].hash(&mut h);
    b[b.len().saturating_sub(32)..].hash(&mut h);
    h.finish()
}

pub(crate) fn row_hash(ranges: &[Vec<Range<usize>>], active: Option<usize>) -> u64 {
    let mut h = DefaultHasher::new();
    ranges.hash(&mut h);
    active.hash(&mut h);
    h.finish().max(1)
}

impl App {
    pub fn find_focused(&self) -> bool {
        self.find.is_open() && self.find.input.focused
    }

    /// Steps Find's machine and carries out what it asks.
    fn find_step(&mut self, event: Event, tx: &mut Tx) {
        let effects = step(&mut self.find.state, event);
        if !self.find.is_open() {
            self.find.input.clear();
            self.find.release();
        }
        for effect in effects {
            match effect {
                Effect::FocusInput => {
                    self.composer.focused = false;
                    self.find.input.focused = true;
                    let len = self.find.input.text.len();
                    self.find.input.set_selection(0, len);
                }
                // Desktop leaves nothing focused once the bar closes, by Escape or its button
                // (`blur-input`; `esc-find`).
                Effect::BlurInput => {
                    self.find.input.focused = false;
                    self.find.input.cancel_composition();
                    // The bar's field leaves the tree: nothing takes its focus, not the next control.
                    tx.blur();
                }
                Effect::Reveal => self.seek_active(tx),
            }
        }
    }

    /// Opens Find in the active session (or focuses and selects its query when already open).
    pub fn open_find(&mut self, tx: &mut Tx) {
        let Route::Session { id, .. } = (*self.route).clone() else { return };
        self.close_picker(false);
        self.sync_find_index();
        self.find_step(Event::Open { session: &id }, tx);
        self.sync_find_index();
        tx.instant();
    }

    /// Opens Find on `query` with message `message`'s first match lit and in view once the
    /// matches are known: a search result's jump to what matched.
    pub fn open_find_at(&mut self, query: &str, message: &str, tx: &mut Tx) {
        self.open_find(tx);
        self.find.input.replace_text(query.to_owned());
        typed(self, tx);
        self.find.target = Target::Message(message.to_owned());
        // An answer complete on the key lights the message now.
        if let Some(at) = self.find.results().and_then(|r| r.first_in_message(message)) {
            self.find.target = Target::First;
            let _ = step(&mut self.find.state, Event::MatchesSynced { kept: Some(at) });
            self.seek_active(tx);
        }
    }

    /// Find's target is a message the open session hasn't loaded, and older pages remain.
    fn find_target_waits(&self) -> bool {
        let (Target::Message(message), Route::Session { id, .. }) = (&self.find.target, &*self.route) else { return false };
        let loaded = self.transcripts.get(id).is_some_and(|t| t.messages.iter().any(|m| m.id() == message));
        !loaded && (!self.pages.first_read_done(id) || self.pages.more_above(id))
    }

    /// Closes Find, clearing the query and highlights; nothing keeps the focus.
    pub fn close_find(&mut self, tx: &mut Tx) {
        if !self.find.is_open() {
            return;
        }
        // The bar goes as it came, at once, and the header's crumbs take its room back with it.
        tx.instant();
        self.find_step(Event::Close, tx);
    }

    /// Steps forward (`1`) or backward (`-1`) through the session's matches, wrapping around.
    pub fn step_find(&mut self, delta: i32, tx: &mut Tx) {
        self.sync_find_index();
        let count = self.find.count();
        self.find_step(Event::Move { delta, count }, tx);
    }

    /// Opens a folded long user prompt when the lit match falls in or past its 40 pt fading
    /// region (`search-controller.ts`: `rect.bottom > fold.bottom - FOLD_FADE`).
    fn unfold_if_hidden(&mut self, sid: &str, row_key: &str, end_byte: usize) -> bool {
        if !self.settings.fold_user {
            return false;
        }
        let Some(id) = row_key.strip_prefix("u-") else { return false };
        let fold_key = crate::fold::key(id);
        if self.expanded.contains(&fold_key) {
            return false;
        }
        let Some(body) = self.transcripts.get(sid).and_then(|t| {
            t.messages.iter().find_map(|m| match m {
                Message::User { id: mid, text } if mid == id => Some(text.clone()),
                _ => None,
            })
        }) else {
            return false;
        };
        let shown = self.bubble_text(id, &body);
        let quotes = !opencode_kit::model::user_comments(id).is_empty() && !body.trim().is_empty();
        let wrap_w = crate::fold::text_width(crate::fold::viewport_width(), quotes);
        if crate::fold::lines(&shown, wrap_w) <= crate::fold::OVER {
            return false;
        }
        let end = shown.ceil_char_boundary(end_byte.min(shown.len()));
        let prefix = &shown[..end];
        let (_, measured_h) = crate::fold::measure(prefix, wrap_w);
        let hard_h = prefix.split('\n').count() as f32 * crate::view::BUBBLE.line_px();
        let clear = crate::fold::SHOW as f32 * crate::view::BUBBLE.line_px() - 40.0;
        if measured_h.max(hard_h) <= clear {
            return false;
        }
        self.expanded.insert(fold_key);
        true
    }

    fn seek_active(&mut self, tx: &mut Tx) {
        let Route::Session { id: sid, .. } = (*self.route).clone() else { return };
        let Some((row_key, end_byte)) = self.find.lit().map(|(h, local)| (h.key.to_string(), h.ranges[local].last().map_or(0, |r| r.end))) else { return };
        if self.unfold_if_hidden(&sid, &row_key, end_byte) {
            tx.instant();
        }
        let scroll_key = format!("scroll-{sid}");
        // Mount the match's row, then land the match where Desktop's `probeSeek` puts it
        // (`search-controller.ts:293-298`, scrolled with `behavior: "auto"`): centred in the
        // scroll view, which runs under the composer to the page's bottom, less the sticky
        // header's 64 (its 48 and the 16 its fade takes, `[data-session-title]`'s height).
        tx.scroll_center_in_instant(&scroll_key, &row_key);
        tx.scroll_center_in_instant(&scroll_key, ACTIVE_KEY);
        if let (Some([_, top, _, _]), Some([_, page_y, _, page_h])) = (self.message_nav.view, self.message_nav.page) {
            let header = if self.narrow() { 0.0 } else { session_view::style::HEADER_H + 16.0 };
            // `scroll_key_to` places the top of Rune's highlight *line box*. Blink instead places
            // the text range, already inset by half the leading. Centre the line, not an 18 pt
            // glyph box with its top mistaken for the line's (3–4 device pixels low at 1.875x).
            let line = if row_key.starts_with("u-") { session_view::style::BUBBLE } else { session_view::style::MD };
            tx.scroll_key_to(ACTIVE_KEY, header + (page_y + page_h - top - line.line_px()) / 2.0);
        }
    }

    /// Keeps Find's session up with the route: another session's timeline keeps Find open on
    /// that session's matches (`session.changed`), and leaving the session view closes it, as
    /// Desktop's search goes with its session screen (`disposed`).
    fn sync_find_session(&mut self) -> Option<String> {
        let Route::Session { id, .. } = &*self.route else {
            let _ = step(&mut self.find.state, Event::Left);
            self.find.input.clear();
            if !self.find.is_open() {
                self.find.release();
            }
            self.find.input.focused = false;
            return None;
        };
        let id = id.clone();
        if matches!(&self.find.state, State::Open { session, .. } if *session != id) {
            let _ = step(&mut self.find.state, Event::SessionChanged { session: &id });
            retire(std::mem::take(&mut self.find.answer));
        }
        Some(id)
    }

    /// Brings Find's index and matches up to the open session and the query. A key matches at
    /// most [`KEY_BUDGET`] rows itself: past that, it lights the rows on screen and the worker
    /// matches the rest (and makes the index when the transcript changed).
    pub(crate) fn sync_find_index(&mut self) {
        let current = self.sync_find_session();
        // Settings covers the session, which keeps its Find as it was.
        if self.settings.open {
            return;
        }
        let current = current.unwrap_or_default();
        if !self.find.is_open() || current.is_empty() {
            retire(std::mem::take(&mut self.find.answer));
            return;
        }
        let Some(shape) = self.find_shape(&current) else {
            retire(std::mem::take(&mut self.find.answer));
            return;
        };
        let q = self.find.query().to_owned();
        let indexed = self.find.index.as_ref().is_some_and(|i| i.source.shape == shape);
        if q.is_empty() {
            retire(std::mem::take(&mut self.find.answer));
        } else if self.find.answered() != Some(q.as_str()) {
            let fold = crate::language::search_fold();
            let answer = match self.find.index.clone() {
                Some(index) if indexed => match index.candidates_within(&q, KEY_BUDGET) {
                    // Few rows can match: all of them, now.
                    Some(rows) => Answer::Complete { query: q.clone(), shape, results: Arc::new(index.search(&rows, &q, fold)) },
                    // Too many: the first match in the session, which official lights and brings into
                    // view on every new query (`search-machine.ts` `queryChanged`, `seek(.., 0)`), found
                    // by scanning from the start, and the rows on screen; the worker counts the rest.
                    None => {
                        let mut rows = self.find_rows_on_screen(&current, &index);
                        let first = index.first_match(&q, fold, KEY_BUDGET);
                        rows.extend(first);
                        rows.sort_unstable();
                        rows.dedup();
                        Answer::Partial { query: q.clone(), results: Arc::new(index.search(&rows, &q, fold)), reveal: first.is_none() }
                    }
                },
                // The index is a change behind (or not made yet): what shows lights when it lands.
                _ => Answer::Partial { query: q.clone(), results: Arc::default(), reveal: true },
            };
            retire(std::mem::replace(&mut self.find.answer, answer));
            // A new query lights its first match (`queryChanged`), and only a match that is the
            // first in the session is lit before the rest are counted.
            // Find opened on a search result's message lights that message's first match once
            // every match is known.
            let first = match (&self.find.answer, &self.find.target) {
                (Answer::Partial { .. }, Target::Message(_)) | (Answer::Partial { reveal: true, .. }, _) => None,
                (Answer::Complete { results, .. }, Target::Message(id)) => results.first_in_message(id).or(Some(0)),
                _ => Some(0),
            };
            // The message isn't loaded yet: older pages are read (`poll`) and the target waits.
            if matches!(self.find.answer, Answer::Complete { .. }) && !self.find_target_waits() {
                self.find.target = Target::First;
            }
            let _ = step(&mut self.find.state, Event::MatchesSynced { kept: first });
        }
        // The index for this shape, once (keys never restart it).
        if !indexed && self.find.indexing != (Indexing::Running { shape }) {
            self.find.indexing = Indexing::Running { shape };
            let source = self.find.source.clone().unwrap_or_default();
            let prev = self.find.index.clone();
            self.find.indexer.request((source, prev), |(source, prev): (Arc<Source>, Option<Arc<Index>>)| Arc::new(Index::build(source, prev.as_deref())));
        }
        // Every match of the query, when a key couldn't match it all (or matched it on the last
        // shape), once the index for this shape is in.
        let stale = matches!(&self.find.answer, Answer::Complete { shape: s, .. } if *s != shape);
        let wanted = !q.is_empty() && (matches!(self.find.answer, Answer::Partial { .. }) || stale);
        let job = Matching::Running { shape, query: q.clone() };
        if let Some(index) = self.find.index.clone().filter(|_| indexed && wanted && self.find.matching != job) {
            self.find.matching = job;
            let fold = crate::language::search_fold();
            self.find.matcher.request((index, q), move |(index, q): (Arc<Index>, String)| {
                let rows = index.candidates(&q);
                let results = Arc::new(index.search(&rows, &q, fold));
                (index.source.shape, q, results)
            });
        }
    }

    /// The open transcript's shape, once its rows are copied into [`Find::source`]. A change is
    /// copied a slice a frame ([`COPY_BUDGET`] rows that changed), so opening Find on 100,000
    /// messages spreads its copy over a few frames; the shape before stays meanwhile.
    fn find_shape(&mut self, current: &str) -> Option<u64> {
        let t = self.transcripts.get(current)?;
        let revert = self.session(current).and_then(|i| i.revert.as_deref());
        let shown = t.visible_len(revert);
        // Outlines read as their words only while a plugin draws them (Rune UI).
        let outlines = crate::plugin_host::surfaces::code_block_lang(self, "ui");
        let live = t.busy() && !self.stops.has(current) && revert.is_none();
        let key = (current.to_owned(), rune::Tracked::version(&self.transcripts) << 2 | u64::from(live) << 1 | u64::from(outlines), shown, self.slack_links.version());
        if let Some((k, shape)) = &self.find.checked
            && *k == key
        {
            return Some(*shape);
        }
        let copying = std::mem::take(&mut self.find.copying);
        let copying = self.copy_rows(t, &key, copying, live, outlines);
        let before = self.find.checked.as_ref().map(|c| c.1).filter(|_| self.find.source.as_ref().is_some_and(|s| s.shape != 0));
        match copying {
            Copying::Done(source) => {
                let shape = source.shape;
                if self.find.source.as_ref().is_none_or(|s| s.shape != shape) {
                    retire(self.find.source.replace(Arc::new(source)));
                } else {
                    retire(source);
                }
                self.find.checked = Some((key, shape));
                Some(shape)
            }
            running => {
                self.find.copying = running;
                before
            }
        }
    }

    /// Copies the session's searchable rows (`u-{id}` and `t-{a.id}-{n}`, in timeline order; a
    /// reply still streaming keeps its last part even while empty), [`COPY_BUDGET`] rows a call.
    /// A copy under way goes on over the transcript as it is now, so one that changes every frame
    /// still lands; the next change starts another.
    fn copy_rows(&self, t: &Transcript, key: &CopyKey, copying: Copying, live: bool, outlines: bool) -> Copying {
        let shown = key.2;
        let (mut next, mut done, mut hash) = match copying {
            Copying::Running { next, done, hash } => (next, done, hash),
            _ => {
                let mut hash = DefaultHasher::new();
                (live, outlines).hash(&mut hash);
                (Source::new(0), 0, hash)
            }
        };
        let last_reply = live.then(|| t.messages[..shown].iter().rposition(|m| matches!(m, Message::Assistant(_)))).flatten()
            .filter(|&i| t.messages[i + 1..shown].iter().all(|m| matches!(m, Message::Assistant(_))));
        let push = |next: &mut Source, hash: &mut DefaultHasher, id: &str, n: u32, kind: Kind, body: &str| {
            let sig = str_sig(body);
            (id, n, kind as u8, sig).hash(hash);
            next.push(id, n, kind, body, sig);
        };
        let budget = next.len() + COPY_BUDGET;
        while done < shown && next.len() < budget {
            match &t.messages[done] {
                Message::Assistant(a) => {
                    let mut texts = 0u32;
                    for (pi, part) in a.parts.iter().enumerate() {
                        let Part::Text { text } = part else { continue };
                        let n = texts;
                        texts += 1;
                        if text.trim().is_empty() && !(last_reply == Some(done) && pi + 1 == a.parts.len()) {
                            continue;
                        }
                        let kind = if outlines && (text.contains("```ui") || text.contains("~~~ui")) { Kind::Outline } else { Kind::Reply };
                        push(&mut next, &mut hash, &a.id, n, kind, text);
                    }
                }
                Message::User { id, text } if !text.trim().is_empty() || !opencode_kit::model::user_comments(id).is_empty() => {
                    let shown = self.bubble_text(id, text);
                    push(&mut next, &mut hash, id, 0, Kind::Bubble, &shown);
                }
                _ => {}
            }
            done += 1;
        }
        if done < shown {
            return Copying::Running { next, done, hash };
        }
        next.len().hash(&mut hash);
        next.shape = hash.finish().max(1);
        Copying::Done(next)
    }

    /// Rows of `index` the timeline shows now (its last drawn frame).
    fn find_rows_on_screen(&self, session: &str, index: &Index) -> Vec<usize> {
        let mut rows: Vec<usize> = self.ends.recall.marks(session).visible().iter().filter_map(|k| index.row_of(k)).collect();
        rows.sort_unstable();
        rows.dedup();
        rows
    }

    /// Lands the worker's index and matches: true when what shows changed.
    fn land_find(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        if let Some(index) = self.find.indexer.take() {
            retire(self.find.index.replace(index));
            self.find.indexing = Indexing::Idle;
            changed = true;
        }
        let Some((shape, query, results)) = self.find.matcher.take() else { return changed };
        self.find.matching = Matching::Idle;
        if self.find.query() != query {
            return true;
        }
        let reveal = matches!(self.find.answer, Answer::Partial { reveal: true, .. });
        let lit = self.find.lit().map(|(h, local)| (h.key.clone(), local));
        retire(std::mem::replace(&mut self.find.answer, Answer::Complete { query, shape, results }));
        // A target whose message is still to load keeps waiting, lighting the first match meanwhile.
        let waits = self.find_target_waits();
        let target = if waits { Target::First } else { std::mem::take(&mut self.find.target) };
        let reveal = (reveal || target != Target::First) && !waits;
        let kept = match target {
            Target::Message(id) => self.find.results().and_then(|r| r.first_in_message(&id)).or(Some(0)),
            Target::First if reveal => Some(0),
            Target::First => lit.and_then(|(key, local)| self.find.results()?.place(&key, local)),
        };
        let _ = step(&mut self.find.state, Event::MatchesSynced { kept });
        if reveal && self.find.count() > 0 {
            self.seek_active(tx);
        }
        true
    }
}

/// Called from `find-input`'s `on_input_change`: updates matches and seeks the first hit in the
/// keystroke's own commit. Whitespace or case that leave the query as it was keep the lit match
/// (`query.changed`).
pub fn typed(s: &mut App, tx: &mut Tx) {
    // A query typed here lights its first match, wherever Find was opened from.
    s.find.target = Target::First;
    if s.find.answered() == Some(s.find.query()) {
        tx.instant();
        return;
    }
    let _ = step(&mut s.find.state, Event::QueryChanged);
    s.sync_find_index();
    if s.find.count() == 0 {
        tx.instant();
        return;
    }
    s.seek_active(tx);
}

pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let landed = s.land_find(tx);
    // Find opened on a message that isn't loaded: the page before the oldest loaded one is read,
    // as the reader's scroll to the top would, until the message is in or nothing is left.
    if s.find_target_waits()
        && let Route::Session { id, .. } = (*s.route).clone()
    {
        s.read_older(&id, tx);
    }
    if !s.find.is_open() {
        return landed;
    }
    let was = (s.find.checked.as_ref().map(|c| c.1), s.find.count(), s.find.counting());
    s.sync_find_index();
    // A copy under way goes on next frame.
    let copying = matches!(s.find.copying, Copying::Running { .. });
    landed || copying || !s.find.is_open() || (s.find.checked.as_ref().map(|c| c.1), s.find.count(), s.find.counting()) != was
}

/// Global Find shortcuts: ⌘F opens Find in a session or focuses session search on Home;
/// ⌘G and ⇧⌘G step while session Find is open.
pub fn shortcut(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.settings.open || s.opening.is_some() {
        return false;
    }
    if Bind::Find.matches_tx(ev, tx) {
        // The narrow session has no inline Find bar. Leave the editor as the input destination;
        // the explicit Find command remains available independently of this shortcut.
        if s.narrow() && matches!(*s.route, Route::Session { .. }) { return false; }
        return match *s.route {
            Route::Session { .. } => { s.open_find(tx); true }
            Route::Home => { s.focus_home_search(tx); true }
            Route::Draft(_) => false,
        };
    }
    let step = [(Bind::FindNext, 1), (Bind::FindPrev, -1)].into_iter().find(|(b, _)| b.matches_tx(ev, tx));
    if let Some((_, d)) = step.filter(|_| matches!(*s.route, Route::Session { .. }) && s.find.is_open()) {
        s.step_find(d, tx);
        return true;
    }
    false
}

/// Escape when Find is open (outside a busy composer's stop).
pub fn dismiss_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let take = ev.key == Key::Escape && s.find.is_open() && matches!(*s.route, Route::Session { .. });
    // Desktop leaves nothing focused once Escape closes the bar (`esc-find`).
    if take { s.close_find(tx); }
    take
}

fn input_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.find.input.is_composing() {
        return false;
    }
    if ev.key == Key::Escape {
        s.close_find(tx);
        return true;
    }
    let m = ev.mods;
    if !m.alt && !m.cmd && !m.ctrl {
        let step = match ev.key {
            Key::Enter => Some(if m.shift { -1 } else { 1 }),
            Key::Down if !m.shift => Some(1),
            Key::Up if !m.shift => Some(-1),
            _ => None,
        };
        if let Some(d) = step {
            s.step_find(d, tx);
            return true;
        }
    }
    s.switcher_key(ev, tx) || s.shortcut(ev, tx)
}

/// The session header's Find bar (`TimelineSearchBar`).
pub fn bar(s: &App, p: &Pal) -> Option<El> {
    if !s.find.is_open() {
        return None;
    }
    // `{activeDisplay}/{count}` while the field holds anything, a space included (`showCount`).
    let total = s.find.count();
    let count = (!s.find.input.text.is_empty()).then(|| {
        let shown = if total > 0 { s.find.active_index() + 1 } else { 0 };
        // Until the worker has matched the rows off screen, the total isn't known.
        let total = if s.find.counting() { "…".to_owned() } else { total.to_string() };
        text(format!("{shown}/{total}")).key("find-count").font(COUNT).color(p.muted)
    });
    // `text-input-v2`: its input elevation at rest, the focus outline (and no shadow) while focused.
    let bar = row().key("find-bar");
    let bar = if s.find.input.focused { bar.border(1.0, p.v2.border_focus) } else { bar.box_shadow(Pal::ring_out(p.border)) };
    Some(
        bar
            .transition(Transition::none())
            .h(28.0)
            .w(200.0)
            .pl(8.0)
            .pr(4.0)
            // The value (icon and field, 6 apart), the count and the close button, 8 apart.
            .gap(8.0)
            .center_y()
            .radius(6.0)
            .fill(p.bg_base)
            .child(row().key("find-value").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y().child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted)).child(
                session_view::style::field(&s.find.input, UI)
                    .key("find-input")
                    .spellcheck(s.settings.spellcheck)
                    .single_line()
                    .truncate_fade()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .h(20.0)
                    .color(p.text)
                    .placeholder(opencode_i18n::t!("session.search.placeholder"), p.faint)
                    .caret(p.text, p.selection).selection_style(p.field_selection)
                    .on_edit(|s: &mut App| &mut s.find.input)
                    .on_input_change(typed)
                    .on_input_key(input_key),
            ))
            .children(count)
            .child(
                row()
                    .key("find-close")
                    .role("button")
                    // Desktop names it after the field and gives it no tooltip (`search-bar.tsx`).
                    .label_static(opencode_i18n::t!("session.search.placeholder"))
                    .size(20.0, 20.0)
                    .center()
                    .radius(2.0)
                    .hover(p.hover)
                    .press(p.pressed)
                    .hover_motion(INSTANT, INSTANT)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.close_find(tx))
                    .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_muted)),
            ),
    )
}

/// Native macOS Edit menu items (`keys_first` so focused fields and `on_key` see them first),
/// with the keys their commands have now: a group after Select All, ahead of the items AppKit
/// adds to the menu's end (AutoFill, Dictation, Emoji & Symbols).
pub fn menu_items() -> Vec<MenuItem<App>> {
    let item = |title, id, key: Bind, f: fn(&mut App, &mut Tx)| {
        crate::menubar::keyed(MenuItem::new(title, f).id(id).menu("Edit").after("selectAll:"), key).keys_first()
    };
    [
        MenuItem::separator().menu("Edit").after("selectAll:"),
        item(opencode_i18n::t!("rune.menu.edit.find"), "Edit/Find", Bind::Find, |s, tx| match *s.route {
            Route::Session { .. } => s.open_find(tx),
            Route::Home => s.focus_home_search(tx),
            Route::Draft(_) => {}
        }),
        item(opencode_i18n::t!("rune.menu.edit.findNext"), "Edit/FindNext", Bind::FindNext, |s, tx| s.step_find(1, tx)),
        item(opencode_i18n::t!("rune.menu.edit.findPrevious"), "Edit/FindPrevious", Bind::FindPrev, |s, tx| s.step_find(-1, tx)),
    ]
    .into()
}
