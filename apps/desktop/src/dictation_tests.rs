//! Dictation against a stand-in Hex on a local port (never the real one) and the fake.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rune::{Env, Key, KeyEvent, Mods, Runtime, TextEdit};

use crate::App;
use crate::dictation::{Dictation, FAKE_WORDS};

/// A stand-in Hex capture API, answering each request from `reply` and recording the paths it
/// was asked for. Returns its endpoint file.
fn fake_hex(name: &str, reply: fn(&str) -> (u16, &'static str)) -> (PathBuf, Arc<Mutex<Vec<String>>>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let log = log.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                log.lock().unwrap().push(path.clone());
                let (status, body) = reply(&path);
                let kind = if path.ends_with("/levels") { "text/event-stream" } else { "application/json" };
                let mut c = c;
                let _ = write!(c, "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });
    let file = std::env::temp_dir().join(format!("desktop-hex-{name}-{}.json", std::process::id()));
    std::fs::write(&file, format!(r#"{{"port":{port},"token":"t","apiVersion":"2","pid":1}}"#)).unwrap();
    (file, seen)
}

fn happy(path: &str) -> (u16, &'static str) {
    match path {
        "/dictations" => (201, r#"{"id":4,"ownerToken":"owner-token-owner-token-owner-token","sampleRate":16000}"#),
        p if p.ends_with("/finish") => (200, r#"{"transcript":" check the retry budget ","durationMs":4000}"#),
        p if p.ends_with("/levels") => (200, "data: {\"rmsDb\":-20.0,\"peakDb\":-12.0}\n\n"),
        _ => (204, ""),
    }
}

fn app(d: Dictation) -> Runtime<App> {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    app.dictation = d;
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt
}

fn shows(rt: &Runtime<App>, s: &str) -> bool {
    rt.elements().into_iter().any(|e| !e.exiting && e.text.as_deref() == Some(s))
}

fn has(rt: &Runtime<App>, key: &str) -> bool {
    rt.elements().into_iter().any(|e| !e.exiting && e.path.ends_with(key))
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
}

fn shortcut() -> KeyEvent {
    KeyEvent { key: Key::Command('m'), mods: Mods { cmd: true, shift: true, ..Mods::default() } }
}

fn key(name: Key) -> KeyEvent {
    KeyEvent { key: name, mods: Mods::default() }
}

/// A draft with the caret between "Before" and "merge", focused.
fn draft(rt: &mut Runtime<App>, t: &mut f64) {
    tap(rt, "prompt");
    step(rt, t, 1);
    rt.update(|s, _| {
        s.composer = TextEdit::new("Before merge");
        s.composer.focused = true;
        s.composer.set_caret(6);
    });
    step(rt, t, 1);
}

/// `frames` frames of 1/60 s (`Runtime::frame` takes the time since the last one).
fn step(rt: &mut Runtime<App>, t: &mut f64, frames: usize) {
    for _ in 0..frames {
        *t += 1.0 / 60.0;
        rt.frame(1.0 / 60.0);
    }
}

/// Frames, with real time passing for Hex's thread, until `done`.
fn until(rt: &mut Runtime<App>, t: &mut f64, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(&rt.state) {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
        step(rt, t, 1);
    }
}

#[test]
fn the_mic_records_and_a_second_press_puts_the_words_at_the_caret() {
    let (file, seen) = fake_hex("words", happy);
    let mut rt = app(Dictation::at(file));
    let mut t = 0.0;
    draft(&mut rt, &mut t);
    tap(&mut rt, "mic");
    step(&mut rt, &mut t, 2);
    assert!(rt.state.dictation.recording());
    assert!(shows(&rt, "Listening…"));
    assert!(rt.state.composer.focused, "the composer keeps focus");
    until(&mut rt, &mut t, |s| s.dictation.level > 0.5);
    assert!(rt.find("dictation-level").is_some());

    rt.key(shortcut());
    step(&mut rt, &mut t, 1);
    assert!(shows(&rt, "Transcribing…"));
    until(&mut rt, &mut t, |s| !s.composer.text.starts_with("Before merge"));
    assert_eq!(rt.state.composer.text, "Before check the retry budget merge");
    assert_eq!(rt.state.composer.focus, "Before check the retry budget".len(), "the caret after the words");
    assert!(rt.state.composer.focused);
    step(&mut rt, &mut t, 1);
    assert!(!shows(&rt, "Listening…") && !shows(&rt, "Transcribing…"));
    assert_eq!(rt.state.sends, 0, "never sent");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.first().map(String::as_str), Some("/dictations"));
    assert!(seen.contains(&"/dictations/4/levels".to_owned()) && seen.contains(&"/dictations/4/finish".to_owned()), "{seen:?}");
}

#[test]
fn escape_throws_the_recording_away() {
    let (file, seen) = fake_hex("cancel", happy);
    let mut rt = app(Dictation::at(file));
    let mut t = 0.0;
    draft(&mut rt, &mut t);
    rt.key(shortcut());
    step(&mut rt, &mut t, 1);
    assert!(rt.state.dictation.recording());
    until(&mut rt, &mut t, |_| seen.lock().unwrap().iter().any(|p| p == "/dictations"));
    rt.key(key(Key::Escape));
    step(&mut rt, &mut t, 1);
    assert!(!rt.state.dictation.recording() && !shows(&rt, "Listening…"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !seen.lock().unwrap().iter().any(|p| p == "/dictations/4/cancel") {
        assert!(Instant::now() < deadline, "no cancel: {:?}", seen.lock().unwrap());
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(500));
    step(&mut rt, &mut t, 2);
    assert_eq!(rt.state.composer.text, "Before merge");
    assert!(rt.state.composer.focused);
}

#[test]
fn hex_refusing_says_why_beside_the_mic() {
    for (name, reply, why) in [
        ("busy", (|_: &str| (409, r#"{"code":"dictation-busy"}"#)) as fn(&str) -> (u16, &'static str), hex_client::BUSY),
        ("outdated", |_: &str| (503, r#"{"code":"service-capture-unavailable"}"#), hex_client::OUTDATED),
    ] {
        let (file, _) = fake_hex(name, reply);
        let mut rt = app(Dictation::at(file));
        let mut t = 0.0;
        draft(&mut rt, &mut t);
        tap(&mut rt, "mic");
        until(&mut rt, &mut t, |s| s.dictation.error().is_some());
        step(&mut rt, &mut t, 1);
        assert!(shows(&rt, why), "{name}: {:?}", rt.state.dictation.error());
        assert!(!rt.state.dictation.recording() && rt.state.composer.text == "Before merge");
        // It goes after a few seconds.
        rt.frame(4.1);
        step(&mut rt, &mut t, 1);
        assert!(!shows(&rt, why));
    }

    // An endpoint file left behind by a Hex that crashed: nobody on its port.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let file = std::env::temp_dir().join(format!("desktop-hex-gone-{}.json", std::process::id()));
    std::fs::write(&file, format!(r#"{{"port":{port},"token":"t","apiVersion":"2","pid":1}}"#)).unwrap();
    let mut rt = app(Dictation::at(file));
    let mut t = 0.0;
    rt.key(shortcut());
    until(&mut rt, &mut t, |s| s.dictation.error().is_some());
    step(&mut rt, &mut t, 1);
    assert!(shows(&rt, hex_client::NOT_RUNNING));
}

#[test]
fn without_hex_s_endpoint_file_there_is_no_mic() {
    let mut rt = app(Dictation::at(PathBuf::from("/nonexistent/local-api.json")));
    let mut t = 0.0;
    step(&mut rt, &mut t, 1);
    assert!(!has(&rt, "mic") && has(&rt, "submit"));
    // The shortcut still says why nothing happens.
    rt.key(shortcut());
    step(&mut rt, &mut t, 1);
    assert!(shows(&rt, hex_client::NOT_RUNNING));
    assert!(!rt.state.dictation.recording());
}

#[test]
fn holding_the_shortcut_talks_until_it_comes_up_and_a_tap_toggles() {
    let mut rt = app(Dictation::fake());
    let mut t = 0.0;
    draft(&mut rt, &mut t);
    let m_up = KeyEvent { key: Key::Text("m".into()), mods: Mods::default() };

    // A tap: it keeps listening after the key comes up, and the next press finishes.
    rt.key(shortcut());
    step(&mut rt, &mut t, 6);
    rt.key_up(shortcut());
    step(&mut rt, &mut t, 1);
    assert!(rt.state.dictation.recording(), "a tap toggles on");
    rt.key(shortcut());
    step(&mut rt, &mut t, 1);
    assert!(!rt.state.dictation.recording());
    step(&mut rt, &mut t, 40);
    assert_eq!(rt.state.composer.text, format!("Before {FAKE_WORDS} merge"));

    // A hold: the release (⌘ already up) finishes it.
    rt.key(shortcut());
    step(&mut rt, &mut t, 30);
    assert!(rt.state.dictation.recording());
    rt.key_up(m_up);
    step(&mut rt, &mut t, 1);
    assert!(!rt.state.dictation.recording(), "a hold past 300 ms is push-to-talk");
    assert!(shows(&rt, "Transcribing…"));
}

#[test]
fn the_storybook_story_listens_on_the_fake() {
    let mut rt = app(Dictation::fake());
    rt.update(|s, _| crate::stories::scene(s, "Dictation"));
    let mut t = 0.0;
    step(&mut rt, &mut t, 30);
    assert!(rt.state.dictation.recording() && shows(&rt, "Listening…"));
    assert!(rt.state.dictation.level > 0.05, "the fake talks: {}", rt.state.dictation.level);
    rt.key(key(Key::Text("f".into())));
    step(&mut rt, &mut t, 40);
    assert_eq!(rt.state.composer.text, format!("Looks right. Before you merge, {FAKE_WORDS}"));
    rt.key(key(Key::Text("e".into())));
    step(&mut rt, &mut t, 1);
    assert!(shows(&rt, hex_client::NOT_RUNNING));
    rt.key(key(Key::Text("e".into())));
    step(&mut rt, &mut t, 1);
    assert!(shows(&rt, hex_client::BUSY));
}
