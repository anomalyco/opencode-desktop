use super::*;
use std::io::{Read as _, Write as _};

#[test]
fn spans_reach_a_collector_as_one_otlp_request() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    start("otlp-lite-test", "9.9.9", Some(listener.local_addr().unwrap()));
    let root = Span::root("open").attr("item.id", "t1").attr("rows", 3).attr("held", true);
    record(root.ctx(), "read", (Instant::now(), Instant::now()), vec![("ms", 1.5.into())]);
    root.end_at(Instant::now());

    let (mut conn, _) = listener.accept().unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut raw = Vec::new();
    let mut buf = [0; 4096];
    let body = loop {
        let n = conn.read(&mut buf).unwrap();
        assert!(n > 0, "closed before the body");
        raw.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&raw);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let len: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
            if body.len() >= len { break (head.to_owned(), body.to_owned()); }
        }
    };
    conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").unwrap();
    assert!(body.0.starts_with("POST /v1/traces HTTP/1.1"));
    let v: Value = serde_json::from_str(&body.1).unwrap();
    let rs = &v["resourceSpans"][0];
    assert_eq!(rs["resource"]["attributes"][0]["value"]["stringValue"], "otlp-lite-test");
    let resource = |key: &str| rs["resource"]["attributes"].as_array().unwrap().iter().find(|a| a["key"] == key).unwrap()["value"].clone();
    assert!(!resource("host.name")["stringValue"].as_str().unwrap().is_empty());
    assert_eq!(resource("process.pid")["intValue"], std::process::id().to_string());
    let spans = rs["scopeSpans"][0]["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 2, "both spans in one batch");
    let (read, open) = (&spans[0], &spans[1]);
    assert_eq!(open["name"], "open");
    assert_eq!(read["parentSpanId"], open["spanId"]);
    assert_eq!(read["traceId"], open["traceId"]);
    let attr = |k: &str| open["attributes"].as_array().unwrap().iter().find(|a| a["key"] == k).unwrap()["value"].clone();
    assert_eq!(attr("item.id"), json!({"stringValue": "t1"}));
    assert_eq!(attr("rows"), json!({"intValue": "3"}));
    assert_eq!(attr("held"), json!({"boolValue": true}));
}

#[test]
fn capture_collects_this_threads_spans_instead_of_sending() {
    let (_, spans) = capture(|| Span::root("a").end_at(Instant::now()));
    assert_eq!(spans.len(), 1);
    assert!(spans[0].get("parentSpanId").is_none());
    assert_eq!(spans[0]["traceId"].as_str().unwrap().len(), 32);
}

#[test]
fn endpoints_are_loopback_only() {
    assert_eq!(endpoint("1"), MOTEL.parse().ok());
    assert_eq!(endpoint("http://127.0.0.1:4318"), "127.0.0.1:4318".parse().ok());
    assert_eq!(endpoint("http://localhost:4318/v1/traces"), "127.0.0.1:4318".parse().ok());
    assert_eq!(endpoint("127.0.0.1:9"), "127.0.0.1:9".parse().ok());
    for off in ["0", "", "http://10.0.0.2:4318", "https://otel.example.com"] { assert_eq!(endpoint(off), None, "{off}"); }
}
