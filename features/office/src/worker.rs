//! One thread per open file (`office_preview::engine`): this side starts it and passes jobs on.
//! The engines are either in this exe or, with the `library` feature, in a library of their own
//! that loads when the first file opens (`library.rs`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use office_preview::Kind;
use office_preview::engine::Start;
pub use office_preview::engine::{Job, Reply, Scene, SheetFrame};

#[cfg(feature = "library")]
use super::library::start as engine;

/// Starts the thread for `bytes`; its answers come on `replies` and each one calls `wake`.
pub fn start(kind: Kind, bytes: Vec<u8>, cjk: Option<PathBuf>, replies: Sender<Reply>, wake: Arc<dyn Fn() + Send + Sync>) -> Option<Sender<Job>> {
    let (jobs, queued) = std::sync::mpsc::channel::<Job>();
    let (failed, woke) = (replies.clone(), wake.clone());
    let reply = Box::new(move |r: Reply| {
        let ok = replies.send(r).is_ok();
        wake();
        ok
    });
    let start = Start { kind, bytes, cjk, now_serial: now_serial(), reply };
    // The engines' library loads at the first file, and the system checks a library it has not
    // seen before (0.2 s on macOS after an install or update): never on the caller's thread. The
    // file shows as opening until the document's thread answers, as it does while a file parses.
    let relay = move || {
        let worker = match engine(start) {
            Ok(worker) => worker,
            Err(message) => {
                failed.send(Reply::Failed(message)).ok();
                woke();
                return;
            }
        };
        for job in queued {
            if !worker.send(job) {
                break;
            }
        }
    };
    std::thread::Builder::new().name("office-jobs".into()).spawn(relay).ok()?;
    Some(jobs)
}

#[cfg(not(feature = "library"))]
fn engine(start: Start) -> Result<office_preview::engine::Worker, String> {
    office_preview::engine::start(start).ok_or_else(|| "The file could not be opened".to_owned())
}

/// The local time as a 1900-system serial (`xlsx-wasm` `now_serial`): days since 1899-12-30.
pub fn now_serial() -> Option<f64> {
    let ms = crate::wall_ms();
    let local = ms + i64::from(rune::LocalTime::at(ms).offset) * 1000;
    Some(local as f64 / 86_400_000.0 + 25_569.0)
}
