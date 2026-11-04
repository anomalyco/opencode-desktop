use opencode_kit::client::Write;
use opencode_kit::model::Message;
use rune::{Key, KeyEvent, Mods};
use serde_json::json;

use crate::fixture_runtime as runtime;
use crate::settings::Settings;

fn chord() -> KeyEvent {
    KeyEvent { key: Key::Command('r'), mods: Mods { cmd: true, shift: true, alt: true, ..Mods::default() } }
}

fn frames(rt: &mut rune::Runtime<crate::App>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

fn on() -> rune::Runtime<crate::App> {
    let mut rt = runtime();
    rt.update(|s, _| s.settings.record_glitches = true);
    frames(&mut rt, 3);
    rt
}

#[test]
fn off_by_default_nothing_records_and_the_chord_says_how_to_turn_it_on() {
    let mut rt = runtime();
    frames(&mut rt, 3);
    assert!(!rt.state.settings.record_glitches);
    assert!(rt.rewind().is_none());
    rt.key(chord());
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.glitch.scrub.is_open());
    assert!(rt.state.toast.current.is_some());
}

#[test]
fn turned_on_it_records_and_the_chord_freezes_until_escape() {
    let mut rt = on();
    assert!(rt.rewind().is_some());
    assert!(rt.state.glitch.rewind.frames() > 0);
    rt.key(chord());
    rt.frame(1.0 / 60.0);
    assert!(rt.state.glitch.scrub.is_open());
    assert!(rt.state.glitch.rewind.is_paused());
    assert!(rt.find("rewind-bar").is_some());
    // Keys go to the scrubber, not the app underneath.
    rt.key(KeyEvent { key: Key::Command('b'), mods: Mods { cmd: true, ..Mods::default() } });
    assert!(rt.state.glitch.scrub.is_open());
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.glitch.scrub.is_open());
    assert!(!rt.state.glitch.rewind.is_paused());
}

#[test]
fn turning_it_off_lets_the_recorder_go() {
    let mut rt = on();
    rt.update(|s, _| s.settings.record_glitches = false);
    frames(&mut rt, 2);
    assert!(rt.rewind().is_none());
}

#[test]
fn return_sends_the_bundle_to_the_open_session() {
    let mut rt = on();
    let writes = rt.state.fake_writes();
    let session = crate::stories::sid(&rt.state);
    rt.key(chord());
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.glitch.scrub.set_note("the header pops"));
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    let sent: Vec<_> = writes.try_iter().filter_map(|w| match w { Write::Send { session, prompt } => Some((session, prompt)), _ => None }).collect();
    assert_eq!(sent.len(), 1);
    let (to, prompt) = &sent[0];
    assert_eq!(to, &session);
    assert!(prompt.text.contains("> the header pops"), "{}", prompt.text);
    assert_eq!(prompt.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["contact.png", "trace.jsonl"]);
    for a in &prompt.attachments {
        assert!(prompt.text.contains(&format!("Attached file: `{}`", a.path)));
        assert!(std::path::Path::new(&a.path).is_file(), "{} was written", a.path);
    }
    assert!(!rt.state.glitch.scrub.is_open());
    let last = rt.state.transcripts[&session].messages.last().cloned();
    assert!(matches!(last, Some(Message::User { text, .. }) if text.contains("the header pops")));
    if let Some(dir) = std::path::Path::new(&prompt.attachments[0].path).parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn it_is_kept_in_settings() {
    let mut s = Settings::default();
    assert!(!s.record_glitches);
    s.record_glitches = true;
    assert_eq!(s.to_json()["developer"]["recordGlitches"], true);
    assert!(Settings::from_json(&s.to_json()).record_glitches);
    assert!(!Settings::from_json(&json!({})).record_glitches);
}

#[test]
fn the_story_freezes_on_the_cards_one_frame_blip() {
    let mut rt = crate::stories::story("Dev/Rewind", None);
    for _ in 0..100 {
        rt.frame(1.0 / 60.0);
    }
    let scrub = &crate::stories::world(&rt).glitch.scrub;
    assert!(scrub.is_open(), "frozen at 1.4 s");
    let clip = scrub.clip().unwrap();
    assert!(clip.has_jump(scrub.frame()), "on the jump");
    let labels: Vec<String> = clip.changes(scrub.frame()).iter().map(|c| c.label()).collect();
    assert!(labels.iter().any(|l| l.contains("glitch-demo-card") && l.contains("18")), "{labels:?}");
    assert_eq!(scrub.selection().len(), 9);
}
