//! The worker threads the host's plugin work runs on off the app's thread: health checks,
//! server reads, log lines.
//!
//! The workers start with the first job and live as long as the app. On Windows a thread's start
//! and exit run every loaded library's thread-attach and -detach code under the loader lock,
//! which the app's thread then waits on while it draws: a thread started for a press stalled
//! the window for over 100 ms in a debug build. Long-lived workers pay that once.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

/// Four: a long job (the SSH plugin's sign-in listener) leaves the others free.
const WORKERS: usize = 4;

type Job = Box<dyn FnOnce() + Send>;

fn queue() -> &'static Mutex<Sender<Job>> {
    static QUEUE: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..WORKERS {
            let rx = rx.clone();
            let spawned = std::thread::Builder::new().name(format!("plugin-worker-{i}")).spawn(move || work(&rx));
            if let Err(e) = spawned {
                eprintln!("plugin worker {i} didn't start: {e}");
            }
        }
        Mutex::new(tx)
    })
}

fn work(rx: &Mutex<Receiver<Job>>) {
    loop {
        let job = match rx.lock() {
            Ok(rx) => rx.recv(),
            Err(_) => return,
        };
        match job {
            Err(_) => return,
            Ok(job) => job(),
        }
    }
}

/// Writes a plugin host log line from a worker: stderr can block (redirected to a file, it
/// held the app's thread for hundreds of milliseconds), so the app's thread never writes it.
pub fn log(line: String) {
    run(move || eprintln!("{line}"));
}

/// Runs host work `job` on a worker.
pub fn run(job: impl FnOnce() + Send + 'static) {
    let sent = queue().lock().map(|tx| tx.send(Box::new(job)));
    if !matches!(sent, Ok(Ok(()))) {
        eprintln!("plugin job dropped: no worker");
    }
}
