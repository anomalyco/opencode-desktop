//! A long question's scene and story (p273): the dock keeps to part of the pane
//! (`session_view::dock::question`), its header and footer stay put, and the question and its
//! answers scroll between them; the keys bring the highlighted answer into sight.

use rune::{Key, KeyEvent, Mods, Play, Tx};
use serde_json::{Value, json};

use crate::stories::{Trigger, emit, replay};
use crate::{App, knobs};

/// Three questions, the first a pick-any with nine long answers.
pub fn form(s: &App) -> Value {
    let fixture = &s.fixture["question"];
    let options = |list: &[(&str, &str)]| list.iter().map(|(label, description)| json!({ "value": label, "label": label, "description": description })).collect::<Vec<_>>();
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

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("question-long", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let q = form(s);
        s.apply_event("form.created", &q);
    }),
]);

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Question dock/Long question", KEYS).knobs(&[knobs::QUESTION_DOCK]),
]);

const KEYS: &[Trigger] = &[replay(open), Trigger { key: 'w', label: "Walk the answers with ↓ and ↑", run: |s, tx| {
    tx.clear();
    walk(s, tx);
} }];

/// The agent asks, then ↓ walks down every answer and ↑ back up.
fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.3, |s, tx| {
        let q = form(s);
        emit(s, "form.created", q, tx);
    });
    tx.after(1.2, walk);
}

fn walk(_: &mut App, tx: &mut Play<App>) {
    tx.after(0.0, down);
}

const BEAT: f64 = 0.32;

fn down(s: &mut App, tx: &mut Play<App>) {
    if !press(s, Key::Down, tx) {
        tx.after(BEAT * 2.0, up);
    } else {
        tx.after(BEAT, down);
    }
}

fn up(s: &mut App, tx: &mut Play<App>) {
    if press(s, Key::Up, tx) {
        tx.after(BEAT, up);
    }
}

/// Presses `key` in the dock; whether the highlight moved.
fn press(s: &mut App, key: Key, tx: &mut Tx) -> bool {
    let before = s.question.focus;
    s.question_key(&KeyEvent { key, mods: Mods::default() }, tx);
    s.question.focus != before
}

#[cfg(test)]
#[path = "long_tests.rs"]
mod tests;
