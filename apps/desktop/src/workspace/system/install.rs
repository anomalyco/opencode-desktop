//! `opencode-desktop --uninstall`: the installed app removes itself, as Apps & features asks
//! (`win_install` describes the install, `apps/installer` lays it out). The app is its own
//! uninstaller, as it is its own update helper, so an update never leaves a stale one behind.
//!
//! ```text
//! opencode-desktop.exe --uninstall [--quiet] [--delete-data] [--log <file>]
//! %TEMP%\opencode-rune-uninstall-<pid>.exe --uninstall-cleanup --wait <pid> --root <install>
//!     [--caches] [--delete-data] [--log <file>]
//! ```
//!
//! Without `--quiet` it asks first, with "Also delete my settings, tabs, drafts and caches" unticked:
//! like OpenCode Desktop's uninstaller it keeps them unless asked. It closes the copies running
//! from the install, removes the shortcuts, the uninstall entry, the URL schemes and the
//! notification identity, then copies itself to the temp folder and starts the copy, which
//! waits for this process to exit and deletes the install folder, the updater's folders beside
//! it, and with `--delete-data` the app's caches, logs, settings and browser profile,
//! then itself. Only the folder the uninstall entry names is ever deleted, never the browser
//! engine every app with a browser pane shares, and while another build of the app is installed
//! (another channel, a pull request's) its settings and caches, which every build shares, stay.
// The confirmation is a Win32 task dialog; each block says what it relies on.
#![allow(unsafe_code)]

use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
/// How long a running copy has to close on its own before it's ended.
const CLOSE_GRACE: Duration = Duration::from_secs(10);

/// Uninstalls and exits, when this run is the uninstaller.
pub fn run_if_asked() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("--uninstall") => uninstall(&Args::parse(&args[1..])),
        Some("--uninstall-cleanup") => cleanup(&Args::parse(&args[1..])),
        _ => return,
    };
    std::process::exit(code);
}

#[derive(Default)]
struct Args {
    quiet: bool,
    caches: bool,
    delete_data: bool,
    wait: u32,
    root: PathBuf,
    log: Option<PathBuf>,
}

impl Args {
    fn parse(args: &[String]) -> Args {
        let mut a = Args::default();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            match flag.as_str() {
                "--quiet" => a.quiet = true,
                "--caches" => a.caches = true,
                "--delete-data" => a.delete_data = true,
                "--wait" => a.wait = it.next().and_then(|p| p.parse().ok()).unwrap_or(0),
                "--root" => a.root = it.next().map(PathBuf::from).unwrap_or_default(),
                "--log" => a.log = it.next().map(PathBuf::from),
                _ => {}
            }
        }
        a
    }

    fn log(&self, line: &str) {
        eprintln!("{line}");
        let Some(path) = &self.log else { return };
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            writeln!(f, "[uninstall pid {}] {line}", std::process::id()).ok();
        }
    }
}

fn uninstall(a: &Args) -> i32 {
    let p = crate::launch::product();
    let Some(root) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) else { return 1 };
    let registered = win_install::installed_root(&p);
    if !registered.as_deref().is_some_and(|r| same(r, &root)) {
        a.log(&format!("{} isn't where {} is installed ({registered:?}); nothing removed", root.display(), p.name()));
        if !a.quiet {
            dialog::error(&p, &format!("This copy of {} wasn't installed by its setup, so there's nothing to uninstall here.", p.name()));
        }
        return 1;
    }
    // Every build of the app (another channel, a pull request's) keeps its settings and caches
    // in the same folders, so they stay while another is installed.
    let others = win_install::other_installs(&p);
    let delete_data = if a.quiet {
        a.delete_data && others.is_empty()
    } else {
        match dialog::confirm(&p, others.is_empty()) {
            Some(delete_data) => delete_data,
            None => return 2,
        }
    };
    if !others.is_empty() {
        a.log(&format!("keeps the settings and caches, which {} also uses", others.join(", ")));
    }
    a.log(&format!("uninstalling {} {} from {}{}", p.name(), p.version(), root.display(), if delete_data { ", with its data" } else { "" }));
    if !win_install::close_running(&root, CLOSE_GRACE) {
        a.log("a running copy didn't close; nothing removed");
        if !a.quiet {
            dialog::error(&p, &format!("Close {} and try again.", p.name()));
        }
        return 1;
    }
    let started = rune::time::Instant::now();
    for line in win_install::unregister(&p, &root) {
        a.log(&line);
    }
    a.log(&format!("unregistered in {} µs", started.elapsed().as_micros()));
    // The caches and logs are every copy's, an unpacked one's too, which no uninstall entry
    // names: they go only with the user's data, as OpenCode Desktop's uninstaller keeps its app
    // data unless asked (`deleteAppDataOnUninstall: false`).
    match start_cleanup(&root, others.is_empty() && delete_data, delete_data, a.log.as_deref()) {
        Ok(pid) => a.log(&format!("cleanup (pid {pid}) removes the files once this process exits")),
        Err(e) => {
            a.log(&format!("couldn't start the cleanup: {e}"));
            return 1;
        }
    }
    0
}

/// Copies this executable to the temp folder and starts it to delete the install, which a
/// running program can't do to its own folder.
fn start_cleanup(root: &Path, caches: bool, delete_data: bool, log: Option<&Path>) -> std::io::Result<u32> {
    let temp = std::env::temp_dir();
    let copy = temp.join(format!("opencode-rune-uninstall-{}.exe", std::process::id()));
    std::fs::copy(std::env::current_exe()?, &copy)?;
    let mut cmd = Command::new(&copy);
    cmd.arg("--uninstall-cleanup").arg("--wait").arg(std::process::id().to_string()).arg("--root").arg(root);
    if caches {
        cmd.arg("--caches");
    }
    if delete_data {
        cmd.arg("--delete-data");
    }
    if let Some(log) = log {
        cmd.arg("--log").arg(log);
    }
    Ok(cmd.current_dir(&temp).creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW).spawn()?.id())
}

fn cleanup(a: &Args) -> i32 {
    if a.root.as_os_str().is_empty() {
        return 2;
    }
    wait_for_exit(a.wait);
    let mut ok = true;
    let mut remove = |dir: &Path| {
        if !dir.exists() {
            return;
        }
        let gone = win_install::remove_dir(dir);
        a.log(&format!("{} {}", if gone { "removed" } else { "couldn't remove" }, dir.display()));
        ok &= gone;
    };
    remove(&a.root);
    for dir in updater_folders(&a.root) {
        remove(&dir);
    }
    let app = crate::dirs::app();
    // The browser engine's folder is shared by every app with a browser pane: only this
    // install's socket, and with its data its browser profile, are its own.
    let engine = crate::dirs::engine();
    let exe = a.root.join(win_install::EXE);
    if let Some(engine) = &engine {
        std::fs::remove_file(engine.cache().join(browser_pane::installer::socket_name_for(&exe, 0))).ok();
    }
    // Caches, logs and the updater's downloads: `%LOCALAPPDATA%\rune-opencode` (separate folders
    // under `RUNE_DIRS_ROOT`).
    if let Some(app) = app.filter(|_| a.caches) {
        remove(app.cache());
        remove(app.logs());
        remove(app.state());
    }
    if let Some(app) = app.filter(|_| a.delete_data) {
        remove(app.data());
        remove(app.config());
    }
    if let Some(engine) = engine.as_ref().filter(|_| a.delete_data) {
        remove(&browser_pane::profile::profile_dir_for(engine.state(), &exe));
    }
    a.log(if ok { "uninstalled" } else { "uninstalled; some files stay until they're free" });
    delete_self();
    i32::from(!ok)
}

/// The updater's staged and previous versions beside the install (`updater::windows`).
fn updater_folders(root: &Path) -> Vec<PathBuf> {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(parent) = root.parent() else { return Vec::new() };
    std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&format!(".{name}.")))
        .map(|e| e.path())
        .collect()
}

/// The temp copy deletes itself once it has exited.
fn delete_self() {
    let Ok(exe) = std::env::current_exe() else { return };
    let script = format!("ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"", exe.display());
    let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    Command::new(system.join("System32").join("cmd.exe"))
        .args(["/d", "/c"])
        .raw_arg(&script)
        .current_dir(std::env::temp_dir())
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .spawn()
        .ok();
}

fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    if pid == 0 {
        return;
    }
    // SAFETY: a wait-only handle, closed once; a process already gone answers null.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !handle.is_null() {
            WaitForSingleObject(handle, 30_000);
            CloseHandle(handle);
        }
    }
}

fn same(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

mod dialog {
    use windows_sys::Win32::Foundation::S_OK;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::{
        TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDCBF_CANCEL_BUTTON, TDCBF_CLOSE_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_POSITION_RELATIVE_TO_WINDOW,
        TD_ERROR_ICON, TaskDialogIndirect,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::IDCANCEL;
    use windows_sys::core::BOOL;

    use win_install::Product;

    const UNINSTALL: i32 = 100;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Asks before uninstalling: Some(whether to delete the user's data), or None to keep it all.
    /// `own_data`: no other build of the app shares the settings, so they may go too.
    pub fn confirm(p: &Product, own_data: bool) -> Option<bool> {
        let title = wide(&format!("Uninstall {}", p.name()));
        let instruction = wide(&format!("Uninstall {}?", p.name()));
        let content = wide(if own_data {
            "This removes the app and its Start menu and desktop shortcuts. Your OpenCode sessions stay on the server."
        } else {
            "This removes the app and its Start menu and desktop shortcuts. Its settings stay for the other OpenCode (Rune) installed, and your OpenCode sessions stay on the server."
        });
        let verification = wide("Also delete my settings, tabs, drafts and caches");
        let label = wide("Uninstall");
        let buttons = [TASKDIALOG_BUTTON { nButtonID: UNINSTALL, pszButtonText: label.as_ptr() }];
        // SAFETY: the module handle of this executable, whose icon resource 1 the dialog shows.
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        // SAFETY: a zeroed config is the documented default for every field set below.
        let mut config: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
        config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        config.hInstance = instance;
        config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW;
        config.dwCommonButtons = TDCBF_CANCEL_BUTTON;
        config.pszWindowTitle = title.as_ptr();
        // MAKEINTRESOURCE(1): the exe's icon.
        config.Anonymous1.pszMainIcon = std::ptr::without_provenance(1);
        config.pszMainInstruction = instruction.as_ptr();
        config.pszContent = content.as_ptr();
        config.cButtons = buttons.len() as u32;
        config.pButtons = buttons.as_ptr();
        config.nDefaultButton = UNINSTALL;
        config.pszVerificationText = if own_data { verification.as_ptr() } else { std::ptr::null() };
        let (mut button, mut checked): (i32, BOOL) = (IDCANCEL, 0);
        // SAFETY: every string and the button array outlive the modal call, which writes the two out-values.
        let hr = unsafe { TaskDialogIndirect(&config, &mut button, std::ptr::null_mut(), &mut checked) };
        (hr == S_OK && button == UNINSTALL).then_some(checked != 0)
    }

    pub fn error(p: &Product, message: &str) {
        let title = wide(&format!("Uninstall {}", p.name()));
        let instruction = wide(&format!("Couldn't uninstall {}", p.name()));
        let content = wide(message);
        // SAFETY: as in `confirm`.
        let mut config: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
        config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION;
        config.dwCommonButtons = TDCBF_CLOSE_BUTTON;
        config.pszWindowTitle = title.as_ptr();
        config.Anonymous1.pszMainIcon = TD_ERROR_ICON;
        config.pszMainInstruction = instruction.as_ptr();
        config.pszContent = content.as_ptr();
        // SAFETY: the strings outlive the modal call.
        unsafe { TaskDialogIndirect(&config, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
    }
}
