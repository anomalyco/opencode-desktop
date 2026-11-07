//! "Type your own answer": where its text sits, its keys, its dash, and gaps on submit.

use std::sync::mpsc::Receiver;

use opencode_kit::client::Write;
use rune::{ElementInfo, Key, KeyEvent, Mods, Runtime, TextEdit};
use serde_json::{Value, json};

use crate::App;

const FORM: &str = "frm_fx0001";
const LONG: &str = "Fake clock for the loop, but keep one slow end-to-end test on real timers so a regression in the scheduler still shows up somewhere";

fn ev(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn cmd(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { cmd: true, ..Mods::default() } }
}

fn txt(s: &str) -> KeyEvent {
    ev(Key::Text(s.into()))
}

/// The fixture's question, changed by `edit`, asked on the open session; replies go to a
/// channel, never a server.
fn asked_with(edit: impl FnOnce(&mut Value)) -> (Runtime<App>, Receiver<Write>) {
    let mut app = App::fixture_session();
    // These cover the keyboard-first dock; Classic is the default.
    app.settings.new_question_ui = true;
    app.composer = TextEdit::new("half a thought");
    let mut q = app.fixture["question"].clone();
    edit(&mut q);
    app.apply_event("form.created", &q);
    let r = app.fake_writes();
    let mut rt = app.into_runtime();
    rt.poll = None;
    settle(&mut rt);
    (rt, r)
}

fn asked() -> (Runtime<App>, Receiver<Write>) {
    asked_with(|_| {})
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
    }
}

fn press(rt: &mut Runtime<App>, e: KeyEvent) {
    rt.key(e);
    rt.frame(1.0 / 120.0);
}

fn type_in(rt: &mut Runtime<App>, s: &str) {
    for c in s.chars() {
        press(rt, txt(&c.to_string()));
    }
}

fn typed(rt: &Runtime<App>, key: &str) -> TextEdit {
    rt.state.question.typed.get(&format!("{FORM}/{key}")).cloned().unwrap_or_else(|| TextEdit::new(""))
}

fn editing(rt: &Runtime<App>) -> bool {
    typed(rt, &format!("q{}", rt.state.question.tab)).focused
}

/// The live element whose path has `within` and whose key is `key`.
fn el(rt: &Runtime<App>, within: &str, key: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key) && e.path.contains(within))
        .unwrap_or_else(|| panic!("{within} … {key}"))
}

/// The typed answer starts where every answer's title starts, in the same row inset, and
/// stays inside its row, wrapping onto more lines as the row grows, whether or not it's being
/// typed in.
#[test]
fn typed_text_sits_where_titles_do() {
    let (mut rt, _) = asked();
    let check = |rt: &Runtime<App>, what: &str, lines: f32| {
        let title = el(rt, "q-opt-1/", "label").rect;
        let row = el(rt, "q-options-0", "q-custom").rect;
        let option = el(rt, "q-options-0", "q-opt-1").rect;
        let input = el(rt, "q-custom", "q-input").rect;
        assert!((input[0] - title[0]).abs() < 0.5, "{what}: text at x {} vs the titles' {}", input[0], title[0]);
        assert_eq!((row[0], row[2]), (option[0], option[2]), "{what}: the same row as an answer");
        assert!(input[0] + input[2] <= row[0] + row[2] - 10.0 + 0.5, "{what}: ends inside the row's inset: {input:?} in {row:?}");
        assert!(input[1] >= row[1] + 9.5 && input[1] + input[3] <= row[1] + row[3] - 9.5, "{what}: inside the row: {input:?} in {row:?}");
        assert!((row[3] - (20.0 * lines + 20.0)).abs() < 0.5, "{what}: {lines} line(s) in a {} pt row", row[3]);
    };
    check(&rt, "placeholder", 1.0);
    press(&mut rt, txt("4"));
    type_in(&mut rt, "Both");
    settle(&mut rt);
    assert!(editing(&rt));
    check(&rt, "typing", 1.0);
    let edit = typed(&rt, "q0");
    rt.update(|s, _| {
        let t = s.typed_mut(FORM, "q0");
        *t = TextEdit::new(LONG);
        t.focused = edit.focused;
    });
    settle(&mut rt);
    check(&rt, "long", 2.0);
    press(&mut rt, ev(Key::Escape));
    settle(&mut rt);
    assert!(!editing(&rt));
    check(&rt, "long, left", 2.0);
    let fill = el(&rt, "q-options-0", "q-custom").fill;
    let opt = rt.state.question.focus;
    assert_eq!(opt, 3, "still highlighted");
    assert!(fill.a > 0.0, "with the answers' highlight");
}

/// ↑ on the field's first line goes to the answer above, ↓ comes back to it, and a key types
/// on where the text left off. On a wrapped answer the first ↑ moves up a line.
#[test]
fn up_and_down_leave_and_return() {
    let (mut rt, _) = asked();
    press(&mut rt, txt("4"));
    type_in(&mut rt, "Both");
    press(&mut rt, ev(Key::Up));
    assert!(!editing(&rt), "↑ leaves a one-line answer");
    assert_eq!(rt.state.question.focus, 2);
    assert_eq!(typed(&rt, "q0").text, "Both", "and keeps it");
    assert!(rt.state.question.custom_on(&format!("{FORM}/q0")), "still the answer");
    press(&mut rt, ev(Key::Down));
    assert_eq!(rt.state.question.focus, 3);
    press(&mut rt, txt("!"));
    assert!(editing(&rt));
    assert_eq!(typed(&rt, "q0").text, "Both!");

    rt.update(|s, _| {
        let t = s.typed_mut(FORM, "q0");
        *t = TextEdit::new(LONG);
        t.focused = true;
    });
    settle(&mut rt);
    press(&mut rt, ev(Key::Up));
    assert!(editing(&rt), "the caret on the second line moves up a line");
    assert!(typed(&rt, "q0").focus < LONG.len());
    press(&mut rt, ev(Key::Up));
    assert!(!editing(&rt), "and from the first line leaves");
    assert_eq!(rt.state.question.focus, 2);
}

/// In the field, digits type and ⌘A selects only the field's words.
#[test]
fn digits_type_and_select_all_stays_in_the_field() {
    let (mut rt, _) = asked();
    press(&mut rt, txt("4"));
    type_in(&mut rt, "Wait 2 or 3");
    assert_eq!(typed(&rt, "q0").text, "Wait 2 or 3");
    assert!(rt.state.question.picks.get(&format!("{FORM}/q0")).is_none_or(|p| p.is_empty()), "no answer picked");
    assert_eq!(rt.state.question.focus, 3);
    press(&mut rt, cmd(Key::Command('a')));
    let t = typed(&rt, "q0");
    assert_eq!((t.anchor.min(t.focus), t.anchor.max(t.focus)), (0, t.text.len()), "all the field's words");
    assert_eq!(rt.state.composer.anchor, rt.state.composer.focus, "nothing in the composer");
    assert_eq!(rt.state.composer.text, "half a thought");
    assert!(editing(&rt));
}

/// Esc leaves the field first and keeps its words; Esc again dismisses, as on Desktop.
#[test]
fn escape_leaves_the_field_then_dismisses() {
    let (mut rt, writes) = asked();
    press(&mut rt, txt("4"));
    type_in(&mut rt, "Both");
    press(&mut rt, ev(Key::Escape));
    assert!(!editing(&rt));
    assert!(writes.try_recv().is_err(), "the first Esc only leaves the field");
    assert_eq!(typed(&rt, "q0").text, "Both");
    press(&mut rt, ev(Key::Escape));
    assert!(matches!(writes.try_recv(), Ok(Write::CancelForm { .. })));
}

/// A typed answer fills its question's dash as a pick does.
#[test]
fn a_typed_answer_fills_its_dash() {
    let (mut rt, _) = asked();
    let accent = crate::palette::pal(true).bg_accent;
    press(&mut rt, txt("4"));
    type_in(&mut rt, "Both");
    press(&mut rt, ev(Key::Enter));
    settle(&mut rt);
    assert_eq!(rt.state.question.tab, 1);
    assert_eq!(el(&rt, "q-seg-0", "bar").fill, accent, "answered");
    assert_ne!(el(&rt, "q-seg-1", "bar").fill, accent, "the current one");
    // Emptied, it isn't.
    press(&mut rt, ev(Key::Left));
    press(&mut rt, txt("4"));
    for _ in 0..4 {
        press(&mut rt, ev(Key::Backspace));
    }
    press(&mut rt, ev(Key::Right));
    settle(&mut rt);
    assert_ne!(el(&rt, "q-seg-0", "bar").fill, accent);
}

/// ⌘↵ in the last question's field sends its words with the rest.
#[test]
fn the_last_questions_typed_answer_goes_in_the_reply() {
    let (mut rt, writes) = asked();
    press(&mut rt, txt("1"));
    press(&mut rt, ev(Key::Enter));
    assert_eq!(rt.state.question.tab, 1);
    press(&mut rt, txt("4"));
    type_in(&mut rt, "All of them");
    press(&mut rt, cmd(Key::Enter));
    let Ok(Write::Form { answer, .. }) = writes.try_recv() else { panic!("no reply") };
    assert_eq!(answer, json!({ "q0": "Fake clock", "q1": ["All of them"] }));
}

/// Submit with a required question unanswered goes to it and sends nothing; one that may be
/// skipped is left out of the reply, as Desktop does.
#[test]
fn submitting_with_gaps() {
    let required = |q: &mut Value| {
        for f in q["fields"].as_array_mut().unwrap() {
            f["required"] = true.into();
        }
    };
    let (mut rt, writes) = asked_with(required);
    press(&mut rt, ev(Key::Right));
    press(&mut rt, txt("2"));
    press(&mut rt, cmd(Key::Enter));
    assert!(writes.try_recv().is_err(), "nothing sent");
    assert_eq!(rt.state.question.tab, 0, "on the unanswered question");
    assert!(rt.state.question_shown());

    let (mut rt, writes) = asked();
    press(&mut rt, ev(Key::Right));
    press(&mut rt, txt("2"));
    press(&mut rt, cmd(Key::Enter));
    let Ok(Write::Form { answer, .. }) = writes.try_recv() else { panic!("no reply") };
    assert_eq!(answer, json!({ "q1": ["Cap at 30 s"] }));
}
