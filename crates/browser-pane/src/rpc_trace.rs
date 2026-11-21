//! Scratch timing probe (never merged): with `OPENCODE_BROWSER_RPC_TRACE=DIR`, one line per event
//! to `DIR/pane.log`: microseconds since the first event, thread, event, detail.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

static OUT: OnceLock<Option<(Mutex<std::fs::File>, std::time::Instant)>> = OnceLock::new();

pub fn t(event: &str, detail: impl std::fmt::Display) {
    let Some((file, at)) = OUT
        .get_or_init(|| {
            let dir = std::env::var_os("OPENCODE_BROWSER_RPC_TRACE")?;
            let file = std::fs::OpenOptions::new().create(true).append(true).open(std::path::Path::new(&dir).join("pane.log")).ok()?;
            Some((Mutex::new(file), std::time::Instant::now()))
        })
        .as_ref()
    else {
        return;
    };
    let line = format!("{} {} {event} {detail}\n", at.elapsed().as_micros(), std::thread::current().name().unwrap_or("?"));
    let _ = file.lock().map(|mut f| f.write_all(line.as_bytes()));
}
