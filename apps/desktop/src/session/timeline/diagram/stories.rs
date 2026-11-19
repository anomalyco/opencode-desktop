//! A reply with a sequence diagram too wide for the column, for the storybook, the fixture and
//! the motion gate.
//!
//! `--scene diagram-wide` puts it in the fixture session. The story (Timeline › Diagram full
//! size) adds it again on R; O opens it, Z pinches in, P pans, 0 fits it, C closes, and A plays
//! the lot.

use opencode_kit::model::Message;
use rune::{Key, KeyEvent, Mods, Play};

use crate::App;
use crate::genui::demo::{finish, stream};
use crate::stories::{Story, Trigger, replay, sid};

pub const ASK: &str = "Walk me through a checkout request, end to end.";

/// Seven lanes with long names: far wider than the column.
pub const WIDE: &str = "sequenceDiagram
  autonumber
  participant B as Browser on a laptop
  participant E as Edge cache in front
  participant A as Auth service cluster
  participant P as Payments API server
  participant W as Billing worker pool
  participant S as Search index shard
  participant L as Audit log
  B->>E: a request arrives at the edge
  E->>A: is this session token still valid?
  A-->>E: yes, here is the account scope
  E->>P: forward the call with its scope
  P->>W: enqueue the invoice with the scope
  W->>S: index the invoice
  S-->>W: ok
  W->>L: record the change
  W-->>P: done
  P-->>E: the response
  E-->>B: render the receipt
";

pub fn reply() -> String {
    format!("Here's the whole path, from the browser to the receipt:\n\n```mermaid\n{WIDE}```\n\nOnly the billing workers write to the audit log.\n")
}

const ASK_ID: &str = "msg_zzwide0";
const REPLY_ID: &str = "msg_zzwide1";

/// Adds the question and the finished reply to the open session.
pub fn add(s: &mut App) {
    s.finish_stream();
    let id = sid(s);
    if let Some(t) = s.transcripts.get_mut(&id) {
        t.messages.push(Message::User { id: ASK_ID.into(), text: ASK.into() });
    }
    stream(s, REPLY_ID, &reply(), 0.0);
    while let Some((_, ty, data)) = s.fixture_run.playback.pop_front() {
        s.apply_event(&ty, &data);
    }
}

fn again(s: &mut App, tx: &mut Play<App>) {
    s.timeline_fx.diagram = Default::default();
    finish(s, tx);
    add(s);
}

fn open(s: &mut App, tx: &mut Play<App>) {
    super::open_source(s, WIDE, tx);
}

/// A pinch in about the window's middle, as the fingers report it: small steps for half a second.
fn pinch(_: &mut App, tx: &mut Play<App>) {
    for i in 0..30 {
        tx.after(i as f64 / 60.0, |s, tx| {
            let [w, h] = s.timeline_fx.diagram.fit.get().win;
            super::lightbox::pinch(s, 1.03, w * 0.6, h * 0.45);
            tx.instant();
        });
    }
}

/// Two fingers dragging the diagram left and up for half a second.
fn pan(_: &mut App, tx: &mut Play<App>) {
    for i in 0..30 {
        tx.after(i as f64 / 60.0, |s, tx| {
            super::lightbox::pan(s, -9.0, -3.0);
            tx.instant();
        });
    }
}

fn press(tx: &mut Play<App>, key: Key) {
    let cmd = matches!(key, Key::Command(_));
    tx.press(KeyEvent { key, mods: Mods { cmd, ..Default::default() } });
}

fn fit(_: &mut App, tx: &mut Play<App>) {
    press(tx, Key::Command('0'));
}

fn close(_: &mut App, tx: &mut Play<App>) {
    press(tx, Key::Escape);
}

/// Open, pinch in, pan, fit, close, with time to see each.
fn all(s: &mut App, tx: &mut Play<App>) {
    again(s, tx);
    tx.after(0.8, open);
    tx.after(1.8, pinch);
    tx.after(2.8, pan);
    tx.after(3.8, fit);
    tx.after(4.8, close);
}

pub const KEYS: &[Trigger] = &[
    replay(again),
    Trigger { key: 'o', label: "Open full size", run: open },
    Trigger { key: 'z', label: "Pinch in", run: pinch },
    Trigger { key: 'p', label: "Pan", run: pan },
    Trigger { key: '0', label: "Fit (⌘0)", run: fit },
    Trigger { key: 'c', label: "Close (Esc)", run: close },
    Trigger { key: 'a', label: "Open, pinch, pan, fit, close", run: all },
];

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::session("Timeline/Diagram full size", KEYS).knobs(crate::stories::ROW_KNOBS),
]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("diagram-wide", |s| {
        crate::open_scene(s);
        add(s);
    }),
]);
