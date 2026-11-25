//! `opencode-desktop --record-launch-pages <out>`: one launch that nobody waits for, which writes
//! the list of the image's pages a launch uses (`image_pages`) to `<out>` and ends by itself.
//!
//! The package step runs it once on the just-built app (`apps/installer/scripts/package.ps1`),
//! and the list ships beside the exe for a build's first launch on a profile. The run is a
//! launch as a person's is, with a window, so that it uses the pages theirs will; what makes it
//! safe to start on a build machine is all here, so that no caller can ask for half of it:
//!
//! - **Its own root.** [`begin`] makes a folder under the temporary folder and points
//!   `RUNE_DIRS_ROOT` at it, so every folder the app reads and writes is in there, and the
//!   instance it claims (`launch::instance_id`, the data folder's name) is its own: a launch of
//!   the real app is never handed to this one. The folder is removed when the run ends.
//! - **The demo's session** (`stories/demo.rs`), not a server's: no server is looked for or
//!   started, no database is opened (`storage::config`), and a session with a reply is on screen,
//!   so the list holds the pages that draw one.
//! - **No request of any kind** ([`on`] is asked at each site, and the launch marks
//!   `record.offline`): the update service is not started, the browser engine is not looked for or
//!   downloaded, a plugin's `Http:FetchAsync` and `Server:RequestAsync` are refused, telemetry is
//!   off (`OPENCODE_RUNE_OTEL=0`), and the toasts' activator is not served.
//! - **Bounded.** The app quits as its Quit command does once the list is written (about a second
//!   after its first input), and a run still going after [`LIMIT`] ends itself.
//!
//! Exit code 0: the list is at `<out>`. Otherwise one line on stderr says why: 2 the run could
//! not start, 3 the app quit with no list written, 4 it ran past [`LIMIT`].
//!
//! Not the app's to decide: whether the window is seen. A caller that must show nothing starts
//! this on a desktop of its own, as the package step does.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

const FLAG: &str = "--record-launch-pages";
/// How long the whole run may take: a launch and its list are a few seconds.
const LIMIT: Duration = Duration::from_secs(45);

enum State {
    /// An ordinary launch.
    Off,
    /// The launch runs and its list is not at `out` yet.
    Running { out: PathBuf, root: PathBuf },
    /// The list is written, or could not be (`why`): the next turn quits the app.
    Listed { root: PathBuf, why: Option<&'static str> },
    /// The app was asked to quit: its last step removes `root`.
    Leaving { root: PathBuf, why: Option<&'static str> },
    /// Over: the process ends.
    Left,
}

enum Event {
    /// `main` found the flag.
    Asked { out: PathBuf, root: PathBuf },
    /// `image_pages` wrote the list, or says why not.
    Listed(Option<&'static str>),
    /// A turn of the app (`trace::poll`).
    Turn,
    /// The app's last step on its way out (`quit_last`).
    Ended,
    /// [`LIMIT`] passed.
    Late,
}

pub enum Effect {
    None,
    /// Look again shortly: the list is not written yet.
    Look,
    /// Quit the app as its Quit command does.
    Quit,
    /// Remove `root` and end, with `why` when there is no list.
    Leave { root: PathBuf, why: Option<&'static str> },
}

static STATE: Mutex<State> = Mutex::new(State::Off);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn step(state: &mut State, event: Event) -> Effect {
    match (std::mem::replace(state, State::Left), event) {
        (State::Off, Event::Asked { out, root }) => {
            *state = State::Running { out, root };
            Effect::None
        }
        (State::Running { root, .. }, Event::Listed(why)) => {
            *state = State::Listed { root, why };
            Effect::None
        }
        (State::Running { out, root }, Event::Turn) => {
            *state = State::Running { out, root };
            Effect::Look
        }
        (State::Listed { root, why }, Event::Turn) => {
            *state = State::Leaving { root, why };
            Effect::Quit
        }
        (State::Running { root, .. }, Event::Ended) => Effect::Leave { root, why: Some("the app quit before its list was written") },
        (State::Listed { root, why } | State::Leaving { root, why }, Event::Ended) => Effect::Leave { root, why },
        (State::Running { root, .. } | State::Listed { root, .. } | State::Leaving { root, .. }, Event::Late) => {
            Effect::Leave { root, why: Some("it was still running after 45 s") }
        }
        (was, _) => {
            *state = was;
            Effect::None
        }
    }
}

fn send(event: Event) -> Effect {
    step(&mut state(), event)
}

/// Whether this run records a launch's pages: asked wherever the app would make a request.
pub fn on() -> bool {
    !matches!(*state(), State::Off)
}

/// Where the list goes, while it is still to be written.
pub fn out() -> Option<PathBuf> {
    match &*state() {
        State::Running { out, .. } => Some(out.clone()),
        _ => None,
    }
}

/// In `main`, before any other thread exists and before anything reads the app's folders.
#[allow(unsafe_code)]
pub fn begin() {
    let mut args = std::env::args().skip(1);
    if !args.any(|a| a == FLAG) {
        return;
    }
    fn refuse(why: &str) -> ! {
        eprintln!("opencode-rune: {FLAG} did not start: {why}");
        std::process::exit(2)
    }
    if !cfg!(windows) {
        refuse("only the Windows build keeps a list of its launch's pages");
    }
    // Absolute: the app has left its working folder by now (`launch::leave_install_folder`).
    let Some(out) = args.next().map(PathBuf::from).filter(|p| p.is_absolute()) else {
        refuse("it takes the absolute path of the file to write");
    };
    let root = std::env::temp_dir().join(format!("opencode-rune-pages-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&root) {
        refuse(&format!("{}: {e}", root.display()));
    }
    // SAFETY: `main` calls this before it starts a thread, so nothing reads the environment
    // meanwhile.
    unsafe {
        std::env::set_var("RUNE_DIRS_ROOT", &root);
        std::env::set_var("OPENCODE_RUNE_OTEL", "0");
    }
    send(Event::Asked { out, root });
    std::thread::Builder::new()
        .name("record-pages-limit".into())
        .spawn(|| {
            std::thread::sleep(LIMIT);
            if let Effect::Leave { root, why } = send(Event::Late) {
                leave(root, why, 4);
            }
        })
        .ok();
}

/// `image_pages`: the list is at its place, or `why` it is not.
pub fn listed(why: Option<&'static str>) {
    send(Event::Listed(why));
}

/// A turn of the app: what the run wants of it now.
pub fn turn() -> Effect {
    send(Event::Turn)
}

/// The app's last step on its way out: the root goes, and a run with no list says so and ends
/// non-zero. With a list the process ends as every quit does, with 0.
pub fn ended() {
    if let Effect::Leave { root, why } = send(Event::Ended) {
        leave(root, why, 3);
    }
}

fn leave(root: PathBuf, why: Option<&'static str>, code: i32) {
    // A file the app still holds keeps its folder for a moment.
    let mut gone = false;
    for _ in 0..10 {
        gone = std::fs::remove_dir_all(&root).is_ok() || !root.exists();
        if gone {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !gone {
        eprintln!("opencode-rune: {FLAG} could not remove {}", root.display());
    }
    if let Some(why) = why {
        eprintln!("opencode-rune: the launch's page list was not written: {why}");
        std::process::exit(code);
    }
}
