//! Answering a subagent from its own page.
use crate::pickers::PickerHost as _;
use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::client::{Incoming, Prompt, Write};
use opencode_kit::model::Message;
use rune::{Key, KeyEvent, Mods, Runtime};

use super::stories::open_child;
use crate::fixture_ids::{CHILD, MAIN};
use crate::{App, Route};

const DT: f64 = 1.0 / 60.0;

struct Live {
    rt: Runtime<App>,
    writes: Receiver<Write>,
    /// Kept open: the app's inbox is its other end.
    _server: Sender<Incoming>,
}

/// The fixture's finished reviewer open on a live source whose writes land in a channel.
fn live() -> Live {
    let mut app = App::fixture_session();
    open_child(&mut app);
    let writes = app.fake_writes();
    let (server, inbox) = channel();
    app.inbox = Some(inbox);
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    Live { rt, writes, _server: server }
}

fn send(l: &mut Live, text: &str) -> (String, Prompt) {
    l.rt.update(|s, tx| {
        s.composer = rune::TextEdit::new(text);
        s.send(tx);
    });
    l.rt.frame(DT);
    l.writes.try_iter().find_map(|w| match w {
        Write::SendMovingHome { session, prompt } => Some((session, prompt)),
        _ => None,
    }).expect("no reply sent")
}

fn echoed(l: &Live, text: &str) -> bool {
    l.rt.state.transcripts.get(CHILD).is_some_and(|t| t.messages.iter().any(|m| matches!(m, Message::User { text: t, .. } if t == text)))
}

fn tap(l: &mut Live, key: &str) {
    let r = l.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).rect;
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    l.rt.pointer_move(x, y);
    l.rt.pointer_down(x, y);
    l.rt.pointer_up(x, y);
    l.rt.frame(DT);
}

fn esc(l: &mut Live) {
    l.rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    l.rt.frame(DT);
}

#[test]
fn a_reply_goes_to_the_subagent_as_its_own_agent_and_model() {
    let mut l = live();
    let info = l.rt.state.session(CHILD).unwrap().clone();
    let (session, prompt) = send(&mut l, "also check the timeout path");
    assert_eq!(session, CHILD);
    assert_eq!(prompt.agent.as_deref(), Some(info.agent.as_str()));
    assert_eq!(prompt.model.as_ref().map(|m| m.id.as_str()), Some(info.model.id.as_str()));
    assert!(echoed(&l, "also check the timeout path"), "it shows on the press");
    assert_eq!(l.rt.state.route, Route::here(CHILD.into()), "and the page stays put");
}

#[test]
fn the_pickers_show_the_subagents_choice_not_the_main_sessions() {
    let l = live();
    let info = l.rt.state.session(CHILD).unwrap();
    let picked = l.rt.state.selection();
    assert_eq!((picked.agent.as_str(), picked.model.id.as_str()), (info.agent.as_str(), info.model.id.as_str()));
}

/// The words under the element keyed `key`.
fn under(l: &Live, key: &str) -> Vec<String> {
    let path = l.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).path;
    let mut under: Vec<String> = l
        .rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.path.starts_with(&path) && e.path != path)
        .filter_map(|e| e.text)
        .collect();
    under.sort();
    under
}

/// Under a subagent's composer: how the run went and the way back. The agent, model and
/// effort it ran with are in the pickers just above it, so the line doesn't say them again
/// (Kit, Nov 7: "some of this information is the same").
#[test]
fn the_bar_under_a_subagent_leaves_the_agent_and_model_to_the_pickers() {
    let mut l = live();
    l.rt.frame(DT);
    let info = l.rt.state.session(CHILD).unwrap().clone();
    let bar = under(&l, "subagent-bar");
    assert!(bar.iter().any(|t| t == "Done"), "the run's outcome: {bar:?}");
    let time = format!("bar-time-{CHILD}");
    assert!(l.rt.elements().into_iter().any(|e| !e.exiting && e.key.as_deref() == Some(&*time)), "and how long it took: {bar:?}");
    assert!(bar.iter().any(|t| t == "Main session"), "and the way back: {bar:?}");
    let agent = opencode_kit::model::capitalize(&info.agent);
    let model = l.rt.state.pickers.model_name(&info.model);
    let effort = opencode_kit::model::capitalize(&info.model.variant);
    for said in [agent.as_str(), model.as_str(), effort.as_str()] {
        assert!(!bar.iter().any(|t| t == said), "{said} isn't said twice: {bar:?}");
    }
    assert_eq!(under(&l, "agent"), vec![agent], "the picker still names the agent");
    assert!(under(&l, "model").contains(&model), "and the model");
}

#[test]
fn esc_goes_back_only_from_an_empty_composer() {
    let mut l = live();
    tap(&mut l, "prompt");
    assert!(l.rt.state.composer.focused);
    l.rt.update(|s, _| s.composer.replace_text("half a thought"));
    esc(&mut l);
    assert_eq!(l.rt.state.route, Route::here(CHILD.into()), "typed text keeps the page");
    l.rt.update(|s, _| s.composer.replace_text(""));
    esc(&mut l);
    assert_eq!(l.rt.state.route, Route::here(MAIN.into()));
}

#[test]
fn drafts_stay_with_their_own_session() {
    let mut l = live();
    l.rt.update(|s, _| s.show_draft());
    l.rt.update(|s, tx| {
        s.composer = rune::TextEdit::new("for the reviewer");
        s.open(MAIN, tx);
        s.show_draft();
    });
    l.rt.frame(DT);
    assert_eq!(l.rt.state.composer.text, "", "the reviewer's draft stays on its page");
    l.rt.update(|s, tx| {
        s.composer = rune::TextEdit::new("for the main session");
        s.open(CHILD, tx);
        s.show_draft();
    });
    l.rt.frame(DT);
    assert_eq!(l.rt.state.composer.text, "for the reviewer");
    l.rt.update(|s, tx| {
        s.open(MAIN, tx);
        s.show_draft();
    });
    l.rt.frame(DT);
    assert_eq!(l.rt.state.composer.text, "for the main session");
}

#[test]
fn a_main_session_prompt_sends_as_before() {
    let mut l = live();
    l.rt.update(|s, tx| s.open(MAIN, tx));
    l.rt.frame(DT);
    l.rt.update(|s, tx| {
        s.composer = rune::TextEdit::new("hi");
        s.send(tx);
    });
    assert!(l.writes.try_iter().any(|w| matches!(w, Write::Send { session, .. } if session == MAIN)));
}
