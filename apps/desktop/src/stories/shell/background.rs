//! The storybook's Move to background story (Timeline › Move to background): a shell call
//! blocks the open run and "Move to background ⌃B" fades in beside Working. B moves it as ⌃B
//! does and S steers, which moves it too; either way the call returns and the hint fades out.
//! Variants: beside Waiting for shell, beside Working (a backgroundable Code Mode script), and
//! at the lane's start while the thought the model is writing stands in for the words.

use rune::{Play, TextEdit};
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, emit, replay, sid};

pub const VARIANTS: &[(&str, usize)] = &[("Waiting for shell", 0), ("Working", 1), ("Thinking", 2)];

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'b', label: "Move to background (⌃B)", run: |s, tx| { s.move_to_background(tx); } },
    Trigger { key: 's', label: "Steer", run: steer },
];

const REPLY: &str = "msg_zzzzstorybackground";

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id);
    emit(s, "session.execution.started", json!({ "assistantMessageID": REPLY }), tx);
    emit(s, "session.step.started", json!({ "assistantMessageID": REPLY, "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }), tx);
    match *s.fixture_run.variant {
        1 => script(s, tx),
        2 => {
            shell(s, tx);
            emit(s, "session.reasoning.started", json!({ "assistantMessageID": REPLY }), tx);
            emit(s, "session.reasoning.delta", json!({ "assistantMessageID": REPLY, "delta": "The suite takes a while; meanwhile the flake in the reconnect test looks like a timer race." }), tx);
        }
        _ => shell(s, tx),
    }
}

/// A Code Mode script that can move to the background, so the status line says Working.
fn script(s: &mut App, tx: &mut Play<App>) {
    let call = format!("call_story_script_{}", *s.clock as u64);
    emit(s, "session.tool.called", json!({ "assistantMessageID": REPLY, "id": call, "name": "execute", "input": { "code": "await tools.shell({ command: \"cargo test -p opencode-desktop\" })" } }), tx);
    emit(s, "session.tool.progress", json!({ "assistantMessageID": REPLY, "id": call, "metadata": { "background": true } }), tx);
}

fn shell(s: &mut App, tx: &mut Play<App>) {
    let call = format!("call_story_shell_{}", *s.clock as u64);
    emit(s, "session.tool.called", json!({ "assistantMessageID": REPLY, "id": call, "name": "shell", "input": { "command": "cargo test -p opencode-desktop" } }), tx);
}

fn steer(s: &mut App, tx: &mut Play<App>) {
    *s.composer = TextEdit::new("Stop waiting on the tests and fix the flake first");
    s.submit(false, tx);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Move to background", KEYS).variants(VARIANTS),
]);
