//! Processes plugins start (`HostApiVtable::process_spawn`), supervised by the host: each runs
//! with its window hidden, is killed when its plugin unloads or swaps, and on Windows sits in a
//! job object that kills it when the app exits however it does (a crash included).
//!
//! The app's thread only queues a spawn: a long-lived supervisor thread starts the process and
//! the threads that read its output and wait for it, so no thread starts on the app's thread
//! (on Windows that runs every library's thread-attach code under the loader lock). Output and
//! the exit land in an inbox and wake the plugin's slot; `drain` hands them to the plugin's
//! callback on the half that spawned it. A process at rest costs the app's thread nothing.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use opencode_plugin::abi::{AbiEvent, AbiStr, EVENT_PROCESS_EXIT, EVENT_PROCESS_OUTPUT};
use rune::Tx;

use super::instance::{CallbackEnv, Scope};
use super::run::act;
use super::vtable::{WakeShared, slot_bit};
use crate::App;

/// What a plugin asked to start.
pub struct Spec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: String,
    pub stdin: Vec<u8>,
    pub keep_stdin: bool,
    pub timeout: Option<Duration>,
}

/// One running process, on the app's thread: whose callback hears it.
struct Proc {
    slot: usize,
    generation: u32,
    scope: Scope,
    events: CallbackEnv,
    ctl: Arc<Ctl>,
}

/// What the supervisor's threads and the app's thread share about one process.
#[derive(Default)]
struct Ctl {
    /// The process, while it can still be killed; the waiter empties it as it exits.
    kill: Mutex<Option<Killer>>,
    stdin: Mutex<Option<ChildStdin>>,
    /// 1 when its timeout killed it, 2 when a kill did.
    how: std::sync::atomic::AtomicU8,
}

enum Msg {
    Out { stream: u8, text: String },
    Exit { code: Option<i64>, error: String },
}

/// The plugins' processes and the inbox their threads fill.
#[derive(Default)]
pub struct Processes {
    next: u32,
    table: HashMap<u32, Proc>,
    inbox: Arc<Mutex<Vec<(u32, Msg)>>>,
}

impl Processes {
    /// Queues `spec` for slot `slot`'s `scope` half; `events` hears it. The handle.
    pub fn spawn(&mut self, slot: usize, generation: u32, scope: Scope, spec: Spec, events: CallbackEnv, wake: &Arc<WakeShared>) -> u32 {
        self.next = self.next.wrapping_add(1).max(1);
        let handle = self.next;
        let ctl = Arc::new(Ctl::default());
        self.table.insert(handle, Proc { slot, generation, scope, events, ctl: ctl.clone() });
        let (inbox, wake2, bit) = (self.inbox.clone(), wake.clone(), slot_bit(slot));
        let sink: Sink = Arc::new(move |msg| {
            if let Ok(mut inbox) = inbox.lock() {
                inbox.push((handle, msg));
            }
            wake2.wake(bit, false);
        });
        let job = Job { spec, ctl, sink };
        let sent = supervisor().lock().ok().is_some_and(|tx| tx.send(job).is_ok());
        if !sent {
            if let Ok(mut inbox) = self.inbox.lock() {
                inbox.push((handle, Msg::Exit { code: None, error: "the process supervisor isn't running".into() }));
            }
            wake.wake(slot_bit(slot), false);
        }
        handle
    }

    /// Writes to slot `slot`'s process `handle`'s stdin, closing it after when `close`.
    pub fn write(&self, slot: usize, handle: u32, bytes: &[u8], close: bool) -> bool {
        let Some(p) = self.table.get(&handle).filter(|p| p.slot == slot) else { return false };
        let Ok(mut stdin) = p.ctl.stdin.lock() else { return false };
        let ok = stdin.as_mut().is_some_and(|s| s.write_all(bytes).and_then(|()| s.flush()).is_ok());
        if close {
            *stdin = None;
        }
        ok
    }

    /// Kills slot `slot`'s process `handle`.
    pub fn kill(&self, slot: usize, handle: u32) {
        if let Some(p) = self.table.get(&handle).filter(|p| p.slot == slot) {
            p.ctl.terminate(2);
        }
    }

    /// Kills every process of slot `slot` whose generation `gone` picks: the plugin unloaded,
    /// failed, or swapped. Their exits are dropped with them.
    pub fn reap(&mut self, slot: usize, gone: impl Fn(u32) -> bool) {
        self.table.retain(|_, p| {
            let gone = p.slot == slot && gone(p.generation);
            if gone {
                p.ctl.terminate(2);
            }
            !gone
        });
    }

}

/// Hands every queued output and exit to its plugin's callback. Nothing queued costs one lock.
pub fn drain(app: &mut App, tx: &mut Tx) -> bool {
    let queued = match app.plugins.shared.processes.inbox.lock() {
        Ok(mut inbox) if !inbox.is_empty() => std::mem::take(&mut *inbox),
        _ => return false,
    };
    let mut changed = false;
    let view = |s: &str| AbiStr { ptr: s.as_ptr(), len: s.len(), _lt: std::marker::PhantomData };
    for (handle, msg) in queued {
        let Some(p) = app.plugins.shared.processes.table.get(&handle) else { continue };
        let (slot, generation, scope, cb, how) = (p.slot, p.generation, p.scope, p.events.cb, p.ctl.how.load(std::sync::atomic::Ordering::Acquire));
        let (kind, text, extra, x, y, ended) = match msg {
            Msg::Out { stream, text } => (EVENT_PROCESS_OUTPUT, text, String::new(), f32::from(stream), 0.0, false),
            Msg::Exit { code, error } => (EVENT_PROCESS_EXIT, code.map(|c| c.to_string()).unwrap_or_default(), error, 0.0, f32::from(how), true),
        };
        if let Some(func) = cb.func {
            let event = AbiEvent { kind, mods: handle, x, y, text: view(&text), extra: view(&extra) };
            // SAFETY: the plugin's own callback with its data, on the state of the generation that
            // spawned it; the event's strings outlive the call.
            if let super::run::Ran::Done(_, c) = act(app, tx, slot, scope, Some(generation), |state, ctx| unsafe { func(state, ctx, cb.data, &event) }) {
                changed |= c;
            }
        }
        if ended {
            app.plugins.shared.processes.table.remove(&handle);
        }
    }
    changed
}

/// Where a process's output and exit go.
type Sink = Arc<dyn Fn(Msg) + Send + Sync>;

struct Job {
    spec: Spec,
    ctl: Arc<Ctl>,
    sink: Sink,
}

impl Job {
    fn send(&self, msg: Msg) {
        (self.sink)(msg);
    }
}

/// A process started for the Luau host: write to it, kill it, ask whether it runs.
#[derive(Clone)]
pub struct Control(Arc<Ctl>);

impl Control {
    pub fn write(&self, bytes: &[u8], close: bool) -> bool {
        let Ok(mut stdin) = self.0.stdin.lock() else { return false };
        let ok = stdin.as_mut().is_some_and(|s| s.write_all(bytes).and_then(|()| s.flush()).is_ok());
        if close {
            *stdin = None;
        }
        ok
    }

    pub fn kill(&self) {
        self.0.terminate(2);
    }

    pub fn running(&self) -> bool {
        self.0.kill.lock().is_ok_and(|k| k.is_some())
    }

    /// 1 when its timeout killed it, 2 when a kill did.
    pub fn how(&self) -> u8 {
        self.0.how.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// What a process sends the Luau host.
pub enum Event {
    Output { stream: u8, text: String },
    Exit { code: Option<i64>, error: String },
}

/// Starts `spec` on the supervisor, as a plugin's process (window hidden, in the app's job);
/// `sink` hears its output and exit on the supervisor's threads.
pub fn launch(spec: Spec, sink: impl Fn(Event) + Send + Sync + 'static) -> Control {
    let ctl = Arc::new(Ctl::default());
    let sink: Sink = Arc::new(move |msg| {
        sink(match msg {
            Msg::Out { stream, text } => Event::Output { stream, text },
            Msg::Exit { code, error } => Event::Exit { code, error },
        })
    });
    let job = Job { spec, ctl: ctl.clone(), sink: sink.clone() };
    if !supervisor().lock().ok().is_some_and(|tx| tx.send(job).is_ok()) {
        sink(Msg::Exit { code: None, error: "the process supervisor isn't running".into() });
    }
    Control(ctl)
}

fn supervisor() -> &'static Mutex<Sender<Job>> {
    static TX: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let started = std::thread::Builder::new().name("plugin-processes".into()).spawn(move || {
            for job in rx {
                start(job);
            }
        });
        if let Err(e) = started {
            super::tasks::log(format!("plugin process supervisor didn't start: {e}"));
        }
        Mutex::new(tx)
    })
}

/// Starts the supervisor with the plugins, so the first spawn finds it running.
pub fn warm() {
    supervisor();
}

/// On the supervisor: starts `job`'s process and the threads that read and wait for it.
fn start(job: Job) {
    let spec = &job.spec;
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    if !spec.cwd.is_empty() {
        cmd.current_dir(&spec.cwd);
    }
    platform::hide(&mut cmd);
    let mut child: Child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return job.send(Msg::Exit { code: None, error: format!("{}: {e}", spec.program) }),
    };
    platform::adopt(&child);
    let mut stdin = child.stdin.take();
    if let Some(s) = stdin.as_mut()
        && !spec.stdin.is_empty()
    {
        s.write_all(&spec.stdin).and_then(|()| s.flush()).ok();
    }
    if spec.keep_stdin {
        if let Ok(mut slot) = job.ctl.stdin.lock() {
            *slot = stdin;
        }
    } else {
        drop(stdin);
    }
    if let Ok(mut kill) = job.ctl.kill.lock() {
        *kill = Some(Killer::of(&child));
    }
    let job = Arc::new(job);
    let (read_done, readers_done) = channel::<()>();
    let readers = [(1u8, child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>)), (2, child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>))]
        .into_iter()
        .filter_map(|(stream, pipe)| {
            let (pipe, job, done) = (pipe?, job.clone(), read_done.clone());
            std::thread::Builder::new()
                .name("plugin-process-read".into())
                .spawn(move || {
                    read(pipe, stream, &job);
                    done.send(()).ok();
                })
                .ok()
        })
        .count();
    drop(read_done);
    if let Some(after) = job.spec.timeout {
        let ctl = job.ctl.clone();
        std::thread::Builder::new()
            .name("plugin-process-timeout".into())
            .spawn(move || {
                std::thread::sleep(after);
                ctl.terminate(1);
            })
            .ok();
    }
    let waiter = job.clone();
    let spawned = std::thread::Builder::new().name("plugin-process-wait".into()).spawn(move || {
        let status = child.wait();
        // Under the kill lock: a kill from now on finds nothing, so it never reaches a handle the
        // system may have handed to another process once `child` drops.
        if let Ok(mut kill) = waiter.ctl.kill.lock() {
            *kill = None;
        }
        drop(child);
        // Its output ends with it, unless a process it started holds its pipes open (`wsl.exe`
        // hands them to its service): then the exit doesn't wait on them.
        for _ in 0..readers {
            if readers_done.recv_timeout(Duration::from_millis(500)).is_err() {
                break;
            }
        }
        if let Ok(mut stdin) = waiter.ctl.stdin.lock() {
            *stdin = None;
        }
        let msg = match status {
            Ok(s) => Msg::Exit { code: s.code().map(i64::from), error: String::new() },
            Err(e) => Msg::Exit { code: None, error: e.to_string() },
        };
        waiter.send(msg);
    });
    if spawned.is_err() {
        job.ctl.terminate(2);
    }
}

/// Reads one pipe to its end, sending decoded chunks.
fn read(mut pipe: Box<dyn Read + Send>, stream: u8, job: &Job) {
    let mut buf = vec![0u8; 8192];
    let mut decoder = Decoder::default();
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let text = decoder.decode(&buf[..n]);
                if !text.is_empty() {
                    job.send(Msg::Out { stream, text });
                }
            }
        }
    }
    let rest = decoder.flush();
    if !rest.is_empty() {
        job.send(Msg::Out { stream, text: rest });
    }
}

/// Desktop's output decoder (`runtime.ts`): a stream whose first chunk reads as UTF-16LE (a BOM,
/// or zero high bytes, as `wsl.exe` writes) stays UTF-16LE; anything else is UTF-8. Bytes split
/// across chunks carry over.
#[derive(Default)]
struct Decoder {
    utf16: Option<bool>,
    carry: Vec<u8>,
}

impl Decoder {
    fn decode(&mut self, chunk: &[u8]) -> String {
        let utf16 = *self.utf16.get_or_insert_with(|| utf16_like(chunk));
        let mut bytes = std::mem::take(&mut self.carry);
        bytes.extend_from_slice(chunk);
        if utf16 {
            let even = bytes.len() & !1;
            self.carry = bytes.split_off(even);
            let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            let text = String::from_utf16_lossy(&units);
            return text.strip_prefix('\u{feff}').map(str::to_owned).unwrap_or(text);
        }
        let valid = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => bytes.len(),
        };
        self.carry = bytes.split_off(valid);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn flush(&mut self) -> String {
        let rest = std::mem::take(&mut self.carry);
        String::from_utf8_lossy(&rest).into_owned()
    }
}

fn utf16_like(chunk: &[u8]) -> bool {
    if chunk.starts_with(&[0xff, 0xfe]) {
        return true;
    }
    let pairs = chunk.len() / 2;
    if pairs < 2 {
        return false;
    }
    let odd = (0..pairs).filter(|i| chunk[i * 2 + 1] == 0).count();
    let even = (0..pairs).filter(|i| chunk[i * 2] == 0).count();
    odd >= pairs.div_ceil(3) && even * 2 <= odd
}

impl Ctl {
    /// Kills the process if it still runs; `how` says why (1 timeout, 2 kill).
    fn terminate(&self, how: u8) {
        let Ok(kill) = self.kill.lock() else { return };
        if let Some(k) = kill.as_ref() {
            self.how.compare_exchange(0, how, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire).ok();
            k.kill();
        }
    }
}

/// Kills a process the waiter thread owns.
struct Killer {
    #[cfg(windows)]
    handle: isize,
    #[cfg(unix)]
    pid: i32,
}

impl Killer {
    fn of(child: &Child) -> Killer {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            Killer { handle: child.as_raw_handle() as isize }
        }
        #[cfg(unix)]
        {
            Killer { pid: child.id() as i32 }
        }
    }

    fn kill(&self) {
        #[cfg(windows)]
        // SAFETY: the waiter keeps the child, and so its handle, open while this is reachable.
        unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(self.handle as _, 1);
        }
        #[cfg(unix)]
        // SAFETY: the waiter hasn't reaped the child while this is reachable, so the pid is its.
        unsafe {
            libc::kill(self.pid, libc::SIGTERM);
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::OnceLock;

    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    /// No console window for a console program (`windowsHide`).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn hide(cmd: &mut Command) {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    /// The job every plugin process joins: closing it, as the app's exit does, kills them all.
    fn job() -> Option<isize> {
        static JOB: OnceLock<Option<isize>> = OnceLock::new();
        *JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls with valid, zeroed arguments; the handle stays open for the
            // app's life on purpose.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let size = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
                (SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const info).cast(), size) != 0).then_some(job as isize)
            }
        })
    }

    pub fn adopt(child: &Child) {
        use std::os::windows::io::AsRawHandle;
        if let Some(job) = job() {
            // SAFETY: both handles are open; a process already in a job that forbids nesting just
            // stays out of ours.
            unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) };
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::process::{Child, Command};

    pub fn hide(_: &mut Command) {}

    pub fn adopt(_: &Child) {}
}
