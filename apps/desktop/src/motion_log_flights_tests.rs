use super::super::{Events, Feed, Frame, Watch, Worker};
use super::*;
use rune::ElementInfo;

const PROMPT: [f32; 4] = [100.0, 500.0, 600.0, 40.0];
const REST: [f32; 4] = [400.0, 300.0, 300.0, 50.0];
const BUBBLE: &str = "root/timeline/u-msg_1/#0/bubble";

fn el(path: &str, rect: [f32; 4], text: Option<&str>) -> ElementInfo {
    super::super::stub_el(path, None, rect, text)
}

/// A critically damped spring from `from` to `to`, `k` frames in; exactly `to` once within 0.005.
fn spring(from: [f32; 4], to: [f32; 4], k: usize) -> [f32; 4] {
    let t = k as f32 / 8.0;
    let left = (1.0 + t) * (-t).exp();
    let mut r = [0.0; 4];
    for a in 0..4 {
        let d = (from[a] - to[a]) * left;
        r[a] = if d.abs() < 0.005 { to[a] } else { to[a] + d };
    }
    r
}

/// Five frames of the prompt holding words, then the send: its words clear as the bubble mounts
/// at `path(0)`, and the bubble follows `path(k)`. Every element is mounted the first frame it
/// shows, as `Worker::take` does. Returns the lines and the watch.
fn send(path: impl Fn(usize) -> [f32; 4]) -> (Vec<Value>, Watch) {
    let mut watch = Watch::default();
    let events = Events::default();
    let mut seen = std::collections::HashSet::new();
    let mut lines = Vec::new();
    for k in 0..400 {
        let mut els = vec![el("root/composer/editor/prompt", PROMPT, (k < 5).then_some("9f86d081884c7d65"))];
        if k >= 5 { els.push(el(BUBBLE, path(k - 5), None)); }
        for e in &els {
            if seen.insert(e.path.clone()) { watch.mount(&e.path, e.key.as_deref()); }
        }
        lines.extend(watch.push(&els, Window::new((800.0, 600.0)), false, false, &events, k as f64 * 8.33));
    }
    (lines, watch)
}

/// The flight rules logged; a snap also shows as the existing log's `jump`.
fn rules(lines: &[Value]) -> Vec<&str> {
    lines.iter().filter(|l| l["flight"] == true).map(|l| l["rule"].as_str().unwrap()).collect()
}

#[test]
fn a_clean_flight_logs_nothing() {
    let (lines, watch) = send(|k| spring([90.0, 495.0, 300.0, 50.0], REST, k));
    assert!(lines.is_empty(), "{lines:#?}");
    assert_eq!(watch.flights.landed, 1);
    assert_eq!(watch.flights.flying(), 0);
}

#[test]
fn a_flight_that_starts_away_from_its_source_is_logged() {
    let (lines, _) = send(|k| spring([100.0, 40.0, 300.0, 50.0], REST, k));
    assert_eq!(rules(&lines), ["wrong-start"], "{lines:#?}");
    let line = &lines[0];
    assert_eq!(line["flight"], true);
    assert_eq!(line["keys"][0], "bubble");
    assert_eq!(line["source"]["key"], "prompt");
    assert_eq!(line["window"], json!([800.0, 600.0]));
    assert_eq!(line["zoom"], 1.0);
    assert_eq!(line["at_end"], true);
    let frames = line["frames"].as_array().unwrap();
    assert_eq!(frames[0]["at"], "start");
    assert_eq!(frames[0]["rect"], json!([100.0, 40.0, 300.0, 50.0]));
    assert_eq!(frames.last().unwrap()["at"], "rest");
    assert_eq!(frames.last().unwrap()["rect"], json!(REST));
    assert!(frames.iter().filter(|f| f["at"] == "mid").count() == 4);
    assert!(line["detail"].as_str().unwrap().contains("outside its source prompt"), "{}", line["detail"]);
}

#[test]
fn a_flight_from_off_the_window_is_logged() {
    let (lines, _) = send(|k| spring([-320.0, 300.0, 300.0, 50.0], REST, k));
    assert_eq!(rules(&lines), ["wrong-start", "wrong-side"], "{lines:#?}");
}

#[test]
fn a_flight_that_heads_away_first_is_logged() {
    // Ten frames left, away from where it lands, then a spring there.
    let away = [30.0, 495.0, 300.0, 50.0];
    let (lines, _) = send(|k| if k < 10 { [90.0 - 6.0 * k as f32, 495.0, 300.0, 50.0] } else { spring(away, REST, k - 10) });
    assert_eq!(rules(&lines), ["reversal"], "{lines:#?}");
    assert!(lines[0]["detail"].as_str().unwrap().starts_with("x moves 60.0 pt away"), "{}", lines[0]["detail"]);
}

#[test]
fn a_flight_that_overshoots_far_is_logged() {
    // An underdamped spring: y swings well past where it lands.
    let (lines, _) = send(|k| {
        let t = k as f32 / 8.0;
        let left = (-0.25 * t).exp() * t.cos();
        if left.abs() * 195.0 < 0.005 { REST } else { [400.0, 300.0 + 195.0 * left, 300.0, 50.0] }
    });
    assert!(rules(&lines).contains(&"reversal"), "{lines:#?}");
    assert!(lines.iter().any(|l| l["detail"].as_str().unwrap().starts_with("y swings")), "{lines:#?}");
}

#[test]
fn a_flight_that_snaps_at_the_end_is_logged() {
    // It springs toward 20 pt below its place and jumps there when 3 pt from that.
    let short = [400.0, 320.0, 300.0, 50.0];
    let (lines, _) = send(|k| {
        let r = spring([90.0, 495.0, 300.0, 50.0], short, k);
        if (r[1] - short[1]).abs() < 3.0 { REST } else { r }
    });
    assert_eq!(rules(&lines), ["end-snap"], "{lines:#?}");
    let snap = lines.iter().find(|l| l["rule"] == "end-snap").unwrap();
    let frames = snap["frames"].as_array().unwrap();
    let end = frames.iter().find(|f| f["at"] == "end").unwrap();
    assert!(end["rect"][1].as_f64().unwrap() > 320.0);
}

#[test]
fn a_flight_at_the_wrong_size_is_logged() {
    // It flies 10% large, as under a zoom it didn't account for, and takes its size at the end.
    let big = [90.0, 495.0, 330.0, 55.0];
    let (lines, _) = send(|k| {
        let r = spring(big, [REST[0], REST[1], 330.0, 55.0], k);
        if (r[0] - REST[0]).abs() < 0.01 && (r[1] - REST[1]).abs() < 0.01 { REST } else { [r[0], r[1], 330.0, 55.0] }
    });
    assert!(rules(&lines).contains(&"size"), "{lines:#?}");
    let size = lines.iter().find(|l| l["rule"] == "size").unwrap();
    assert!(size["detail"].as_str().unwrap().contains("+10.0%"), "{}", size["detail"]);
}

#[test]
fn a_bubble_that_mounts_while_the_prompt_keeps_its_words_is_no_flight() {
    // As a session opening: the bubble mounts and moves, but no words left a source for it.
    let mut watch = Watch::default();
    let events = Events::default();
    for k in 0..200 {
        let mut els = vec![el("root/composer/editor/prompt", PROMPT, Some("9f86d081884c7d65"))];
        if k >= 5 { els.push(el(BUBBLE, spring([100.0, 40.0, 300.0, 50.0], REST, k - 5), None)); }
        if k == 5 { watch.mount(BUBBLE, Some("bubble")); }
        assert!(watch.push(&els, Window::new((800.0, 600.0)), false, false, &events, k as f64 * 8.33).is_empty());
        assert_eq!(watch.flights.flying(), 0);
    }
    assert_eq!(watch.flights.landed, 0);
}

#[test]
fn destinations_and_sources_are_named_as_rune_names_them() {
    assert_eq!(capture("u-{q}/bubble", "timeline/u-msg_1/#0/#1/bubble", "bubble").as_deref(), Some("msg_1"));
    assert_eq!(capture("u-{q}/bubble", "timeline/t-msg_1/#0/bubble", "bubble"), None);
    assert_eq!(capture("queued-{q}", "queue-list/queued-msg_2", "queued-msg_2").as_deref(), Some("msg_2"));
    assert_eq!(capture("queued-{q}", "queue-list/queued-msg_2/label", "label"), None);
    assert!(selects("queued-msg_2/label", "dock/queued-msg_2/#0/label", Some("label")));
    assert!(!selects("queued-msg_2/label", "dock/queued-msg_3/#0/label", Some("label")));
    assert!(selects("prompt", "root/composer/editor/prompt", Some("prompt")));
}

/// Real sends in the fixture app, through the live log's feed and worker: the bubble's flight
/// out of the composer (at 100% and zoomed in), and a queued row's while a reply streams, are
/// followed to rest and log nothing.
#[test]
fn real_sends_are_followed_and_log_nothing() {
    use crate::motion_gate::{Circ, DT, Run};
    for (circ, queue) in [(Circ::default(), false), (Circ { zoomed: true, ..Circ::default() }, false), (Circ { unplayed: true, ..Circ::default() }, true)] {
        let mut r = Run::new(circ);
        if queue {
            r.start_stream();
            r.play(1.2);
        }
        r.type_prompt();
        let mut feed = Feed::default();
        let mut worker = Worker::new(std::env::temp_dir().join("motion-log-flight-test.jsonl"));
        let mut lines = Vec::new();
        for k in 0..200 {
            if k == 10 { r.rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods { cmd: queue, ..Default::default() } }); }
            r.rt.frame(DT);
            let mut f = feed.frame(&r.rt, k == 10, k as f64 * 8.33);
            lines.extend(worker.take(&mut f));
            feed.spare.push(std::mem::take(&mut f.els));
        }
        let lines: Vec<&Value> = lines.iter().filter(|l| l["flight"] == true).collect();
        assert!(lines.is_empty(), "{}: {lines:#?}", circ.name());
        assert_eq!(worker.watch.flights.landed, 1, "{}: the send's flight is followed", circ.name());
    }
}

/// What following a flight costs the watch thread per frame while one is in the air: two workers
/// take the same frames of a real send, one following flights and one not, timed alternately.
/// The UI thread's share is the window's zoom and timeline end, read into each frame.
#[test]
#[ignore = "CPU benchmark; run --profile perf --ignored --nocapture --test-threads=1"]
fn flight_log_benchmark() {
    use crate::motion_gate::{Circ, DT, Run};
    use std::time::Instant;
    assert!(!cfg!(debug_assertions), "use --profile perf");
    let copy = |f: &Frame| Frame {
        seq: f.seq, els: f.els.clone(), paths: f.paths.clone(), reset: f.reset, window: f.window, input: f.input,
        load: f.load, events: f.events.clone(), ms: f.ms,
    };
    let (mut on, mut off, mut diff, mut ui, mut flying_frames) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), 0);
    for sample in 0..8 {
        let mut r = Run::new(Circ::default());
        r.type_prompt();
        let mut feed = Feed::default();
        let mut a = Worker::new(std::env::temp_dir().join("flight-bench-a.jsonl"));
        let mut b = Worker::new(std::env::temp_dir().join("flight-bench-b.jsonl"));
        b.watch.flights.off = true;
        for k in 0..160 {
            if k == 10 { r.rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods::default() }); }
            r.rt.frame(DT);
            let start = Instant::now();
            let mut fa = feed.frame(&r.rt, k == 10, k as f64 * 8.33);
            let ui_ms = start.elapsed().as_secs_f64() * 1000.0;
            let mut fb = copy(&fa);
            let flying = a.watch.flights.flying() > 0;
            let time = |w: &mut Worker, f: &mut Frame| {
                let start = Instant::now();
                std::hint::black_box(w.take(f));
                start.elapsed().as_secs_f64() * 1000.0
            };
            let (ta, tb) = if k % 2 == 0 { let ta = time(&mut a, &mut fa); (ta, time(&mut b, &mut fb)) } else { let tb = time(&mut b, &mut fb); (time(&mut a, &mut fa), tb) };
            if sample > 0 && flying {
                flying_frames += 1;
                on.push(ta);
                off.push(tb);
                diff.push(ta - tb);
                ui.push(ui_ms);
            }
            feed.spare.push(fa.els);
        }
        assert_eq!(a.watch.flights.landed, 1);
    }
    for (name, v) in [("worker_following_ms", &mut on), ("worker_baseline_ms", &mut off), ("following_minus_baseline_ms", &mut diff), ("ui_ms", &mut ui)] {
        v.sort_by(f64::total_cmp);
        let at = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
        println!("METRIC name={name} p50={:.4} p95={:.4} max={:.4} n={} flying_frames={flying_frames}", at(0.5), at(0.95), at(1.0), v.len());
    }
}
