//! Previous and Next message (⌥⌘[ and ⌥⌘], Ctrl+Alt+[ and ] elsewhere), as Desktop's
//! `navigateMessage` steps (`session/timeline/interaction.ts:89-120, 205-222`): from the prompt
//! whose turn crosses a line 100 below the timeline's top, to the prompt before or after it,
//! its top at the timeline's top (`cursor.ts:124-134`); past the last prompt the timeline
//! returns to its end and follows it again.

use std::collections::HashMap;
use std::time::Duration;

use rune::Tx;
use session_view::timeline::{RowContent, rows};

use opencode_kit::model::Message;

use crate::{App, Route};

/// Desktop reads the prompt in view at this line under the timeline's top (`promptInView`).
const LINE: f32 = 100.0;

/// Where the session's timeline is, and the prompt row a step is landing.
#[derive(Default)]
pub struct MessageNav {
    /// The timeline region's rect in window points, from its last layout.
    pub view: Option<[f32; 4]>,
    /// The prompt's row: revealed in the step's frame, then put at the top in the next, once a
    /// virtual row that wasn't built is laid out.
    landing: Option<String>,
}

impl App {
    /// Steps `offset` prompts from the one in view.
    pub fn navigate_message(&mut self, offset: i32, tx: &mut Tx) {
        let Route::Session(id) = &self.route else { return };
        let Some(sid) = self.displayed_session(id).map(str::to_owned) else { return };
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
        s.message_nav.landing = None;
        tx.scroll_to_bottom_instant(scroll);
        return;
    }
    let row = list[target as usize].1.clone();
    tx.scroll_into_view_in(scroll, row.clone());
    s.message_nav.landing = Some(row);
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

/// Puts a landing prompt at the timeline's top, the frame after its step revealed it.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let Some(row) = s.message_nav.landing.take() else { return false };
    // The row's own key: its content can carry the same key (a memo), so a path through the
    // timeline wouldn't name it.
    tx.scroll_key_to(row, 0.0);
    true
}
