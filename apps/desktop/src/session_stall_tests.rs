//! A frozen server is waited out: the read asks again on its own, nothing fails on screen,
//! and the app says "still trying" only once the silence has lasted.
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use std::time::{Duration, Instant};

use rune::Tx;
use serde_json::json;

use crate::session_pages::{Fetch, Key, Pages};
use crate::App;

const ID: &str = "ses_frozen";

fn page() -> serde_json::Value {
    json!({"data":[{"id":"msg_last","type":"user","text":"hey","time":{"created":1}}], "cursor":{"next":null}})
}

/// Times out `silent` times, as a frozen server does, then answers.
fn frozen(silent: usize, asked: Arc<AtomicUsize>) -> Fetch {
    Arc::new(move |_| {
        if asked.fetch_add(1, Ordering::SeqCst) < silent { Err("GET /api/session/x/message: timeout: global".into()) } else { Ok(page()) }
    })
}

fn open(fetch: Fetch, after: Duration) -> App {
    let pages = Pages::new(fetch, None, Arc::new(|| {}));
    pages.request(Key { id: ID.into(), updated: 1 }, true);
    let mut app = App::empty();
    app.pages.stall_after(after);
    app.route = crate::Route::Session(ID.into());
    app.pages.set(pages);
    app
}

fn wait(app: &mut App, until: impl Fn(&App) -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        app.poll_pages(&mut Tx::default());
        if until(app) { return true; }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn a_short_freeze_is_never_shown() {
    let asked = Arc::new(AtomicUsize::new(0));
    let mut app = open(frozen(2, asked.clone()), Duration::from_secs(30));
    assert!(wait(&mut app, |a| a.transcripts.contains_key(ID)));
    assert_eq!(asked.load(Ordering::SeqCst), 3);
    assert!(app.toast.current().is_none());
}

#[test]
fn a_long_silence_says_still_trying_and_clears_when_it_answers() {
    let asked = Arc::new(AtomicUsize::new(0));
    let mut app = open(frozen(5, asked), Duration::from_millis(60));
    assert!(wait(&mut app, |a| a.toast.current().is_some()));
    let toast = app.toast.current().cloned().unwrap();
    assert_eq!(toast.title, "OpenCode isn't answering. Still trying.");
    assert!(toast.description.is_none());
    assert!(wait(&mut app, |a| a.transcripts.contains_key(ID)));
    assert!(wait(&mut app, |a| a.toast.current().is_none()));
}

#[test]
fn the_event_stream_coming_back_ends_the_silence() {
    let mut app = open(frozen(usize::MAX, Arc::new(AtomicUsize::new(0))), Duration::from_millis(40));
    assert!(wait(&mut app, |a| a.toast.current().is_some()));
    app.stream_back();
    assert!(app.toast.current().is_none());
}

#[test]
fn a_missing_session_still_fails_at_once_in_plain_words() {
    let fetch: Fetch = Arc::new(|_| Err("GET /api/session/x/message: http status: 404".into()));
    let mut app = open(fetch, Duration::from_secs(30));
    assert!(wait(&mut app, |a| a.toast.current().is_some()));
    assert_eq!(app.toast.current().cloned().unwrap().title, "Couldn't load this session");
}
