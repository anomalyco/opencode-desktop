//! The user's shell folders, where Windows says they are (a Desktop OneDrive moved, a Start
//! menu redirected by policy).
// Known folders and shell notifications are COM-era Win32 calls; each block says what it relies on.
#![allow(unsafe_code)]

use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_ProgramFiles, FOLDERID_Programs,
    FOLDERID_RoamingAppData, FOLDERID_UserProgramFiles, FOLDERID_Windows, KF_FLAG_DEFAULT, KF_FLAG_DONT_VERIFY, KNOWN_FOLDER_FLAG,
    SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, SHGetKnownFolderPath,
};
use windows::core::GUID;

fn known_with(id: &GUID, flags: KNOWN_FOLDER_FLAG) -> Option<PathBuf> {
    // SAFETY: the shell allocates the path, which is copied out and then freed once.
    unsafe {
        let path = SHGetKnownFolderPath(id, flags, None).ok()?;
        let out = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.0 as *const _));
        out
    }
}

fn known(id: &GUID) -> Option<PathBuf> {
    known_with(id, KF_FLAG_DEFAULT)
}

fn known_unverified(id: &GUID) -> Option<PathBuf> {
    known_with(id, KF_FLAG_DONT_VERIFY)
}

fn env_dir(name: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(name)?);
    dir.is_absolute().then_some(dir)
}

/// `%LOCALAPPDATA%\Programs`, where per-user apps install.
///
/// Queries `FOLDERID_UserProgramFiles` with `KF_FLAG_DONT_VERIFY` so a fresh profile where
/// `%LOCALAPPDATA%\Programs` does not exist yet still resolves instead of failing with
/// `ERROR_FILE_NOT_FOUND` (and without creating the folder before the install starts). Falls back
/// to `FOLDERID_LocalAppData\Programs`, `%LOCALAPPDATA%\Programs`,
/// `FOLDERID_Profile\AppData\Local\Programs` and `%USERPROFILE%\AppData\Local\Programs` when
/// the shell cannot resolve the known folder.
pub fn user_programs() -> Option<PathBuf> {
    known_unverified(&FOLDERID_UserProgramFiles)
        .or_else(|| known_unverified(&FOLDERID_LocalAppData).map(|dir| dir.join("Programs")))
        .or_else(|| env_dir("LOCALAPPDATA").map(|dir| dir.join("Programs")))
        .or_else(|| known_unverified(&FOLDERID_Profile).map(|dir| dir.join(r"AppData\Local\Programs")))
        .or_else(|| env_dir("USERPROFILE").map(|dir| dir.join(r"AppData\Local\Programs")))
}

/// The Start menu's Programs folder.
pub fn programs() -> Option<PathBuf> {
    known(&FOLDERID_Programs)
}

pub fn desktop() -> Option<PathBuf> {
    known(&FOLDERID_Desktop)
}

/// The user's Documents folder (moved by OneDrive or policy, too).
pub fn documents() -> Option<PathBuf> {
    known(&FOLDERID_Documents)
}

/// The user's home folder, where a shortcut starts the app, so no process holds the install
/// folder open against an update's swap.
pub fn profile() -> Option<PathBuf> {
    known(&FOLDERID_Profile)
}

/// Where the taskbar keeps its pinned shortcuts.
pub fn taskbar_pins() -> Option<PathBuf> {
    Some(known(&FOLDERID_RoamingAppData)?.join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"))
}

/// `%LOCALAPPDATA%`, where Windows says it is (no environment variable steers it).
pub fn local_app_data() -> Option<PathBuf> {
    known(&FOLDERID_LocalAppData)
}

/// `%APPDATA%`, where Windows says it is.
pub fn roaming_app_data() -> Option<PathBuf> {
    known(&FOLDERID_RoamingAppData)
}

/// The user's folders an uninstall must never delete as a whole: the profile, the app data
/// roots, the program folders, Windows, the Desktop, Documents, Downloads and the Start menu.
pub fn protected_folders() -> Vec<PathBuf> {
    let user_programs = known_unverified(&FOLDERID_UserProgramFiles)
        .or_else(|| known_unverified(&FOLDERID_LocalAppData).map(|dir| dir.join("Programs")));
    [
        &FOLDERID_Profile,
        &FOLDERID_LocalAppData,
        &FOLDERID_RoamingAppData,
        &FOLDERID_ProgramFiles,
        &FOLDERID_Windows,
        &FOLDERID_Desktop,
        &FOLDERID_Documents,
        &FOLDERID_Downloads,
        &FOLDERID_Programs,
    ]
    .into_iter()
    .filter_map(known_unverified)
    .chain(user_programs)
    .collect()
}

/// Tells Explorer the URL schemes changed, so it stops using what it cached.
pub fn associations_changed() {
    // SAFETY: a notification with no items.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}
