//! Find in a session (⌘F): Desktop's `TimelineSearchBar` and `createTimelineSearchController`
//! (`session/timeline/search-bar.tsx`, `search-controller.ts`).
//!
//! Matches are indexed across the whole visible transcript (mounted or not), cached per message
//! so streaming replies only re-scan the changed part, and hashed per row so `rune::memo` only
//! rebuilds mounted rows whose highlights or active match actually changed.
//!
//! All crate entrypoints operate on non-generic `&dyn FindHost` / `&mut dyn FindHost`, keeping
//! thin `H: FindHost + 'static` wrappers strictly at `rune::El` / `MenuItem` callback boundaries
//! in [`view`].

#![forbid(unsafe_code)]

pub mod machine;
pub mod matches;
pub mod scan;
pub mod transcript_index;
pub mod view;

use std::borrow::Cow;
use std::sync::Arc;

use opencode_kit::model::Transcript;
use opencode_ui::keymap::Bind;
use rune::{Key, KeyEvent, Latest, TextEdit, Tx, Waker};
use session_view::disclosure::Disclosures;

pub use machine::{Effect, Event, State, transition};
pub use matches::{ACTIVE_KEY, markdown_mark, seek_active, text_mark, unfold_if_hidden};
use scan::{Answer, CopyKey, Copying, Indexing, Matching, Target};
pub use scan::sync_index;
pub use transcript_index::{Index, KEY_BUDGET, Kind, Results, RowHit, Source, TranscriptIndex, retire, row_hash};
pub use view::{bar, menu_items};

/// Object-safe host boundary for session timeline Find (`⌘F` / `Ctrl+F`).
pub trait FindHost {
    fn find(&self) -> &Find;
    fn find_mut(&mut self) -> &mut Find;

    /// Active session ID when viewing a session timeline (`None` on Home/Draft).
    fn active_session_id(&self) -> Option<&str>;
    /// Whether the window is currently on Home (where `⌘F` / Edit › Find focuses Home search).
    fn is_home(&self) -> bool {
        false
    }

    /// Loaded transcript for `session`, if any.
    fn transcript(&self, session: &str) -> Option<&Transcript>;
    /// Monotonic version that changes whenever `session`'s transcript changes.
    fn transcript_version(&self, session: &str) -> u64;
    /// Revert point message ID for `session`, if any (`SessionInfo::revert`).
    fn session_revert(&self, _session: &str) -> Option<&str> {
        None
    }
    /// Whether `session`'s active run has been locally stopped by the user.
    fn is_session_stopped(&self, _session: &str) -> bool {
        false
    }
    /// Whether a plugin renders fenced code blocks of `language` (e.g. `"ui"` for Rune UI outlines).
    fn code_block_lang(&self, language: &str) -> bool {
        language == "ui"
    }

    /// Prompt text as shown in user bubble `id` (e.g. with `@` mention tokens formatted).
    fn bubble_text<'a>(&self, _id: &str, body: &'a str) -> Cow<'a, str> {
        Cow::Borrowed(body)
    }

    /// Timeline disclosure states (used to unfold long user prompts hiding an active match).
    fn expanded(&self) -> &Disclosures;
    fn expanded_mut(&mut self) -> &mut Disclosures;
    /// Whether long user prompts are folded in the timeline.
    fn fold_user_messages(&self) -> bool {
        true
    }

    /// Mutable access to the composer's focused flag (cleared when Find opens and focuses its input).
    fn composer_focused_mut(&mut self) -> &mut bool;

    /// Keys of timeline rows currently visible on screen for `session` (from its last drawn frame).
    fn visible_timeline_rows(&self, _session: &str) -> Vec<String> {
        Vec::new()
    }
    /// Last laid-out timeline viewport rect `[x, y, w, h]` in window points.
    fn timeline_view_rect(&self) -> Option<[f32; 4]> {
        None
    }
    /// Last laid-out session page rect `[x, y, w, h]` in window points.
    fn timeline_page_rect(&self) -> Option<[f32; 4]> {
        None
    }
    /// Whether the window is in its narrow/mobile layout.
    fn is_narrow(&self) -> bool {
        false
    }

    /// Whether `session` may still have older unloaded transcript pages above the loaded slice.
    fn has_unloaded_older_messages(&self, _session: &str) -> bool {
        false
    }
    /// Requests the next older page of messages for `session`.
    fn read_older_messages(&mut self, _session: &str, _tx: &mut Tx) {}

    /// Whether the Settings overlay is currently open (suspends Find shortcuts and index syncing).
    fn settings_open(&self) -> bool {
        false
    }
    /// Whether a session is still opening (suspends Find shortcuts).
    fn session_opening(&self) -> bool {
        false
    }
    /// Closes any open inline picker before Find opens.
    fn close_picker(&mut self) {}
    /// Focuses Home's session search input (`⌘F` / Edit › Find on Home).
    fn focus_home_search(&mut self, _tx: &mut Tx) {}
    /// Fallback handler for unhandled keys inside the Find input (e.g. switcher & global shortcuts).
    fn handle_input_fallback_key(&mut self, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
        false
    }

    /// Whether spellcheck is enabled on text inputs.
    fn spellcheck(&self) -> bool {
        false
    }
    /// Current UI zoom scale.
    fn zoom(&self) -> f32 {
        1.0
    }
    /// Optional window waker for background index/match workers.
    fn waker(&self) -> Option<Waker> {
        None
    }
    /// BCP-47 language tag used for locale-aware search folding (e.g. Turkish/Azerbaijani `I`).
    fn search_fold_language(&self) -> &str {
        opencode_i18n::locale().tag()
    }
}

#[derive(Default)]
pub struct Find {
    pub state: State,
    pub input: TextEdit,

    /// The open session's transcript as last checked, and its shape.
    pub(crate) checked: Option<(CopyKey, u64)>,
    pub(crate) copying: Copying,
    /// Its rows, copied when its shape changes while Find is open, and their index.
    pub(crate) source: Option<Arc<Source>>,
    pub(crate) index: Option<Arc<Index>>,
    pub(crate) answer: Answer,
    pub(crate) indexing: Indexing,
    pub(crate) indexer: Latest<Arc<Index>>,
    pub(crate) matching: Matching,
    pub(crate) matcher: Latest<(u64, String, Arc<Results>)>,
    pub(crate) target: Target,
}

impl Find {
    pub fn set_waker(&mut self, waker: Waker) {
        self.indexer.set_waker(waker.clone());
        self.matcher.set_waker(waker);
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, State::Open { .. })
    }

    pub fn is_focused(&self) -> bool {
        self.is_open() && self.input.focused
    }

    /// A closed Find holds nothing: its index is made again (on the worker) when it opens.
    pub(crate) fn release(&mut self) {
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
}

/// Connects the window waker to Find's background index and match workers.
pub fn setup(s: &mut dyn FindHost, waker: Waker) {
    s.find_mut().set_waker(waker);
}

/// Whether the Find bar is open and its input field holds keyboard focus.
pub fn is_focused(s: &dyn FindHost) -> bool {
    s.find().is_focused()
}

/// Steps Find's machine and carries out what it asks.
fn apply_event(s: &mut dyn FindHost, event: Event<'_>, tx: &mut Tx) {
    let effects = {
        let f = s.find_mut();
        let effects = f.state.step(event);
        if !f.is_open() {
            f.input.clear();
            f.release();
        }
        effects
    };
    for effect in effects {
        match effect {
            Effect::FocusInput => {
                *s.composer_focused_mut() = false;
                let input = &mut s.find_mut().input;
                input.focused = true;
                let len = input.text.len();
                input.set_selection(0, len);
            }
            // Desktop leaves nothing focused once the bar closes, by Escape or its button
            // (`blur-input`; `esc-find`).
            Effect::BlurInput => {
                let input = &mut s.find_mut().input;
                input.focused = false;
                input.cancel_composition();
                // The bar's field leaves the tree: nothing takes its focus, not the next control.
                tx.blur();
            }
            Effect::Reveal => seek_active(s, tx),
        }
    }
}

/// Opens Find in the active session (or focuses and selects its query when already open).
pub fn open(s: &mut dyn FindHost, tx: &mut Tx) {
    let Some(id) = s.active_session_id().map(str::to_owned) else { return };
    s.close_picker();
    sync_index(s);
    apply_event(s, Event::Open { session: &id }, tx);
    sync_index(s);
    tx.instant();
}

/// Opens Find on `query` with message `message`'s first match lit and in view once the
/// matches are known: a search result's jump to what matched.
pub fn open_at(s: &mut dyn FindHost, query: &str, message: &str, tx: &mut Tx) {
    open(s, tx);
    s.find_mut().input.replace_text(query.to_owned());
    typed(s, tx);
    let at = {
        let f = s.find_mut();
        f.target = Target::Message(message.to_owned());
        // An answer complete on the key lights the message now.
        f.results().and_then(|r| r.first_in_message(message))
    };
    if let Some(at) = at {
        let f = s.find_mut();
        f.target = Target::First;
        drop(f.state.step(Event::MatchesSynced { kept: Some(at) }));
        seek_active(s, tx);
    }
}

/// Edit › Find menu action: opens session Find on a session route, or focuses Home search on Home.
pub fn open_from_menu(s: &mut dyn FindHost, tx: &mut Tx) {
    if s.active_session_id().is_some() {
        open(s, tx);
    } else if s.is_home() {
        s.focus_home_search(tx);
    }
}

/// Closes Find, clearing the query and highlights; nothing keeps the focus.
pub fn close(s: &mut dyn FindHost, tx: &mut Tx) {
    if !s.find().is_open() {
        return;
    }
    // The bar goes as it came, at once, and the header's crumbs take its room back with it.
    tx.instant();
    apply_event(s, Event::Close, tx);
}

/// Steps forward (`1`) or backward (`-1`) through the session's matches, wrapping around.
pub fn step(s: &mut dyn FindHost, delta: i32, tx: &mut Tx) {
    sync_index(s);
    let count = s.find().count();
    apply_event(s, Event::Move { delta, count }, tx);
}

/// Called from `find-input`'s `on_input_change`: updates matches and seeks the first hit in the
/// keystroke's own commit. Whitespace or case that leave the query as it was keep the lit match
/// (`query.changed`).
pub fn typed(s: &mut dyn FindHost, tx: &mut Tx) {
    // A query typed here lights its first match, wherever Find was opened from.
    let same = {
        let f = s.find_mut();
        f.target = Target::First;
        f.answered() == Some(f.query())
    };
    if same {
        tx.instant();
        return;
    }
    drop(s.find_mut().state.step(Event::QueryChanged));
    sync_index(s);
    if s.find().count() == 0 {
        tx.instant();
        return;
    }
    seek_active(s, tx);
}

/// Polls Find's background index/match workers, older-page target reads, and incremental row copy.
pub fn poll(s: &mut dyn FindHost, tx: &mut Tx) -> bool {
    let landed = scan::land(s, tx);
    // Find opened on a message that isn't loaded: the page before the oldest loaded one is read,
    // as the reader's scroll to the top would, until the message is in or nothing is left.
    if scan::target_waits(s)
        && let Some(id) = s.active_session_id().map(str::to_owned)
    {
        s.read_older_messages(&id, tx);
    }
    if !s.find().is_open() {
        return landed;
    }
    let was = {
        let f = s.find();
        (f.checked.as_ref().map(|c| c.1), f.count(), f.counting())
    };
    sync_index(s);
    let f = s.find();
    // A copy under way goes on next frame.
    let copying = matches!(f.copying, Copying::Running { .. });
    landed || copying || !f.is_open() || (f.checked.as_ref().map(|c| c.1), f.count(), f.counting()) != was
}

/// Global Find shortcuts: ⌘F opens Find in a session or focuses session search on Home;
/// ⌘G and ⇧⌘G step while session Find is open.
pub fn shortcut(s: &mut dyn FindHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.settings_open() || s.session_opening() {
        return false;
    }
    if Bind::Find.matches_tx(ev, tx) {
        // The narrow session has no inline Find bar. Leave the editor as the input destination;
        // the explicit Find command remains available independently of this shortcut.
        if s.is_narrow() && s.active_session_id().is_some() {
            return false;
        }
        if s.active_session_id().is_some() {
            open(s, tx);
            return true;
        }
        if s.is_home() {
            s.focus_home_search(tx);
            return true;
        }
        return false;
    }
    let delta = [(Bind::FindNext, 1), (Bind::FindPrev, -1)].into_iter().find(|(b, _)| b.matches_tx(ev, tx));
    if let Some((_, d)) = delta.filter(|_| s.active_session_id().is_some() && s.find().is_open()) {
        step(s, d, tx);
        return true;
    }
    false
}

/// Escape when Find is open (outside a busy composer's stop).
pub fn dismiss_key(s: &mut dyn FindHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let take = ev.key == Key::Escape && s.find().is_open() && s.active_session_id().is_some();
    // Desktop leaves nothing focused once Escape closes the bar (`esc-find`).
    if take {
        close(s, tx);
    }
    take
}

/// Key handler for `find-input`.
pub fn input_key(s: &mut dyn FindHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.find().input.is_composing() {
        return false;
    }
    if ev.key == Key::Escape {
        close(s, tx);
        return true;
    }
    let m = ev.mods;
    if !m.alt && !m.cmd && !m.ctrl {
        let delta = match ev.key {
            Key::Enter => Some(if m.shift { -1 } else { 1 }),
            Key::Down if !m.shift => Some(1),
            Key::Up if !m.shift => Some(-1),
            _ => None,
        };
        if let Some(d) = delta {
            step(s, d, tx);
            return true;
        }
    }
    s.handle_input_fallback_key(ev, tx)
}
