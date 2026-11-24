//! The installing itself, shared by the pages and `--silent`.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use win_install::{EXE, Product};

/// The app: a zip of its install folder (see `build.rs`).
static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
const CREATE_SUSPENDED: u32 = 0x0000_0004;
const DETACHED_PROCESS: u32 = 0x0000_0008;
/// How long a running copy has to close on its own before setup ends it.
const CLOSE_GRACE: Duration = Duration::from_secs(10);

pub fn product() -> Product {
    Product::from_build(
        option_env!("OPENCODE_RUNE_WINDOW_TITLE"),
        option_env!("OPENCODE_RUNE_BUNDLE_ID"),
        option_env!("OPENCODE_RUNE_BUILD_CHANNEL"),
        option_env!("OPENCODE_RUNE_URL_SCHEME"),
        option_env!("OPENCODE_RUNE_VERSION").filter(|v| !v.is_empty()).unwrap_or(env!("CARGO_PKG_VERSION")),
    )
}

/// What the command line asked for.
#[derive(Clone, Default)]
pub struct Args {
    pub silent: bool,
    pub desktop_shortcut: bool,
    pub launch: bool,
    pub dir: Option<PathBuf>,
    pub log: Option<PathBuf>,
}

impl Args {
    fn parse() -> Args {
        let mut a = Args::default();
        let mut it = std::env::args().skip(1);
        while let Some(flag) = it.next() {
            match flag.as_str() {
                "--silent" | "/S" => a.silent = true,
                "--desktop-shortcut" => a.desktop_shortcut = true,
                "--launch" => a.launch = true,
                "--dir" => a.dir = it.next().map(PathBuf::from),
                "--log" => a.log = it.next().map(PathBuf::from),
                _ => {}
            }
        }
        a
    }

    pub fn log(&self, line: &str) {
        let Some(path) = &self.log else { return };
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            writeln!(f, "[setup pid {}] {line}", std::process::id()).ok();
        }
    }
}

pub fn run() -> i32 {
    let args = Args::parse();
    let p = product();
    let Some(root) = args.dir.clone().or_else(|| win_install::default_root(&p)) else {
        args.log("no folder to install in");
        return 1;
    };
    if !args.silent {
        return crate::pages::run(&p, &root, &args);
    }
    match install(&p, &root, args.desktop_shortcut, &|line| args.log(line)) {
        Ok(()) if args.launch => launch(&root, &args),
        Ok(()) => {
            // Nobody waits on a silent install's last moment: Windows' check of the new exe is paid here.
            first_start(&root, &args.log).join().ok();
            0
        }
        Err(e) => {
            args.log(&format!("couldn't install: {e}"));
            1
        }
    }
}

/// Whether a copy of the app runs from `root`, which installing closes.
pub fn running(root: &Path) -> bool {
    !win_install::running_under(root).is_empty()
}

/// Installs the payload at `root` and registers it.
pub fn install(p: &Product, root: &Path, desktop_shortcut: bool, log: &dyn Fn(&str)) -> Result<(), String> {
    if PAYLOAD.is_empty() {
        return Err("This setup was built without the app, so there's nothing to install.".into());
    }
    let started = std::time::Instant::now();
    log(&format!("installing {} {} into {}", p.name(), p.version(), root.display()));
    if !win_install::close_running(root, CLOSE_GRACE) {
        return Err(format!("{} is still running. Close it and try again.", p.name()));
    }
    let parent = root.parent().ok_or("The install folder has no parent folder.")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let staging = beside(root, &format!("setup-{}", std::process::id()));
    let result = unpack(&staging).and_then(|app| swap(root, &app, log));
    win_install::remove_dir(&staging);
    result?;
    // A version the updater staged for the install it replaced would go back in on the next restart.
    win_install::remove_dir(&beside(root, "staged"));
    let unpacked = started.elapsed();
    let (changed, flushed) = std::thread::scope(|scope| {
        let writing = scope.spawn(|| flush(root));
        (win_install::register(p, root, desktop_shortcut), writing.join().unwrap_or_default())
    });
    let changed = changed?;
    log(&format!("Windows wrote {} programs to the disk in {} µs", flushed.0, flushed.1.as_micros()));
    log(&format!("registered {} (Start menu{}, Apps & features, {}://, notification identity)", p.name(), if desktop_shortcut { ", desktop" } else { "" }, p.scheme()));
    changed.iter().for_each(|line| log(line));
    log(&format!("installed in {} µs ({} µs to put the files in place, {} µs to register)", started.elapsed().as_micros(), unpacked.as_micros(), (started.elapsed() - unpacked).as_micros()));
    Ok(())
}

/// Unpacks the payload into `staging` with Windows' own bsdtar (as the updater does); returns
/// the app's folder in it. bsdtar reads its arguments in the ANSI code page, where a user folder
/// named `测试` arrives as `??`: so the zip goes in on standard input and `staging` is the
/// working directory, and no path is an argument.
fn unpack(staging: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let zip = staging.join("payload.zip");
    std::fs::write(&zip, PAYLOAD).map_err(|e| format!("couldn't write the app: {e}"))?;
    let archive = std::fs::File::open(&zip).map_err(|e| format!("couldn't read the app: {e}"))?;
    let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let out = Command::new(system.join("System32").join("tar.exe"))
        .args(["-xf", "-"])
        .current_dir(staging)
        .stdin(archive)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("couldn't run tar: {e}"))?;
    std::fs::remove_file(&zip).ok();
    if !out.status.success() {
        return Err(format!("couldn't unpack the app: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    if staging.join(EXE).is_file() {
        return Ok(staging.to_path_buf());
    }
    let dirs: Vec<PathBuf> = std::fs::read_dir(staging).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    match dirs.as_slice() {
        [one] if one.join(EXE).is_file() => Ok(one.clone()),
        _ => Err(format!("The app in this setup has no {EXE}.")),
    }
}

/// Has Windows write every program and library of the new install to the disk now, while the
/// Installing page still shows; returns how many and how long it took.
///
/// Windows doesn't map a file as an image while bytes of it are only in its file cache: the
/// first run of a build `tar` had just unpacked waited 775 to 1,481 ms for that (the 146 MiB
/// library, n = 8, a folder Windows Defender doesn't scan), against 26 to 29 ms once the files
/// were flushed, and it is per file, not per MiB. Left alone it takes Windows 20 to 60 s.
/// The setup's own first start (`first_start`) begins when the last page shows, so a Launch
/// within a second of that page waited with it: 1,030 to 1,454 ms, against 15 to 80 ms after
/// a flush. The flush takes 1 to 2 s of the Installing page instead.
fn flush(root: &Path) -> (usize, Duration) {
    let started = std::time::Instant::now();
    let programs = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe") || x.eq_ignore_ascii_case("dll")))
        .filter(|path| OpenOptions::new().write(true).open(path).and_then(|file| file.sync_all()).is_ok())
        .count();
    (programs, started.elapsed())
}

/// Puts `app` at `root` with renames on one volume; a previous install goes aside first and comes
/// back if the new one can't take its place.
fn swap(root: &Path, app: &Path, log: &dyn Fn(&str)) -> Result<(), String> {
    if !root.exists() {
        return rename(app, root).map_err(|e| format!("couldn't put the app in {}: {e}", root.display()));
    }
    let old = beside(root, &format!("old-{}", std::process::id()));
    rename(root, &old).map_err(|e| format!("couldn't move the installed version aside: {e}"))?;
    if let Err(e) = rename(app, root) {
        rename(&old, root).ok();
        return Err(format!("couldn't put the app in {}: {e}", root.display()));
    }
    log("replaced the installed version");
    win_install::remove_dir(&old);
    Ok(())
}

fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = None;
    for _ in 0..40 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

/// `<parent>\.<folder>.<what>`, beside the install as the updater keeps its folders.
fn beside(root: &Path, what: &str) -> PathBuf {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    root.parent().unwrap_or(root).join(format!(".{name}.{what}"))
}

/// Makes a process from the installed exe and ends it before a line of it runs, on a thread of
/// its own (as the updater does for a staged update: `features/updater/src/windows.rs`).
///
/// Windows Defender checks a new executable the first time a process is made from it, inside
/// `CreateProcess`, before that process exists; opening, reading or mapping the file does not
/// get the answer early. For this exe, unpacked as here into a folder Defender scans, that is
/// 372,854 to 704,455 µs of the person's first start (the signed build of 23 Nov, n = 3; an
/// unsigned one 900,000 µs). Windows keeps the answer for the file, so paid here, while the last
/// page shows, their own first start (its Launch box, or the Start menu later) has its process
/// after 2,000 to 3,500 µs (the updater's measure of the same call).
pub fn first_start(root: &Path, log: &Option<PathBuf>) -> std::thread::JoinHandle<()> {
    let (exe, args) = (root.join(EXE), Args { log: log.clone(), ..Args::default() });
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let child = Command::new(&exe).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW).spawn();
        match child {
            Ok(mut child) => {
                child.kill().ok();
                child.wait().ok();
                args.log(&format!("Windows checked {} in {} µs", exe.display(), started.elapsed().as_micros()));
            }
            Err(e) => args.log(&format!("couldn't check {} ahead: {e}", exe.display())),
        }
        // The GPU's cache for this exe, made now in a process nobody waits for (`--warm-gpu`):
        // Direct3D keeps it per user and per exe path, so the first launch finds its pipelines
        // compiled and its cache databases made. Below normal, so the last page stays smooth.
        let warm = Command::new(&exe)
            .arg("--warm-gpu")
            .env_remove("__COMPAT_LAYER")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
            .spawn();
        if let Err(e) = warm {
            args.log(&format!("couldn't warm the GPU's cache for {}: {e}", exe.display()));
        }    })
}
/// Starts the installed app, in the user's home folder, as its Start menu shortcut does.
///
/// Without `__COMPAT_LAYER`: Windows starts a setup program with its Program Compatibility
/// Assistant's layer (`DetectorsAppHealth`) in the environment, and a process created with that
/// variable runs with ntdll's shim engine (`apphelp.dll` hooking every module before `main`):
/// the app's first start would take 6,320 µs longer than the Start menu's, and so would every
/// start the app itself made afterwards (an update's relaunch).
pub fn launch(root: &Path, args: &Args) -> i32 {
    let exe = root.join(EXE);
    let home = std::env::var_os("USERPROFILE").map_or_else(|| root.to_path_buf(), PathBuf::from);
    match Command::new(&exe).current_dir(home).env_remove("__COMPAT_LAYER").creation_flags(DETACHED_PROCESS).spawn() {
        Ok(child) => {
            args.log(&format!("started {} (pid {})", exe.display(), child.id()));
            0
        }
        Err(e) => {
            args.log(&format!("couldn't start {}: {e}", exe.display()));
            1
        }
    }
}
