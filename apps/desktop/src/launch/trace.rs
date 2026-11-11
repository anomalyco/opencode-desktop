//! `--launch-trace <file>`: one JSON line per run with the launch's and quit's marks, for
//! `harness\launch-bench.ts` and `perf\launch.bench.ts`. Every mark is nanoseconds since the
//! process was created (the OS's creation time, so the time before `main` counts too), from the
//! monotonic clock. Rune's own steps (`rune::marks`: window, GPU, renderer, fonts, first frame,
//! input ready, quit hooks, windows dropped) and the app's (`main.entered`, `state.restored`,
//! `plugins.started`, `server.answered`, `content.live`, `quit.saved`, `storage.flushed`, …)
//! share one list.
//!
//! `--launch-trace-quit <ms>` quits the app as its Quit command does, `ms` after the window
//! shows live content (or after the first frame, when no server answers within
//! [`CONTENT_WAIT`]), so a bench times the quit of a launch that settled.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use rune::time::Instant;

use rune::Tx;
use rune::marks::mark;

use crate::{App, Route};

/// How long a traced quit waits for live content before it quits anyway.
const CONTENT_WAIT: Duration = Duration::from_secs(10);

struct Boot {
    /// `main` entered, on the monotonic clock.
    at: Instant,
    /// Nanoseconds from the process's creation to `at`.
    since_created: u64,
    /// The process's creation time (FILETIME, 100 ns since 1601), so a bench can line its own
    /// clock up with the marks.
    created: u64,
    /// Page faults (soft and hard) before `main`'s first line: the loader and static initialisers.
    faults: u32,
}

static BOOT: OnceLock<Boot> = OnceLock::new();
/// After the first drawn frame: the process's IO so far, and (`--launch-trace-threads`) its threads.
static FIRST_IO: OnceLock<(Io, Vec<(u32, String)>)> = OnceLock::new();

/// The process's IO counters (`GetProcessIoCounters`): every read, write and other operation,
/// files and devices alike.
#[derive(Clone, Copy, Default)]
struct Io {
    read_ops: u64,
    read_bytes: u64,
    write_ops: u64,
    write_bytes: u64,
    other_ops: u64,
    faults: u32,
}

impl Io {
    fn json(self) -> serde_json::Value {
        serde_json::json!({ "read_ops": self.read_ops, "read_bytes": self.read_bytes, "write_ops": self.write_ops, "write_bytes": self.write_bytes, "other_ops": self.other_ops })
    }
}
static QUIT_SENT: AtomicBool = AtomicBool::new(false);

/// First thing in `main`.
pub fn begin() {
    BOOT.get_or_init(|| {
        let at = Instant::now();
        let (created, now) = process_clock();
        Boot { at, since_created: now.saturating_sub(created).saturating_mul(100), created, faults: page_faults() }
    });
    mark("main.entered");
    #[cfg(target_os = "windows")]
    super::io_trace::start();
}

/// Whether this run writes a trace or prints its milestones.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::arg("--launch-trace").is_some() || echoes())
}

/// `--launch-marks`: each launch milestone printed as a line the moment the app sees it
/// (`launch-mark <name> <ns since creation> at=<epoch ms>`), for strobe's `launch: { milestones }`.
fn echoes() -> bool {
    static ECHO: OnceLock<bool> = OnceLock::new();
    *ECHO.get_or_init(|| crate::flag("--launch-marks"))
}

/// The milestones `--launch-marks` prints.
const MILESTONES: [&str; 5] = ["frame.first.presented", "text.complete", "input.ready", "server.answered", "content.live"];

/// Prints the milestones reached since the last call.
fn echo() {
    static PRINTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let done = PRINTED.load(Ordering::Relaxed);
    if !echoes() || done == MILESTONES.len() {
        return;
    }
    let marks = rune::marks::marks();
    let mut printed = done;
    for name in MILESTONES.iter().skip(done) {
        let Some((_, t)) = marks.iter().find(|(n, _)| n == name) else { break };
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let at = ns(*t);
        // Epoch ms of the mark, for strobe's milestone `at` group: FILETIME is 100 ns since 1601.
        let epoch_ms = BOOT.get().map_or(0, |b| (b.created.saturating_sub(116_444_736_000_000_000) / 10_000).saturating_add(at / 1_000_000));
        let _ = writeln!(out, "launch-mark {name} {at} at={epoch_ms}");
        let _ = out.flush();
        printed += 1;
    }
    PRINTED.store(printed, Ordering::Relaxed);
}

/// Nanoseconds from the process's creation to `t`.
fn ns(t: Instant) -> u64 {
    let Some(boot) = BOOT.get() else { return 0 };
    match t.checked_duration_since(boot.at) {
        Some(d) => boot.since_created.saturating_add(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)),
        None => boot.since_created.saturating_sub(u64::try_from((boot.at - t).as_nanos()).unwrap_or(u64::MAX)),
    }
}

/// After a frame drew: the first with the session's messages, and the first with live content
/// (the window's server answered and Home, a draft or the session with messages shows).
pub fn drawn(s: &App) {
    if !on() {
        return;
    }
    let _ = FIRST_IO.get_or_init(|| (io_counters(), crate::flag("--launch-trace-threads").then(threads).unwrap_or_default()));
    let rows = s.painted_session().map_or(0, |(_, rows)| rows);
    if rows > 0 {
        mark("content.rows");
    }
    let shows = rows > 0 || !matches!(s.route, Route::Session { .. });
    if shows && !crate::launch::waiting(s) && !crate::launch::offline(s) {
        mark("content.live");
    }
    echo();
}

/// `poll`: a traced quit, once, when it's due.
pub fn poll(tx: &mut Tx) {
    echo();
    let Some(after) = quit_after() else { return };
    if QUIT_SENT.load(Ordering::Relaxed) {
        return;
    }
    let marks = rune::marks::marks();
    let at = |name: &str| marks.iter().find(|(n, _)| *n == name).map(|(_, t)| *t);
    let Some(first) = at("frame.first.presented") else {
        tx.wake_after(Duration::from_millis(50));
        return;
    };
    let due = at("content.live").unwrap_or(first + CONTENT_WAIT) + after;
    let now = Instant::now();
    if now < due {
        // Content may land before the fallback: look again soon.
        tx.wake_after((due - now).min(Duration::from_millis(50)));
        return;
    }
    if !QUIT_SENT.swap(true, Ordering::Relaxed) {
        mark("quit.requested");
        tx.quit();
    }
}

fn quit_after() -> Option<Duration> {
    static AFTER: OnceLock<Option<Duration>> = OnceLock::new();
    *AFTER.get_or_init(|| crate::arg("--launch-trace-quit").and_then(|ms| ms.parse().ok()).map(Duration::from_millis))
}

/// The text renderer the run draws with, named for the trace (`text_rendering::poll`).
pub fn renderer(name: &'static str) {
    let _ = RENDERER.set(name);
}

static RENDERER: OnceLock<&'static str> = OnceLock::new();

/// Last thing in `main`: appends the run's line to the `--launch-trace` file.
pub fn finish(windows: usize) {
    let Some(path) = crate::arg("--launch-trace") else { return };
    mark("main.return");
    let Some(boot) = BOOT.get() else { return };
    let marks: serde_json::Map<String, serde_json::Value> = rune::marks::marks().into_iter().map(|(n, t)| (n.to_owned(), ns(t).into())).collect();
    let metrics: serde_json::Map<String, serde_json::Value> = rune::marks::metrics().into_iter().map(|(n, v)| (n.to_owned(), v.into())).collect();
    let notes: serde_json::Map<String, serde_json::Value> = rune::marks::notes().into_iter().map(|(n, v)| (n.to_owned(), v.into())).collect();
    let line = serde_json::json!({
        "v": 1,
        "pid": std::process::id(),
        "created_filetime": boot.created,
        "renderer": RENDERER.get().copied().unwrap_or("default"),
        "windows": windows,
        "ui_tid": ui_thread(),
        "page_faults": { "main": boot.faults, "first_frame": FIRST_IO.get().map(|(io, _)| io.faults), "exit": page_faults() },
        "io_first_frame": FIRST_IO.get().map(|(io, _)| io.json()),
        "io_exit": io_counters().json(),
        "io": io_calls(&ns),
        "threads": FIRST_IO.get().map(|(_, t)| t.clone()).unwrap_or_default().into_iter().chain(if crate::flag("--launch-trace-threads") { threads() } else { Vec::new() }).map(|(id, name)| serde_json::json!([id, name])).collect::<Vec<_>>(),
        "marks_ns": marks,
        "metrics": metrics,
        "notes": notes,
        "boot_record": crate::storage::boot_bytes().map(|(all, plugins)| serde_json::json!({ "bytes": all, "plugin_bytes": plugins })),
    });
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

/// The process's creation time and now, as FILETIMEs.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn process_clock() -> (u64, u64) {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let zero = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user, mut now) = (zero(), zero(), zero(), zero(), zero());
    // SAFETY: the pseudo-handle of this process and five FILETIMEs this frame owns.
    unsafe {
        GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user);
        GetSystemTimePreciseAsFileTime(&mut now);
    }
    let value = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    (value(created), value(now))
}

/// `--launch-trace-io`'s calls, each with its time (ns since creation), thread, how long it took and
/// the app frames that made it.
#[cfg(target_os = "windows")]
fn io_calls(ns: &dyn Fn(Instant) -> u64) -> serde_json::Value {
    let calls = super::io_trace::take();
    let reads = (super::io_trace::READ_CALLS.load(Ordering::Relaxed), super::io_trace::READ_BYTES.load(Ordering::Relaxed));
    serde_json::json!({
        "read_calls": reads.0,
        "read_bytes": reads.1,
        "calls": calls.into_iter().map(|(c, sites)| serde_json::json!({ "ns": ns(c.at), "took_ns": c.took_ns, "tid": c.tid, "op": c.op, "path": c.path, "ok": c.ok, "sites": sites })).collect::<Vec<_>>(),
    })
}

#[cfg(not(target_os = "windows"))]
fn io_calls(_: &dyn Fn(Instant) -> u64) -> serde_json::Value {
    serde_json::Value::Null
}

/// The UI thread's OS id, which ETW traces name threads by.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn ui_thread() -> u32 {
    // SAFETY: no arguments; it reads the calling thread's id.
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

#[cfg(not(target_os = "windows"))]
fn ui_thread() -> u32 {
    0
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn io_counters() -> Io {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessIoCounters, IO_COUNTERS};
    // SAFETY: an IO_COUNTERS this frame owns, filled for this process's pseudo-handle.
    let c = unsafe {
        let mut c: IO_COUNTERS = std::mem::zeroed();
        GetProcessIoCounters(GetCurrentProcess(), &mut c);
        c
    };
    Io { read_ops: c.ReadOperationCount, read_bytes: c.ReadTransferCount, write_ops: c.WriteOperationCount, write_bytes: c.WriteTransferCount, other_ops: c.OtherOperationCount, faults: page_faults() }
}

#[cfg(not(target_os = "windows"))]
fn io_counters() -> Io {
    Io::default()
}

/// This process's page faults so far, soft and hard (`GetProcessMemoryInfo`).
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn page_faults() -> u32 {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: a PROCESS_MEMORY_COUNTERS this frame owns, sized for the call.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb);
        c.PageFaultCount
    }
}

#[cfg(not(target_os = "windows"))]
fn page_faults() -> u32 {
    0
}

/// This process's live threads and their names (`SetThreadDescription`, which Rust's named threads
/// set), for naming the threads in an ETW trace.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn threads() -> Vec<(u32, String)> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
    use windows_sys::Win32::System::Threading::{GetThreadDescription, OpenThread, THREAD_QUERY_LIMITED_INFORMATION};
    let pid = std::process::id();
    let mut out = Vec::new();
    // SAFETY: a thread snapshot walked with a THREADENTRY32 this frame owns; each thread handle and
    // description is released after it's read.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap.is_null() || snap as isize == -1 {
            return out;
        }
        let mut e: THREADENTRY32 = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snap, &mut e) != 0;
        while more {
            if e.th32OwnerProcessID == pid {
                let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, e.th32ThreadID);
                let mut name = String::new();
                if !h.is_null() {
                    let mut text: windows_sys::core::PWSTR = std::ptr::null_mut();
                    if GetThreadDescription(h, &mut text) >= 0 && !text.is_null() {
                        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
                        name = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
                        windows_sys::Win32::Foundation::LocalFree(text.cast());
                    }
                    CloseHandle(h);
                }
                out.push((e.th32ThreadID, name));
            }
            more = Thread32Next(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(not(target_os = "windows"))]
fn threads() -> Vec<(u32, String)> {
    Vec::new()
}

/// Elsewhere the marks count from `main`.
#[cfg(not(target_os = "windows"))]
fn process_clock() -> (u64, u64) {
    (0, 0)
}
