use super::*;
use std::time::Instant;

#[test]
fn cold_reconnect_after_first_503() {
    let measure = |initial, nudge| {
        let (base, mock) = crate::client::mock_replies(vec![("503 Service Unavailable", "{}".into()), ("200 OK", String::new())]);
        let server = Server::new(&base);
        let (tx, rx) = channel();
        let wake: Waker = std::sync::Arc::new(|| {});
        let worker = std::thread::spawn(move || retry_events(&server, Duration::from_secs(2), &tx, &wake, initial));
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Incoming::Status(_))));
        let start = Instant::now();
        if nudge {
            wake_reconnects();
        }
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Incoming::Reconnected)));
        let elapsed = start.elapsed();
        drop(rx);
        wake_reconnects();
        mock.join().unwrap();
        worker.join().unwrap();
        elapsed
    };
    let before = measure(2000, false);
    let after = measure(250, false);
    let nudged = measure(3000, true);
    println!(
        "cold 503 to live event stream: before {} ms, after {} ms; focus/network wake {} ms",
        before.as_millis(),
        after.as_millis(),
        nudged.as_millis()
    );
    assert!(after < before);
    assert!(nudged < before);
}
