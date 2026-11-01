use super::*;

fn el(path: &str, parent: Option<usize>, rect: [f32; 4]) -> ElementInfo {
    stub_el(path, parent, rect, None)
}

/// A timeline: a viewport, its content offset by `y`, and three rows riding it, in view at the top.
fn timeline(y: f32) -> Vec<ElementInfo> {
    let mut els = vec![el("root/scroll", None, [0.0, 0.0, 800.0, 600.0])];
    let mut content = el("root/scroll/content", Some(0), [0.0, y, 800.0, 2000.0]);
    content.visible = [0.0, 0.0, 800.0, 600.0];
    els.push(content);
    for i in 0..3 {
        els.push(el(&format!("root/scroll/content/row-{i}"), Some(1), [0.0, y + 100.0 * i as f32 + 300.0, 800.0, 80.0]));
    }
    els
}

/// Feeds frames (elements and whether user input landed) and returns the lines written.
fn run(frames: impl IntoIterator<Item = (Vec<ElementInfo>, bool)>) -> Vec<Value> {
    let mut watch = Watch::default();
    let events = Events::default();
    let mut lines = Vec::new();
    for (k, (els, input)) in frames.into_iter().enumerate() {
        lines.extend(watch.push(&els, Window::new((800.0, 600.0)), input, false, &events, k as f64 * 8.33));
    }
    lines
}

#[test]
fn a_row_that_jumps_with_no_input_is_logged_once() {
    let frames = (0..40).map(|k| {
        let mut els = timeline(-1000.0);
        els.push(el("root/card", None, [20.0, if k < 20 { 300.0 } else { 120.0 }, 200.0, 40.0]));
        (els, false)
    });
    let lines = run(frames);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    let line = &lines[0];
    assert_eq!(line["rule"], "jump");
    assert_eq!(line["keys"][0], "card");
    assert_eq!(line["deltas"][0]["axis"], "y");
    assert_eq!(line["deltas"][0]["delta"], -180.0);
    assert!(line["frames"].as_array().unwrap().len() <= CONTEXT);
    assert!(line["time"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn the_same_row_drawn_twice_is_logged_once() {
    let frames = (0..30).map(|k| {
        let mut els = timeline(-1000.0);
        els.push(el("root/scroll/content/row-0#dup1", Some(1), [0.0, 300.0, 800.0, 80.0]));
        els[5].opacity = 1.0 - k as f32 / 60.0;
        (els, false)
    });
    let lines = run(frames);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0]["rule"], "twice");
    assert_eq!(lines[0]["keys"][0], "row-0#dup1");
}

#[test]
fn an_animated_scroll_and_a_user_scroll_log_nothing() {
    // A spring from rest down to the end of the timeline.
    let spring = (0..90).map(|k| {
        let t = k as f32 / 10.0;
        (timeline(-1400.0 * (1.0 - (1.0 + t) * (-t).exp())), false)
    });
    assert!(run(spring).is_empty());
    // Wheel ticks: the content steps 40 pt in the frame each input lands, still between.
    let wheel = |input: bool| (0..40).map(move |k| (timeline(-40.0 * (k / 10) as f32), input && k > 0 && k % 10 == 0));
    assert!(run(wheel(true)).is_empty());
    let lines = run(wheel(false));
    assert_eq!(lines.len(), 3, "the same steps with no one scrolling: {lines:#?}");
}

#[test]
fn the_ring_stays_inside_its_caps() {
    let mut watch = Watch::default();
    let events = Events::default();
    for k in 0..200 {
        watch.push(&timeline(-1000.0), Window::new((800.0, 600.0)), false, false, &events, k as f64);
        assert!(watch.ring.len() <= FRAMES && watch.bytes <= BYTES);
    }
    let mut events = Events::default();
    for _ in 0..100 { events.note("session.text.delta"); }
    for k in 0..40 { events.note(&format!("type.{k}")); }
    assert_eq!(events.recent.len(), EVENTS);
}

#[test]
fn iso_times() {
    assert_eq!(iso(0.0), "1970-01-01T00:00:00.000Z");
    assert_eq!(iso(1_793_136_005_123.0), "2026-10-27T21:20:05.123Z");
}

/// The log's cost per drawn frame on the 800-turn benchmark timeline: on the UI thread, Rune's
/// light snapshot and the frame handed over (`Feed::frame`); on the watch thread, taking it
/// (`Worker::take`); and the check on a frame that triggers it, also on the watch thread.
#[test]
#[ignore = "CPU benchmark; run --profile perf --ignored --nocapture --test-threads=1"]
fn motion_log_benchmark() {
    assert!(!cfg!(debug_assertions), "use --profile perf");
    let mut rt = rune::Runtime::new(crate::perf::fixture(), crate::view::app, (1280.0, 820.0), rune::Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..120 { if !rt.frame(1.0 / 120.0) { break; } }
    let (mut snapshot_ms, mut buf) = (Vec::new(), Vec::new());
    let (mut ui_ms, mut worker_ms, mut elements_ms, mut check_ms, mut elements) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), 0);
    for sample in 0..6 {
        let mut feed = Feed::default();
        let mut worker = Worker::new(std::env::temp_dir().join("motion-log-bench.jsonl"));
        for k in 0..240 {
            // Scroll up and back down, and stream words, so the frames move.
            rt.wheel(900.0, 350.0, if (k / 40) % 2 == 0 { 24.0 } else { -24.0 });
            rt.update(|s, _| { s.apply_event("session.text.delta", &serde_json::json!({"sessionID":"bench", "assistantMessageID":"stream", "ordinal":0, "delta":" word"})); });
            rt.frame(1.0 / 120.0);
            let start = Instant::now();
            let mut frame = feed.frame(&rt, false, k as f64 * 8.33);
            let ui = start.elapsed().as_secs_f64() * 1000.0;
            elements = frame.els.len();
            let start = Instant::now();
            std::hint::black_box(worker.take(&mut frame));
            let taken = start.elapsed().as_secs_f64() * 1000.0;
            feed.spare.push(frame.els);
            let start = Instant::now();
            rt.motion_elements(&mut buf);
            let snapshot = start.elapsed().as_secs_f64() * 1000.0;
            // For comparison, the full list the first version read on the UI thread.
            let start = Instant::now();
            std::hint::black_box(rt.elements());
            let listed = start.elapsed().as_secs_f64() * 1000.0;
            if sample > 0 && k > 0 { snapshot_ms.push(snapshot); ui_ms.push(ui); worker_ms.push(taken); elements_ms.push(listed); }
        }
        let start = Instant::now();
        std::hint::black_box(check(&worker.watch.recording(rt.viewport)));
        if sample > 0 { check_ms.push(start.elapsed().as_secs_f64() * 1000.0); }
    }
    for (name, v) in [("ui_ms", &mut ui_ms), ("snapshot_ms", &mut snapshot_ms), ("worker_ms", &mut worker_ms), ("elements_ms", &mut elements_ms), ("check_ms", &mut check_ms)] {
        v.sort_by(f64::total_cmp);
        let at = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
        println!("METRIC name={name} p50={:.4} p95={:.4} max={:.4} n={} elements={elements}", at(0.5), at(0.95), at(1.0), v.len());
    }
}
