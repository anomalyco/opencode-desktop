//! Windows: finding and starting the apps Open in lists, as Desktop's main process does
//! (`desktop/src/main/files/apps.ts` and `openPath`, `revealPath` in `files/index.ts`).
//!
//! An app is found by the command Desktop opens it with: `where` on the `PATH`, where a `.exe`
//! wins and a `.cmd` or `.bat` shim (VS Code's `code.cmd`) leads to the `.exe` it starts, then an
//! executable named like the command beside it; failing those, the folder its installer puts it
//! in. File Explorer shows a verified local directory or reveals a file with it selected
//! (`SHOpenFolderAndSelectItems`, never executing files); PowerShell (5 or 7) opens in its own
//! console in the folder, Windows Terminal in a new window there (using `-d .` with `.current_dir`
//! so folder names containing `;` work safely).

use std::ffi::OsString;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::Launcher;

/// A console process (`where`, `cmd`) starts without a window of its own.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Default)]
pub struct Win;

impl Launcher for Win {
    fn installed(&self, apps: &[&'static str]) -> Vec<&'static str> {
        apps.iter().copied().filter(|app| resolve(app, |k| std::env::var_os(k)).is_some()).collect()
    }

    fn open(&self, path: &str, app: Option<&str>) -> Result<(), String> {
        // Verify local fixed drive, non-subst-UNC, and zero reparse points BEFORE touching disk
        // or launching any external application.
        let verified = browser_policy::verify_local_path(path).map_err(|e| e.to_string())?;
        let clean = verified.path;
        let dir = if verified.is_dir {
            PathBuf::from(&clean)
        } else {
            Path::new(&clean).parent().unwrap_or_else(|| Path::new(&clean)).to_path_buf()
        };
        match app {
            None => {
                // Only open a verified local fixed-drive non-symlink, non-CLSID directory directly
                // in Explorer; files and CLSID-suffixed folders fall back to safe reveal (never
                // default-app execution).
                if !verified.is_dir || browser_policy::is_clsid_folder_name(&clean) {
                    return self.reveal(&clean);
                }
                if harness_log::record("openIn", &[("path", &clean)]) {
                    return Ok(());
                }
                if rune::is_headless() {
                    return Ok(());
                }
                start(Command::new("explorer.exe").arg(&clean))
            }
            Some(app) => {
                let exe = resolve(app, |k| std::env::var_os(k)).ok_or_else(|| format!("{app} isn't installed"))?;
                if harness_log::record("openIn", &[("path", &clean), ("app", app)]) {
                    return Ok(());
                }
                if is_powershell(&exe) {
                    if rune::is_headless() {
                        return Ok(());
                    }
                    return powershell(&exe, &dir);
                }
                if is_terminal(&exe) {
                    if rune::is_headless() {
                        return Ok(());
                    }
                    // Windows Terminal splits its command line on `;` even inside quoted arguments.
                    // Pass `-d .` and set the folder via `.current_dir(&dir)` so repository names
                    // containing `;` work safely without ever placing `dir` on `wt`'s command line.
                    return start(Command::new(exe).args(["-w", "new", "-d", "."]).current_dir(&dir));
                }
                if rune::is_headless() {
                    return Ok(());
                }
                // `clean` is guaranteed by `verify_local_path` to begin with `<drive>:\` (never `-`).
                start(Command::new(exe).arg(&clean))
            }
        }
    }

    fn reveal(&self, path: &str) -> Result<(), String> {
        let verified = browser_policy::verify_local_path(path).map_err(|e| e.to_string())?;
        if harness_log::record("reveal", &[("path", &verified.path)]) {
            return Ok(());
        }
        if rune::is_headless() {
            return Ok(());
        }
        harness_log::reveal(&verified.path).map(|_| ()).map_err(|e| e.to_string())
    }
}

fn is_terminal(exe: &Path) -> bool {
    exe.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("wt"))
}

/// Opens a user-confirmed allow-listed app-scheme URI via Windows Shell association
/// (`ShellExecuteExW` with `lpVerb = "open"` and `SEE_MASK_FLAG_NO_UI`, never splicing a shell
/// command line ourselves).
///
/// Windows' Shell association engine resolves both classic `HKCR\<scheme>\shell\open\command`
/// (`"%1"` substitution) and modern MSIX/UWP `DelegateExecute` handlers. Because classic `%1`
/// substitution injects the URI text inside `"%1"`, `browser_policy::is_strict_rfc3986_uri` is
/// re-verified here to ban `"`, `\`, spaces/whitespace, controls, `< > ` ^ { | }`, non-ASCII, and
/// malformed `%XX`, preventing quote break-out and argument injection.
#[allow(unsafe_code)]
pub(crate) fn open_app_uri(uri: &str) -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    if !browser_policy::is_strict_rfc3986_uri(uri) {
        return false;
    }
    let verb: Vec<u16> = OsStr::new("open").encode_wide().chain([0]).collect();
    let target: Vec<u16> = OsStr::new(uri).encode_wide().chain([0]).collect();
    // SAFETY: zero-initialized `SHELLEXECUTEINFOW` with exact `cbSize`, valid NUL-terminated
    // UTF-16 `verb` and `target` buffers live across the synchronous `ShellExecuteExW` call.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = target.as_ptr();
    info.nShow = SW_SHOWNORMAL;
    unsafe { ShellExecuteExW(&mut info) != 0 }
}

/// Starts `cmd` on its own: the app keeps running after the launch answers.
fn start(cmd: &mut Command) -> Result<(), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Windows PowerShell or PowerShell 7.
fn is_powershell(exe: &Path) -> bool {
    exe.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("powershell") || s.eq_ignore_ascii_case("pwsh"))
}

/// PowerShell takes its working directory rather than a path, and needs a console of its own:
/// `start` opens one, as Desktop's `openPath` does. `dir` goes only to `.current_dir`, never
/// onto `cmd.exe`'s command line.
fn powershell(exe: &Path, dir: &Path) -> Result<(), String> {
    let status = Command::new("cmd.exe")
        .args(["/d", "/c", "start", ""])
        .arg(exe)
        .arg("-NoExit")
        .current_dir(dir)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("start exited with {status}")) }
}

/// The executable that command `app` starts (Desktop's `resolveWindowsAppPath`), else where its
/// installer puts it. `var` reads the environment.
fn resolve(app: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let found = where_is(app);
    let ext = |p: &Path, e: &str| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(e));
    if let Some(exe) = found.iter().find(|p| ext(p, "exe")) {
        return Some(exe.clone());
    }
    for file in &found {
        let shims = if ext(file, "cmd") || ext(file, "bat") {
            vec![file.clone()]
        } else if file.extension().is_none() {
            vec![file.with_extension("cmd"), file.with_extension("bat")]
        } else {
            vec![]
        };
        if let Some(exe) = shims.iter().filter(|s| s.exists()).find_map(|s| from_shim(s)) {
            return Some(exe);
        }
    }
    if let Some(exe) = named_like(app, &found) {
        return Some(exe);
    }
    // Never fall back to spawning a `.cmd`/`.bat` shim directly (cmd.exe metacharacter exposure).
    installed_at(app, var).or_else(|| found.into_iter().find(|p| ext(p, "exe")))
}

/// What `where` finds for `app` on the `PATH`, in order: each folder of `PATH` with each
/// `PATHEXT` extension (and the name as given, when it has one), as `where.exe` searches, without
/// starting it (a process each: 7,600-26,500 µs on a Windows dev box, five at every launch). Each folder is
/// asked once for the names starting `app.` rather than once per extension (14 apps x the
/// `PATH`'s folders x 11 extensions opened 6,309 files, 184 ms off the UI thread).
fn where_is(app: &str) -> Vec<PathBuf> {
    let exts: Vec<String> = std::env::var_os("PATHEXT")
        .map(|v| v.to_string_lossy().split(';').filter(|e| !e.is_empty()).map(str::to_lowercase).collect())
        .unwrap_or_else(|| [".com", ".exe", ".bat", ".cmd"].map(String::from).to_vec());
    let Some(path) = std::env::var_os("PATH") else { return vec![] };
    let named = Path::new(app).extension().is_some();
    let lower = app.to_lowercase();
    std::env::split_paths(&path)
        .flat_map(|dir| {
            let names = files_like(&dir, app);
            let wanted = named.then(|| lower.clone()).into_iter().chain(exts.iter().map(|e| format!("{lower}{e}")));
            wanted.filter_map(|w| names.iter().find(|n| n.to_lowercase() == w).map(|n| dir.join(n))).collect::<Vec<_>>()
        })
        .collect()
}

/// The files (not folders) in `dir` named `app` or `app.*`: one directory query.
#[allow(unsafe_code)]
fn files_like(dir: &Path, app: &str) -> Vec<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
        WIN32_FIND_DATAW,
    };
    let pattern: Vec<u16> = dir.join(format!("{app}*")).as_os_str().encode_wide().chain([0]).collect();
    let mut out = Vec::new();
    // SAFETY: a NUL-terminated pattern, a WIN32_FIND_DATAW this frame owns, and the find handle
    // closed after the last name.
    unsafe {
        let mut data: WIN32_FIND_DATAW = std::mem::zeroed();
        let find = FindFirstFileExW(pattern.as_ptr(), FindExInfoBasic, (&raw mut data).cast(), FindExSearchNameMatch, std::ptr::null(), FIND_FIRST_EX_LARGE_FETCH);
        if find == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return out;
        }
        loop {
            if data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                let len = data.cFileName.iter().position(|&c| c == 0).unwrap_or(data.cFileName.len());
                out.push(String::from_utf16_lossy(&data.cFileName[..len]));
            }
            if FindNextFileW(find, &mut data) == 0 {
                break;
            }
        }
        FindClose(find);
    }
    out
}

/// The `.exe` a `.cmd` or `.bat` shim starts: a quoted path, relative to the shim's folder when it
/// starts with `%~dp0`.
fn from_shim(shim: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(shim).ok()?;
    let base = shim.parent()?;
    text.split('"').map(str::trim).filter(|t| t.to_ascii_lowercase().contains(".exe")).find_map(|token| {
        let lower = token.to_ascii_lowercase();
        if let Some(at) = lower.find("%~dp0") {
            let resolved = token[at + 5..]
                .split(['\\', '/'])
                .filter(|p| !p.is_empty() && *p != ".")
                .fold(base.to_path_buf(), |dir, part| {
                    if part == ".." { dir.parent().map(Path::to_path_buf).unwrap_or(dir) } else { dir.join(part) }
                });
            if resolved.exists() {
                return Some(resolved);
            }
        }
        let path = PathBuf::from(token);
        path.exists().then_some(path)
    })
}

/// An `.exe` named like `app` (letters and digits, either containing the other) in the folders of
/// what `where` found, or up to two above them.
fn named_like(app: &str, found: &[PathBuf]) -> Option<PathBuf> {
    let key = squash(app);
    if key.is_empty() {
        return None;
    }
    found.iter().flat_map(|f| f.ancestors().skip(1).take(3)).find_map(|dir| {
        std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
            p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"))
                && p.file_stem().is_some_and(|s| {
                    let name = squash(&s.to_string_lossy());
                    !name.is_empty() && (name.contains(&key) || key.contains(&name))
                })
        })
    })
}

fn squash(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// Where each app's installer puts it, for an install that didn't add it to the `PATH`.
fn installed_at(app: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let under = |root: &str, rel: &str| var(root).map(|r| PathBuf::from(r).join(rel));
    let candidates = match app {
        "code" => vec![under("LOCALAPPDATA", r"Programs\Microsoft VS Code\Code.exe"), under("ProgramFiles", r"Microsoft VS Code\Code.exe")],
        "cursor" => vec![under("LOCALAPPDATA", r"Programs\cursor\Cursor.exe"), under("ProgramFiles", r"Cursor\Cursor.exe")],
        "zed" => vec![under("LOCALAPPDATA", r"Programs\Zed\Zed.exe"), under("ProgramFiles", r"Zed\Zed.exe")],
        "powershell" => vec![under("SystemRoot", r"System32\WindowsPowerShell\v1.0\powershell.exe")],
        "pwsh" => vec![under("ProgramFiles", r"PowerShell\7\pwsh.exe"), under("LOCALAPPDATA", r"Microsoft\powershell\pwsh.exe")],
        // The Store's execution alias: a zero-byte reparse point, so it's found by name, not size.
        "wt" => vec![under("LOCALAPPDATA", r"Microsoft\WindowsApps\wt.exe")],
        "Sublime Text" => vec![under("ProgramFiles", r"Sublime Text\sublime_text.exe"), under("ProgramFiles", r"Sublime Text 3\sublime_text.exe")],
        _ => vec![],
    };
    candidates.into_iter().flatten().find(|p| p.exists())
}
