//! The storybook's Move to background story (Timeline › Move to background): a shell call
//! blocks the open run and "Move to background ⌃B" fades in beside Working. B moves it as ⌃B
//! does and S steers, which moves it too; either way the call returns and the hint fades out.

use rune::{Play, TextEdit};
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, emit, replay, sid};

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
    shell(s, tx);
}

fn shell(s: &mut App, tx: &mut Play<App>) {
    let call = format!("call_story_shell_{}", s.clock as u64);
    emit(s, "session.tool.called", json!({ "assistantMessageID": REPLY, "id": call, "name": "shell", "input": { "command": "cargo test -p opencode-desktop" } }), tx);
}

fn steer(s: &mut App, tx: &mut Play<App>) {
    s.composer = TextEdit::new("Stop waiting on the tests and fix the flake first");
    s.submit(false, tx);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Move to background", KEYS),
]);
