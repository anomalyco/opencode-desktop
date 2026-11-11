//! The install's keys, all under `HKEY_CURRENT_USER`: its Apps & features entry, the app's URL
//! scheme, and its notification identity.

use std::path::{Path, PathBuf};

use windows_registry::CURRENT_USER;

use crate::{EXE, HOMEPAGE, OPENCODE_SCHEME, PUBLISHER, Product};

/// The toast icon the install folder ships (the release zip carries it beside the app).
pub const NOTIFICATION_ICON: &str = "notification-icon.png";

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

fn entry_key(p: &Product) -> String {
    format!(r"{UNINSTALL}\{}", p.id())
}

/// Apps & features' entry: the name, version, publisher, icon and size, and the app itself as
/// the uninstaller (`--uninstall`; `--quiet` for a silent one). The entry has no Modify or
/// Repair: the setup installs over an install.
pub fn write_entry(p: &Product, root: &Path, size_kb: u32) -> Result<(), String> {
    let key = CURRENT_USER.create(entry_key(p)).map_err(|e| format!("uninstall entry: {e}"))?;
    let exe = root.join(EXE);
    let exe = exe.to_string_lossy();
    let strings = [
        ("DisplayName", p.name().to_owned()),
        ("DisplayVersion", p.version().to_owned()),
        ("DisplayIcon", format!("{exe},0")),
        ("Publisher", PUBLISHER.to_owned()),
        ("InstallLocation", root.to_string_lossy().into_owned()),
        ("UninstallString", format!("\"{exe}\" --uninstall")),
        ("QuietUninstallString", format!("\"{exe}\" --uninstall --quiet")),
        ("URLInfoAbout", HOMEPAGE.to_owned()),
    ];
    for (name, value) in strings {
        key.set_string(name, value).map_err(|e| format!("uninstall entry {name}: {e}"))?;
    }
    for (name, value) in [("NoModify", 1), ("NoRepair", 1), ("EstimatedSize", size_kb)] {
        key.set_u32(name, value).map_err(|e| format!("uninstall entry {name}: {e}"))?;
    }
    Ok(())
}

/// The other builds of the app installed for the user (another channel, a pull request's build):
/// their uninstall entries' keys, which share the product's id up to its first channel suffix.
pub fn other_entries(p: &Product) -> Vec<String> {
    let family = p.family();
    let Ok(key) = CURRENT_USER.open(UNINSTALL) else { return Vec::new() };
    let Ok(names) = key.keys() else { return Vec::new() };
    names.filter(|name| name != p.id() && (name == family || name.starts_with(&format!("{family}.")))).collect()
}

pub fn entry_root(p: &Product) -> Option<PathBuf> {
    let location = CURRENT_USER.open(entry_key(p)).ok()?.get_string("InstallLocation").ok()?;
    (!location.is_empty()).then(|| PathBuf::from(location))
}

pub fn remove_entry(p: &Product) -> Option<String> {
    let key = entry_key(p);
    CURRENT_USER.open(&key).ok()?;
    CURRENT_USER.remove_tree(&key).ok().map(|()| format!("removed HKCU\\{key}"))
}

/// Points `scheme` at `exe`: a link on it starts the app with the link as its argument.
pub fn write_scheme(scheme: &str, name: &str, exe: &Path) -> Result<(), String> {
    let exe = exe.to_string_lossy();
    let key = CURRENT_USER.create(format!(r"Software\Classes\{scheme}")).map_err(|e| format!("{scheme}: {e}"))?;
    let err = |e: windows::core::Error| format!("{scheme}: {e}");
    key.set_string("", format!("URL:{name}")).map_err(err)?;
    key.set_string("URL Protocol", "").map_err(err)?;
    key.create("DefaultIcon").and_then(|k| k.set_string("", format!("\"{exe}\",0"))).map_err(err)?;
    key.create(r"shell\open\command").and_then(|k| k.set_string("", format!("\"{exe}\" \"%1\""))).map_err(err)?;
    Ok(())
}

/// Removes `scheme` when it still starts a program in `root`; another app's (OpenCode Desktop
/// took `opencode` back) stays.
pub fn remove_scheme_if_ours(scheme: &str, root: &Path) -> Option<String> {
    let path = format!(r"Software\Classes\{scheme}");
    let command = CURRENT_USER.open(format!(r"{path}\shell\open\command")).ok()?.get_string("").ok()?;
    let root = root.to_string_lossy().trim_end_matches('\\').to_lowercase();
    if !command.to_lowercase().trim_start_matches('"').starts_with(&format!("{root}\\")) {
        return None;
    }
    CURRENT_USER.remove_tree(&path).ok().map(|()| format!("removed HKCU\\{path}"))
}

/// The name and icon Windows shows on the install's toasts (`AppUserModelId\<id>`), which the app
/// takes without writing (`rune_notify::adopt`).
pub fn write_identity(p: &Product, root: &Path) -> Result<(), String> {
    let key = CURRENT_USER.create(identity_key(p)).map_err(|e| format!("notification identity: {e}"))?;
    key.set_string("DisplayName", p.name()).map_err(|e| format!("notification identity name: {e}"))?;
    let icon = root.join(NOTIFICATION_ICON);
    if icon.is_file() {
        key.set_string("IconUri", icon.to_string_lossy()).map_err(|e| format!("notification identity icon: {e}"))?;
    } else {
        key.remove_value("IconUri").ok();
    }
    Ok(())
}

/// A copy that isn't installed (a zip, a build folder) names its toasts only when nothing did
/// yet: it never repoints an install's identity. True when it wrote the key.
pub fn write_identity_if_absent(p: &Product) -> bool {
    let path = identity_key(p);
    if CURRENT_USER.open(&path).is_ok() {
        return false;
    }
    CURRENT_USER.create(&path).and_then(|k| k.set_string("DisplayName", p.name())).is_ok()
}

fn identity_key(p: &Product) -> String {
    format!(r"Software\Classes\AppUserModelId\{}", p.id())
}

/// Gives `opencode://` back to OpenCode Desktop when an earlier Rune build took it: the scheme
/// still starts an `opencode-desktop.exe` (any Rune build, in any folder) or a program that's
/// gone. It points at OpenCode Desktop's executable when OpenCode Desktop is installed (found from
/// its own Apps & features entry: OpenCode, then Beta, then Dev), as OpenCode Desktop registers it
/// (`app.setAsDefaultProtocolClient`), and is removed otherwise. Any other program's stays.
pub fn hand_back_opencode() -> Option<String> {
    let path = format!(r"Software\Classes\{OPENCODE_SCHEME}");
    let command = CURRENT_USER.open(format!(r"{path}\shell\open\command")).ok()?.get_string("").ok()?;
    let program = PathBuf::from(command.trim_start().trim_start_matches('"').split('"').next().unwrap_or_default().trim());
    let ours = program.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(EXE));
    if !ours && program.is_file() {
        return None;
    }
    CURRENT_USER.remove_tree(&path).ok()?;
    let Some(official) = opencode_desktop() else { return Some(format!("removed HKCU\\{path} (it started {})", program.display())) };
    let key = CURRENT_USER.create(&path).ok()?;
    let written = key.set_string("", format!("URL:{OPENCODE_SCHEME}")).is_ok()
        && key.set_string("URL Protocol", "").is_ok()
        && key.create(r"shell\open\command").and_then(|k| k.set_string("", format!("\"{}\" \"%1\"", official.display()))).is_ok();
    Some(if written { format!("handed HKCU\\{path} back to {} (it started {})", official.display(), program.display()) } else { format!("removed HKCU\\{path}; couldn't point it at {}", official.display()) })
}

/// OpenCode Desktop's executable, from its Apps & features entry: OpenCode, then Beta, then Dev.
fn opencode_desktop() -> Option<PathBuf> {
    let uninstall = CURRENT_USER.open(UNINSTALL).ok()?;
    let rank = |exe: &Path| match exe.file_name().map(|n| n.to_string_lossy().to_lowercase()).as_deref() {
        Some("opencode.exe") => Some(0),
        Some("opencode beta.exe") => Some(1),
        Some("opencode dev.exe") => Some(2),
        _ => None,
    };
    uninstall
        .keys()
        .ok()?
        .filter_map(|name| uninstall.open(&name).ok()?.get_string("DisplayIcon").ok())
        .map(|icon| PathBuf::from(icon.trim().trim_matches('"').rsplit_once(',').map_or(icon.trim(), |(path, _)| path).trim_matches('"')))
        .filter(|exe| exe.is_file())
        .filter_map(|exe| Some((rank(&exe)?, exe)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, exe)| exe)
}

/// The identity the install registered for its toasts, and the notification settings Windows
/// keeps for it.
pub fn remove_identity(p: &Product) -> Vec<String> {
    [
        format!(r"Software\Classes\AppUserModelId\{}", p.id()),
        format!(r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\{}", p.id()),
    ]
    .into_iter()
    .filter(|key| CURRENT_USER.open(key).is_ok() && CURRENT_USER.remove_tree(key).is_ok())
    .map(|key| format!("removed HKCU\\{key}"))
    .collect()
}
