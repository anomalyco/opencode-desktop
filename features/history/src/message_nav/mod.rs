//! Previous and Next message (⌥⌘[ and ⌥⌘], Ctrl+Alt+[ and ] elsewhere), as Desktop's
//! `navigateMessage` steps (`session/timeline/interaction.ts:89-120, 205-222`): from the prompt
//! last stepped to while the reader hasn't scrolled since, else from the prompt whose turn
//! crosses a line 100 below the timeline's top, to the prompt before or after it, its top at the
//! timeline's top (`cursor.ts:124-134`); past the last prompt the timeline returns to its end and
//! follows it again.

pub mod jump;

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use opencode_kit::model::{Message, Transcript};
use rune::{KeyEvent, LiveRegion, ScrollPos, Tx};
use session_view::timeline::{Projection, RowContent, turn_of, turn_words};

pub use jump::{JUMP_OVERLAY_ID, JumpState};

/// Desktop reads the prompt in view at this line under the timeline's top (`promptInView`).
const LINE: f32 = 100.0;

/// Object-safe host boundary for session timeline message navigation (Previous/Next message
/// and the Jump to message dialog).
pub trait MessageNavHost: opencode_kernel::AppEnv + opencode_kernel::AppOverlays {
    fn message_nav(&self) -> &MessageNav;
    fn message_nav_mut(&mut self) -> &mut MessageNav;
    fn jump(&self) -> &JumpState;
    fn jump_mut(&mut self) -> &mut JumpState;

    /// Session ID whose timeline is currently displayed (may differ while opening a target
    /// session; defaults to [`opencode_kernel::AppEnv::active_session_id`]).
    fn displayed_session_id(&self) -> Option<&str> {
        self.active_session_id()
    }
    /// Whether `session` is currently the active route (`Route::Session { id }` == `session`).
    fn is_session_open(&self, session: &str) -> bool {
        self.active_session_id() == Some(session)
    }

    /// Loaded transcript for `session`, if any.
    fn transcript(&self, session: &str) -> Option<&Transcript>;
    /// Projected timeline rows for `session` (`session_view::timeline::rows`).
    fn timeline_rows(&self, session: &str) -> Option<Rc<Projection>>;
    /// Staged revert boundary message ID for `session`, if any (`SessionInfo::revert`).
    fn session_revert(&self, _session: &str) -> Option<&str> {
        None
    }

    /// Whether the timeline for `session` is currently scrolled to its end.
    fn timeline_at_end(&self, _session: &str) -> bool {
        false
    }
    /// Whether older messages remain unloaded above the loaded slice of `session`.
    fn more_above(&self, _session: &str) -> bool {
        false
    }
    /// Whether the first transcript page finished loading and no older pages remain above it.
    fn all_pages_loaded(&self, _session: &str) -> bool {
        true
    }
    /// Requests the next older page of messages for `session` toward a jump target.
    fn read_toward_place(&mut self, _session: &str, _tx: &mut Tx) {}

    /// Spawns off-thread history loading (`jump::fetch_history`) and delivers the resulting
    /// [`jump::HistoryReply`] via [`jump::apply_history_reply`]. Returns `false` if spawning failed.
    fn spawn_history_load(&mut self, _request: jump::HistoryRequest) -> bool {
        false
    }

    /// Announces `words` to assistive readers (e.g. the turn heading after stepping messages).
    fn announce(&mut self, _words: String, _region: LiveRegion) {}

    /// Whether the window is in its narrow/mobile layout (no sticky desktop session header).
    fn is_narrow(&self) -> bool {
        false
    }

    /// Prepares the shell before opening Jump to message (closes conflicting dialogs/pickers/menus,
    /// blurs the composer) and returns whether composer focus should be restored on dismiss.
    fn before_jump_open(&mut self) -> bool {
        false
    }
    /// Restores composer focus to `focused` when Jump to message closes with `restore_focus == true`.
    fn restore_composer_focus(&mut self, _focused: bool) {}
    /// Fallback handler for unhandled keys inside the Jump to message search field.
    fn handle_shortcut(&mut self, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
        false
    }
}

/// Where the session's timeline is, and the prompt a step chose.
#[derive(Default)]
pub struct MessageNav {
    /// The timeline region's rect in window points, from its last layout.
    pub view: Option<[f32; 4]>,
    /// The session page's rect, from its last layout: Desktop's timeline scroll view runs under
    /// the composer to the page's bottom, and Find centres a match in that (`find::seek_active`).
    pub page: Option<[f32; 4]>,
    pub cursor: Cursor,
    /// The open timeline's scroll offset, as its viewport last reported it.
    offset: Rc<Cell<f32>>,
}

/// The prompt a step chose: Desktop's `TimelineCursorState` (`session/timeline/cursor-machine.ts:
/// 15-50`) without the route hash's `applying` and `pending`, which Rune's routes don't carry.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Cursor {
    /// `none`.
    #[default]
    None,
    /// `selected`: prompt `prompt` of session `session`, whose row is `row`.
    Selected { session: String, prompt: String, row: String, seek: Seek },
}

/// A chosen prompt's seek and freshness (`selected`'s `seek` and `fresh`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seek {
    /// Revealed in the step's frame, put at the top in the next, once a virtual row that wasn't
    /// built is laid out (`seek.schedule`).
    Revealing,
    /// Put at the top; where it landed is read at the next poll (`seek.landed`).
    Placing,
    /// Landed at `offset`: the reader hasn't scrolled since (`fresh`), so a step goes on from it.
    Fresh { offset: f32 },
    /// The reader scrolled (`scroll.moved`): a step goes from the prompt in view.
    Stale,
}

#[derive(Clone, Debug)]
pub enum Event<'a> {
    /// `message.jump` to prompt `prompt` (row `row`) of session `session`.
    Jumped { session: &'a str, prompt: &'a str, row: &'a str },
    /// A frame after a step: the seek moves on; `offset` is the timeline's now.
    Frame { offset: f32 },
    /// `clear`: the timeline returned to its end and follows it.
    Cleared,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Put row `row` at the timeline's top (`seek.start`).
    Place(String),
}

/// `transitionTimelineCursor`, for the events Rune has.
pub fn step(cursor: &mut Cursor, event: Event<'_>) -> rune::machine::Effects<Effect> {
    use rune::machine::Effects;
    match event {
        Event::Jumped { session, prompt, row } => {
            *cursor = Cursor::Selected {
                session: session.to_owned(),
                prompt: prompt.to_owned(),
                row: row.to_owned(),
                seek: Seek::Revealing,
            };
            Effects::none()
        }
        Event::Cleared => {
            *cursor = Cursor::None;
            Effects::none()
        }
        Event::Frame { offset } => {
            let Cursor::Selected { row, seek, .. } = cursor else { return Effects::none() };
            match *seek {
                Seek::Revealing => {
                    *seek = Seek::Placing;
                    Effects::one(Effect::Place(row.clone()))
                }
                Seek::Placing => {
                    *seek = Seek::Fresh { offset };
                    Effects::none()
                }
                // Only a fresh cursor cares about the reader's scroll (`hasFreshCursorOrSeek`).
                Seek::Fresh { offset: at } if (offset - at).abs() > 0.5 => {
                    *seek = Seek::Stale;
                    Effects::none()
                }
                Seek::Fresh { .. } | Seek::Stale => Effects::none(),
            }
        }
    }
}

impl MessageNav {
    /// The prompt a step goes on from in session `session`: the one last stepped to while the
    /// reader hasn't scrolled since (`freshCursorMessageID`).
    fn fresh(&self, session: &str) -> Option<String> {
        match &self.cursor {
            Cursor::Selected {
                session: s,
                prompt,
                seek: Seek::Revealing | Seek::Placing | Seek::Fresh { .. },
                ..
            } if s == session => Some(prompt.clone()),
            _ => None,
        }
    }

    /// Records the open timeline's scroll offset (chained onto its viewport's watch).
    pub fn watch(&self) -> impl Fn(ScrollPos) + 'static {
        let offset = self.offset.clone();
        move |at| offset.set(at.offset[1])
    }
}

/// Steps `offset` prompts from the one in view.
///
/// Thin `H: MessageNavHost + 'static` wrapper only for `Tx::element_at_in`'s state downcast;
/// all navigation logic lives in non-generic [`prepare_navigate`] and [`navigate_picked`].
pub fn navigate<H: MessageNavHost + 'static>(host: &mut H, offset: i32, tx: &mut Tx) {
    if let Some((sid, x, y)) = prepare_navigate(host, offset, tx) {
        tx.element_at_in(format!("scroll-{sid}"), x, y, move |s: &mut H, picked, tx| {
            let path = picked.as_ref().map(|p| p.path());
            navigate_picked(s, &sid, path, offset, tx);
        });
    }
}

/// Resolves immediate navigation (fresh cursor or missing layout rect), or returns the
/// `(session_id, x, y)` hit-test probe needed to locate the prompt currently in view.
pub fn prepare_navigate(host: &mut dyn MessageNavHost, offset: i32, tx: &mut Tx) -> Option<(String, f32, f32)> {
    let sid = host.displayed_session_id()?.to_owned();
    if let Some(current) = host.message_nav().fresh(&sid) {
        jump_by(host, &sid, Some(current), offset, tx);
        return None;
    }
    let Some([x, y, w, _]) = host.message_nav().view else {
        jump_by(host, &sid, None, offset, tx);
        return None;
    };
    Some((sid, x + w / 2.0, y + LINE))
}

/// Completes a message step after `Tx::element_at_in` resolves the element path under the probe.
pub fn navigate_picked(host: &mut dyn MessageNavHost, sid: &str, picked_path: Option<&str>, offset: i32, tx: &mut Tx) {
    let current = picked_path.and_then(|path| {
        let (_, turns) = prompts(host, sid);
        path.split('/').find_map(|key| turns.get(key).cloned())
    });
    jump_by(host, sid, current, offset, tx);
}

/// Puts prompt `prompt` of session `sid` at the timeline's top, as a step to it would
/// (Jump to message's `cursor.jump(id, "auto")`). False when the timeline doesn't show it.
pub fn jump_to(host: &mut dyn MessageNavHost, sid: &str, prompt: &str, tx: &mut Tx) -> bool {
    if !prompts(host, sid).0.iter().any(|(id, _)| id == prompt) {
        return false;
    }
    jump_by(host, sid, Some(prompt.to_owned()), 0, tx);
    true
}

/// The ids of session `sid`'s prompts the timeline shows, top to bottom (`visibleUserMessages`).
pub fn prompt_ids(host: &dyn MessageNavHost, sid: &str) -> Vec<String> {
    prompts(host, sid).0.into_iter().map(|(id, _)| id).collect()
}

/// From `current` (the end when none), `offset` prompts on: a prompt, or the end one past the last.
fn jump_by(host: &mut dyn MessageNavHost, sid: &str, current: Option<String>, offset: i32, tx: &mut Tx) {
    let (list, _) = prompts(host, sid);
    if list.is_empty() {
        return;
    }
    let end = list.len() as i32;
    let base = current.and_then(|c| list.iter().position(|(id, _)| *id == c)).map_or(end, |i| i as i32);
    let target = base + offset;
    if !(0..=end).contains(&target) {
        return;
    }
    tx.instant();
    let scroll = format!("scroll-{sid}");
    if target == end {
        drop(step(&mut host.message_nav_mut().cursor, Event::Cleared));
        tx.scroll_to_bottom_instant(scroll);
        return;
    }
    let (prompt, row) = list[target as usize].clone();
    tx.scroll_into_view_in(scroll, row.clone());
    // A reader hears where the step landed, the turn's heading's words: the row is mounted by
    // this scroll, its heading in the tree when the reader looks.
    let more = host.more_above(sid);
    let words = host.transcript(sid).and_then(|t| {
        t.messages.iter().enumerate().find_map(|(i, m)| match m {
            Message::User { id, text } if *id == prompt => Some(turn_words(turn_of(t, i), more, text)),
            _ => None,
        })
    });
    if let Some(words) = words {
        host.announce(words, LiveRegion::Polite);
    }
    drop(step(&mut host.message_nav_mut().cursor, Event::Jumped { session: sid, prompt: &prompt, row: &row }));
    tx.wake_after(Duration::ZERO);
}

/// The session's prompts top to bottom (Desktop's `visibleUserMessages`), each with the row a
/// step lands on (its images over its bubble, else the bubble), and each row's prompt: the one
/// whose turn it's in. Steers still waiting aren't prompts yet.
fn prompts(host: &dyn MessageNavHost, sid: &str) -> (Vec<(String, String)>, HashMap<String, String>) {
    let Some(t) = host.transcript(sid) else { return Default::default() };
    let Some(rows) = host.timeline_rows(sid) else { return Default::default() };
    let mut list: Vec<(String, String)> = Vec::new();
    let mut turns = HashMap::new();
    let mut current: Option<String> = None;
    for r in rows.iter() {
        let prompt = match &r.content {
            RowContent::User { message, .. } => match t.messages.get(*message) {
                Some(Message::User { id, .. }) => Some(id.clone()),
                _ => None,
            },
            RowContent::UserImages { id, .. } if t.messages.iter().any(|m| matches!(m, Message::User { id: i, .. } if i == id)) => Some(id.clone()),
            RowContent::Pending { .. } => {
                current = None;
                None
            }
            _ => None,
        };
        if let Some(id) = prompt {
            if list.last().is_none_or(|(last, _)| *last != id) {
                list.push((id.clone(), r.key.clone()));
            }
            current = Some(id);
        }
        if let Some(id) = &current {
            turns.insert(r.key.clone(), id.clone());
        }
    }
    (list, turns)
}

/// Moves a step's seek on: puts a revealed prompt at the timeline's top the frame after its
/// step, notes where it landed the frame after that, and marks the cursor stale once the
/// timeline moves from there. A timeline back at its end follows it, with no prompt chosen.
pub fn poll(host: &mut dyn MessageNavHost, tx: &mut Tx) -> bool {
    let (session, settled, placing) = match &host.message_nav().cursor {
        Cursor::Selected { session, seek, .. } => (
            session.clone(),
            matches!(seek, Seek::Fresh { .. } | Seek::Stale),
            matches!(seek, Seek::Revealing | Seek::Placing),
        ),
        Cursor::None => return false,
    };
    if settled && (!host.is_session_open(&session) || host.timeline_at_end(&session)) {
        drop(step(&mut host.message_nav_mut().cursor, Event::Cleared));
        return false;
    }
    let offset = host.message_nav().offset.get();
    // Under the header stuck over the timeline (`virtualizer.tsx:394-396`'s `scrollPaddingStart`,
    // which Rune's prompt row reaches with the 16 of turn gap it starts with).
    let top = if host.is_narrow() { 0.0 } else { session_view::style::HEADER_H };
    let effects = step(&mut host.message_nav_mut().cursor, Event::Frame { offset });
    let mut changed = false;
    for effect in effects {
        let Effect::Place(row) = effect;
        // The row's own key: its content can carry the same key (a memo), so a path through
        // the timeline wouldn't name it.
        tx.scroll_key_to(row, top);
        changed = true;
    }
    if placing {
        tx.wake_after(Duration::ZERO);
    }
    changed
}
