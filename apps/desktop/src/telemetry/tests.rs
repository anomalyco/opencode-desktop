use super::*;
use crate::{App, Route, session_pages::Pages, view};
use rune::{Env, Runtime};
use serde_json::json;
use std::sync::Arc;

fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<Value>) {
    otlp_lite::capture(|| {
        drawn(None, (Instant::now(), None, None));
        let r = f();
        OPEN.with(|o| *o.borrow_mut() = None);
        r
    })
}

fn named<'a>(spans: &'a [Value], name: &str) -> &'a Value {
    spans.iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("no {name} span in {:?}", spans.iter().map(|s| &s["name"]).collect::<Vec<_>>()))
}

fn attr<'a>(span: &'a Value, key: &str) -> &'a Value {
    &span["attributes"].as_array().unwrap().iter().find(|a| a["key"] == key).unwrap_or_else(|| panic!("no {key} on {}", span["name"]))["value"]
}

fn wire(n: usize) -> Value {
    let data: Vec<Value> = (0..n).rev().map(|i| json!({"id": format!("msg_{i:04}"), "type": "user", "time": {"created": 1_792_000_000_000i64 + i as i64}, "text": format!("Prompt {i}")})).collect();
    json!({"data": data, "cursor": {"next": "older"}})
}

#[test]
fn an_open_is_one_trace_from_click_to_its_first_painted_frame_then_its_older_page() {
    let (_, spans) = capture(|| {
        open_begin("ses_a", true);
        let t0 = Instant::now();
        let read = Read { source: "server", fetch: Some((t0, t0 + Duration::from_millis(30))), first_byte: Some(t0 + Duration::from_millis(20)), bytes: 35_000,
            parse: Some((t0 + Duration::from_millis(30), t0 + Duration::from_millis(32))), messages: 12, thread: "session-open".into(), ..Default::default() };
        page("ses_a", false, &read, (t0 + Duration::from_millis(33), t0 + Duration::from_millis(34)));
        committed("ses_a", false);
        drawn(Some(("ses_b", 4)), (Instant::now(), Some(2.0), Some(1.0)));
        drawn(Some(("ses_a", 12)), (Instant::now(), Some(2.0), Some(1.0)));
        page("ses_a", true, &Read { source: "server", messages: 68, inline_images: 29, inline_image_bytes: 6_600_000, ..Default::default() }, (Instant::now(), Instant::now()));
        drawn(Some(("ses_a", 80)), (Instant::now(), Some(9.0), Some(1.0)));
        assert!(OPEN.with(|o| o.borrow().is_none()), "the trace ends with the older page's frame");
    });
    let root = named(&spans, "session.open");
    assert_eq!(attr(root, "session.id")["stringValue"], "ses_a");
    assert_eq!(attr(root, "rows")["intValue"], "12");
    assert!(root.get("parentSpanId").is_none());
    for name in ["fetch", "parse", "show", "hold", "first_paint", "history"] {
        let s = named(&spans, name);
        assert_eq!(s["traceId"], root["traceId"], "{name}");
        assert_eq!(s["parentSpanId"], root["spanId"], "{name} hangs off the open");
    }
    assert_eq!(attr(named(&spans, "fetch"), "server_ms")["doubleValue"], 20.0);
    assert_eq!(attr(named(&spans, "history"), "inline_images")["intValue"], "29");
    assert_eq!(named(&spans, "layout")["parentSpanId"], named(&spans, "first_paint")["spanId"]);
    assert_eq!(spans.iter().filter(|s| s["name"] == "first_paint").count(), 1);
    assert!(spans.iter().any(|s| s["name"] == "history.paint"));
    let (start, end) = (|s: &Value| s["startTimeUnixNano"].as_str().unwrap().parse::<u128>().unwrap(), |s: &Value| s["endTimeUnixNano"].as_str().unwrap().parse::<u128>().unwrap());
    assert!(end(root) >= start(root) && start(named(&spans, "fetch")) >= start(root));
    assert!(spans.iter().all(|s| s["traceId"].as_str().unwrap().len() == 32 && s["spanId"].as_str().unwrap().len() == 16));
}

#[test]
fn spans_come_from_the_real_gate_and_page_reader() {
    let (_, spans) = capture(|| {
        let pages = Pages::new(Arc::new(|_| Ok(wire(12))), None, Arc::new(|| {}));
        let mut app = App::empty();
        app.route = Route::here("ses_previous".into());
        app.transcripts.insert("ses_previous".into(), opencode_kit::model::Transcript { session: "ses_previous".into(), messages: vec![] });
        app.pages.set(pages);
        let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.update(|s, tx| s.open("ses_target", tx));
        let start = Instant::now();
        while OPEN.with(|o| o.borrow().as_ref().is_some_and(|o| o.root.is_some())) {
            assert!(start.elapsed() < Duration::from_secs(5), "never painted");
            std::thread::sleep(Duration::from_millis(1));
            rt.update_if(|s, tx| s.poll_pages(tx));
            rt.frame(1.0 / 60.0);
            drawn(rt.state.painted_session(), (Instant::now(), Some(1.0), Some(0.5)));
        }
    });
    let root = named(&spans, "session.open");
    assert_eq!(attr(root, "session.id")["stringValue"], "ses_target");
    let fetch = named(&spans, "fetch");
    assert_eq!(attr(fetch, "source")["stringValue"], "server");
    assert_eq!(attr(fetch, "thread")["stringValue"], "session-open");
    assert_eq!(attr(named(&spans, "parse"), "messages")["intValue"], "12");
    assert_eq!(attr(named(&spans, "hold"), "loading")["boolValue"], false);
    named(&spans, "first_paint");
}

#[test]
fn spans_carry_no_message_text() {
    let (_, spans) = capture(|| {
        open_begin("ses_a", false);
        let messages = opencode_kit::model::parse_messages(&wire(3));
        let read = Read { source: "server", fetch: Some((Instant::now(), Instant::now())), messages: messages.len(), ..Default::default() };
        page("ses_a", false, &read, (Instant::now(), Instant::now()));
        committed("ses_a", false);
        drawn(Some(("ses_a", 3)), (Instant::now(), None, None));
    });
    assert!(!spans.is_empty());
    assert!(!serde_json::to_string(&spans).unwrap().contains("Prompt"));
}

/// Launch reads are process-wide; restored-session tests take turns with them.
static LAUNCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn a_restored_session_is_timed_from_launch_through_its_first_reads_to_a_server_page() {
    let _launch = LAUNCH.lock().unwrap_or_else(|e| e.into_inner());
    let server = || Read { source: "server", fetch: Some((Instant::now(), Instant::now())), ..Default::default() };
    let (_, spans) = capture(|| {
        launched();
        launch_read("/api/project", Instant::now());
        open_restored("ses_a", 0);
        committed("ses_a", false);
        drawn(Some(("ses_a", 0)), (Instant::now(), Some(1.0), None));
        page("ses_a", false, &server(), (Instant::now(), Instant::now()));
        drawn(Some(("ses_a", 12)), (Instant::now(), Some(1.0), None));
    });
    let root = named(&spans, "session.open");
    assert_eq!(attr(root, "restored")["boolValue"], true);
    assert_eq!(attr(root, "outcome")["stringValue"], "painted");
    assert_eq!(attr(named(&spans, "launch.read"), "path")["stringValue"], "/api/project");
    for name in ["launch.read", "launch", "fetch", "first_paint"] { assert_eq!(named(&spans, name)["parentSpanId"], root["spanId"], "{name}"); }
}

#[test]
fn a_restored_session_already_painted_from_disk_ends_when_its_server_page_is_shown() {
    let _launch = LAUNCH.lock().unwrap_or_else(|e| e.into_inner());
    let (_, spans) = capture(|| {
        open_restored("ses_a", 12);
        // Launch draws the disk copy before it commits the restored route.
        drawn(Some(("ses_a", 12)), (Instant::now(), Some(1.0), None));
        committed("ses_a", false);
        assert!(OPEN.with(|o| o.borrow().as_ref().unwrap().root.is_some()), "its disk copy doesn't end it");
        // The server page matches the disk copy, so no frame redraws it.
        page("ses_a", false, &Read { source: "server", fetch: Some((Instant::now(), Instant::now())), ..Default::default() }, (Instant::now(), Instant::now()));
    });
    assert_eq!(attr(named(&spans, "session.open"), "outcome")["stringValue"], "cached");
}

#[test]
fn an_open_never_painted_ends_after_follow_instead_of_at_the_next_open() {
    let (_, spans) = capture(|| {
        open_begin("ses_a", false);
        OPEN.with(|o| { let mut o = o.borrow_mut(); let open = o.as_mut().unwrap(); open.started -= FOLLOW; open.root.as_mut().unwrap().start -= FOLLOW; });
        drawn(Some(("ses_b", 3)), (Instant::now(), Some(1.0), None));
        assert!(OPEN.with(|o| o.borrow().is_none()));
    });
    let root = named(&spans, "session.open");
    assert_eq!(attr(root, "outcome")["stringValue"], "unpainted");
}

#[test]
fn a_restored_trace_started_after_the_cached_frame_does_not_wait_for_a_redraw() {
    let _launch = LAUNCH.lock().unwrap_or_else(|e| e.into_inner());
    let (_, spans) = capture(|| {
        // on_ready starts connecting only after the cached launch frame has drawn.
        drawn(Some(("ses_cached_launch", 10)), (Instant::now(), Some(1.0), None));
        open_restored("ses_cached_launch", 10);
        committed("ses_cached_launch", false);
        page("ses_cached_launch", false, &Read { source: "server", messages: 10, ..Default::default() }, (Instant::now(), Instant::now()));
        assert!(OPEN.with(|o| o.borrow().as_ref().unwrap().root.is_none()), "unchanged cached content needs no second paint");
    });
    assert_eq!(attr(named(&spans, "session.open"), "outcome")["stringValue"], "cached");
}

#[test]
fn an_open_replaced_after_follow_ends_at_follow_not_at_the_click() {
    let (_, spans) = capture(|| {
        open_begin("ses_a", false);
        OPEN.with(|o| { let mut o = o.borrow_mut(); let open = o.as_mut().unwrap(); open.started -= FOLLOW * 100; open.root.as_mut().unwrap().start -= FOLLOW * 100; });
        open_begin("ses_b", false);
    });
    let root = spans.iter().find(|s| s["name"] == "session.open").unwrap();
    assert_eq!(attr(root, "outcome")["stringValue"], "unpainted");
    let (start, end) = (root["startTimeUnixNano"].as_str().unwrap().parse::<u128>().unwrap(), root["endTimeUnixNano"].as_str().unwrap().parse::<u128>().unwrap());
    assert!((end - start).abs_diff(FOLLOW.as_nanos()) < 1_000_000, "{}", end - start);
}
