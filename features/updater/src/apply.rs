//! `opencode-desktop --apply-update …`: the app's own executable as the Windows update helper. It
//! puts a staged update in place once the app has exited, since Windows locks a running
//! program's files (see `windows.rs` for the install model). The app hard-links the staged version's
//! executable into the updates folder (a copy across volumes) and starts it:
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
const RENAME_TRIES: u32 = 128;
/// How many of the swap's tries come 10 ms apart (half a second), before the 250 ms ones (about
/// 20 s more, as before).
const QUICK_TRIES: u32 = 50;

/// Applies the update and exits, when this run is the update helper. `product` supplies the
/// build's Windows install identity for refreshing the Apps & features uninstall entry.
pub fn run_if_asked(product: impl FnOnce() -> win_install::Product) {
    if std::env::args().nth(1).as_deref() == Some("--apply-update") {
        std::process::exit(run(&product()));
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

fn run(product: &win_install::Product) -> i32 {
    let Some(a) = parse() else { return 2 };
    // Every line ends with the microseconds since this code began, so a slow update names its step
    // (a real one took 4.5 s between the old app's exit and the new process, and the log had no times).
    let began = rune::time::Instant::now();
    let log = |line: &str| {
        std::fs::create_dir_all(&a.updates).ok();
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(a.updates.join("apply.log")) {
            writeln!(f, "[pid {}] {line} [+{} us]", std::process::id(), began.elapsed().as_micros()).ok();
        }
    };
    log(&format!("installing {} from {} into {} (this process was made {} us ago: its exe linked and loaded the app's library first)", a.version, a.staged.display(), a.root.display(), process_age_us()));
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
    log("the staged build is identified");
    let mode = match publish(&a.root, &a.staged, &old, &log) {
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
    match win_install::refresh(product, &a.root) {
        Ok(changed) => changed.iter().for_each(|line| log(line)),
        Err(e) => log(&format!("couldn't update the uninstall entry: {e}")),
    }
    log("the uninstall entry is looked at");
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
    // Stripped again here: the app that handed these over may predate the strip.
    let began = rune::time::Instant::now();
    match Command::new(&exe).args(rune::tooling::without_relaunch_drops(a.rest.iter().map(std::ffi::OsString::from).collect())).current_dir(cwd).creation_flags(DETACHED_PROCESS).spawn() {
        Ok(child) => log(&format!("started {} (pid {}); making its process took {} us", exe.display(), child.id(), began.elapsed().as_micros())),
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

/// Swaps the folders, trying again while something still holds the old one after its app exited
/// (a child process of it that is still ending, a scanner's look at one of its files). The first
/// [`QUICK_TRIES`] come 10 ms apart, the rest 250 ms: each wait here is between the old window
/// closing and the new one, and a hold is usually gone in tens of milliseconds. The log names
/// the first refusal and every tenth, with Windows' error.
fn publish(root: &Path, staged: &Path, old: &Path, log: &dyn Fn(&str)) -> std::io::Result<super::windows::bundle::Mode> {
    let mut last = None;
    for attempt in 0..RENAME_TRIES {
        match super::windows::bundle::publish(root, staged, old) {
            Ok(mode) => {
                if attempt > 0 {
                    log(&format!("the folders swapped at try {}", attempt + 1));
                }
                return Ok(mode);
            }
            Err(e) => {
                if attempt % 10 == 0 {
                    log(&format!("the swap was refused at try {}: {e}", attempt + 1));
                }
                last = Some(e);
                if !root.exists() {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(if attempt < QUICK_TRIES { 10 } else { 250 }));
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

/// Microseconds since this process was made. A helper's exe links the staged library beside
/// itself and loads it before any of this code runs (`apps/desktop-thin`): that time is here.
#[allow(unsafe_code, clippy::disallowed_methods)]
fn process_age_us() -> u64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut made, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the pseudo-handle of this process and four out-values of the documented type.
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut made, &mut exited, &mut kernel, &mut user) } == 0 {
        return 0;
    }
    // A FILETIME counts 100 ns from 1601; the system clock is read as time since 1970.
    let made = ((u64::from(made.dwHighDateTime) << 32) | u64::from(made.dwLowDateTime)) / 10;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64) + 11_644_473_600_000_000;
    now.saturating_sub(made)
}

fn beside(root: &Path, what: &str) -> PathBuf {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    root.parent().unwrap_or(root).join(format!(".{name}.{what}"))
}
