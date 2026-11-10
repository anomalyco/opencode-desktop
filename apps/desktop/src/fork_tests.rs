use opencode_kit::model::Message;
use rune::{Key, KeyEvent, Mods, Runtime};

use crate::{App, Route};

const DT: f64 = 1.0 / 60.0;

/// The fixture with 24 more prompts, its fork dialog open, so the list scrolls.
fn long_fork_dialog() -> Runtime<App> {
    let mut app = App::fixture_session();
    app.finish_stream();
    let Route::Session { id: sid, .. } = app.route.clone() else { panic!("no open session") };
    let t = app.transcripts.get_mut(&sid).expect("transcript");
    for i in 0..24 {
        t.messages.push(Message::User { id: format!("msg_more_{i:02}"), text: format!("Prompt number {i}") });
    }
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    rt.update(|s, tx| assert!(s.open_fork_dialog(tx)));
    rt.frame(DT);
    rt
}

fn key(rt: &mut Runtime<App>, k: Key) {
    rt.key(KeyEvent { key: k, mods: Mods::default() });
}

/// Which row is lit each frame for `n` frames.
fn lit(rt: &mut Runtime<App>, n: usize) -> Vec<Option<String>> {
    (0..n).map(|_| { rt.frame(DT); rt.state.fork.active.clone() }).collect()
}

#[test]
fn wrapping_to_the_first_row_under_a_parked_pointer_keeps_the_key_choice_lit() {
    let mut rt = long_fork_dialog();
    let list = rt.state.fork_messages();
    assert!(list.len() > 20, "{} rows", list.len());
    let first = list[0].id.clone();
    let (x, y) = rt.find(&format!("fork-row-{}", list[2].id)).expect("third row");
    rt.pointer_move(x, y);
    rt.frame(DT);
    assert_eq!(rt.state.fork.active.as_deref(), Some(list[2].id.as_str()), "the pointer lights its row");
    // ↑ to the top, then once more wraps to the last row and scrolls it into view.
    for _ in 0..3 {
        key(&mut rt, Key::Up);
    }
    let last = Some(list.last().unwrap().id.clone());
    assert!(lit(&mut rt, 10).iter().all(|a| *a == last), "↑ wraps to the last row and keeps it");
    // ↓ wraps to the first row, scrolling the list back to its top under the parked pointer.
    key(&mut rt, Key::Down);
    let first = Some(first);
    assert!(lit(&mut rt, 10).iter().all(|a| *a == first), "the list scrolling under the pointer doesn't take the row back");
}

