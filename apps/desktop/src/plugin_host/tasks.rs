//! The worker threads plugins' background jobs run on (`HostApiVtable::spawn`, which
//! `LatestTask` uses), and the host's own plugin work off the app's thread (loading a library
//! the user just trusted). Each plugin job holds its library, so a hot reload or an unload
//! never unmaps code a job is still running; the library goes once the last job returns.
//!
//! The workers start with the plugins and live as long as the app. On Windows a thread's start
//! and exit run every loaded library's thread-attach and -detach code under the loader lock,
//! which the app's thread then waits on while it draws: a thread started for a press stalled
//! the window for over 100 ms in a debug build. Long-lived workers pay that once, at launch.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use opencode_plugin::abi::{AbiEvent, Callback, EVENT_JOB};

use super::instance::CallbackEnv;
use super::loader::LoadedLibrary;

/// Four: a plugin's long job (the SSH plugin's sign-in listener) leaves the others free.
const WORKERS: usize = 4;

enum Job {
    Plugin { env: CallbackEnv, delay: Duration },
    Host(Box<dyn FnOnce() + Send>),
}

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
            Ok(Job::Host(job)) => job(),
            Ok(Job::Plugin { env, delay }) => {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                if let Some(func) = env.cb.func {
                    let event = AbiEvent { kind: EVENT_JOB, ..AbiEvent::default() };
                    // SAFETY: the plugin's job with its own data; jobs ignore state and context.
                    // It catches its own panics.
                    let out = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), env.cb.data, &event) };
                    if let Err((_, message)) = out.into_result() {
                        eprintln!("plugin job panicked: {message}");
                    }
                }
                // The job's captures are freed here, then its hold on the library.
                drop(env);
            }
        }
    }
}

fn send(job: Job) {
    let sent = queue().lock().map(|tx| tx.send(job));
    if !matches!(sent, Ok(Ok(()))) {
        eprintln!("plugin job dropped: no worker");
    }
}

/// Starts the workers, with the plugins.
pub fn start() {
    queue();
}

/// Queues plugin job `cb` to run after `delay`.
pub fn spawn(cb: Callback, delay: Duration, lib: Arc<LoadedLibrary>) {
    send(Job::Plugin { env: CallbackEnv::new(cb, lib), delay });
}

/// Writes a plugin host log line from a worker: stderr can block (redirected to a file, it
/// held the app's thread for hundreds of milliseconds), so the app's thread never writes it.
pub fn log(line: String) {
    run(move || eprintln!("{line}"));
}

/// Runs host work `job` on a worker.
pub fn run(job: impl FnOnce() + Send + 'static) {
    send(Job::Host(Box::new(job)));
}
