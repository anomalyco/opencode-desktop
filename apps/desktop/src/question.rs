//! The question dock, drawn by `session_view::dock::question` (Desktop's
//! `session-question-dock.tsx`): this app's wrappers, its knob group and scenes.

use rune::{KeyEvent, Transition, Tx};

pub use session_view::dock::question::Dock;
#[cfg(test)]
use session_view::dock::question::{Intent, Zone, intent};
use session_view::dock::{self, question as q};

use crate::{App, knobs};

impl App {
    /// Whether the composer shows as the question dock.
    pub fn question_shown(&self) -> bool {
        q::asking(self).is_some()
    }

    /// The form the question dock shows.
    pub fn question_id(&self) -> Option<String> {
        q::id(self)
    }

    /// Whether the dock takes letters now: its typed answer is open or highlighted.
    pub fn question_takes_text(&self) -> bool {
        q::takes_text(self)
    }

    /// Opens the current question's typed answer with `text` in it (the storybook's typing
    /// stories).
    pub fn question_type(&mut self, text: &str) {
        q::type_text(self, text);
    }

    /// Goes to the last question (the storybook's Custom answer story).
    pub fn question_last(&mut self) {
        q::go_last(self);
    }

    /// Leaves the typed answer's field as Esc does (the storybook's Custom answer story).
    pub fn question_leave_field(&mut self) {
        q::leave_field(self);
    }

    /// A dock (a question or a permission) opened or closed: the surface springs to its new
    /// face, the timeline pinned above it rides its top edge, and the composer gets the keyboard
    /// back (a subagent's page leaves it with the page, as on opening).
    pub fn morph(&mut self, tx: &mut Tx) {
        dock::morph(self, tx);
    }

    /// A key while the dock shows; false when it isn't the dock's.
    pub fn question_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        q::key(self, ev, tx)
    }
}

/// How the composer's face and the question's swap inside the one surface.
pub fn face_transition(s: &App) -> Transition {
    dock::face_transition(&s.knobs)
}

rune_register::register!(knobs::GROUP_LIST: rune_knobs::Group = [knobs::QUESTION_DOCK]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("question", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let q = s.fixture["question"].clone();
        s.apply_event("form.created", &q);
    }),
    // Answered elsewhere at 1.5 s, asked again at 2.5 s: the morph both ways.
    ("question-morph", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let q = s.fixture["question"].clone();
        s.apply_event("form.created", &q);
        let replied = serde_json::json!({ "id": q["id"], "sessionID": q["sessionID"] });
        s.playback = [(1.5, "form.replied".into(), replied), (2.5, "form.created".into(), q)].into();
    }),
]);

pub(crate) mod long;

#[cfg(test)]
mod custom_tests;
#[cfg(test)]
mod tests;
