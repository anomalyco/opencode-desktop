//! macOS bundle inspection, code-signature + Gatekeeper verification, atomic swap, and relaunch.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Local dev-install bundle ID (`apps/desktop/scripts/bundle.sh`).
/// The release-service auto-updater never touches this bundle.
#[cfg(test)]
pub const LOCAL_DEV_BUNDLE_ID: &str = "anomaly.opencode-rune.dev";

/// Team release bundle ID installed from `https://anoma.ly/release`.
pub const TEAM_BUNDLE_ID: &str = "anomaly.opencode-rune";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleInfo {
    pub path: PathBuf,
    pub id: String,
    pub version: String,
    pub team: Option<String>,
}

/// Returns true only for bundles managed by the team release feed (never local dev builds).
pub fn is_updatable_bundle_id(bundle_id: &str) -> bool {
    bundle_id == TEAM_BUNDLE_ID
}

/// The `.app` bundle the current process is executing inside, or `None` for a bare cargo binary.
#[allow(clippy::disallowed_methods)]
pub fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    bundle_root_from_exe(&exe)
}

pub fn bundle_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    if macos.file_name().and_then(|n| n.to_str()) != Some("MacOS") {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name().and_then(|n| n.to_str()) != Some("Contents") {
        return None;
    }
    let app = contents.parent()?;
    (app.extension().and_then(|e| e.to_str()) == Some("app")).then(|| app.to_path_buf())
}

fn plist_field(plist: &Path, key: &str) -> Result<String, String> {
    let out = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", &format!("Print :{key}")])
        .arg(plist)
        .output()
        .map_err(|e| format!("failed to run PlistBuddy: {e}"))?;
    if !out.status.success() {
        return Err(format!("Info.plist has no {key}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn team_of(app: &Path) -> Option<String> {
    let out = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=4"])
        .arg(app)
        .output()
        .ok()?;
    let combined = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    combined.lines().find_map(|line| {
        let team = line.strip_prefix("TeamIdentifier=")?.trim();
        (team != "not set" && !team.is_empty()).then(|| team.to_owned())
    })
}

pub fn bundle_info(app: &Path) -> Result<BundleInfo, String> {
    let plist = app.join("Contents/Info.plist");
    let id = plist_field(&plist, "CFBundleIdentifier")?;
    let version = plist_field(&plist, "CFBundleShortVersionString")?;
    let team = team_of(app);
    Ok(BundleInfo { path: app.to_path_buf(), id, version, team })
}

pub fn check_replaceable(app: &Path) -> Result<(), String> {
    if app.components().any(|c| c.as_os_str() == "AppTranslocation") {
        return Err("Move OpenCode (Rune) to Applications so it can update itself.".into());
    }
    let parent = app.parent().ok_or_else(|| "App bundle has no parent folder.".to_owned())?;
    let probe = parent.join(format!(".opencode-rune-update-probe-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map(drop)
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|_| format!("{} is not writable.", parent.display()))
}

pub fn unpack_zip(zip: &Path, into: &Path) -> Result<PathBuf, String> {
    let out = Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(zip)
        .arg(into)
        .output()
        .map_err(|e| format!("failed to run ditto: {e}"))?;
    if !out.status.success() {
        return Err(format!("could not unpack update archive: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let mut apps = Vec::new();
    let entries = std::fs::read_dir(into).map_err(|e| format!("could not read unpacked dir: {e}"))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && p.extension().and_then(|e| e.to_str()) == Some("app") {
            apps.push(p);
        }
    }
    if apps.len() != 1 {
        return Err(format!("expected 1 .app in update archive, found {}", apps.len()));
    }
    Ok(apps.remove(0))
}

pub fn verify_bundle(app: &Path, expected_id: &str, expected_team: Option<&str>, check_gatekeeper: bool) -> Result<(), String> {
    let info = bundle_info(app)?;
    if info.id != expected_id {
        return Err(format!("update bundle ID {} did not match {expected_id}", info.id));
    }
    let cs = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--deep"])
        .arg(app)
        .output()
        .map_err(|e| format!("failed to run codesign: {e}"))?;
    if !cs.status.success() {
        return Err(format!("invalid code signature on update: {}", String::from_utf8_lossy(&cs.stderr).trim()));
    }
    if let Some(expected_team) = expected_team {
        if info.team.as_deref() != Some(expected_team) {
            return Err(format!(
                "update signing team {:?} did not match {expected_team}",
                info.team
            ));
        }
        if check_gatekeeper {
            let gk = Command::new("/usr/sbin/spctl")
                .args(["--assess", "--type", "execute"])
                .arg(app)
                .output()
                .map_err(|e| format!("failed to run spctl: {e}"))?;
            if !gk.status.success() {
                return Err(format!(
                    "Gatekeeper rejected update: {}",
                    String::from_utf8_lossy(&gk.stderr).trim()
                ));
            }
        }
    }
    Ok(())
}

/// Atomically swaps `staged` into `dest`, restoring `dest` if the final move fails.
pub fn swap(staged: &Path, dest: &Path) -> Result<(), String> {
    let parent = dest.parent().ok_or_else(|| "app has no parent directory".to_owned())?;
    let name = dest.file_name().ok_or_else(|| "app has no file name".to_owned())?.to_string_lossy();
    let incoming = parent.join(format!(".{name}.update"));
    let outgoing = parent.join(format!(".{name}.previous"));
    for leftover in [&incoming, &outgoing] {
        if leftover.exists() {
            let _ = std::fs::remove_dir_all(leftover);
        }
    }
    move_bundle(staged, &incoming).map_err(|e| format!("could not stage update in {}: {e}", parent.display()))?;
    if let Err(e) = move_bundle(dest, &outgoing) {
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(format!("could not move current app aside: {e}"));
    }
    if let Err(e) = move_bundle(&incoming, dest) {
        let _ = move_bundle(&outgoing, dest);
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(format!("could not move update into place: {e}"));
    }
    let _ = std::fs::remove_dir_all(&outgoing);
    Ok(())
}

fn move_bundle(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let st = Command::new("/usr/bin/ditto").arg(from).arg(to).status()?;
            if !st.success() {
                let _ = std::fs::remove_dir_all(to);
                return Err(io::Error::other("ditto failed across volumes"));
            }
            std::fs::remove_dir_all(from)
        }
        other => other,
    }
}

pub fn relaunch_and_exit(app: &Path) -> Result<(), String> {
    let status = Command::new("/usr/bin/open")
        .arg("-n")
        .arg(app)
        .status()
        .map_err(|e| format!("could not relaunch {}: {e}", app.display()))?;
    if !status.success() {
        return Err(format!("open -n exited with {status}"));
    }
    std::process::exit(0);
}
