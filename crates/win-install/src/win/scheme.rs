//! Whether a launch may write the app's URL scheme (`repair_scheme`), as pure functions of what
//! it read: the registry and the environment are read once by the caller, the rule lives here.

use std::ffi::OsStr;
use std::path::Path;

use crate::EXE;

/// What `repair_scheme` read before deciding to write.
#[derive(Clone, Copy, Debug)]
pub struct SchemeWrite<'a> {
    /// The running exe's file name.
    pub exe_name: Option<&'a OsStr>,
    /// `RUNE_DIRS_ROOT`: set (even empty) for a scratch run.
    pub dirs_root: Option<&'a OsStr>,
    /// The uninstall entry's `InstallLocation`.
    pub entry_root: Option<&'a Path>,
    /// The running exe's folder.
    pub root: &'a Path,
    /// Whether `<root>\opencode-desktop.exe` exists.
    pub exe_present: bool,
    /// The scheme's command as registered now (`shell\open\command`), when the key has `URL Protocol`.
    pub command: Option<&'a str>,
}

/// True only for the registered install, under its own name, outside a scratch run, whose
/// scheme is missing or no longer exactly what `write_scheme` writes. Any other copy (a zip, a
/// build folder, a PR build, a harness run, the exe renamed) writes nothing.
pub fn may_write_scheme(w: &SchemeWrite) -> bool {
    w.exe_name.is_some_and(|n| n.eq_ignore_ascii_case(EXE))
        && w.dirs_root.is_none()
        && w.entry_root.is_some_and(|r| same_path(r, w.root))
        && w.exe_present
        && w.command.is_none_or(|c| !command_is_current(c, &w.root.join(EXE)))
}

/// Whether a scheme's command is exactly what `write_scheme` writes for `exe`: `"<exe>" "%1"`.
/// The path is compared without regard to case: Windows paths are case-insensitive, so a
/// case-only difference starts the same exe and rewriting it would only flip-flop.
pub fn command_is_current(command: &str, exe: &Path) -> bool {
    let Some((program, args)) = command.strip_prefix('"').and_then(|rest| rest.split_once('"')) else { return false };
    same_path(Path::new(program), exe) && args == " \"%1\""
}

/// The same path as the setup or the shell writes it: case and a trailing `\` aside. Any other
/// spelling (an 8.3 name, a junction, a `\\?\` prefix, `/` separators, `..`) compares unequal,
/// which only ever means no write.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}
