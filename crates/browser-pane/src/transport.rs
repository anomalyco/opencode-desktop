//! The engine's socket (Unix domain, AF_UNIX on Windows too), launch and framing threads.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use base64::Engine as _;
use rune::time::Instant;
use rune::Waker;

use crate::frame::{self, TabSurface};
use crate::installer::{ERR_HUNG, ERR_STOPPED, EngineStatus};
use crate::profile;
use crate::protocol::{EngineMsg, HostMsg};

#[cfg(unix)]
pub(crate) use std::os::unix::net::UnixStream;
#[cfg(windows)]
pub(crate) use uds_windows::UnixStream;

/// Each tab's surface by tab id, which frames are pushed to off the UI thread.
pub(crate) type Surfaces = Arc<Mutex<HashMap<u64, TabSurface>>>;

/// How long a new engine may take to open its socket: the first start after an install can take
/// several seconds while the OS scans the freshly unpacked files (Windows Defender does, for
/// each new DLL).
const LISTEN_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the engine may take to answer a request (a page to load, a tab to open, a DevTools
/// call) before the pane says it isn't responding. Any message at all is an answer: a page that
/// loads slowly still says at once that it's loading.
const ANSWER_DEADLINE: Duration = Duration::from_secs(5);

/// What the app shares with the threads that talk to its engine.
#[derive(Clone)]
pub(crate) struct Link {
    pub status: Arc<Mutex<EngineStatus>>,
    pub outbox: Arc<Mutex<Option<Sender<HostMsg>>>>,
    pub inbox_tx: Sender<EngineMsg>,
    pub surfaces: Surfaces,
    pub waker: Option<Waker>,
    /// The engine this app started, while it runs: Retry ends one that stopped answering.
    pub engine: Arc<Mutex<Option<u32>>>,
    /// Its socket, removed once it's gone, so the next connect starts a new engine instead of
    /// reaching the old one while Windows still ends it.
    pub sock: PathBuf,
    /// The tabs that waited when the connect began, by id: sent the moment the engine is
    /// connected, behind Hello, without waiting for the app's next turn (one that's busy drawing
    /// held them for 20-650 ms); `early` names them with the address and size each opened at,
    /// so that turn doesn't send them again and corrects what changed meanwhile.
    pub opens: Vec<(u64, HostMsg)>,
    pub early: crate::client::EarlyOpens,
}

impl Link {
    pub(crate) fn set(&self, status: EngineStatus) {
        *self.status.lock().unwrap() = status;
        self.wake();
    }

    fn wake(&self) {
        if let Some(w) = &self.waker {
            w();
        }
    }
}

/// The engine at `bin`, serving `sock` and keeping its sign-ins in `profile`, in the app's
/// language. On Windows it renders on the GPU Rune draws this app's windows with, so its frames
/// open there.
pub(crate) fn engine_command(bin: &Path, sock: &Path, profile: &Path) -> Command {
    let mut cmd = Command::new(bin);
    cmd.arg("--socket").arg(sock).arg("--lang").arg(chromium_locale(opencode_i18n::locale().id())).env(profile::PROFILE_ENV, profile);
    // The engine's own log goes nowhere, unless its start is being traced (OPENCODE_BROWSER_ENGINE_TRACE).
    let trace = std::env::var_os("OPENCODE_BROWSER_ENGINE_TRACE").is_some();
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(if trace { Stdio::inherit() } else { Stdio::null() });
    #[cfg(windows)]
    if let Some((high, low)) = rune::native_surface::windows::adapter_luid() {
        cmd.arg("--adapter-luid").arg(format!("{high},{low}"));
    }
    cmd
}

/// Chromium's name for the app's language id, where they differ (package-windows.ps1 ships the
/// locales by the same names).
fn chromium_locale(id: &str) -> &str {
    match id {
        "en" => "en-US",
        "zh" => "zh-CN",
        "zht" => "zh-TW",
        "no" => "nb",
        other => other,
    }
}

/// Starts the engine and connects to it as soon as it listens (it says so on its output), on
/// the calling thread.
pub(crate) fn spawn_and_attach(bin: &Path, sock: &Path, profile: &Path, mach_service: Option<String>, link: &Link) {
    let _ = std::fs::remove_file(sock);
    // This app has no engine running (its socket didn't answer), so a held profile is an
    // engine on its way out, most likely the shared one from before per-app engines.
    profile::wait_until_free(profile, profile::UNLOCK_WAIT);
    let mut child = match engine_command(bin, sock, profile).spawn() {
        Ok(child) => child,
        Err(e) => return link.set(EngineStatus::Failed(format!("Couldn't start the browser: {e}").into())),
    };
    let Some(stdout) = child.stdout.take() else { return link.set(EngineStatus::Failed(ERR_STOPPED.into())) };
    let (said, heard) = mpsc::channel();
    let _ = std::thread::Builder::new().name("opencode-browser-listen".into()).spawn(move || {
        let mut line = String::new();
        let _ = said.send(BufReader::new(stdout).read_line(&mut line).is_ok_and(|n| n > 0) && line.trim() == "listening");
    });
    match heard.recv_timeout(LISTEN_TIMEOUT) {
        Ok(true) => {}
        // It exited (or hung) before it listened.
        _ => {
            let _ = child.kill();
            return link.set(EngineStatus::Failed(ERR_STOPPED.into()));
        }
    }
    rune::marks::mark("browser.engine.listening");
    match UnixStream::connect(sock) {
        Ok(stream) => wire_stream(stream, mach_service, link),
        Err(_) => return link.set(EngineStatus::Failed(ERR_STOPPED.into())),
    }
    *link.engine.lock().unwrap() = Some(child.id());
    let link = link.clone();
    let _ = std::thread::Builder::new().name("opencode-browser-watch".into()).spawn(move || {
        wait_for_end(&mut child);
        *link.engine.lock().unwrap() = None;
        lost(&link);
    });
}

/// Blocks until the engine has stopped running. On Windows that's its main thread's end: a
/// killed or crashed engine's main thread ends within ~20 ms, while its process, its exit code
/// and its socket wait until Windows has finished tearing it down (12-60 s measured on blade,
/// with one of its threads still held in the kernel), and the engine's work all runs on its main
/// thread, whose end is the process's. Like the process, the thread is a handle to wait on: no
/// timer, no wake until it ends.
fn wait_for_end(child: &mut Child) {
    #[cfg(windows)]
    if let Some(thread) = proc::main_thread(child.id()) {
        return proc::wait(thread);
    }
    let _ = child.wait();
}

/// Ends the engine this app started, at once (Retry, when it stopped answering). The watcher
/// still holds the engine's own handle, so its pid is still the engine's.
pub(crate) fn end_engine(engine: &Mutex<Option<u32>>) {
    if let Some(pid) = engine.lock().unwrap().take() {
        proc::kill(pid);
    }
}

#[allow(unsafe_code)]
mod proc {
    #[cfg(windows)]
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};

    /// The engine's main thread: of its threads, the one created first (with the process).
    #[cfg(windows)]
    pub(super) fn main_thread(pid: u32) -> Option<HANDLE> {
        use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
        use windows::Win32::System::Threading::{GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SYNCHRONIZE};
        // SAFETY: the snapshot and every thread handle but the one returned are closed here;
        // `entry` and the times are ours to fill.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).ok()?;
            let mut entry = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
            let mut first: Option<(u64, HANDLE)> = None;
            let mut more = Thread32First(snap, &mut entry).is_ok();
            while more {
                if entry.th32OwnerProcessID == pid
                    && let Ok(thread) = OpenThread(THREAD_SYNCHRONIZE | THREAD_QUERY_LIMITED_INFORMATION, false, entry.th32ThreadID)
                {
                    let (mut created, mut a, mut b, mut c) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
                    let at = GetThreadTimes(thread, &mut created, &mut a, &mut b, &mut c).map(|_| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime));
                    match (at, first) {
                        (Ok(at), Some((best, old))) if at < best => {
                            let _ = CloseHandle(old);
                            first = Some((at, thread));
                        }
                        (Ok(at), None) => first = Some((at, thread)),
                        _ => {
                            let _ = CloseHandle(thread);
                        }
                    }
                }
                more = Thread32Next(snap, &mut entry).is_ok();
            }
            let _ = CloseHandle(snap);
            first.map(|(_, thread)| thread)
        }
    }

    #[cfg(windows)]
    pub(super) fn wait(thread: HANDLE) {
        use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
        // SAFETY: `thread` is ours until it's closed here.
        unsafe {
            WaitForSingleObject(thread, INFINITE);
            let _ = CloseHandle(thread);
        }
    }

    #[cfg(windows)]
    pub(super) fn kill(pid: u32) {
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
        // SAFETY: the handle is closed before returning.
        unsafe {
            if let Ok(process) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(process, 1);
                let _ = CloseHandle(process);
            }
        }
    }

    #[cfg(unix)]
    pub(super) fn kill(pid: u32) {
        // SAFETY: a plain signal to the engine's pid.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
/// The engine went away on its own (the app's `shutdown_sidecar` empties the outbox first): the
/// pane says so, and Retry starts it again.
fn lost(link: &Link) {
    if link.outbox.lock().unwrap().take().is_some() {
        rune::marks::mark("browser.engine.lost");
        let _ = std::fs::remove_file(&link.sock);
        link.set(EngineStatus::Failed(ERR_STOPPED.into()));
    }
}

/// Whether the engine answers `msg` with a message of its own soon: a page it loads says it's
/// loading, a tab it opens reports itself, a DevTools call returns.
fn expects_answer(msg: &HostMsg) -> bool {
    matches!(msg, HostMsg::Hello { .. } | HostMsg::OpenTab { .. } | HostMsg::Navigate { .. } | HostMsg::Reload { .. } | HostMsg::CdpCall { .. })
}

/// The oldest request the engine hasn't answered, whether the pane says it isn't responding
/// (and what it showed before), and whether the connection is over.
#[derive(Default)]
struct Asked {
    since: Option<Instant>,
    hung: Option<EngineStatus>,
    over: bool,
}

#[derive(Default)]
struct Answers {
    asked: Mutex<Asked>,
    changed: Condvar,
}

impl Answers {
    fn ask(&self) {
        let mut a = self.asked.lock().unwrap();
        if a.since.is_none() {
            a.since = Some(Instant::now());
            self.changed.notify_all();
        }
    }

    /// Any message answers; one after the pane said the engine isn't responding takes that back.
    fn heard(&self, link: &Link) {
        let mut a = self.asked.lock().unwrap();
        if a.since.is_none() && a.hung.is_none() {
            return;
        }
        a.since = None;
        if let Some(before) = a.hung.take() {
            let mut status = link.status.lock().unwrap();
            if status.is_hung() {
                *status = before;
            }
            drop(status);
            link.wake();
        }
        self.changed.notify_all();
    }

    fn end(&self) {
        self.asked.lock().unwrap().over = true;
        self.changed.notify_all();
    }

    /// Sleeps until a request is asked, then until it's answered or its deadline passes: the
    /// thread only runs while the app waits on the engine.
    fn watch(&self, link: &Link) {
        let mut a = self.asked.lock().unwrap();
        loop {
            if a.over {
                return;
            }
            let Some(since) = a.since.filter(|_| a.hung.is_none()) else {
                a = self.changed.wait(a).unwrap();
                continue;
            };
            let left = ANSWER_DEADLINE.saturating_sub(since.elapsed());
            if !left.is_zero() {
                a = self.changed.wait_timeout(a, left).unwrap().0;
                continue;
            }
            rune::marks::mark("browser.engine.hung");
            let mut status = link.status.lock().unwrap();
            a.hung = Some(std::mem::replace(&mut *status, EngineStatus::Failed(ERR_HUNG.into())));
            drop(status);
            link.wake();
        }
    }
}

pub(crate) fn wire_stream(stream: UnixStream, mach_service: Option<String>, link: &Link) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::channel::<HostMsg>();
    let _ = tx.send(HostMsg::Hello { mach_service, pid: std::process::id() });
    let mut early = link.early.lock().unwrap();
    for (id, open) in &link.opens {
        if let HostMsg::OpenTab { url, width_pt, height_pt, scale, .. } = open {
            early.insert(*id, (url.clone(), (*width_pt, *height_pt, *scale)));
        }
        let _ = tx.send(open.clone());
    }
    *link.outbox.lock().unwrap() = Some(tx);
    drop(early);
    // Connected: the app's next turn sends the tabs that wait (`flush_pending_opens`).
    link.set(EngineStatus::Connecting);

    let answers = Arc::new(Answers::default());
    let (watched, watcher) = (answers.clone(), link.clone());
    std::thread::Builder::new().name("rune-browser-answers".into()).spawn(move || watched.watch(&watcher)).ok();

    let asked = answers.clone();
    std::thread::Builder::new()
        .name("rune-browser-tx".into())
        .spawn(move || {
            let mut writer = stream;
            while let Ok(msg) = rx.recv() {
                let is_shutdown = matches!(msg, HostMsg::Shutdown);
                if let Ok(mut line) = serde_json::to_string(&msg) {
                    line.push('\n');
                    if writer.write_all(line.as_bytes()).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                    if expects_answer(&msg) {
                        asked.ask();
                    }
                }
                if is_shutdown {
                    break;
                }
            }
            // The app let the engine go (or it went): nothing more is waited for.
            asked.end();
        })
        .ok();

    let link = link.clone();
    std::thread::Builder::new()
        .name("rune-browser-rx".into())
        .spawn(move || {
            let mut reader = BufReader::new(reader_stream);
            let mut line = String::new();
            loop {
                line.clear();
                if !matches!(reader.read_line(&mut line), Ok(1..)) { break; }
                // The sidecar writes the type tag first. Borrow the large base64 field directly
                // from the reusable line buffer. Other layouts still go through the wire enum.
                if line.starts_with("{\"type\":\"cpu_frame\",")
                    && let Ok(paint) = serde_json::from_str::<WirePaint>(&line)
                {
                    answers.heard(&link);
                    paint.push(&link.surfaces);
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<EngineMsg>(&line) else { continue };
                answers.heard(&link);
                // Straight to the tab's surface, which wakes the app itself.
                if let EngineMsg::SharedFrame { tab_id, frame_id, width, height, texture, release_fence } = msg {
                    paint_event(PaintEvent::Frame);
                    push_shared(&link.surfaces, tab_id, frame_id, width, height, texture, release_fence);
                    continue;
                }
                if let EngineMsg::CpuFrame { tab_id, frame_id, width, height, bgra_b64 } = msg {
                    WirePaint { tab_id, frame_id, width, height, bgra_b64: &bgra_b64 }.push(&link.surfaces);
                    continue;
                }
                // On reconnect the engine repaints immediately after TabList. Give those paints
                // their destination before the UI thread has a chance to adopt the restored tabs.
                if let EngineMsg::TabList { tabs } = &msg {
                    let mut surfaces = link.surfaces.lock().unwrap();
                    for tab in tabs {
                        surfaces.entry(tab.tab_id).or_insert_with(TabSurface::new);
                    }
                }
                let _ = link.inbox_tx.send(msg);
                link.wake();
            }
            answers.end();
            lost(&link);
        })
        .ok();
}

#[derive(serde::Deserialize)]
struct WirePaint<'a> {
    tab_id: u64,
    frame_id: u64,
    width: u32,
    height: u32,
    bgra_b64: &'a str,
}

impl WirePaint<'_> {
    fn push(self, surfaces: &Surfaces) {
        let surface = surfaces.lock().unwrap().get(&self.tab_id).cloned();
        if let Some(surface) = surface && let Some(bytes) = decode_frame(self.width, self.height, self.bgra_b64) {
            frame::push_bgra(&surface, self.width, self.height, bytes, self.frame_id);
            paint_event(PaintEvent::Frame);
        }
    }
}

pub(crate) enum PaintEvent { Navigate, Frame }

#[derive(Clone, Copy)]
enum PaintTrace { Start, NavigatedBeforeFirst, AwaitNavigation, AwaitPaint, Complete }

impl PaintTrace {
    fn step(&mut self, event: PaintEvent) -> &'static [&'static str] {
        use PaintTrace::*;
        let (next, marks): (_, &[_]) = match (*self, event) {
            (Start, PaintEvent::Navigate) => (NavigatedBeforeFirst, &[]),
            (Start, PaintEvent::Frame) => (AwaitNavigation, &["browser.page.first_frame"]),
            (NavigatedBeforeFirst, PaintEvent::Frame) => (Complete, &["browser.page.first_frame", "browser.page.painted"]),
            (AwaitNavigation, PaintEvent::Navigate) => (AwaitPaint, &[]),
            (AwaitPaint, PaintEvent::Frame) => (Complete, &["browser.page.painted"]),
            _ => return &[],
        };
        *self = next;
        marks
    }
}

pub(crate) fn paint_event(event: PaintEvent) {
    static TRACE: Mutex<PaintTrace> = Mutex::new(PaintTrace::Start);
    let mut trace = TRACE.lock().unwrap();
    for mark in trace.step(event) {
        rune::marks::mark(mark);
    }
}

/// `on_paint` sends one tightly packed, top-down BGRA plane with standard padded base64.
fn decode_frame(width: u32, height: u32, encoded: &str) -> Option<Vec<u8>> {
    let len = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if len == 0 || base64::encoded_len(len, true)? != encoded.len() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    if bytes.len() != len {
        return None;
    }
    Some(bytes)
}

#[cfg(windows)]
fn push_shared(surfaces: &Surfaces, tab_id: u64, frame_id: u64, width: u32, height: u32, texture: String, release_fence: String) {
    let surface = surfaces.lock().unwrap().get(&tab_id).cloned();
    if let Some(surface) = surface {
        let frame = rune::native_surface::windows::SharedTexture::named(texture, width, height)
            .release_fence(release_fence, frame_id);
        let _ = surface.push_shared(frame, frame_id);
    }
}

/// Only a Windows engine sends shared textures.
#[cfg(not(windows))]
fn push_shared(_: &Surfaces, _: u64, _: u64, _: u32, _: u32, _: String, _: String) {}
