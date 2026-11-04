//! A long question keeps to part of the pane (p273): the dock is at most ⌘⇧K → Question dock →
//! Max height of the pane it sits in, so the timeline still shows above it. Its header (1 of 3,
//! the pager) and footer stay put, and the question and its answers scroll between them, fading
//! under each edge while they hide rows; the keys bring the highlighted answer into sight, just
//! far enough.

use rune::{El, Key, KeyEvent, Mods, Play, ScrollSpec, Tx, col};
use serde_json::{Value, json};

use super::{has_custom, rows};
use crate::stories::{Trigger, emit, replay};
use crate::{App, knobs};

/// The footer's height: its padding and the 32 pt buttons.
const FOOTER_H: f32 = 47.0;
/// Never shorter than this while the pane has room (Desktop's floor).
const MIN_H: f32 = 240.0;
/// How far the answers fade under the header and footer, and how clear of them a revealed one stops.
const FADE: f32 = 16.0;

/// The tallest the header and answers get, the footer under them taking the rest of the cap; None
/// on a stage with no pane to share (the composer stories).
pub(super) fn body_max(s: &App) -> Option<f32> {
    let pane = ["story-session", "panel"].into_iter().find_map(|k| rune::memo::layout_with(k, |r| r[3]))?;
    let cap = (pane * s.knobs.get(&knobs::QUESTION_MAX)).max(MIN_H).min(pane);
    Some((cap - FOOTER_H).max(0.0))
}

/// Question `tab`'s text and answers in their own viewport, so a new question starts at its top.
pub(super) fn scroller(tab: usize, page: El) -> El {
    col().key(format!("q-scroll-{tab}")).w_full().shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(FADE).scroll_margin(FADE).child(page)
}

impl App {
    /// Brings the highlighted answer, or "Type your own answer", into sight; on the first answer
    /// as much of the question above it as fits shows too (reveals run in order, each from
    /// where the last left the list).
    pub(super) fn reveal_focus(&self, tx: &mut Tx) {
        let Some(f) = self.asking() else { return };
        let (tab, focus) = self.place(&f);
        let field = &f.fields[tab];
        let scope = format!("q-scroll-{tab}");
        if focus == 0 {
            tx.scroll_into_view_in(scope.clone(), format!("q-details-{tab}"));
        }
        let key = if focus >= rows(field).len() && has_custom(field) { "q-custom".to_owned() } else { format!("q-opt-{focus}") };
        tx.scroll_into_view_in(scope, key);
    }
}

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
