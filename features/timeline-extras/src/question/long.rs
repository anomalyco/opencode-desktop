//! A long question's fixture data and focus-walk helpers (p273): the dock keeps to part of
//! the pane (`session_view::dock::question`), its header and footer stay put, and the question
//! and its answers scroll between them; the keys bring the highlighted answer into sight.

use rune::{Key, KeyEvent, Mods, Tx};
use serde_json::{Value, json};

use super::QuestionHost;

/// Beat in seconds between arrow presses in the long-question walk story.
pub const WALK_BEAT_S: f64 = 0.32;

/// Three questions, the first a pick-any with nine long answers, anchored to `host`'s fixture.
pub fn form(host: &dyn QuestionHost) -> Value {
    let empty = Value::Null;
    form_from_fixture(host.fixture_question().unwrap_or(&empty))
}

/// Three questions, the first a pick-any with nine long answers, using `fixture`'s `id` and
/// `sessionID`.
pub fn form_from_fixture(fixture: &Value) -> Value {
    let options = |list: &[(&str, &str)]| {
        list.iter()
            .map(|(label, description)| json!({ "value": label, "label": label, "description": description }))
            .collect::<Vec<_>>()
    };
    json!({
        "id": fixture["id"],
        "sessionID": fixture["sessionID"],
        "title": "",
        "fields": [
            {
                "key": "q0",
                "type": "multiselect",
                "title": "Angles",
                "description": "Which angles go in the review bundle? Each is one read-only agent that writes its findings with a fix for each. Simplify and code organization are in already.",
                "custom": true,
                "options": options(ANGLES),
            },
            { "key": "q1", "type": "string", "title": "Depth", "description": "How deep should each agent go?", "custom": true, "options": options(&[("Changed files", "Only what this branch touches."), ("Whole module", "Every file in the modules the branch touches.")]) },
            { "key": "q2", "type": "string", "title": "Output", "description": "Where do the findings go?", "custom": false, "options": options(&[("One report", "A single page, grouped by angle."), ("Cards", "One card per finding.")]) },
        ],
    })
}

/// Presses `key` in the question dock; returns whether the highlighted answer moved.
pub fn press(host: &mut dyn QuestionHost, key: Key, tx: &mut Tx) -> bool {
    let before = host.question_dock().focus;
    host.question_key(&KeyEvent { key, mods: Mods::default() }, tx);
    host.question_dock().focus != before
}

/// Steps highlight down by one answer; returns whether it moved.
pub fn step_down(host: &mut dyn QuestionHost, tx: &mut Tx) -> bool {
    press(host, Key::Down, tx)
}

/// Steps highlight up by one answer; returns whether it moved.
pub fn step_up(host: &mut dyn QuestionHost, tx: &mut Tx) -> bool {
    press(host, Key::Up, tx)
}

const ANGLES: &[(&str, &str)] = &[
    ("Module design (Recommended)", "Shallow modules, leaky boundaries, where seams should go, what's hard to test."),
    ("Failure modes (Recommended)", "Swallowed errors, retries without limits, half-finished state after a crash, timeouts, what happens when a dependency is down."),
    ("Tests (Recommended)", "Untested behaviour, tests that only test mocks, slow or flaky tests, missing regression tests for past bugs."),
    ("Observability (Recommended)", "Can you answer \"why did it do that\" from logs or data? Silent decisions, missing timings, unstructured logs."),
    ("Agent docs drift (Recommended)", "AGENTS.md, READMEs and architecture docs that no longer match the code, contradictions, stale commands."),
    ("Naming and domain", "Same concept with two names, two concepts with one name, terms that don't match the domain."),
    ("Dependencies", "Unused or duplicated crates, heavy dependencies for small jobs, versions far behind."),
    ("Performance", "Work on the main thread, quadratic loops over growing lists, allocations in hot paths."),
    ("Security", "Secrets in logs, unchecked input reaching a shell or a path, permissions wider than needed."),
];
