//! Previous and Next message (??[ and ??], Ctrl+Alt+[ and ] elsewhere), as Desktop's
//! `navigateMessage` steps (`session/timeline/interaction.ts:89-120, 205-222`): from the prompt
//! last stepped to while the reader hasn't scrolled since, else from the prompt whose turn
//! crosses a line 100 below the timeline's top, to the prompt before or after it, its top at the
//! timeline's top (`cursor.ts:124-134`); past the last prompt the timeline returns to its end and
//! follows it again.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use rune::Tx;
use session_view::timeline::{RowContent, rows};

use opencode_kit::model::Message;

use crate::{App, Route};

/// Desktop reads the prompt in view at this line under the timeline's top (`promptInView`).
const LINE: f32 = 100.0;

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
pub fn step(cursor: &mut Cursor, event: Event) -> rune::machine::Effects<Effect> {
    use rune::machine::Effects;
    match event {
        Event::Jumped { session, prompt, row } => {
            *cursor = Cursor::Selected { session: session.to_owned(), prompt: prompt.to_owned(), row: row.to_owned(), seek: Seek::Revealing };
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
            Cursor::Selected { session: s, prompt, seek: Seek::Revealing | Seek::Placing | Seek::Fresh { .. }, .. } if s == session => Some(prompt.clone()),
            _ => None,
        }
    }

    /// Records the open timeline's scroll offset (chained onto its viewport's watch).
    pub fn watch(&self) -> impl Fn(rune::ScrollPos) + 'static {
        let offset = self.offset.clone();
        move |at| offset.set(at.offset[1])
    }
}

impl App {
    /// Steps `offset` prompts from the one in view.
    pub fn navigate_message(&mut self, offset: i32, tx: &mut Tx) {
        let Route::Session { id, .. } = &self.route else { return };
        let Some(sid) = self.displayed_session(id).map(str::to_owned) else { return };
        if let Some(current) = self.message_nav.fresh(&sid) {
            return jump(self, &sid, Some(current), offset, tx);
        }
        let Some([x, y, w, _]) = self.message_nav.view else { return jump(self, &sid, None, offset, tx) };
        tx.element_at_in(format!("scroll-{sid}"), x + w / 2.0, y + LINE, move |s: &mut App, picked, tx| {
            let current = picked.and_then(|p| {
                let (_, turns) = prompts(s, &sid);
                p.path().split('/').find_map(|key| turns.get(key).cloned())
            });
            jump(s, &sid, current, offset, tx);
        });
    }
}

/// From `current` (the end when none), `offset` prompts on: a prompt, or the end one past the last.
fn jump(s: &mut App, sid: &str, current: Option<String>, offset: i32, tx: &mut Tx) {
    let (list, _) = prompts(s, sid);
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
        step(&mut s.message_nav.cursor, Event::Cleared);
        tx.scroll_to_bottom_instant(scroll);
        return;
    }
    let (prompt, row) = list[target as usize].clone();
    tx.scroll_into_view_in(scroll, row.clone());
    step(&mut s.message_nav.cursor, Event::Jumped { session: sid, prompt: &prompt, row: &row });
    tx.wake_after(Duration::ZERO);
}

/// The session's prompts top to bottom (Desktop's `visibleUserMessages`), each with the row a
/// step lands on (its images over its bubble, else the bubble), and each row's prompt: the one
/// whose turn it's in. Steers still waiting aren't prompts yet.
fn prompts(s: &App, sid: &str) -> (Vec<(String, String)>, HashMap<String, String>) {
    let Some(t) = s.transcripts.get(sid) else { return Default::default() };
    let mut list: Vec<(String, String)> = Vec::new();
    let mut turns = HashMap::new();
    let mut current: Option<String> = None;
    for r in rows(s, t) {
        let prompt = match &r.content {
            RowContent::User { message, .. } => match &t.messages[*message] {
                Message::User { id, .. } => Some(id.clone()),
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
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let Cursor::Selected { session, seek, .. } = &s.message_nav.cursor else { return false };
    let settled = matches!(seek, Seek::Fresh { .. } | Seek::Stale);
    if settled && (s.route != Route::here(session.clone()) || s.ends.at_end(session)) {
        step(&mut s.message_nav.cursor, Event::Cleared);
        return false;
    }
    let placing = matches!(seek, Seek::Revealing | Seek::Placing);
    let offset = s.message_nav.offset.get();
    // Under the header stuck over the timeline (`virtualizer.tsx:394-396`'s `scrollPaddingStart`,
    // which Rune's prompt row reaches with the 16 of turn gap it starts with).
    let top = if s.narrow() { 0.0 } else { session_view::style::HEADER_H };
    let mut changed = false;
    for effect in step(&mut s.message_nav.cursor, Event::Frame { offset }) {
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
