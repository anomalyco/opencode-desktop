//! Hex backend selection and off-Windows presence watcher thread.

use std::cell::OnceCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use hex_client::Presence;
use rune::Waker;

use crate::DictationHost;

pub(crate) enum Backend {
    /// In-process: ready after `Dictation::prepare`, words `FAKE_TRANSCRIBE` after the stop.
    Fake,
    /// The real Hex: its endpoint file, and its data folder (where Windows' Hex is installed).
    Hex { file: PathBuf, support: PathBuf },
}

impl Backend {
    /// Where Hex is, as the watcher last saw: always running for the fake, never known on
    /// Windows (the mic shows either way), unknown off it until a window's toolbar asks. `wake`
    /// joins that window to the watcher's changes.
    pub(crate) fn watch(&self, wake: Option<&Waker>) -> Option<Presence> {
        match self {
            Backend::Fake => Some(Presence::Running),
            Backend::Hex { .. } if hex_client::MIC_WITHOUT_HEX => None,
            Backend::Hex { file, support } => WATCH.with(|w| {
                let w = w.get_or_init(|| Watch::spawn(file.clone(), support.clone()));
                if let Some(wake) = wake {
                    w.wakes.lock().unwrap_or_else(|e| e.into_inner()).push(wake.clone());
                }
                w.presence()
            }),
        }
    }
}

fn arg(name: &str) -> Option<String> {
    let a = rune::extra_args();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

pub(crate) fn detect(host: &dyn DictationHost) -> Backend {
    let headless = host.is_headless() || arg("--scene").is_some() || arg("--fixture").is_some();
    if std::env::var_os("SEER_HEX").is_none() && headless {
        return Backend::Fake;
    }
    let support = host.hex_support_dir().unwrap_or_default();
    Backend::Hex { file: hex_client::discovery(&support), support }
}

thread_local! {
    /// One watcher per UI thread, shared by its windows.
    static WATCH: OnceCell<Watch> = const { OnceCell::new() };
}

/// Where Hex is, off Windows, kept by a thread that looks once and then again only when Hex's
/// folder (or, until Hex makes it, the folder above it) changes. The thread keeps the watcher,
/// and both last as long as the process.
struct Watch {
    /// `Presence` as 1 + its index; 0 until the thread's first look.
    seen: Arc<AtomicU8>,
    /// The windows to wake when it changes.
    wakes: Arc<Mutex<Vec<Waker>>>,
}

impl Watch {
    fn spawn(file: PathBuf, support: PathBuf) -> Watch {
        use notify::Watcher as _;
        let seen = Arc::new(AtomicU8::new(0));
        let wakes: Arc<Mutex<Vec<Waker>>> = Arc::default();
        let woken = wakes.clone();
        let (changed, changes) = mpsc::channel::<()>();
        let slot = seen.clone();
        drop(std::thread::Builder::new().name("hex-presence".to_owned()).spawn(move || {
            let mut watcher = notify::recommended_watcher(move |_: notify::Result<notify::Event>| {
                let _ = changed.send(()).ok();
            })
            .ok();
            let dir = file.parent().map(PathBuf::from).unwrap_or_default();
            let mut watching: Option<PathBuf> = None;
            loop {
                // Hex's folder once it exists, else the nearest folder above it, for its making.
                let target = dir.ancestors().find(|d| d.is_dir()).map(PathBuf::from);
                if target != watching
                    && let (Some(w), Some(to)) = (watcher.as_mut(), &target)
                {
                    if let Some(old) = &watching {
                        drop(w.unwatch(old));
                    }
                    drop(w.watch(to, notify::RecursiveMode::NonRecursive));
                    watching = target;
                }
                let now = match hex_client::presence(&file, &support) {
                    Presence::Missing => 1,
                    Presence::Stopped => 2,
                    Presence::Running => 3,
                };
                if slot.swap(now, Ordering::AcqRel) != now {
                    for wake in woken.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                        wake();
                    }
                }
                if changes.recv().is_err() {
                    return;
                }
                // One save is several events: look once the folder is quiet.
                while changes.recv_timeout(std::time::Duration::from_millis(50)).is_ok() {}
            }
        }));
        Watch { seen, wakes }
    }

    fn presence(&self) -> Option<Presence> {
        match self.seen.load(Ordering::Acquire) {
            1 => Some(Presence::Missing),
            2 => Some(Presence::Stopped),
            3 => Some(Presence::Running),
            _ => None,
        }
    }
}
