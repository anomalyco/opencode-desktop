//! OS-level process helpers for the local OpenCode background service: detached spawning
//! (matching Node's `spawn(..., { detached: true, windowsHide: true })` in
//! `packages/client/src/service-contender.ts:18-24`), liveness checks, and graceful/forced
//! termination (`packages/client/src/promise/service.ts:122-145`).

use std::collections::BTreeMap;
use std::io;
use std::process::{Child, Command, Stdio};

/// Spawns `program` with `args` and `env` detached from the desktop app's console/session,
/// stdin/stdout ignored and stderr piped so startup failures can be reported.
pub(super) fn spawn_detached(
    program: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
) -> io::Result<Child> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(env);
    // Keep the app's install folder free for Windows updates (`launch::leave_install_folder`).
    if let Some(home) = rune::dirs::home_from(|k| std::env::var_os(k))
        && home.is_dir()
    {
        cmd.current_dir(home);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`: no console window flashes and Ctrl+C /
        // desktop exit signals never propagate to the detached background service.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` is async-signal-safe and called in the freshly forked child before exec,
        // matching libuv's `UV_PROCESS_DETACHED`.
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn()
}

/// Whether OS process `pid` is still alive (matching `!stopped(pid)` via `process.kill(pid, 0)`
/// in `packages/client/src/promise/service.ts:129-135`).
#[allow(unsafe_code)]
pub(super) fn is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only tests whether `pid` exists and is signalable.
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
        // SAFETY: opens a synchronize handle for `pid`, polls with zero timeout, and closes it.
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                return false;
            }
            let done = WaitForSingleObject(handle, 0) == WAIT_OBJECT_0;
            CloseHandle(handle);
            !done
        }
    }
}

/// Sends `SIGTERM` on Unix, or terminates the process on Windows (matching Node's
/// `process.kill(pid, "SIGTERM")` in `packages/client/src/promise/service.ts:149`).
#[allow(unsafe_code)]
pub(super) fn terminate_graceful(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    // SAFETY: sends SIGTERM to the registered service PID.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(windows)]
    terminate_windows(pid);
}

/// Sends `SIGKILL` on Unix, or terminates the process on Windows (`promise/service.ts:153`).
#[allow(unsafe_code)]
pub(super) fn terminate_force(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    // SAFETY: sends SIGKILL to the registered service PID after SIGTERM timed out.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(windows)]
    terminate_windows(pid);
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn terminate_windows(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: opens the target PID with terminate rights, requests termination, and closes handle.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

/// Reads the current user's login shell from the OS user database on Unix (matching Node's
/// `os.userInfo().shell` in `packages/desktop/src/main/service/shell-env.ts:20-26`).
#[cfg(unix)]
#[allow(unsafe_code)]
pub(super) fn login_shell() -> Option<String> {
    use std::ffi::CStr;
    // SAFETY: `getpwuid_r` writes into a stack buffer with explicit length; we copy out `pw_shell`
    // before returning.
    unsafe {
        let uid = libc::getuid();
        let mut pwd = std::mem::zeroed::<libc::passwd>();
        let mut buf = vec![0u8; 4096];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut result);
        if rc != 0 || result.is_null() || pwd.pw_shell.is_null() {
            return None;
        }
        CStr::from_ptr(pwd.pw_shell).to_str().ok().map(str::to_owned)
    }
}
