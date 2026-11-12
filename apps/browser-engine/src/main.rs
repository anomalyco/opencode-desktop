//! Standalone `OpenCode Browser Engine` CEF 156 sidecar daemon.
//!
//! Listens on a private Unix domain socket (AF_UNIX on Windows too), streams GPU frames (2x
//! Retina `IOSurface`s over Mach ports on macOS, D3D11 shared textures on Windows), survives UI
//! hot-reloads (15s reconnect grace window), and exits cleanly after 60s with 0 tabs.
//!
//! The main thread sleeps until something happens: a message from the app, a connection, or
//! the time CEF asked to do its work at (`external_message_pump`). Nothing polls.

// A program the app starts, never from a console: without this Windows opens a console window
// for it.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod d3d_frames;
mod engine;
mod handlers;
#[cfg(target_os = "macos")]
mod mac_app;
mod profile;
mod wire;

use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

use crate::engine::{CefRuntime, EngineManager};
use crate::wire::{EngineMsg, HostMsg};

/// How long the engine waits for its app to come back after it disconnected without a Shutdown.
const RECONNECT_GRACE: Duration = Duration::from_secs(15);
/// How long the engine stays up with no tab open, so a page opened again soon starts at once.
const EMPTY_GRACE: Duration = Duration::from_secs(60);
/// The longest the loop sleeps between CEF's work calls, as cefclient's external pump does in
/// case a scheduling call is missed.
#[cfg(not(target_os = "macos"))]
const MAX_PUMP_DELAY: Duration = Duration::from_secs(1);
/// macOS also pumps the AppKit run loop between CEF's work calls, every 4 ms as before.
#[cfg(target_os = "macos")]
const MAX_PUMP_DELAY: Duration = Duration::from_millis(4);
/// While a tab waits for the app to free a frame texture, how often it looks.
const FRAME_RETRY: Duration = Duration::from_millis(4);

/// What wakes the main thread.
pub enum Event {
    Connected { id: u64, stream: UnixStream },
    Disconnected { id: u64 },
    Host(HostMsg),
    /// CEF wants `cef::do_message_loop_work` in this many milliseconds (now when 0 or less).
    Pump(i64),
}

/// Whether something the engine waits for is there, and since when it isn't.
#[derive(Clone, Copy)]
enum Presence {
    Present,
    AbsentSince(Instant),
}

impl Presence {
    fn step(self, present: bool, now: Instant) -> Self {
        match (self, present) {
            (_, true) => Presence::Present,
            (Presence::Present, false) => Presence::AbsentSince(now),
            (absent, false) => absent,
        }
    }

    fn deadline(self, grace: Duration) -> Option<Instant> {
        match self {
            Presence::Present => None,
            Presence::AbsentSince(since) => Some(since + grace),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--print-cef-dir") {
        if let Some(dir) = option_env!("OPENCODE_CEF_DIR") {
            println!("{dir}");
        } else if let Some(dir) = cef_dll_sys::get_cef_dir() {
            println!("{}", dir.display());
        }
        return;
    }
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));

    let sock_path = arg("--socket").map(PathBuf::from).unwrap_or_else(default_socket_path);
    if let Some(parent) = sock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _ = std::fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).expect("bind browser-engine unix socket");
    // The app waits for this line instead of retrying its connect: from here it connects at once,
    // and its first messages wait in the socket while CEF starts.
    {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"listening\n");
        let _ = out.flush();
    }

    let (events, rx) = mpsc::channel::<Event>();
    let client_writer: Arc<Mutex<Option<(u64, UnixStream)>>> = Arc::new(Mutex::new(None));
    accept_connections(listener, events.clone());

    // The app's GPU (`HIGH,LOW` LUID halves): Chromium renders on it, so its frames open there.
    let adapter_luid = arg("--adapter-luid")
        .and_then(|v| v.split_once(','))
        .and_then(|(high, low)| Some((high.parse::<i32>().ok()?, low.parse::<u32>().ok()?)));
    // The app's language as Chromium names it (`de`, `zh-CN`): page strings and Accept-Language.
    let locale = arg("--lang").cloned();
    let _cef = CefRuntime::initialize(adapter_luid, locale.as_deref(), events.clone());

    let (out_tx, out_rx) = mpsc::channel::<EngineMsg>();
    {
        let writer_slot = client_writer.clone();
        std::thread::Builder::new()
            .name("engine-sock-tx".into())
            .spawn(move || {
                while let Ok(msg) = out_rx.recv() {
                    let Ok(mut line) = serde_json::to_string(&msg) else {
                        continue;
                    };
                    line.push('\n');
                    let mut guard = writer_slot.lock().unwrap();
                    if let Some((_, stream)) = guard.as_mut()
                        && (stream.write_all(line.as_bytes()).is_err() || stream.flush().is_err())
                    {
                        *guard = None;
                    }
                }
            })
            .expect("spawn engine-sock-tx");
    }

    let mut manager = EngineManager::new(out_tx, adapter_luid);
    let start = Instant::now();
    let (mut client, mut tabs) = (Presence::AbsentSince(start), Presence::AbsentSince(start));
    let mut pump_at = Some(start);

    loop {
        let now = Instant::now();
        if pump_at.is_some_and(|at| at <= now) {
            pump_at = None;
            cef::do_message_loop_work();
            manager.poll_picks();
        }
        #[cfg(target_os = "macos")]
        mac_app::pump_run_loop(0.004);
        manager.poll_frames();

        let connected = client_writer.lock().unwrap().is_some();
        client = client.step(connected, now);
        tabs = tabs.step(!manager.is_empty(), now);
        let quit_at = [client.deadline(RECONNECT_GRACE), tabs.deadline(EMPTY_GRACE)].into_iter().flatten().min();
        if quit_at.is_some_and(|at| at <= now) {
            break;
        }
        let wake = [Some(pump_at.unwrap_or(now + MAX_PUMP_DELAY)), quit_at, manager.waits_for_texture().then(|| now + FRAME_RETRY)]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(now + MAX_PUMP_DELAY);
        let event = match rx.recv_timeout(wake.saturating_duration_since(now)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                if pump_at.is_none() {
                    pump_at = Some(Instant::now());
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match event {
            Event::Connected { id, stream } => {
                if let Ok(writer) = stream.try_clone() {
                    *client_writer.lock().unwrap() = Some((id, writer));
                    read_messages(id, stream, events.clone());
                }
            }
            Event::Disconnected { id } => {
                let mut writer = client_writer.lock().unwrap();
                if writer.as_ref().is_some_and(|(current, _)| *current == id) {
                    *writer = None;
                }
            }
            Event::Host(msg) => {
                if !manager.handle(msg) {
                    break;
                }
            }
            Event::Pump(delay_ms) => {
                let at = Instant::now() + Duration::from_millis(delay_ms.max(0) as u64);
                pump_at = Some(pump_at.map_or(at, |current| current.min(at)));
            }
        }
    }

    let _ = std::fs::remove_file(&sock_path);
}

/// Accepts the app's connections on a thread of its own; the last one is the app.
fn accept_connections(listener: UnixListener, events: Sender<Event>) {
    std::thread::Builder::new()
        .name("engine-sock-accept".into())
        .spawn(move || {
            for (id, stream) in (1u64..).zip(listener.incoming()) {
                let Ok(stream) = stream else { continue };
                if events.send(Event::Connected { id, stream }).is_err() {
                    break;
                }
            }
        })
        .expect("spawn engine-sock-accept");
}

/// Reads connection `id`'s messages on a thread of its own until it closes.
fn read_messages(id: u64, stream: UnixStream, events: Sender<Event>) {
    let _ = std::thread::Builder::new().name("engine-sock-rx".into()).spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<HostMsg>(&line)
                && events.send(Event::Host(msg)).is_err()
            {
                return;
            }
        }
        let _ = events.send(Event::Disconnected { id });
    });
}

#[cfg(unix)]
fn default_socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(home)
        .join("Library/Caches/co.anomaly.opencode")
        .join(format!("browser-{uid}.sock"))
}

/// The app always passes `--socket`; this is for running the engine by hand.
#[cfg(windows)]
fn default_socket_path() -> PathBuf {
    std::env::temp_dir().join("co.anomaly.opencode").join("browser.sock")
}
