//! `opencode-desktop --apply-update …`: the app's own executable as the Windows update helper. It
//! puts a staged update in place once the app has exited, since Windows locks a running
//! program's files (see `windows.rs` for the install model). The app copies the staged version's
//! executable into the updates folder and starts it:
//!
//! ```text
//! opencode-desktop.exe --apply-update --wait <pid> --root <install> --staged <next>
//!     --exe opencode-desktop.exe --version <v> --sha256 <hex> --updates <updates folder>
//!     --cwd <app's folder> [--relaunch] -- <app arguments>
//! ```
//!
//! It waits for `<pid>` to exit, renames the install folder aside and the staged folder into
//! its place (both on one volume, so each rename is atomic), records the installed release, and
//! with `--relaunch` starts the new version with the app's arguments. When a rename fails it puts
//! the previous folder back and starts that instead. It runs in the updates folder, never in the
//! install, whose folder a working directory would hold open, opens no window, and logs to
//! `<updates>\apply.log`.
// Waiting on a process is Win32; each block says what it relies on.
#![allow(unsafe_code)]

use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

const DETACHED_PROCESS: u32 = 0x0000_0008;
/// How long the app has to exit after asking for the update.
const EXIT_WAIT_MS: u32 = 120_000;
/// How long a rename keeps trying while a scanner or a straggling child holds a file.
const RENAME_TRIES: u32 = 80;

/// Applies the update and exits, when this run is the update helper.
pub fn run_if_asked() {
    if std::env::args().nth(1).as_deref() == Some("--apply-update") {
        std::process::exit(run());
    }
}

#[derive(Default)]
struct Args {
    wait: u32,
    root: PathBuf,
    staged: PathBuf,
    exe: String,
    version: String,
    sha256: String,
    updates: PathBuf,
    /// The app's working folder, which the new version starts in.
    cwd: PathBuf,
    relaunch: bool,
    rest: Vec<String>,
}

fn parse() -> Option<Args> {
    let mut a = Args::default();
    #[allow(clippy::disallowed_methods)]
    let mut it = std::env::args().skip(2);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--wait" => a.wait = it.next()?.parse().ok()?,
            "--root" => a.root = it.next()?.into(),
            "--staged" => a.staged = it.next()?.into(),
            "--exe" => a.exe = it.next()?,
            "--version" => a.version = it.next()?,
            "--sha256" => a.sha256 = it.next()?,
            "--updates" => a.updates = it.next()?.into(),
            "--cwd" => a.cwd = it.next()?.into(),
            "--relaunch" => a.relaunch = true,
            "--" => {
                a.rest = it.collect();
                break;
            }
            _ => return None,
        }
    }
    (!a.root.as_os_str().is_empty() && !a.staged.as_os_str().is_empty() && !a.exe.is_empty()).then_some(a)
}

fn run() -> i32 {
    let Some(a) = parse() else { return 2 };
    let log = |line: &str| {
        std::fs::create_dir_all(&a.updates).ok();
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(a.updates.join("apply.log")) {
            writeln!(f, "[pid {}] {line}", std::process::id()).ok();
        }
    };
    log(&format!("installing {} from {} into {}", a.version, a.staged.display(), a.root.display()));
    if a.wait != 0 && !wait_for_exit(a.wait) {
        log(&format!("process {} didn't exit; nothing changed", a.wait));
        super::windows::receipt::record_apply_failure(&a.updates, &a.version, "the previous process did not exit");
        return 1;
    }
    log(&format!("process {} exited", a.wait));
    let old = beside(&a.root, &format!("old-{}", std::process::id()));
    let expected = match super::windows::bundle::identify(&a.staged) {
        Ok(expected) => expected,
        Err(e) => {
            log(&format!("staged bundle is unavailable ({e}); nothing changed"));
            super::windows::receipt::record_apply_failure(&a.updates, &a.version, &e.to_string());
            start(&a, &log);
            return 1;
        }
    };
    let mode = match publish(&a.root, &a.staged, &old) {
        Ok(mode) => mode,
        Err(e) => {
            log(&format!("couldn't publish {}: {e}; previous install and stage unchanged", a.version));
            super::windows::receipt::record_apply_failure(&a.updates, &a.version, &e.to_string());
            start(&a, &log);
            return 1;
        }
    };
    if !super::windows::bundle::matches_id(&a.root, &expected) {
        log("could not confirm the staged bundle at the install path");
        super::windows::receipt::record_apply_failure(&a.updates, &a.version, "staged bundle was not confirmed at the install path");
        return 1;
    }
    super::windows::receipt::clear_apply_failure(&a.updates);
    log(&format!("{} is in place ({})", a.version, mode.label()));
    // This is the new version's executable: its version goes in Apps & features.
    match win_install::refresh(&crate::launch::product(), &a.root) {
        Ok(changed) => changed.iter().for_each(|line| log(line)),
        Err(e) => log(&format!("couldn't update the uninstall entry: {e}")),
    }
    start(&a, &log);
    match super::windows::bundle::retire(&old) {
        Ok(m) => log(&format!("retired previous bundle ({})", m.label())),
        Err(e) => log(&format!("previous bundle kept intact for a later launch: {e}")),
    }
    0
}

fn start(a: &Args, log: &dyn Fn(&str)) {
    if !a.relaunch {
        return;
    }
    let exe = a.root.join(&a.exe);
    let cwd = if a.cwd.is_dir() { &a.cwd } else { &a.root };
    match Command::new(&exe).args(&a.rest).current_dir(cwd).creation_flags(DETACHED_PROCESS).spawn() {
        Ok(child) => log(&format!("started {} (pid {})", exe.display(), child.id())),
        Err(e) => log(&format!("couldn't start {}: {e}", exe.display())),
    }
}

fn wait_for_exit(pid: u32) -> bool {
    // SAFETY: a plain handle request; a process that's already gone answers null.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return std::io::Error::last_os_error().raw_os_error() == Some(87); // PID no longer exists
    }
    // SAFETY: `handle` is the open process handle, closed once.
    unsafe {
        let done = WaitForSingleObject(handle, EXIT_WAIT_MS) == WAIT_OBJECT_0;
        CloseHandle(handle);
        done
    }
}

fn publish(root: &Path, staged: &Path, old: &Path) -> std::io::Result<super::windows::bundle::Mode> {
    let mut last = None;
    for _ in 0..RENAME_TRIES {
        match super::windows::bundle::publish(root, staged, old) {
            Ok(mode) => return Ok(mode),
            Err(e) => {
                last = Some(e);
                if !root.exists() {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

fn beside(root: &Path, what: &str) -> PathBuf {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    root.parent().unwrap_or(root).join(format!(".{name}.{what}"))
}
