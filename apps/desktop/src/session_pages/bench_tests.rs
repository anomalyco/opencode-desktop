//! Opt-in click-to-frame benchmark. Only the fixture and a delayed, paginated read mock.
use super::*;
use crate::{App, Route};

const TARGET: &str = "ses_bench_target";
const PARENT: &str = "ses_bench_parent";
const MARKER: &str = "The selected session is ready.";

fn wire(total: usize) -> String {
    let data: Vec<_> = (total.saturating_sub(LATEST)..total).map(|i| json!({
        "id": format!("msg_{i:06}"), "type": "user", "time": {"created": i},
        "text": if i == total - 1 { MARKER.to_owned() } else { format!("Earlier prompt {i}: {}", "Review the implementation and its edge cases. ".repeat(20)) }
    })).collect();
    serde_json::to_string(&json!({"data": data})).unwrap()
}

fn sample(delay: u64, total: usize, child: bool, hover: bool, reopen: bool) -> f64 {
    let data = wire(total);
    let (wake, wakes) = channel();
    let pages = Pages::new(Arc::new(move |path| {
        assert!(path.contains("?limit=12&order=desc"), "{path}");
        std::thread::sleep(Duration::from_millis(delay));
        Ok(serde_json::from_str(&data).unwrap())
    }), None, Arc::new(move || { let _ = wake.send(()); }));
    // The application keeps its pool for its lifetime; exclude worker startup from a click.
    pages.request(Key { id: "ses_worker_warmup".into(), updated: 0 }, true);
    pages.results.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    let mut app = App::empty();
    app.zoom = 1.6;
    app.route = Route::Session(PARENT.into());
    *app.sessions = vec![model::SessionInfo { id: PARENT.into(), title: "Parent session".into(), ..Default::default() },
        model::SessionInfo { id: TARGET.into(), title: "Review the change".into(), parent: child.then(|| PARENT.into()), ..Default::default() }];
    let parent_messages = if child { model::parse_messages(&json!({"data":[{
        "id":"msg_parent", "type":"assistant", "time":{"created":1,"completed":2},
        "content":[{"id":"part_child", "type":"tool", "name":"subagent", "state":{"status":"completed", "input":{"description":"Review the change", "subagent_type":"general"}, "metadata":{"sessionID":TARGET}, "output":"Done"}}]
    }]})) } else { vec![Message::User { id: "msg_previous".into(), text: "Previous session".into() }] };
    app.transcripts.insert(PARENT.into(), model::Transcript { session: PARENT.into(), messages: parent_messages });
    app.pages.set(pages);
    let mut rt = app.into_runtime();
    for _ in 0..3 { rt.frame(1.0 / 60.0); }
    if hover {
        // Use the real card/sidebar handler, not a direct call to prefetch_session.
        let key = if child { "agent-line".to_owned() } else { format!("t-{TARGET}") };
        if !child {
            rt.update(|s, _| s.tabs.push(crate::tabs::Tab::session(TARGET, "Review the change", String::new())));
            for _ in 0..3 { rt.frame(1.0 / 60.0); }
        }
        let (x, y) = rt.find(&key).unwrap_or_else(|| panic!("missing {key}: {:?}", rt.elements().iter().filter_map(|e| e.key.clone()).collect::<Vec<_>>()));
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        // A person reads the row for 600 ms before pressing it (both measured delays fit).
        let until = Instant::now() + Duration::from_millis(600);
        while Instant::now() < until {
            let _ = wakes.recv_timeout(until.saturating_duration_since(Instant::now()));
            rt.update_if(|s, tx| s.poll_pages(tx));
            rt.frame(0.0);
        }
        if !child { assert!(rt.state.pages.pool.as_ref().unwrap().peek(TARGET).is_some(), "sidebar hover at {x},{y} did not prefetch"); }
    }
    let open = |rt: &mut rune::Runtime<App>| {
        let start = Instant::now();
        rt.update(|s, tx| if child { s.open_child(PARENT, TARGET, "Review the change", "general", tx) } else { s.open(TARGET, tx) });
        loop {
            rt.update_if(|s, tx| s.poll_pages(tx));
            rt.frame(1.0 / 60.0);
            if rt.state.painted_session().is_some_and(|(id, n)| id == TARGET && n > 0)
                && rt.elements().iter().any(|e| !e.exiting && e.text.as_deref() == Some(MARKER)) { break; }
            assert!(start.elapsed() < Duration::from_secs(5), "never painted");
            let _ = wakes.recv_timeout(Duration::from_millis(16));
        }
        start.elapsed().as_secs_f64() * 1000.0
    };
    let first = open(&mut rt);
    if !reopen { return first; }
    rt.update(|s, tx| { s.route = Route::Session(PARENT.into()); tx.settle(); });
    rt.frame(0.0);
    open(&mut rt)
}

#[test]
#[ignore = "wall-clock benchmark: run alone with --ignored --nocapture"]
fn session_open_benchmark() {
    for delay in [50, 300] {
        for total in [4, 2000] {
            for child in [false, true] {
                for (mode, hover, reopen) in [("cold", false, false), ("hover", true, false), ("reopen", false, true)] {
                    let mut times: Vec<_> = (0..10).map(|_| sample(delay, total, child, hover, reopen)).skip(1).collect();
                    times.sort_by(f64::total_cmp);
                    println!("METRIC kind={} history={total} delay_ms={delay} mode={mode} median_ms={:.2} p95_ms={:.2}", if child { "child" } else { "top" }, times[4], times[8]);
                }
            }
        }
    }
}
