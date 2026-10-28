//! Headless CPU benchmarks: synthetic replay and an opt-in, GET-only cold-session profile.
use crate::{App, Route, view};
use opencode_kit::model::{Assistant, Message, Part, ToolStatus, Transcript};
use rune::{Env, Runtime};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

pub fn fixture() -> App {
    let mut app = App::empty();
    let mut messages = Vec::new();
    for i in 0..800 {
        messages.push(Message::User { id: format!("u{i:04}"), text: format!("Check implementation {i} and explain the result.") });
        messages.push(Message::Assistant(Assistant {
            id: format!("a{i:04}"), completed: Some(1),
            parts: vec![
                Part::Reasoning { text: "Inspect the boundary and its callers.".into(), started: 0, completed: Some(1) },
                Part::Tool { id: format!("c{i}"), name: "read".into(), title: "src/runtime.rs".into(), status: ToolStatus::Completed,
                    input: serde_json::Value::Null, files: vec![], metadata: serde_json::Value::Null },
                Part::Text { text: format!("## Result {i}\n\nThe **boundary** preserves identity while the next update arrives. {}\n\n```rust\nfn update_{i}(value: usize) -> usize {{\n    value.saturating_add(1)\n}}\n```\n\n- Keep the existing state\n- Apply the new value\n", "Some prose with `code` and [a link](https://example.com). ".repeat(3)) },
            ], ..Default::default()
        }));
    }
    messages.push(Message::Assistant(Assistant {
        id: "stream".into(),
        parts: vec![Part::Text { text: "Streaming answer".into() }],
        ..Default::default()
    }));
    app.route = Route::Session("bench".into());
    app.transcripts.insert("bench".into(), Transcript { session: "bench".into(), messages });
    let mut other = app.transcripts["bench"].clone();
    other.session = "other".into();
    app.transcripts.insert("other".into(), other);
    app
}

fn report(name: &str, values: &mut [f64]) {
    values.sort_by(f64::total_cmp);
    let at = |p: f64| values[((values.len() - 1) as f64 * p).round() as usize];
    println!("METRIC name={name} p50={:.4} p95={:.4} max={:.4} n={}", at(0.5), at(0.95), at(1.0), values.len());
}

#[test]
#[ignore = "CPU benchmark; run --profile perf --ignored --nocapture --test-threads=1"]
fn timeline_benchmark() {
    assert!(!cfg!(debug_assertions), "use --profile perf");
    let mut open = [vec![], vec![]];
    let mut all = [vec![vec![]; 24], vec![vec![]; 24]];
    for sample in 0..8 {
        // Alternate the reference and lazy path to share thermal/cache drift.
        for eager in if sample % 2 == 0 { [true, false] } else { [false, true] } {
            view::EAGER_ROWS.set(eager);
            let variant = usize::from(eager);
            let app = fixture();
            let start = rune::thread_cpu_ms();
            let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let elapsed = rune::thread_cpu_ms() - start;
            if sample > 0 {
                open[variant].push(elapsed);
            }
            for _ in 0..120 {
                if !rt.frame(1.0 / 120.0) {
                    break;
                }
            }
            for mode in 0..4 {
                if mode == 1 {
                    rt.update(|_, tx| tx.scroll_to_bottom_instant("scroll-bench"));
                    rt.frame(0.0);
                    for _ in 0..120 {
                        if !rt.frame(1.0 / 120.0) {
                            break;
                        }
                    }
                }
                for frame in 0..32 {
                    rune::take_frame_phases();
                    let alloc = procwatch::alloc::thread_allocations();
                    let start = rune::thread_cpu_ms();
                    if mode == 0 {
                        rt.wheel(900.0, 350.0, 96.0);
                    } else if mode == 1 {
                        rt.update(|s, _| { s.apply_event("session.text.delta", &serde_json::json!({"sessionID":"bench", "assistantMessageID":"stream", "ordinal":0, "delta":" another word"})); });
                    } else if mode == 2 {
                        rt.update(|s, _| {
                        s.apply_event("session.prompted", &serde_json::json!({"sessionID":"bench", "messageID":format!("arrival{frame}"), "text":"Please inspect the next boundary."}));
                    });
                    } else {
                        rt.update(|s, tx| {
                            s.route = Route::Session(if frame % 2 == 0 { "other" } else { "bench" }.into());
                            tx.settle();
                        });
                    }
                    rt.frame(1.0 / 120.0);
                    let elapsed = rune::thread_cpu_ms() - start;
                    let allocations = (procwatch::alloc::thread_allocations() - alloc) as f64;
                    let phases = rune::take_frame_phases();
                    if sample > 0 && frame > 0 {
                        let p = rune::PHASES;
                        for (i, value) in
                            [elapsed, phases[p], phases[p + 2], phases[p + 3], allocations, phases[p + 1]].into_iter().enumerate()
                        {
                            all[variant][mode * 6 + i].push(value);
                        }
                    }
                }
            }
        }
    }
    view::EAGER_ROWS.set(false);
    for (variant, name) in ["lazy", "eager"].iter().enumerate() {
        report(&format!("{name}_open_cpu_ms"), &mut open[variant]);
        for (mode, label) in ["scroll", "stream", "arrival", "switch"].iter().enumerate() {
            for (i, metric) in ["cpu_ms", "view_ms", "layout_ms", "text_ms", "allocations", "commit_ms"].iter().enumerate() {
                report(&format!("{name}_{label}_{metric}"), &mut all[variant][mode * 6 + i]);
            }
        }
    }
}

thread_local! {
    /// The candidate side of `frame_benchmark`'s A/B: production code under test reads it
    /// through `candidate()` and keeps its shipped behaviour outside tests.
    pub(crate) static CANDIDATE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// A technique `frame_benchmark` can turn off for its baseline.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Experiment {
    /// Another family's stream doesn't commit the view.
    QuietBackground,
}

/// The technique the benchmark's baseline turns off; every other one stays on.
const UNDER_TEST: Experiment = Experiment::QuietBackground;

/// Whether `e` is on: always, except in the baseline half of `frame_benchmark`.
pub(crate) fn candidate(e: Experiment) -> bool {
    e != UNDER_TEST || CANDIDATE.get()
}

const SCENARIOS: [&str; 7] = ["scroll", "stream", "arrival", "switch", "typing", "idle", "background"];
const METRICS: [&str; 7] = ["cpu_ms", "view_ms", "commit_ms", "layout_ms", "text_ms", "allocations", "commits"];

/// One frame's CPU, phases, allocations and commits.
fn measured(rt: &mut Runtime<App>, step: impl FnOnce(&mut Runtime<App>)) -> [f64; 7] {
    rune::take_frame_phases();
    let commits = rt.stats.commits;
    let alloc = procwatch::alloc::thread_allocations();
    let start = rune::thread_cpu_ms();
    step(rt);
    let cpu = rune::thread_cpu_ms() - start;
    let allocations = (procwatch::alloc::thread_allocations() - alloc) as f64;
    let phases = rune::take_frame_phases();
    let p = rune::PHASES;
    [cpu, phases[p], phases[p + 1], phases[p + 2], phases[p + 3], allocations, (rt.stats.commits - commits) as f64]
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..240 {
        if !rt.frame(1.0 / 120.0) {
            break;
        }
    }
}

/// The whole app, with its poll hook, at 120 Hz: open, scroll, stream, arrival, switch,
/// composer typing, idle (must commit nothing) and another session streaming.
/// Alternates `CANDIDATE` off/on per sample (AB, BA, ...); the first two pairs are warmup.
#[test]
#[ignore = "CPU benchmark; run --profile perf --ignored --nocapture --test-threads=1"]
fn frame_benchmark() {
    assert!(!cfg!(debug_assertions), "use --profile perf");
    let mut open: [Vec<[f64; 7]>; 2] = [vec![], vec![]];
    let mut all: [Vec<Vec<[f64; 7]>>; 2] = [vec![vec![]; SCENARIOS.len()], vec![vec![]; SCENARIOS.len()]];
    let mut idle_wakes = [0usize; 2];
    for sample in 0..10 {
        for variant in if sample % 2 == 0 { [0, 1] } else { [1, 0] } {
            CANDIDATE.set(variant == 1);
            let keep = sample > 1;
            let app = fixture();
            rune::take_frame_phases();
            let alloc = procwatch::alloc::thread_allocations();
            let start = rune::thread_cpu_ms();
            let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let cpu = rune::thread_cpu_ms() - start;
            let allocations = (procwatch::alloc::thread_allocations() - alloc) as f64;
            let phases = rune::take_frame_phases();
            let p = rune::PHASES;
            if keep {
                open[variant].push([cpu, phases[p], phases[p + 1], phases[p + 2], phases[p + 3], allocations, rt.stats.commits as f64]);
            }
            rt.poll = Some(crate::poll);
            settle(&mut rt);
            for (mode, &scenario) in SCENARIOS.iter().enumerate() {
                match scenario {
                    "stream" | "typing" | "idle" => {
                        rt.update(|_, tx| tx.scroll_to_bottom_instant("scroll-bench"));
                        settle(&mut rt);
                    }
                    _ => {}
                }
                if scenario == "typing" {
                    rt.update(|s, _| s.composer.focused = true);
                    settle(&mut rt);
                }
                for frame in 0..32 {
                    let values = measured(&mut rt, |rt| {
                        match scenario {
                            "scroll" => rt.wheel(900.0, 350.0, 96.0),
                            // As poll takes events: a commit only when the event changed what shows.
                            "stream" => { rt.update_if(|s, _| {
                                s.apply_event("session.text.delta", &serde_json::json!({"sessionID":"bench", "assistantMessageID":"stream", "ordinal":0, "delta":" another word"}))
                            }); }
                            "arrival" => rt.update(|s, _| {
                                s.apply_event("session.prompted", &serde_json::json!({"sessionID":"bench", "messageID":format!("arrival{frame}"), "text":"Please inspect the next boundary."}));
                            }),
                            "switch" => rt.update(|s, tx| {
                                s.route = Route::Session(if frame % 2 == 0 { "other" } else { "bench" }.into());
                                tx.settle();
                            }),
                            "typing" => rt.key(rune::KeyEvent { key: rune::Key::Text("a".into()), mods: rune::Mods::default() }),
                            // Another session streams while this one is open.
                            "background" => { rt.update_if(|s, _| {
                                s.apply_event("session.text.delta", &serde_json::json!({"sessionID":"other", "assistantMessageID":"stream", "ordinal":0, "delta":" elsewhere"}))
                            }); }
                            _ => {}
                        }
                        if !rt.frame(1.0 / 120.0) && scenario == "idle" && frame > 0 {
                            return;
                        }
                        if scenario == "idle" && frame > 0 {
                            idle_wakes[variant] += usize::from(keep);
                        }
                    });
                    if keep && frame > 0 {
                        all[variant][mode].push(values);
                    }
                }
                if scenario == "typing" {
                    assert!(rt.state.composer.text.len() >= 32, "typing reaches the composer");
                    rt.update(|s, _| { s.composer = rune::TextEdit::new(""); s.composer.focused = false; });
                    settle(&mut rt);
                }
            }
        }
    }
    CANDIDATE.set(true);
    for (variant, name) in ["base", "candidate"].iter().enumerate() {
        for (i, metric) in METRICS.iter().enumerate() {
            report(&format!("{name}_open_{metric}"), &mut open[variant].iter().map(|v| v[i]).collect::<Vec<_>>());
        }
        for (mode, scenario) in SCENARIOS.iter().enumerate() {
            for (i, metric) in METRICS.iter().enumerate() {
                let mut values: Vec<f64> = all[variant][mode].iter().map(|v| v[i]).collect();
                report(&format!("{name}_{scenario}_{metric}"), &mut values);
                if *metric == "commits" {
                    println!("METRIC name={name}_{scenario}_commits_total sum={}", values.iter().sum::<f64>());
                }
            }
        }
        println!("METRIC name={name}_idle_frames_requesting_another count={}", idle_wakes[variant]);
    }
}

/// Another family's stream updates its transcript without rebuilding the view; the open
/// session's and its subagents' streams still do.
#[test]
fn only_the_open_family_streams_commit() {
    let mut app = fixture();
    app.sessions.push(opencode_kit::model::SessionInfo { id: "bench".into(), ..Default::default() });
    app.sessions.push(opencode_kit::model::SessionInfo { id: "child".into(), parent: Some("bench".into()), ..Default::default() });
    let mut child = app.transcripts["other"].clone();
    child.session = "child".into();
    app.transcripts.insert("child".into(), child);
    let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
    rt.frame(0.0);
    settle(&mut rt);
    let delta = |rt: &mut Runtime<App>, sid: &str| {
        let before = rt.stats.commits;
        let data = serde_json::json!({"sessionID": sid, "assistantMessageID": "stream", "ordinal": 0, "delta": " word"});
        rt.update_if(|s, _| s.apply_event("session.text.delta", &data));
        rt.frame(1.0 / 120.0);
        rt.stats.commits - before
    };
    assert_eq!(delta(&mut rt, "other"), 0, "a background session's stream draws nothing");
    assert!(rt.state.transcripts["other"].messages.iter().any(|m| matches!(m, Message::Assistant(a) if a.id == "stream"
        && matches!(&a.parts[..], [Part::Text { text }] if text.ends_with(" word")))), "its transcript still takes the words");
    assert_eq!(delta(&mut rt, "bench"), 1, "the open session's stream draws");
    assert_eq!(delta(&mut rt, "child"), 1, "a subagent's stream draws on its parent's page");
}

/// Where a commit's time goes before Rune sees the tree: the row descriptions, and the poll hook.
#[test]
#[ignore = "CPU probe; run --profile perf --ignored --nocapture --test-threads=1"]
fn view_parts_probe() {
    assert!(!cfg!(debug_assertions), "use --profile perf");
    let mut app = fixture();
    let mut rows = vec![];
    let mut polls = vec![];
    let mut row_allocs = 0;
    let mut poll_allocs = 0;
    let mut tx = rune::Tx::default();
    for n in 0..40 {
        let alloc = procwatch::alloc::thread_allocations();
        let start = rune::thread_cpu_ms();
        let keys = view::row_keys(&app, &app.transcripts["bench"]);
        std::hint::black_box(&keys);
        if n > 0 { rows.push(rune::thread_cpu_ms() - start); }
        row_allocs = procwatch::alloc::thread_allocations() - alloc;
        let alloc = procwatch::alloc::thread_allocations();
        let start = rune::thread_cpu_ms();
        std::hint::black_box(crate::poll(&mut app, n as f64 / 120.0, &mut tx));
        if n > 0 { polls.push(rune::thread_cpu_ms() - start); }
        poll_allocs = procwatch::alloc::thread_allocations() - alloc;
    }
    report("rows_cpu_ms", &mut rows);
    report("poll_cpu_ms", &mut polls);
    println!("METRIC rows_allocations={row_allocs} poll_allocations={poll_allocs}");
}

/// Opt-in GET-only live benchmark. No setup(), writes, saved UI, window or GPU.
/// Capture private pages with scripts/bench-cold-fetch.py, then set COLD_SESSION_DIR.
#[test]
#[ignore = "live GET benchmark; COLD_SESSION_DIR=... cargo test --release -p opencode-desktop cold_session_benchmark -- --ignored --nocapture"]
fn cold_session_benchmark() {
    use opencode_kit::{client::{self, Incoming, Request, Server}, model};
    use std::{sync::{Arc, mpsc::channel}, time::{Duration, Instant}};
    assert!(!cfg!(debug_assertions), "use --release");
    let dir = std::env::var("COLD_SESSION_DIR").expect("capture private pages first");
    let server = Server::new("http://127.0.0.1:4096");
    let (requests, writes, inbox) = client::connect(server.clone(), Arc::new(|| {}));
    drop(writes); // Never send a live write. SSE is read and discarded, not applied.
    let receive = |id: &str| loop {
        match inbox.recv_timeout(Duration::from_secs(20)).expect("fetch timeout") {
            Incoming::Messages { session, messages } if session == id => break messages,
            Incoming::Status(e) if e.starts_with("GET ") => panic!("{e}"),
            _ => {}
        }
    };
    let cases: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/cases.json")).unwrap()).unwrap();
    for case in cases.as_array().unwrap() {
        let (name, id) = (case[0].as_str().unwrap(), case[1].as_str().unwrap());
        let text = std::fs::read_to_string(format!("{dir}/{name}.json")).unwrap();
        let mut metrics = vec![vec![]; 12];
        for sample in 0..8 {
            let start = Instant::now();
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            let json_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let parsed = model::parse_messages(&value);
            let model_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert!(!parsed.is_empty());
            let start = Instant::now();
            let direct = server.get(&format!("/api/session/{id}/message?limit=80&order=desc")).unwrap();
            let direct_ms = start.elapsed().as_secs_f64() * 1000.0;
            drop(direct);

            // Fresh transcript/UI state, but the already-running application's text system
            // and the server's DB caches can be warm. No disk or transcript cache shortcut.
            let mut app = App::empty();
            app.tabs.push(crate::Tab { id: id.into(), title: name.into(), directory: String::new() });
            let (publish, arrivals) = channel();
            app.inbox = Some(arrivals);
            let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
            rt.frame(0.0);

            // A click immediately after a preceding session's messages arrive. Its pending
            // and inbox reads are still queued, exactly as App::open enqueues them.
            requests.send(Request::Session(id.into())).unwrap();
            requests.send(Request::Pending(id.into())).unwrap();
            requests.send(Request::Inbox(id.into())).unwrap();
            drop(receive(id));
            let click = Instant::now();
            rt.update(|s, tx| s.open(id, tx)); // Fixture source: no persistence or live writes.
            requests.send(Request::Session(id.into())).unwrap();
            rt.frame(0.0); // Loading shell can paint while the worker fetches.
            let messages = receive(id);
            let ready_ms = click.elapsed().as_secs_f64() * 1000.0;
            publish.send(Incoming::Messages { session: id.into(), messages }).unwrap();
            let start = Instant::now();
            rt.update(|s, tx| { crate::poll(s, 0.0, tx); });
            let merge_ms = start.elapsed().as_secs_f64() * 1000.0;
            rune::take_frame_phases();
            let start = Instant::now();
            rt.frame(0.0);
            let frame_ms = start.elapsed().as_secs_f64() * 1000.0;
            let phases = rune::take_frame_phases();
            let painted_ms = click.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(rt.scroll_at_bottom(&format!("scroll-{id}")), Some(true), "first transcript frame is at the tail");
            let start = Instant::now();
            rt.wheel(900.0, 350.0, 96.0);
            rt.frame(1.0 / 120.0);
            let wheel_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(rt.scroll_at_bottom(&format!("scroll-{id}")), Some(false), "first wheel moves away from the tail");
            if sample > 0 {
                let p = rune::PHASES;
                for (values, value) in metrics.iter_mut().zip([
                    json_ms, model_ms, ready_ms, merge_ms, frame_ms, painted_ms,
                    wheel_ms, phases[p], phases[p + 1], phases[p + 2], phases[p + 3],
                    direct_ms,
                ]) { values.push(value); }
            }
        }
        for (label, values) in ["json_ms", "model_ms", "queued_ready_ms", "merge_ms", "frame_ms", "queued_drawlist_ms", "wheel_ms", "view_cpu_ms", "commit_cpu_ms", "layout_cpu_ms", "text_cpu_ms", "direct_get_ms"].into_iter().zip(&mut metrics) {
            report(&format!("{name}_{label}"), values);
        }
    }
}

#[test]
#[ignore = "private captured pages; --release COLD_SESSION_DIR"]
fn cold_image_benchmark() {
    use std::{sync::{Arc, mpsc::channel}, time::{Duration, Instant}};
    let dir = std::env::var("COLD_SESSION_DIR").unwrap();
    assert!(!cfg!(debug_assertions));
    let cases: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/cases.json")).unwrap()).unwrap();
    for case in cases.as_array().unwrap() {
        let (name, id) = (case[0].as_str().unwrap(), case[1].as_str().unwrap());
        let raw = std::fs::read_to_string(format!("{dir}/{name}.json")).unwrap();
        let value = serde_json::from_str(&raw).unwrap();
        let mut times = vec![];
        for n in 0..8 {
            let mut app = App::empty();
            app.zoom = 1.3;
            app.route = Route::Session(id.into());
            app.transcripts.insert(id.into(), Transcript { session: id.into(), messages: opencode_kit::model::parse_messages(&value) });
            let (wake, woke) = channel();
            app.attach.set_waker(Arc::new(move || { let _ = wake.send(()); }));
            let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
            let start = Instant::now();
            rt.frame(0.0);
            let images = rt.state.attach.image_handles();
            if images.is_empty() { println!("METRIC name={name}_mounted_images count=0"); break; }
            for img in &images { img.wait().unwrap(); }
            // The worker, not mouse movement or a timer, wakes the redraw.
            woke.recv_timeout(Duration::from_secs(10)).expect("image completion must wake the app");
            rt.update(|_, _| {});
            rt.frame(0.0);
            if n > 0 { times.push(start.elapsed().as_secs_f64() * 1000.0); }
        }
        if !times.is_empty() { report(&format!("{name}_frame_and_images_ms"), &mut times); }
    }
}

#[test]
#[ignore = "live GET-only latest-page benchmark; --release COLD_SESSION_DIR"]
fn latest_page_benchmark() {
    use crate::session_pages::{Key, Pages};
    use opencode_kit::client::Server;
    use std::{sync::Arc, time::{Duration, Instant}};
    assert!(!cfg!(debug_assertions));
    let dir = std::env::var("COLD_SESSION_DIR").unwrap();
    let cases: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/cases.json")).unwrap()).unwrap();
    for case in cases.as_array().unwrap() {
        let (name, id) = (case[0].as_str().unwrap(), case[1].as_str().unwrap());
        let mut cold = vec![];
        let mut warm = vec![];
        for n in 0..8 {
            let server = Server::new("http://127.0.0.1:4096");
            let pages = Pages::new(Arc::new(move |path| server.read(path)), None, Arc::new(|| {}));
            let start = Instant::now();
            pages.request(Key { id: id.into(), updated: 0 }, true);
            let page = pages.results.recv_timeout(Duration::from_secs(20)).unwrap().unwrap();
            let ready = start.elapsed();
            let mut app = App::empty();
            app.route = Route::Session(id.into());
            app.zoom = 1.3;
            app.attach.adopt(&page.images);
            app.transcripts.insert(id.into(), Transcript { session: id.into(), messages: page.messages.clone() });
            let frame = Instant::now();
            let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let elapsed = (ready + frame.elapsed()).as_secs_f64() * 1000.0;
            if n > 0 { cold.push(elapsed); }
            let mut app = App::empty();
            app.zoom = 1.3;
            app.pages = Some(pages);
            let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let click = Instant::now();
            rt.update(|s, tx| s.open(id, tx));
            rt.frame(0.0);
            assert!(!rt.state.transcripts[id].messages.is_empty());
            assert_eq!(rt.scroll_at_bottom(&format!("scroll-{id}")), Some(true));
            if n > 0 { warm.push(click.elapsed().as_secs_f64() * 1000.0); }
        }
        report(&format!("{name}_latest_cold_ms"), &mut cold);
        report(&format!("{name}_prefetched_click_ms"), &mut warm);
    }
}

#[test]
#[ignore = "private captured pages; --release COLD_SESSION_DIR"]
fn prefetched_image_benchmark() {
    use std::{sync::Arc, time::Instant};
    let dir = std::env::var("COLD_SESSION_DIR").unwrap();
    assert!(!cfg!(debug_assertions));
    let cases: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/cases.json")).unwrap()).unwrap();
    for case in cases.as_array().unwrap() {
        let (name, id) = (case[0].as_str().unwrap(), case[1].as_str().unwrap());
        let mut value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/{name}.json")).unwrap()).unwrap();
        // Captures are newest-first 80-message pages. Production only prepares the latest 12;
        // older history remains demand-loaded, and deliberately exceeds the predecode budget.
        value["data"].as_array_mut().unwrap().truncate(crate::session_pages::LATEST);
        let mut clicks = vec![];
        let mut preparation = vec![];
        for n in 0..8 {
            let mut app = App::empty();
            app.zoom = 1.3;
            app.route = Route::Session(id.into());
            let messages = opencode_kit::model::parse_messages(&value);
            let source = messages.clone();
            let start = Instant::now();
            let images = std::thread::spawn(move || {
                let cache = rune::ImageCache::new(Arc::new(|| {}));
                let images = crate::attachments::prepare_images(&source, &cache);
                for image in &images { image.wait(); }
                images
            }).join().unwrap();
            if n > 0 { preparation.push(start.elapsed().as_secs_f64() * 1000.0); }
            let start = Instant::now();
            app.attach.adopt(&images);
            app.transcripts.insert(id.into(), Transcript { session: id.into(), messages });
            let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let mounted = rt.state.attach.image_handles();
            assert!(mounted.iter().all(|img| img.is_ready()), "{name}: every requested image must have pixels on first paint");
            if n > 0 { clicks.push(start.elapsed().as_secs_f64() * 1000.0); }
        }
        report(&format!("{name}_image_preparation_ms"), &mut preparation);
        report(&format!("{name}_prefetched_images_click_ms"), &mut clicks);
    }
}
