//! Demo mode: a scripted agent turn that plays in the real app from the fixture, the same every
//! time, with nothing sent anywhere (`--scene demo`; `demo_script.rs` is the story).
//!
//! ⌥⌘R plays it again from the start and ⌥⌘P pauses or resumes it. A pause holds the script's
//! events and stops the app's clock of record, so running times stand still too.

use rune::{Key, KeyEvent, Mods, Tx};
use serde_json::Value;

use opencode_kit::model::{SessionInfo, Transcript};

use crate::stories::demo_script::{self, Ids};
use crate::tabs::Tab;
use crate::{App, Route};

#[derive(Default)]
pub struct Demo {
    /// Which play this is; each one's sessions and messages have their own ids.
    run: u32,
    /// While paused: the app's ms clock when it paused, and the held events, each as seconds
    /// after the pause.
    paused: Option<(i64, Vec<demo_script::Event>)>,
}

/// `--scene demo`: the run never reads or writes Kit's server (`App::load`).
pub fn requested() -> bool {
    crate::arg("--scene").as_deref() == Some("demo")
}

/// The scene: a fresh session in the fixture's first project, and the script queued from now.
pub fn start(s: &mut App) {
    if !matches!(*s.source, crate::Source::Fixture) {
        return;
    }
    // A send in the fixture answers with its canned reply; the script is the reply here.
    s.fixture["reply"] = Value::Array(Vec::new());
    s.fixture["stream"]["played"] = true.into();
    s.demo.run += 1;
    s.demo.paused = None;
    let ids = Ids::new(s.demo.run);
    clear(s);
    let from = s.sessions.first().cloned().unwrap_or_default();
    s.sessions.insert(0, SessionInfo {
        id: ids.session.clone(),
        project: from.project,
        directory: from.directory.clone(),
        title: "New session".into(),
        model: from.model,
        agent: "build".into(),
        created: *s.now_ms,
        updated: *s.now_ms,
        ..Default::default()
    });
    s.transcripts.insert(ids.session.clone(), Transcript { session: ids.session.clone(), messages: Vec::new() });
    s.tabs.push(Tab::session(ids.session.clone(), "", from.directory));
    *s.route = Route::here(ids.session.clone());
    s.composer.clear();
    let now = *s.clock;
    for (t, ty, data) in demo_script::events(&ids) {
        s.playback.push_back((now + t, ty, data));
    }
    s.playback.make_contiguous().sort_by(|a, b| a.0.total_cmp(&b.0));
    s.wake_at_playback();
}

/// Drops the earlier play: its sessions, tab, inbox, question and queued events.
fn clear(s: &mut App) {
    let mine = |id: &str| id.starts_with("ses_demo");
    s.playback.clear();
    s.sessions.retain(|x| !mine(&x.id));
    s.tabs.retain(|t| !mine(&t.id));
    s.transcripts.retain(|id, _| !mine(id));
    s.running.retain(|id| !mine(id));
    s.inboxes.retain(|id, _| !mine(id));
    s.pending.retain(|p| !mine(p.session()));
}

/// One of the script's own events (`demo.*`), in the frame it's due.
pub fn apply(s: &mut App, ty: &str, data: &Value, tx: &mut Tx) {
    match ty {
        "demo.type" => {
            s.composer.focused = true;
            s.composer.insert(data["text"].as_str().unwrap_or(""));
        }
        "demo.send" => s.submit(false, tx),
        "demo.child" => {
            let Route::Session { id: parent, .. } = (*s.route).clone() else { return };
            let id = data["id"].as_str().unwrap_or("").to_owned();
            let from = s.session(&parent).cloned().unwrap_or_default();
            s.sessions.push(SessionInfo {
                id: id.clone(),
                parent: Some(parent),
                project: from.project,
                directory: from.directory,
                title: data["title"].as_str().unwrap_or("").into(),
                model: from.model,
                agent: "general".into(),
                created: *s.now_ms,
                updated: *s.now_ms,
                ..Default::default()
            });
            s.transcripts.insert(id.clone(), Transcript { session: id, messages: Vec::new() });
        }
        "demo.pick" => {
            let key = KeyEvent { key: Key::Text(data["key"].as_str().unwrap_or("1").into()), mods: Mods::default() };
            s.question_key(&key, tx);
        }
        _ => {}
    }
}

/// ⌥⌘R replays from the start, ⌥⌘P pauses or resumes; only once the demo has played.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    use crate::keymap::Bind;
    if s.demo.run == 0 {
        return false;
    }
    if Bind::DemoReplay.matches_tx(ev, tx) {
        start(s);
        tx.settle();
        return true;
    }
    if Bind::DemoPause.matches_tx(ev, tx) {
        toggle_pause(s);
        return true;
    }
    false
}

fn toggle_pause(s: &mut App) {
    match s.demo.paused.take() {
        None => {
            let now = *s.clock;
            let held = s.playback.drain(..).map(|(t, ty, d)| ((t - now).max(0.0), ty, d)).collect();
            s.demo.paused = Some((*s.now_ms, held));
        }
        Some((_, held)) => {
            let now = *s.clock;
            for (t, ty, d) in held {
                s.playback.push_back((now + t, ty, d));
            }
            s.wake_at_playback();
        }
    }
}

/// Each frame while paused, the app's ms clock stays where the pause stopped it.
pub fn poll(s: &mut App) {
    if let Some((at, _)) = s.demo.paused {
        *s.base_ms = at - (*s.clock * 1000.0) as i64;
        *s.now_ms = at;
    }
}

rune_register::register!(crate::stories::scenes::SCENE_LIST: crate::stories::scenes::Scene = [("demo", start)]);

