//! The question dock (`session_view::dock::question`, Desktop's `session-question-dock.tsx`):
//! object-safe [`QuestionHost`] boundary, reply state machine ([`reply`]), and long-question
//! fixtures ([`long`]).

use rune::{KeyEvent, Transition, Tx};
use rune_knobs::Knobs;
use serde_json::{Value, json};

pub use session_view::dock::knobs::QUESTION_DOCK;
pub use session_view::dock::question::Dock;

pub mod long;
pub mod reply;

/// Object-safe host boundary for the question dock, its keyboard handling, morph transitions,
/// and story/scene helpers.
pub trait QuestionHost {
    fn question_dock(&self) -> &Dock;
    fn question_dock_mut(&mut self) -> &mut Dock;
    fn knobs(&self) -> &Knobs;

    /// Whether the composer currently displays as the question dock.
    fn question_shown(&self) -> bool;
    /// The active form id displayed in the question dock, if any.
    fn question_id(&self) -> Option<String>;
    /// Whether the question dock accepts character input (custom answer open or highlighted).
    fn question_takes_text(&self) -> bool;

    /// Opens the current question's typed answer with `text` in it.
    fn question_type(&mut self, text: &str);
    /// Navigates to the last question in the active form.
    fn question_last(&mut self);
    /// Leaves the typed answer field as Escape does.
    fn question_leave_field(&mut self);

    /// Runs the composer ⇄ dock morph spring and restores composer focus when the dock closes.
    fn dock_morph(&mut self, tx: &mut Tx);
    /// Handles a key event while the question dock is active; returns `false` if unhandled.
    fn question_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool;

    /// Optional fixture question JSON used by scenes and stories (`long::form`).
    fn fixture_question(&self) -> Option<&Value> {
        None
    }
}

/// Whether the composer shows as the question dock.
pub fn shown(host: &dyn QuestionHost) -> bool {
    host.question_shown()
}

/// The form the question dock shows.
pub fn id(host: &dyn QuestionHost) -> Option<String> {
    host.question_id()
}

/// Whether the dock takes letters now: its typed answer is open or highlighted.
pub fn takes_text(host: &dyn QuestionHost) -> bool {
    host.question_takes_text()
}

/// Opens the current question's typed answer with `text` in it.
pub fn type_text(host: &mut dyn QuestionHost, text: &str) {
    host.question_type(text);
}

/// Goes to the last question.
pub fn go_last(host: &mut dyn QuestionHost) {
    host.question_last();
}

/// Leaves the typed answer's field as Esc does.
pub fn leave_field(host: &mut dyn QuestionHost) {
    host.question_leave_field();
}

/// A dock (a question or a permission) opened or closed: springs to its new face, keeps the
/// timeline edge riding its top, and returns keyboard focus to the composer when appropriate.
pub fn morph(host: &mut dyn QuestionHost, tx: &mut Tx) {
    host.dock_morph(tx);
}

/// A key while the dock shows; false when it isn't the dock's.
pub fn key(host: &mut dyn QuestionHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    host.question_key(ev, tx)
}

/// How the composer's face and the question's swap inside the one surface.
pub fn face_transition(host: &dyn QuestionHost) -> Transition {
    session_view::dock::face_transition(host.knobs())
}

/// [`face_transition`] parameterized directly by `knobs`, usable without a full [`QuestionHost`].
pub fn face_transition_with_knobs(knobs: &Knobs) -> Transition {
    session_view::dock::face_transition(knobs)
}

/// Builds a `form.replied` playback event payload matching `question`'s `id` and `sessionID`.
pub fn scene_replied_payload(question: &Value) -> Value {
    json!({ "id": question["id"], "sessionID": question["sessionID"] })
}
