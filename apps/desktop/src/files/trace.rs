//! `--files-trace <file>`: one JSON line for each file read that lands while it's given: when
//! the change reached the app, when the read started and ended on its worker, how long the UI
//! thread took to swap the text in, and the first frame drawn after it with its update and
//! encode times, plus every frame drawn from the change on, so a late one shows. Times are µs
//! from the first line's process anchor. It records; it never passes or fails anything, and
//! without the flag every call is one check.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::sync::mpsc::Sender;

use rune::time::Instant;
use serde_json::{Value, json};

static OUT: OnceLock<Option<(Sender<String>, Instant)>> = OnceLock::new();

fn out() -> Option<&'static (Sender<String>, Instant)> {
    OUT.get_or_init(|| {
        let path = crate::arg("--files-trace")?;
        let (sender, lines) = std::sync::mpsc::channel::<String>();
        // Written by a thread of its own: never a file write on the UI thread.
        rune::task::serve("files-trace", lines, move |line: String| {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{line}");
            }
            ControlFlow::Continue(())
        })
        .ok()?;
        Some((sender, Instant::now()))
    })
    .as_ref()
}

struct Pending {
    line: Value,
    from: Instant,
}

thread_local! {
    /// When each file's change reached the app, until its read lands.
    static CHANGED: RefCell<HashMap<(String, String), Instant>> = RefCell::new(HashMap::new());
    /// Landed reads waiting for the frame that shows them.
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    /// Frames drawn since the oldest change still followed: (start, end, update ms, encode ms).
    static FRAMES: RefCell<Vec<(Instant, Instant, f64, f64)>> = const { RefCell::new(Vec::new()) };
}

fn us(origin: Instant, at: Instant) -> u64 {
    at.saturating_duration_since(origin).as_micros() as u64
}

/// A change to `path` reached the app (`filesystem.changed`).
pub(crate) fn changed(sid: &str, path: &str) {
    if out().is_none() {
        return;
    }
    CHANGED.with(|c| {
        c.borrow_mut().entry((sid.to_owned(), path.to_owned())).or_insert_with(Instant::now);
    });
}

/// A read landed and its text was swapped in, from `swap` until now.
pub(crate) fn landed(sid: &str, path: &str, attempt: u64, times: [Instant; 3], swap: Instant, bytes: usize, lines: u32) {
    let Some((_, origin)) = out() else { return };
    let end = Instant::now();
    let changed = CHANGED.with(|c| c.borrow_mut().remove(&(sid.to_owned(), path.to_owned())));
    let line = json!({
        "path": path,
        "attempt": attempt,
        "bytes": bytes,
        "lines": lines,
        "changed_us": changed.map(|t| us(*origin, t)),
        "read_start_us": us(*origin, times[0]),
        "read_bytes_us": us(*origin, times[1]),
        "read_ready_us": us(*origin, times[2]),
        "swap_start_us": us(*origin, swap),
        "swap_end_us": us(*origin, end),
        "swap_us": end.saturating_duration_since(swap).as_micros() as u64,
    });
    PENDING.with(|p| p.borrow_mut().push(Pending { line, from: changed.unwrap_or(times[0]) }));
}

/// Each frame a window ran (`on_frame_encoded`): the first drawn one after a swap completes its
/// line.
pub(crate) fn frame(f: &rune::FrameEncoded) {
    let Some((sender, origin)) = out() else { return };
    if f.outcome() != rune::FrameOutcome::Drawn {
        return;
    }
    let following = CHANGED.with(|c| !c.borrow().is_empty()) || PENDING.with(|p| !p.borrow().is_empty());
    if !following {
        FRAMES.with(|fr| fr.borrow_mut().clear());
        return;
    }
    let (update, encode) = (f.update_ms().unwrap_or(0.0), f.encode_ms().unwrap_or(0.0));
    FRAMES.with(|fr| {
        let mut fr = fr.borrow_mut();
        if fr.len() >= 4096 {
            fr.drain(..2048);
        }
        fr.push((f.started().unwrap_or(f.at()), f.at(), update, encode));
    });
    let done: Vec<Pending> = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
    for mut pending in done {
        let frames: Vec<Value> = FRAMES.with(|fr| {
            fr.borrow().iter().filter(|(_, end, _, _)| *end >= pending.from).map(|(start, end, u, e)| json!([us(*origin, *start), us(*origin, *end), u, e])).collect()
        });
        pending.line["drawn_us"] = json!(us(*origin, f.at()));
        pending.line["drawn_update_ms"] = json!(update);
        pending.line["drawn_encode_ms"] = json!(encode);
        pending.line["frames"] = Value::Array(frames);
        let _ = sender.send(pending.line.to_string());
    }
}
