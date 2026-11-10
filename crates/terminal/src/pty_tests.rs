use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::{GhosttyKey, KeyInput, RestoreState, ScrollOp, SelectionOp, TerminalConfig, TerminalSession};

fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(pred(), "condition not met within {timeout:?}");
}

#[test]
fn off_thread_stream_coalesces_bursts_and_roundtrips_persistence() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let (session, handle) = TerminalSession::spawn_stream(
        TerminalConfig {
            cols: 80,
            rows: 24,
            ..TerminalConfig::default()
        },
        None,
        Arc::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        }),
    )
    .unwrap();

    // Cursor-only sequence keeps terminal unrevealed until real printable text arrives.
    handle.feed(b"\x1b[?25h");
    std::thread::sleep(Duration::from_millis(15));
    assert!(!session.revealed());

    // Feed 5,000 colored lines across many chunks: worker coalesces wakes instead of waking 5,000x.
    for i in 0..5_000 {
        handle.feed(format!("\x1b[32mOK\x1b[0m line-{i:04}\r\n"));
    }
    wait_until(Duration::from_secs(3), || {
        session.snapshot().viewport_text().contains("line-4999")
    });
    assert!(session.revealed());
    assert!(
        wakes.load(Ordering::Relaxed) < 500,
        "burst coalescing must keep UI wakes far below 5,000 chunks"
    );

    // Input encoding & selection run off the main thread.
    let mut key = KeyInput::press(GhosttyKey::Backspace);
    key.cmd = true;
    session.send_key(key);
    wait_until(Duration::from_secs(1), || handle.take_written() == b"\x15");

    session.select(SelectionOp::All);
    wait_until(Duration::from_secs(1), || {
        session
            .selected_text()
            .is_some_and(|s| s.contains("line-4999"))
    });

    // Scroll up and capture persisted state, then restore into a new session.
    session.scroll(ScrollOp::Delta(-10));
    wait_until(Duration::from_secs(1), || !session.snapshot().scrollbar.at_bottom);
    let saved = session
        .persist_snapshot(Duration::from_secs(2))
        .expect("persisted snapshot");
    assert!(!saved.buffer.is_empty());

    let (restored, _) = TerminalSession::spawn_stream(
        TerminalConfig {
            cols: saved.cols,
            rows: saved.rows,
            ..TerminalConfig::default()
        },
        Some(RestoreState {
            buffer: saved.buffer,
            cols: Some(saved.cols),
            rows: Some(saved.rows),
            scroll_y: Some(saved.scroll_y as usize),
            cursor: Some(saved.cursor),
        }),
        Arc::new(|| {}),
    )
    .unwrap();
    assert!(restored.revealed());
    assert!(restored.snapshot().viewport_text().contains("OK line-"));
}
