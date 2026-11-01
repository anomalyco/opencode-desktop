//! Replays a saved session page through the real reader, opening gate and view, sending the
//! open's spans to Motel. The page stays outside the repo: `OPEN_REPLAY_PAGE` names a newest-first
//! `?limit=80&order=desc` response. A local server hands out the latest page and then the next
//! by cursor, after `OPEN_REPLAY_COLD_MS` on the first read (a cold server). No OpenCode server.
//!
//! `OPENCODE_RUNE_OTEL=1 OPEN_REPLAY_PAGE=… cargo test --release -p opencode-desktop open_replay -- --ignored --nocapture`
use crate::{App, Route, session_pages::Pages, telemetry, view};
use opencode_kit::client::Server;
use rune::{Env, Runtime};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc::channel};
use std::time::{Duration, Instant};

const ID: &str = "ses_replay";

fn env_ms(name: &str) -> Duration { Duration::from_millis(std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(0)) }

/// `?limit=N&order=desc` is the newest N; `cursor=after-K` the N after the newest K.
fn serve(data: Vec<Value>, cold: Duration) -> String {
    let data = Arc::new(data);
    let first = Arc::new(AtomicBool::new(true));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (data, first) = (data.clone(), first.clone());
            std::thread::spawn(move || {
                let mut s = stream;
                let mut buf = vec![0; 8192];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                let param = |k: &str| path.split(['?', '&']).find_map(|p| p.strip_prefix(&format!("{k}=")).map(str::to_owned));
                let limit: usize = param("limit").and_then(|l| l.parse().ok()).unwrap_or(80);
                let from: usize = param("cursor").and_then(|c| c.strip_prefix("after-").and_then(|k| k.parse().ok())).unwrap_or(0);
                let to = (from + limit).min(data.len());
                let mut page = json!({"data": data[from..to]});
                if to < data.len() { page["cursor"] = json!({"next": format!("after-{to}")}); }
                let body = page.to_string();
                if first.swap(false, Ordering::SeqCst) { std::thread::sleep(cold); }
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(body.as_bytes());
            });
        }
    });
    base
}

/// Steps frames as the app wakes, telling telemetry about each as the window's hooks do.
fn step(rt: &mut Runtime<App>, woke: &std::sync::mpsc::Receiver<()>, last: &mut Instant) -> f64 {
    let _ = woke.recv_timeout(Duration::from_millis(1));
    let start = Instant::now();
    rt.update_if(|s, tx| { s.clock += start.duration_since(*last).as_secs_f64(); s.poll_pages(tx) });
    rt.frame(last.elapsed().as_secs_f64());
    *last = Instant::now();
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    telemetry::drawn(rt.state.painted_session(), (Instant::now(), Some(ms), None));
    ms
}

#[test]
#[ignore = "replays a private saved page; OPEN_REPLAY_PAGE=… --release --ignored --nocapture"]
fn open_replay() {
    let path = std::env::var("OPEN_REPLAY_PAGE").expect("OPEN_REPLAY_PAGE");
    let mut page: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let data = page["data"].take().as_array().cloned().unwrap();
    let runs = std::env::var("OPEN_REPLAY_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    for run in 0..runs {
        let base = serve(data.clone(), env_ms("OPEN_REPLAY_COLD_MS"));
        let (wake, woke) = channel();
        let wake = Arc::new(Mutex::new(wake));
        let server = Server::new(&base);
        let w = wake.clone();
        let pages = Pages::new(Arc::new(move |path| server.read_timed(path).map(|(v, t)| {
            telemetry::note_read(t.first_byte, t.bytes, t.parsing);
            v
        })), None, Arc::new(move || { let _ = w.lock().unwrap().send(()); }));
        let mut app = App::empty();
        app.attach.set_waker(Arc::new(move || { let _ = wake.lock().unwrap().send(()); }));
        app.route = Route::Session("ses_previous".into());
        app.transcripts.insert("ses_previous".into(), opencode_kit::model::Transcript { session: "ses_previous".into(), messages: vec![] });
        app.pages.set(pages);
        let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let click = Instant::now();
        let mut last = Instant::now();
        rt.update(|s, tx| { s.clock = 0.0; s.open(ID, tx); });
        let (mut painted, mut older, mut worst) = (None, None, 0f64);
        while older.is_none() || click.elapsed() < Duration::from_secs(3) {
            assert!(click.elapsed() < Duration::from_secs(20), "never finished");
            let ms = step(&mut rt, &woke, &mut last);
            worst = worst.max(ms);
            let rows = rt.state.painted_session().filter(|(id, _)| *id == ID).map_or(0, |(_, n)| n);
            if painted.is_none() && rows > 0 { painted = Some(click.elapsed()); }
            if older.is_none() && rows > crate::session_pages::LATEST { older = Some((click.elapsed(), ms)); }
        }
        let (older, older_frame) = older.unwrap();
        println!("METRIC run={run} first_paint_ms={:.1} older_painted_ms={:.1} older_frame_ms={older_frame:.1} worst_frame_ms={worst:.1}",
            painted.unwrap().as_secs_f64() * 1000.0, older.as_secs_f64() * 1000.0);
        // The export thread batches for 200 ms.
        std::thread::sleep(Duration::from_millis(600));
    }
}
