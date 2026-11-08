//! The user's shell folders, where Windows says they are (a Desktop OneDrive moved, a Start
//! menu redirected by policy).
// Known folders and shell notifications are COM-era Win32 calls; each block says what it relies on.
#![allow(unsafe_code)]

use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Profile, FOLDERID_Programs, FOLDERID_RoamingAppData, FOLDERID_UserProgramFiles, KF_FLAG_DEFAULT,
    SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, SHGetKnownFolderPath,
};
use windows::core::GUID;

fn known(id: &GUID) -> Option<PathBuf> {
    // SAFETY: the shell allocates the path, which is copied out and then freed once.
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let out = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.0 as *const _));
        out
    }
}

/// `%LOCALAPPDATA%\Programs`, where per-user apps install.
pub fn user_programs() -> Option<PathBuf> {
    known(&FOLDERID_UserProgramFiles)
}

/// The Start menu's Programs folder.
pub fn programs() -> Option<PathBuf> {
    known(&FOLDERID_Programs)
}

pub fn desktop() -> Option<PathBuf> {
    known(&FOLDERID_Desktop)
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

/// Tells Explorer the URL schemes changed, so it stops using what it cached.
pub fn associations_changed() {
    // SAFETY: a notification with no items.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}
