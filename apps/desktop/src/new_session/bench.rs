//! Send on the new-session page → the prompt on screen, and → Working, against a stand-in server
//! that answers after a local or a distant server's delays. The app runs its real frames on a
//! live source in real time; nothing touches a real OpenCode server.
use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_kit::client::{self, Server};

use super::mock::{self, Latency};
use crate::{App, Route, Source};

const PROMPT: &str = "Find out why the sidebar flickers on reconnect";

/// Milliseconds from Send to the first frame drawn, to the prompt showing and to Working
/// showing, over one start.
fn once(latency: Latency) -> (f64, f64, f64) {
    let mock = mock::serve(latency);
    let server = Server::new(&mock.base);
    let (requests, writes, inbox) = client::connect(server.clone(), Arc::new(|| {}));
    let mut app = App::fixture_session();
    let fetch = server.clone();
    app.pages.set(crate::session_pages::Pages::new(Arc::new(move |path: &str| fetch.read(path)), None, Arc::new(|| {})));
    app.source = Source::Live { server, requests: Some(requests), writes: Some(writes), allow_send: true };
    app.inbox = Some(inbox);
    app.default_dir = "/tmp/bench".into();
    let mut rt = app.into_runtime();
    rt.update(|s, tx| s.new_session(tx));
    rt.frame(0.0);
    rt.update(|s, _| s.composer = rune::TextEdit::new(PROMPT));
    rt.frame(1.0 / 120.0);
    let start = Instant::now();
    rt.update(|s, tx| s.send(tx));
    let (mut first, mut prompt, mut working) = (None, None, None);
    let mut last = start;
    while start.elapsed() < Duration::from_secs(5) && (prompt.is_none() || working.is_none()) {
        let now = Instant::now();
        rt.frame((now - last).as_secs_f64());
        last = now;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        first.get_or_insert(ms);
        let on = matches!(rt.state.route, Route::Session { .. });
        let els = rt.elements();
        if prompt.is_none() && on && els.iter().any(|e| e.path.contains("/u-") && e.text.as_deref() == Some(PROMPT) && e.opacity > 0.5) {
            prompt = Some(ms);
        }
        if working.is_none() && on && els.iter().any(|e| e.key.as_deref() == Some("working") && e.opacity > 0.5) {
            working = Some(ms);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(rt);
    drop(mock);
    (first.unwrap(), prompt.expect("the prompt never showed"), working.expect("Working never showed"))
}

#[test]
#[ignore = "timing benchmark; run --profile perf --ignored --nocapture --test-threads=1 new_session_benchmark"]
fn new_session_benchmark() {
    assert!(!cfg!(debug_assertions), "use --profile perf");
    for (before, latency) in [(true, mock::LOCAL), (true, mock::REMOTE), (false, mock::LOCAL), (false, mock::REMOTE)] {
        super::BEFORE.set(before);
        let name = format!("{}_{}", if before { "before" } else { "after" }, latency.name);
        let _ = once(latency);
        let (mut frame, mut prompt, mut working) = (vec![], vec![], vec![]);
        for _ in 0..20 {
            let (f, p, w) = once(latency);
            frame.push(f);
            prompt.push(p);
            working.push(w);
        }
        crate::perf::report(&format!("{name}_send_to_first_frame_ms"), &mut frame);
        crate::perf::report(&format!("{name}_send_to_prompt_ms"), &mut prompt);
        crate::perf::report(&format!("{name}_send_to_working_ms"), &mut working);
    }
}
