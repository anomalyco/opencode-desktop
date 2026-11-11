//! Windows: finding and starting the apps Open in lists, as Desktop's main process does
//! (`desktop/src/main/files/apps.ts` and `openPath`, `revealPath` in `files/index.ts`).
//!
//! An app is found by the command Desktop opens it with: `where` on the `PATH`, where a `.exe`
//! wins and a `.cmd` or `.bat` shim (VS Code's `code.cmd`) leads to the `.exe` it starts, then an
//! executable named like the command beside it; failing those, the folder its installer puts it
//! in. File Explorer shows a folder or opens a file in its default app, and reveals a file with it
//! selected; PowerShell opens in its own console in the folder.

use std::ffi::OsString;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::Launcher;

/// A console process (`where`, `cmd`) starts without a window of its own.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Default)]
pub struct Win;

impl Launcher for Win {
    fn installed(&self, apps: &[&'static str]) -> Vec<&'static str> {
        apps.iter().copied().filter(|app| resolve(app, |k| std::env::var_os(k)).is_some()).collect()
    }

    fn open(&self, path: &str, app: Option<&str>) -> Result<(), String> {
        let path = native(path);
        match app {
            None => start(Command::new("explorer.exe").arg(&path)),
            Some(app) => {
                let exe = resolve(app, |k| std::env::var_os(k)).ok_or_else(|| format!("{app} isn't installed"))?;
                if is_powershell(&exe) {
                    return powershell(&exe, &path);
                }
                start(Command::new(exe).arg(&path))
            }
        }
    }

    fn reveal(&self, path: &str) -> Result<(), String> {
        let path = native(path);
        if !Path::new(&path).exists() {
            return Err(format!("{path} doesn't exist"));
        }
        // Explorer reads `/select,` and the quoted path itself, so they go as written.
        start(Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")))
    }
}

/// `path` with Windows separators, which Explorer needs.
fn native(path: &str) -> String {
    path.replace('/', "\\")
}

/// Starts `cmd` on its own: the app keeps running after the launch answers.
fn start(cmd: &mut Command) -> Result<(), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn is_powershell(exe: &Path) -> bool {
    exe.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("powershell"))
}

/// PowerShell takes its working directory rather than a path, and needs a console of its own:
/// `start` opens one, as Desktop's `openPath` does.
fn powershell(exe: &Path, path: &str) -> Result<(), String> {
    let at = Path::new(path);
    let dir = if at.is_dir() { at } else { at.parent().unwrap_or(at) };
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
        let shims = if ext(file, "cmd") || ext(file, "bat") { vec![file.clone()] } else if file.extension().is_none() { vec![file.with_extension("cmd"), file.with_extension("bat")] } else { vec![] };
        if let Some(exe) = shims.iter().filter(|s| s.exists()).find_map(|s| from_shim(s)) {
            return Some(exe);
        }
    }
    if let Some(exe) = named_like(app, &found) {
        return Some(exe);
    }
    installed_at(app, var).or_else(|| found.into_iter().next())
}

/// What `where` finds for `app` on the `PATH`, in order: each folder of `PATH` with each
/// `PATHEXT` extension (and the name as given, when it has one), as `where.exe` searches, without
/// starting it (a process each: 7,600-26,500 µs on blade, five at every launch).
fn where_is(app: &str) -> Vec<PathBuf> {
    let exts: Vec<String> = std::env::var_os("PATHEXT").map(|v| v.to_string_lossy().split(';').filter(|e| !e.is_empty()).map(str::to_owned).collect()).unwrap_or_else(|| [".COM", ".EXE", ".BAT", ".CMD"].map(String::from).to_vec());
    let Some(path) = std::env::var_os("PATH") else { return vec![] };
    let named = Path::new(app).extension().is_some();
    std::env::split_paths(&path)
        .flat_map(|dir| {
            let exact = named.then(|| dir.join(app));
            exact.into_iter().chain(exts.iter().map(move |e| dir.join(format!("{app}{e}"))))
        })
        .filter(|p| p.is_file())
        .collect()
}

/// The `.exe` a `.cmd` or `.bat` shim starts: a quoted path, relative to the shim's folder when it
/// starts with `%~dp0`.
fn from_shim(shim: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(shim).ok()?;
    let base = shim.parent()?;
    text.split('"').map(str::trim).filter(|t| t.to_ascii_lowercase().contains(".exe")).find_map(|token| {
        let lower = token.to_ascii_lowercase();
        if let Some(at) = lower.find("%~dp0") {
            let resolved = token[at + 5..].split(['\\', '/']).filter(|p| !p.is_empty() && *p != ".").fold(base.to_path_buf(), |dir, part| {
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
            p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) && p.file_stem().is_some_and(|s| {
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
        "Sublime Text" => vec![under("ProgramFiles", r"Sublime Text\sublime_text.exe"), under("ProgramFiles", r"Sublime Text 3\sublime_text.exe")],
        _ => vec![],
    };
    candidates.into_iter().flatten().find(|p| p.exists())
}
