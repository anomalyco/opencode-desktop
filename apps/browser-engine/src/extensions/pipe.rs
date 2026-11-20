//! Private, in-process anonymous CDP pipe to Chromium's `DevToolsPipeHandler`.
//!
//! Zero TCP ports, zero named pipes, zero filesystem sockets.
//! - Windows: two `CreatePipe` pairs with `bInheritHandle = FALSE` (`lpPipeAttributes = None`),
//!   passed to Chromium inside this same process via `--remote-debugging-pipe` and
//!   `--remote-debugging-io-pipes=<read>,<write>`. Because `bInheritHandle = FALSE`, no CEF
//!   renderer/GPU/utility child process can inherit or open the pipe handles.
//! - Unix (macOS / Linux): FDs 3 and 4 are reserved with `O_CLOEXEC` before `UnixListener::bind`
//!   so the app IPC socket never occupies FD 3/4; two `pipe()` pairs are dup'd above FD 4 first
//!   and then placed on FDs 3 and 4 with `FD_CLOEXEC` set on every descriptor so child processes
//!   never inherit them, paired with `--remote-debugging-pipe`.

#![allow(unsafe_code)]

use std::fs::File;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// Hard ceiling on a single NUL-delimited CDP frame read from the pipe (4 MiB).
/// A hostile extension calling `__ocExtCall` with a giant payload cannot OOM the engine.
const MAX_CDP_MSG_BYTES: usize = 4 * 1024 * 1024;

/// Unix only: reserves FDs 3 and 4 with `O_CLOEXEC` placeholders before `UnixListener::bind` so
/// the engine's IPC listener socket lands on FD >= 5 and is never clobbered when
/// `CdpPipe::create()` places Chromium's `--remote-debugging-pipe` ends on FDs 3 and 4.
/// Must be called single-threaded before any other FD opens in `main()`.
#[cfg(unix)]
pub fn reserve_unix_fds() {
    // SAFETY: opening `/dev/null` with `O_CLOEXEC` only fills vacant low FDs <= 4.
    unsafe {
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
            placeholders.push(fd);
        }
        // Keep FDs 3 and 4 occupied until `create_unix_pipes()` replaces them atomically via dup2.
        for fd in placeholders {
            if fd < 3 {
                libc::close(fd);
            }
        }
    }
}

#[cfg(not(unix))]
pub fn reserve_unix_fds() {}

/// Unix only: releases reserved FDs 3 and 4 if all `--extension` args failed pre-checks and
/// `--remote-debugging-pipe` will not be enabled.
#[cfg(unix)]
pub fn release_reserved_unix_fds() {
    // SAFETY: only called when `reserve_unix_fds` ran and `CdpPipe::create` was skipped.
    unsafe {
        libc::close(3);
        libc::close(4);
    }
}

#[cfg(not(unix))]
pub fn release_reserved_unix_fds() {}

pub struct CdpPipe {
    writer: Arc<Mutex<Option<File>>>,
    adopted_by_cef: AtomicBool,
    /// Windows only: raw HANDLE values (`browser_read,browser_write`) owned by Chromium once
    /// `cef::initialize` starts `DevToolsPipeHandler`.
    #[cfg(windows)]
    io_pipes_arg: String,
    #[cfg(windows)]
    browser_handles: Mutex<Option<(isize, isize)>>,
}

impl CdpPipe {
    /// Creates the private anonymous pipe pair, pre-writes `Target.setAutoAttach` into the kernel
    /// pipe buffer before `cef::initialize()` starts (eliminating any startup race with
    /// `--disable-extensions-except` service workers), and starts the background reader thread.
    pub fn create(events: Sender<crate::Event>) -> Option<Self> {
        #[cfg(windows)]
        let (our_write, our_read, browser_read_raw, browser_write_raw) = create_win_pipes()?;
        #[cfg(unix)]
        let (our_write, our_read) = create_unix_pipes()?;

        let pipe = Self {
            writer: Arc::new(Mutex::new(Some(our_write))),
            adopted_by_cef: AtomicBool::new(false),
            #[cfg(windows)]
            io_pipes_arg: format!("{browser_read_raw},{browser_write_raw}"),
            #[cfg(windows)]
            browser_handles: Mutex::new(Some((browser_read_raw, browser_write_raw))),
        };

        // Pre-write Target.setAutoAttach into the OS pipe buffer before CEF boots so Chromium's
        // DevToolsPipeHandler processes it before any extension service worker runs line 1.
        pipe.send(&json!({
            "id": 1,
            "method": "Target.setAutoAttach",
            "params": {
                "autoAttach": true,
                "waitForDebuggerOnStart": true,
                "flatten": true
            }
        }));

        spawn_reader(our_read, events);
        Some(pipe)
    }

    /// Marks the browser-owned handles/FDs as transferred to Chromium (`cef::initialize` started).
    pub fn mark_adopted_by_cef(&self) {
        self.adopted_by_cef.store(true, Ordering::Relaxed);
        #[cfg(windows)]
        {
            *self.browser_handles.lock().unwrap() = None;
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

    /// Sends one CDP JSON command (NUL-terminated) to Chromium's `DevToolsPipeHandler`.
    pub fn send(&self, msg: &Value) {
        let mut bytes = msg.to_string().into_bytes();
        bytes.push(0);
        if let Some(ref mut file) = *self.writer.lock().unwrap() {
            let _ = file.write_all(&bytes);
            let _ = file.flush();
        }
    }

    /// Pipelines multiple CDP JSON commands in a single write syscall.
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
        if let Some(ref mut file) = *self.writer.lock().unwrap() {
            let _ = file.write_all(&buf);
            let _ = file.flush();
        }
    }

    /// Closes our write end so Chromium's `DevToolsPipeHandlerReadThread` sees clean EOF before
    /// `cef::shutdown()`.
    pub fn close(&self) {
        let _ = self.writer.lock().unwrap().take();
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
            // SAFETY: FDs 3 and 4 belong to this pipe when `cef::initialize` never adopted them.
            unsafe {
                libc::close(3);
                libc::close(4);
            }
        }
    }
}

fn spawn_reader(mut reader: File, events: Sender<crate::Event>) {
    let _ = std::thread::Builder::new()
        .name("engine-ext-cdp-rx".into())
        .spawn(move || {
            let mut chunk = vec![0u8; 64 * 1024];
            let mut acc = Vec::with_capacity(4096);
            let mut skipping = false;
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for &b in &chunk[..n] {
                            if b == 0 {
                                if !skipping
                                    && !acc.is_empty()
                                    && let Ok(val) = serde_json::from_slice::<Value>(&acc)
                                    && events.send(crate::Event::ExtensionCdp(val)).is_err()
                                {
                                    return;
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

    let mut to_browser = [-1i32; 2];
    let mut from_browser = [-1i32; 2];
    // SAFETY: single-threaded POSIX pipe setup; every descriptor is moved >= 5 before placing
    // Chromium's ends on FDs 3 and 4 with FD_CLOEXEC so no child process inherits any end.
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
        let dup_ok = libc::dup2(browser_read_tmp, 3) >= 0 && libc::dup2(browser_write_tmp, 4) >= 0;
        libc::close(browser_read_tmp);
        libc::close(browser_write_tmp);
        if !dup_ok {
            libc::close(our_write_fd);
            libc::close(our_read_fd);
            return None;
        }
        libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(4, libc::F_SETFD, libc::FD_CLOEXEC);
        Some((File::from_raw_fd(our_write_fd), File::from_raw_fd(our_read_fd)))
    }
}
