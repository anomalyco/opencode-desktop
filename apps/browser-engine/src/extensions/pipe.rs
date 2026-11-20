//! Private, in-process anonymous CDP pipe to Chromium's `DevToolsPipeHandler`.
//!
//! Zero TCP ports, zero named pipes, zero filesystem sockets.
//! - Windows: two `CreatePipe` pairs with `bInheritHandle = FALSE` (`lpPipeAttributes = None`),
//!   passed to Chromium inside this same process via `--remote-debugging-pipe` and
//!   `--remote-debugging-io-pipes=<read>,<write>`. Because `bInheritHandle = FALSE`, no CEF
//!   renderer/GPU/utility child process can inherit or open the pipe handles.
//! - Unix (macOS / Linux): verifies FDs 3 and 4 are vacant before reserving them with `O_CLOEXEC`
//!   placeholders (never clobbering pre-existing caller FDs); two `pipe()` pairs are dup'd >= 5
//!   first, placed on FDs 3 and 4 with `FD_CLOEXEC` (rolling back FD 3 on partial `dup2` failure),
//!   paired with `--remote-debugging-pipe`.
//! - Both directions use bounded `sync_channel` queues (256 inbound / 512 outbound) with a
//!   coalesced `Pump(0)` wake and a dedicated writer thread, so neither a hostile extension flood
//!   nor a stalled browser reader can OOM or deadlock the engine main thread.

#![allow(unsafe_code)]

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Write};
#[cfg(unix)]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};

use serde_json::{Value, json};

/// Hard ceiling on a single NUL-delimited CDP frame read from the pipe (4 MiB).
const MAX_CDP_MSG_BYTES: usize = 4 * 1024 * 1024;
/// Bounded inbound CDP queue capacity (applies OS pipe backpressure against tight JS floods).
const INBOUND_CAP: usize = 256;
/// Bounded outbound security/control CDP queue (`Fetch.*`, `Target.*`, `Debugger.*`, `Extensions.*`)
/// drained strictly ahead of RPC replies so RPC floods can never drop or starve control commands.
const CONTROL_OUTBOUND_CAP: usize = 512;
/// Bounded outbound RPC/event push queue (`__ocExtRecv`).
const RPC_OUTBOUND_CAP: usize = 512;

/// Bitmask of Unix FDs 3 (bit 0) and 4 (bit 1) that were confirmed vacant and reserved by us.
#[cfg(unix)]
static RESERVED_FDS: AtomicU8 = AtomicU8::new(0);

/// Messages delivered from the background CDP pipe reader to the engine main loop.
pub enum PipeInbound {
    Message(Value),
    FrameDropped,
    Closed,
}

/// Unix only: checks whether FDs 3 and 4 are vacant and reserves them with `O_CLOEXEC`
/// placeholders before `UnixListener::bind` so the engine's IPC socket lands on FD >= 5.
/// Never clobbers or closes pre-existing open FDs 3 or 4 inherited from the environment.
#[cfg(unix)]
pub fn reserve_unix_fds() {
    // SAFETY: inspects vacant FDs 3 and 4 via `fcntl(F_GETFD)` and fills only vacant slots with
    // `/dev/null` (`O_CLOEXEC`), recording exactly which slots we own in `RESERVED_FDS`.
    unsafe {
        let mut owned: u8 = 0;
        let fd3_vacant = libc::fcntl(3, libc::F_GETFD) < 0;
        let fd4_vacant = libc::fcntl(4, libc::F_GETFD) < 0;
        if !fd3_vacant || !fd4_vacant {
            RESERVED_FDS.store(0, Ordering::SeqCst);
            return;
        }
        let mut placeholders = Vec::new();
        loop {
            let fd = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            if fd < 0 {
                break;
            }
            if fd > 4 {
                libc::close(fd);
                break;
            }
            if fd == 3 {
                owned |= 1;
            } else if fd == 4 {
                owned |= 2;
            }
            placeholders.push(fd);
        }
        for fd in placeholders {
            if fd < 3 {
                libc::close(fd);
            }
        }
        RESERVED_FDS.store(owned, Ordering::SeqCst);
    }
}

#[cfg(not(unix))]
pub fn reserve_unix_fds() {}

/// Unix only: releases ONLY the FDs 3/4 that `reserve_unix_fds()` confirmed vacant and opened.
#[cfg(unix)]
pub fn release_reserved_unix_fds() {
    let owned = RESERVED_FDS.swap(0, Ordering::SeqCst);
    // SAFETY: closes only descriptors opened by `reserve_unix_fds()` above.
    unsafe {
        if (owned & 1) != 0 {
            libc::close(3);
        }
        if (owned & 2) != 0 {
            libc::close(4);
        }
    }
}

#[cfg(not(unix))]
pub fn release_reserved_unix_fds() {}

struct OutboundQueues {
    control: VecDeque<Vec<u8>>,
    rpc: VecDeque<Vec<u8>>,
    closed: bool,
}

pub struct CdpPipe {
    outbound: Arc<(Mutex<OutboundQueues>, Condvar)>,
    adopted_by_cef: AtomicBool,
    /// Coalesces `Event::Pump(0)` wakeups from the reader thread.
    wake_pending: Arc<AtomicBool>,
    /// Windows only: raw HANDLE values (`browser_read,browser_write`) owned by Chromium once
    /// `cef::initialize` starts `DevToolsPipeHandler`.
    #[cfg(windows)]
    io_pipes_arg: String,
    #[cfg(windows)]
    browser_handles: Mutex<Option<(isize, isize)>>,
}

impl CdpPipe {
    /// Creates the private anonymous pipe pair, pre-writes `Target.setAutoAttach` directly into
    /// the kernel pipe buffer before `cef::initialize()` starts, spawns bounded reader & writer
    /// threads, and returns `(pipe, inbound_rx, wake_pending)`.
    pub fn create(events: SyncSender<crate::Event>) -> Option<(Self, Receiver<PipeInbound>, Arc<AtomicBool>)> {
        #[cfg(windows)]
        let (mut our_write, our_read, browser_read_raw, browser_write_raw) = create_win_pipes()?;
        #[cfg(unix)]
        let (mut our_write, our_read) = create_unix_pipes()?;

        // Pre-write Target.setAutoAttach directly into the OS pipe buffer before CEF boots so
        // Chromium's DevToolsPipeHandler processes it before any extension service worker runs
        // line 1.
        let mut init_cmd = json!({
            "id": 1,
            "method": "Target.setAutoAttach",
            "params": {
                "autoAttach": true,
                "waitForDebuggerOnStart": true,
                "flatten": true
            }
        })
        .to_string()
        .into_bytes();
        init_cmd.push(0);
        if our_write.write_all(&init_cmd).is_err() || our_write.flush().is_err() {
            #[cfg(windows)]
            {
                use windows::Win32::Foundation::{CloseHandle, HANDLE};
                // SAFETY: handles were just created and not yet adopted by CEF.
                unsafe {
                    let _ = CloseHandle(HANDLE(browser_read_raw as *mut _));
                    let _ = CloseHandle(HANDLE(browser_write_raw as *mut _));
                }
            }
            #[cfg(unix)]
            release_reserved_unix_fds();
            return None;
        }

        let outbound = Arc::new((
            Mutex::new(OutboundQueues {
                control: VecDeque::new(),
                rpc: VecDeque::new(),
                closed: false,
            }),
            Condvar::new(),
        ));
        spawn_writer(our_write, outbound.clone());

        let (in_tx, in_rx) = mpsc::sync_channel::<PipeInbound>(INBOUND_CAP);
        let wake_pending = Arc::new(AtomicBool::new(false));
        spawn_reader(our_read, in_tx, wake_pending.clone(), events);

        let pipe = Self {
            outbound,
            adopted_by_cef: AtomicBool::new(false),
            wake_pending: wake_pending.clone(),
            #[cfg(windows)]
            io_pipes_arg: format!("{browser_read_raw},{browser_write_raw}"),
            #[cfg(windows)]
            browser_handles: Mutex::new(Some((browser_read_raw, browser_write_raw))),
        };
        Some((pipe, in_rx, wake_pending))
    }

    /// Marks the browser-owned handles/FDs as transferred to Chromium (`cef::initialize` started).
    pub fn mark_adopted_by_cef(&self) {
        self.adopted_by_cef.store(true, Ordering::Relaxed);
        #[cfg(windows)]
        {
            *self.browser_handles.lock().unwrap() = None;
        }
        #[cfg(unix)]
        {
            RESERVED_FDS.store(0, Ordering::SeqCst);
        }
    }

    /// Value for `--remote-debugging-io-pipes` on Windows (`None` on Unix where FDs 3/4 are used).
    pub fn io_pipes_switch(&self) -> Option<&str> {
        #[cfg(windows)]
        {
            Some(&self.io_pipes_arg)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Non-blocking enqueue of one high-priority control CDP command (`Fetch.*`, `Target.*`,
    /// `Debugger.*`, `Extensions.*`, etc.) drained ahead of RPC replies.
    pub fn send(&self, msg: &Value) {
        let mut bytes = msg.to_string().into_bytes();
        bytes.push(0);
        let (lock, cv) = &*self.outbound;
        let mut q = lock.lock().unwrap();
        if !q.closed && q.control.len() < CONTROL_OUTBOUND_CAP {
            q.control.push_back(bytes);
            cv.notify_one();
        }
    }

    /// Non-blocking enqueue of a lower-priority extension RPC reply / event push (`__ocExtRecv`).
    pub fn send_rpc(&self, msg: &Value) {
        let mut bytes = msg.to_string().into_bytes();
        bytes.push(0);
        let (lock, cv) = &*self.outbound;
        let mut q = lock.lock().unwrap();
        if !q.closed && q.rpc.len() < RPC_OUTBOUND_CAP {
            q.rpc.push_back(bytes);
            cv.notify_one();
        }
    }

    /// Non-blocking enqueue of a pipelined high-priority control CDP command batch.
    pub fn send_batch(&self, msgs: &[Value]) {
        if msgs.is_empty() {
            return;
        }
        let mut buf = Vec::with_capacity(msgs.len() * 128);
        for msg in msgs {
            let s = msg.to_string();
            buf.extend_from_slice(s.as_bytes());
            buf.push(0);
        }
        let (lock, cv) = &*self.outbound;
        let mut q = lock.lock().unwrap();
        if !q.closed && q.control.len() < CONTROL_OUTBOUND_CAP {
            q.control.push_back(buf);
            cv.notify_one();
        }
    }

    /// Closes our write end so Chromium's `DevToolsPipeHandlerReadThread` sees clean EOF before
    /// `cef::shutdown()`.
    pub fn close(&self) {
        let (lock, cv) = &*self.outbound;
        let mut q = lock.lock().unwrap();
        q.closed = true;
        q.control.clear();
        q.rpc.clear();
        cv.notify_all();
        self.wake_pending.store(false, Ordering::Relaxed);
    }
}

impl Drop for CdpPipe {
    fn drop(&mut self) {
        self.close();
        #[cfg(windows)]
        if let Some((r, w)) = self.browser_handles.lock().unwrap().take() {
            use windows::Win32::Foundation::{CloseHandle, HANDLE};
            // SAFETY: only reached if `cef::initialize` never adopted these handles.
            unsafe {
                let _ = CloseHandle(HANDLE(r as *mut _));
                let _ = CloseHandle(HANDLE(w as *mut _));
            }
        }
        #[cfg(unix)]
        if !self.adopted_by_cef.load(Ordering::Relaxed) {
            release_reserved_unix_fds();
        }
    }
}

fn spawn_writer(mut writer: File, outbound: Arc<(Mutex<OutboundQueues>, Condvar)>) {
    let _ = std::thread::Builder::new()
        .name("engine-ext-cdp-tx".into())
        .spawn(move || {
            let (lock, cv) = &*outbound;
            loop {
                let next = {
                    let mut q = lock.lock().unwrap();
                    loop {
                        if let Some(bytes) = q.control.pop_front() {
                            break Some(bytes);
                        }
                        if let Some(bytes) = q.rpc.pop_front() {
                            break Some(bytes);
                        }
                        if q.closed {
                            break None;
                        }
                        q = cv.wait(q).unwrap();
                    }
                };
                let Some(bytes) = next else { break };
                if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                    break;
                }
            }
        });
}

fn wake_main(wake_pending: &AtomicBool, events: &SyncSender<crate::Event>) {
    if !wake_pending.swap(true, Ordering::AcqRel) {
        let _ = events.try_send(crate::Event::Pump(0));
    }
}

fn spawn_reader(
    mut reader: File,
    inbound: SyncSender<PipeInbound>,
    wake_pending: Arc<AtomicBool>,
    events: SyncSender<crate::Event>,
) {
    let _ = std::thread::Builder::new()
        .name("engine-ext-cdp-rx".into())
        .spawn(move || {
            let mut chunk = vec![0u8; 64 * 1024];
            let mut acc = Vec::with_capacity(4096);
            let mut skipping = false;
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => {
                        let _ = inbound.try_send(PipeInbound::Closed);
                        wake_main(&wake_pending, &events);
                        break;
                    }
                    Ok(n) => {
                        for &b in &chunk[..n] {
                            if b == 0 {
                                if skipping {
                                    let _ = inbound.send(PipeInbound::FrameDropped);
                                    wake_main(&wake_pending, &events);
                                } else if !acc.is_empty()
                                    && let Ok(val) = serde_json::from_slice::<Value>(&acc)
                                {
                                    if inbound.send(PipeInbound::Message(val)).is_err() {
                                        return;
                                    }
                                    wake_main(&wake_pending, &events);
                                }
                                acc.clear();
                                skipping = false;
                            } else if !skipping {
                                if acc.len() >= MAX_CDP_MSG_BYTES {
                                    acc.clear();
                                    skipping = true;
                                } else {
                                    acc.push(b);
                                }
                            }
                        }
                    }
                }
            }
        });
}

#[cfg(windows)]
fn create_win_pipes() -> Option<(File, File, isize, isize)> {
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Pipes::CreatePipe;

    // 256 KiB buffer so pre-written messages and batched CDP replies never stall.
    const BUF_SIZE: u32 = 256 * 1024;
    let mut to_browser_read = HANDLE::default();
    let mut to_browser_write = HANDLE::default();
    let mut from_browser_read = HANDLE::default();
    let mut from_browser_write = HANDLE::default();

    // SAFETY: `lpPipeAttributes = None` sets `bInheritHandle = FALSE`, keeping every handle
    // private to this browser process (uninheritable by renderer/GPU/utility child processes).
    unsafe {
        if CreatePipe(&mut to_browser_read, &mut to_browser_write, None, BUF_SIZE).is_err() {
            return None;
        }
        if CreatePipe(&mut from_browser_read, &mut from_browser_write, None, BUF_SIZE).is_err() {
            let _ = CloseHandle(to_browser_read);
            let _ = CloseHandle(to_browser_write);
            return None;
        }
        let our_write = File::from_raw_handle(to_browser_write.0);
        let our_read = File::from_raw_handle(from_browser_read.0);
        Some((our_write, our_read, to_browser_read.0 as isize, from_browser_write.0 as isize))
    }
}

#[cfg(unix)]
fn create_unix_pipes() -> Option<(File, File)> {
    use std::os::unix::io::FromRawFd;

    // Fail closed if FDs 3 and 4 were not both vacant and reserved by us at startup.
    if RESERVED_FDS.load(Ordering::SeqCst) != 0b11 {
        return None;
    }

    let mut to_browser = [-1i32; 2];
    let mut from_browser = [-1i32; 2];
    // SAFETY: single-threaded POSIX pipe setup; every descriptor is moved >= 5 before placing
    // Chromium's ends on our reserved FDs 3 and 4 with FD_CLOEXEC so no child inherits any end.
    unsafe {
        if libc::pipe(to_browser.as_mut_ptr()) != 0 {
            return None;
        }
        if libc::pipe(from_browser.as_mut_ptr()) != 0 {
            libc::close(to_browser[0]);
            libc::close(to_browser[1]);
            return None;
        }
        for &fd in &[to_browser[0], to_browser[1], from_browser[0], from_browser[1]] {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let our_write_fd = libc::fcntl(to_browser[1], libc::F_DUPFD_CLOEXEC, 5);
        let our_read_fd = libc::fcntl(from_browser[0], libc::F_DUPFD_CLOEXEC, 5);
        let browser_read_tmp = libc::fcntl(to_browser[0], libc::F_DUPFD_CLOEXEC, 5);
        let browser_write_tmp = libc::fcntl(from_browser[1], libc::F_DUPFD_CLOEXEC, 5);
        libc::close(to_browser[0]);
        libc::close(to_browser[1]);
        libc::close(from_browser[0]);
        libc::close(from_browser[1]);
        if our_write_fd < 0 || our_read_fd < 0 || browser_read_tmp < 0 || browser_write_tmp < 0 {
            for fd in [our_write_fd, our_read_fd, browser_read_tmp, browser_write_tmp] {
                if fd >= 0 {
                    libc::close(fd);
                }
            }
            return None;
        }
        let dup3_ok = libc::dup2(browser_read_tmp, 3) >= 0;
        let dup4_ok = dup3_ok && libc::dup2(browser_write_tmp, 4) >= 0;
        libc::close(browser_read_tmp);
        libc::close(browser_write_tmp);
        if !dup4_ok {
            if dup3_ok {
                libc::close(3);
                RESERVED_FDS.fetch_and(!1, Ordering::SeqCst);
            }
            libc::close(our_write_fd);
            libc::close(our_read_fd);
            return None;
        }
        libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(4, libc::F_SETFD, libc::FD_CLOEXEC);
        Some((File::from_raw_fd(our_write_fd), File::from_raw_fd(our_read_fd)))
    }
}
