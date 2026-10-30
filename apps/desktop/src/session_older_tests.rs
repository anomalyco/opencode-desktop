//! Scrolling up a long session reads older pages by cursor and lands each above what's on
//! screen without moving it, until the session's first message.
use std::sync::{Arc, Mutex, mpsc::{Receiver, channel}};
use std::time::{Duration, Instant};

use rune::{Env, Runtime};
use serde_json::{Value, json};

use crate::session_pages::Pages;
use crate::{App, Route, view};

const ID: &str = "ses_long";
const SCROLL: &str = "scroll-ses_long";
const DT: f64 = 1.0 / 120.0;
/// The latest 12, the first history page of 68, then three pages of 20.
const TOTAL: usize = 140;

fn message(i: usize) -> Value {
    let t = 1_792_000_000_000i64 + i as i64 * 1000;
    if i % 2 == 0 {
        json!({"id": format!("msg_{i:05}"), "type": "user", "time": {"created": t}, "text": format!("Prompt {i}: why does the row re-mount?")})
    } else {
        json!({"id": format!("msg_{i:05}"), "type": "assistant", "time": {"created": t, "completed": t + 500}, "finish": "stop",
            "content": [{"type": "text", "text": format!("Answer {i}. {}", "The key changed with the status, so the list treated it as new. ".repeat(1 + i % 5 * 3))}]})
    }
}

/// Newest-first pages like the server's: the latest page, then `limit` before cursor `cN`
/// (N messages from the end), its `next` cursor gone at the session's start. Older pages
/// wait for `release`.
fn serve(requests: Arc<Mutex<Vec<String>>>, release: Receiver<()>) -> crate::session_pages::Fetch {
    let release = Mutex::new(release);
    Arc::new(move |path: &str| {
        requests.lock().unwrap().push(path.to_owned());
        let arg = |k: &str| path.split(['?', '&']).find_map(|p| p.strip_prefix(k)).map(str::to_owned);
        let limit: usize = arg("limit=").unwrap().parse().unwrap();
        let from: usize = arg("cursor=c").map_or(0, |c| c.parse().unwrap());
        if limit == 20 { release.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap(); }
        let end = TOTAL - from;
        let start = end.saturating_sub(limit);
        let data: Vec<Value> = (start..end).rev().map(message).collect();
        let next = (start > 0).then(|| format!("c{}", from + (end - start)));
        Ok(json!({"data": data, "cursor": {"next": next}}))
    })
}

/// Text drawn in the timeline's viewport, by path, at its top edge.
fn shown(rt: &Runtime<App>) -> Vec<(String, f32)> {
    let elements = rt.elements();
    let v = elements.iter().find(|e| e.key.as_deref() == Some(SCROLL)).unwrap().rect;
    elements.into_iter()
        .filter(|e| !e.exiting && e.path.contains(SCROLL) && e.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
        .filter(|e| e.rect[1] < v[1] + v[3] && e.rect[1] + e.rect[3] > v[1])
        .map(|e| (e.path, e.rect[1]))
        .collect()
}

fn step(rt: &mut Runtime<App>) {
    rt.update_if(|s, tx| s.poll_pages(tx));
    rt.frame(DT);
}

fn rest(rt: &mut Runtime<App>) {
    for _ in 0..240 { step(rt); }
}

fn len(rt: &Runtime<App>) -> usize {
    rt.state.transcripts.get(ID).map_or(0, |t| t.messages.len())
}

/// Polls until `done`, with real time passing for the page workers and the history delay.
fn until(rt: &mut Runtime<App>, woke: &Receiver<()>, what: &str, done: impl Fn(&Runtime<App>) -> bool) {
    let start = Instant::now();
    while !done(rt) {
        assert!(start.elapsed() < Duration::from_secs(10), "{what}");
        let _ = woke.recv_timeout(Duration::from_millis(2));
        step(rt);
    }
}

#[test]
fn scrolling_up_reads_older_pages_without_moving_what_is_on_screen() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (release, gate) = channel();
    let (wake, woke) = channel();
    let wake = Mutex::new(wake);
    let pages = Pages::new(serve(requests.clone(), gate), None, Arc::new(move || { let _ = wake.lock().unwrap().send(()); }));
    let mut app = App::empty();
    app.route = Route::Session("ses_previous".into());
    app.transcripts.insert("ses_previous".into(), opencode_kit::model::Transcript { session: "ses_previous".into(), messages: vec![] });
    app.pages.set(pages);
    let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|s, tx| s.open(ID, tx));
    until(&mut rt, &woke, "the first 80 land", |rt| len(rt) == 80 && rt.state.opening.is_none());
    rest(&mut rt);
    assert_eq!(rt.scroll_at_bottom(SCROLL), Some(true));
    assert!(!requests.lock().unwrap().iter().any(|p| p.contains("limit=20")), "nothing older is read at the end");

    let v = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(SCROLL)).unwrap().rect;
    let (x, y) = (v[0] + v[2] / 2.0, v[1] + v[3] / 2.0);
    let mut landed = 0;
    while rt.state.pages.older(ID).is_some_and(Option::is_some) {
        // Up to the very top: the page lands above the first row on screen.
        let asked = requests.lock().unwrap().len();
        for _ in 0..400 {
            if requests.lock().unwrap().len() > asked && rt.state.pages.older(ID).is_some() { break; }
            rt.wheel(x, y, 600.0);
            step(&mut rt);
        }
        rest(&mut rt);
        let path = requests.lock().unwrap().last().cloned().unwrap();
        assert!(path.contains(&format!("?limit=20&cursor=c{}", len(&rt))), "{path}");

        // Slower than the hold, it shows it's coming, over the timeline, moving nothing.
        std::thread::sleep(Duration::from_millis(160));
        let before = shown(&rt);
        step(&mut rt);
        assert!(rt.find("older-loading").is_some(), "slow pages show a ring");
        assert_eq!(shown(&rt), before, "the ring moves nothing");

        let n = len(&rt);
        release.send(()).unwrap();
        let start = Instant::now();
        while len(&rt) == n {
            assert!(start.elapsed() < Duration::from_secs(10), "an older page lands");
            let _ = woke.recv_timeout(Duration::from_millis(2));
            let before = shown(&rt);
            step(&mut rt);
            if len(&rt) == n { continue; }
            // The frame it lands in and the frames after, as the rows above mount and measure.
            for _ in 0..60 {
                let after = shown(&rt);
                let same: Vec<_> = before.iter().filter_map(|(p, y0)| after.iter().find(|(q, _)| q == p).map(|(_, y1)| (p, y1 - y0))).collect();
                assert!(!same.is_empty(), "what was on screen is still there");
                for (p, dy) in same { assert!(dy.abs() <= 0.5, "{p} moved {dy} pt when older messages landed"); }
                step(&mut rt);
            }
        }
        assert!(rt.find("older-loading").is_none());
        landed += 1;
    }
    assert_eq!(landed, 3);
    assert_eq!(len(&rt), TOTAL);
    assert_eq!(rt.state.pages.older(ID), Some(&None), "the cursor ran out");

    // At the start, nothing more is read and the first message shows.
    let asked = requests.lock().unwrap().len();
    for _ in 0..60 { rt.wheel(x, y, 600.0); step(&mut rt); }
    rest(&mut rt);
    assert_eq!(requests.lock().unwrap().len(), asked);
    assert!(shown(&rt).iter().any(|(p, _)| p.contains("msg_00000")), "the session's first message");
}
