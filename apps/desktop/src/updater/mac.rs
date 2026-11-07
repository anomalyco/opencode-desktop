//! macOS: team builds (`anomaly.opencode-rune`) update from Anomaly's release feed. A download is
//! verified (size, SHA-256, code signature, signing team, Gatekeeper) and unpacked into a private
//! folder under `~/Library/Caches/anomaly.opencode-rune/Updates`; Restart to update checks the
//! staged bundle again, swaps it in atomically (the old bundle comes back when the move fails),
//! and opens the new one. Local dev bundles (`anomaly.opencode-rune.dev`) and bare `cargo`
//! binaries never update.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::engine::{self, Engine, Facts};
use super::feed;
use super::platform::Platform;

/// Local dev-install bundle ID (`apps/desktop/scripts/bundle.sh`), which never updates.
#[cfg(test)]
pub const LOCAL_DEV_BUNDLE_ID: &str = "anomaly.opencode-rune.dev";

/// Team release bundle ID installed from the release feed.
pub const TEAM_BUNDLE_ID: &str = "anomaly.opencode-rune";

/// Only bundles the team release feed manages update (never local dev builds).
pub fn is_updatable_bundle_id(bundle_id: &str) -> bool {
    bundle_id == TEAM_BUNDLE_ID
}
const EXPECTED_SIGNING_TEAM: &str = "5NZ4Q7NXJ4";

static STAGE_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleInfo {
    pub path: PathBuf,
    pub id: String,
    pub version: String,
    pub team: Option<String>,
}

struct Staged {
    version: String,
    dir: PathBuf,
    app: PathBuf,
    sha256: String,
}

pub struct Mac {
    engine: Engine,
    bundle: BundleInfo,
    check_team: Option<String>,
    check_gatekeeper: bool,
    staged: Mutex<Option<Staged>>,
}

/// The updater for the running bundle, when it's a team build; `None` keeps it off.
pub fn platform(exe: &Path, build_channel: &str, updates: PathBuf) -> Option<Mac> {
    let app = bundle_root_from_exe(&exe.canonicalize().ok()?)?;
    let bundle = bundle_info(&app).ok()?;
    if !is_updatable_bundle_id(&bundle.id) {
        return None;
    }
    let service = feed::env("OPENCODE_RUNE_RELEASE_URL").unwrap_or_else(|| feed::DEFAULT_SERVICE_URL.to_owned());
    let local_loopback_test = feed::loopback(&service) && bundle.team.is_none() && feed::env("OPENCODE_RUNE_SKIP_GATEKEEPER").as_deref() == Some("1");
    let (check_team, check_gatekeeper) = if local_loopback_test { (None, false) } else { (bundle.team.clone().or_else(|| Some(EXPECTED_SIGNING_TEAM.to_owned())), true) };
    sweep_old_stages(&updates);
    let facts = Facts { version: bundle.version.clone(), channel: feed::feed_channel(build_channel), service, updates };
    Some(Mac::new(facts, bundle, check_team, check_gatekeeper))
}

impl Mac {
    pub fn new(facts: Facts, bundle: BundleInfo, check_team: Option<String>, check_gatekeeper: bool) -> Self {
        Mac { engine: Engine::new(facts), bundle, check_team, check_gatekeeper, staged: Mutex::new(None) }
    }

    /// Swaps the staged bundle in, without opening it (`install` opens it).
    pub fn apply(&self) -> Result<(), String> {
        let staged = self.staged.lock().ok().and_then(|mut s| s.take()).ok_or_else(|| "No staged update ready to install.".to_owned())?;
        if let Err(e) = verify_bundle(&staged.app, &self.bundle.id, self.check_team.as_deref(), self.check_gatekeeper) {
            std::fs::remove_dir_all(&staged.dir).ok();
            return Err(e);
        }
        swap(&staged.app, &self.bundle.path)?;
        engine::write_installed_sha(&self.engine.facts.updates, &staged.version, &staged.sha256);
        std::fs::remove_dir_all(&staged.dir).ok();
        Ok(())
    }
}

impl Platform for Mac {
    fn version(&self) -> &str {
        &self.bundle.version
    }

    fn check(&self) -> Result<Option<String>, String> {
        self.engine.check()
    }

    fn stage(&self, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
        let found = self.engine.found().ok_or_else(|| "No update to download".to_owned())?;
        if let Ok(staged) = self.staged.lock()
            && let Some(st) = staged.as_ref()
            && st.sha256 == found.target.sha256
            && st.app.exists()
        {
            return Ok(());
        }
        check_replaceable(&self.bundle.path)?;
        let root = &self.engine.facts.updates;
        std::fs::create_dir_all(root).map_err(|e| format!("could not create update dir: {e}"))?;
        let dir = root.join(format!("stage-{}-{}", std::process::id(), STAGE_SEQ.fetch_add(1, Ordering::Relaxed)));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not create stage dir: {e}"))?;
        let result = (|| {
            let zip = dir.join("update.zip");
            let found = self.engine.download(&zip, progress)?;
            let unpacked = dir.join("unpacked");
            std::fs::create_dir_all(&unpacked).map_err(|e| format!("could not create unpack dir: {e}"))?;
            let app = unpack_zip(&zip, &unpacked)?;
            std::fs::remove_file(&zip).ok();
            verify_bundle(&app, &self.bundle.id, self.check_team.as_deref(), self.check_gatekeeper)?;
            Ok(Staged { version: found.version, dir: dir.clone(), app, sha256: found.target.sha256 })
        })();
        match result {
            Ok(staged) => {
                if let Ok(mut slot) = self.staged.lock()
                    && let Some(prev) = slot.replace(staged)
                {
                    std::fs::remove_dir_all(prev.dir).ok();
                }
                Ok(())
            }
            Err(e) => {
                std::fs::remove_dir_all(&dir).ok();
                Err(e)
            }
        }
    }

    fn install(&self) -> Result<(), String> {
        self.apply()?;
        relaunch(&self.bundle.path)
    }

    fn auto_install(&self) -> bool {
        feed::env("OPENCODE_RUNE_AUTO_INSTALL_ON_READY").as_deref() == Some("1")
    }
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
    let out = Command::new("/usr/libexec/PlistBuddy").args(["-c", &format!("Print :{key}")]).arg(plist).output().map_err(|e| format!("failed to run PlistBuddy: {e}"))?;
    if !out.status.success() {
        return Err(format!("Info.plist has no {key}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn team_of(app: &Path) -> Option<String> {
    let out = Command::new("/usr/bin/codesign").args(["--display", "--verbose=4"]).arg(app).output().ok()?;
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
    Ok(BundleInfo { path: app.to_path_buf(), id, version, team: team_of(app) })
}

fn check_replaceable(app: &Path) -> Result<(), String> {
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

fn unpack_zip(zip: &Path, into: &Path) -> Result<PathBuf, String> {
    let out = Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(zip).arg(into).output().map_err(|e| format!("failed to run ditto: {e}"))?;
    if !out.status.success() {
        return Err(format!("could not unpack update archive: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let entries = std::fs::read_dir(into).map_err(|e| format!("could not read unpacked dir: {e}"))?;
    let mut apps: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir() && p.extension().and_then(|e| e.to_str()) == Some("app")).collect();
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
    let cs = Command::new("/usr/bin/codesign").args(["--verify", "--strict", "--deep"]).arg(app).output().map_err(|e| format!("failed to run codesign: {e}"))?;
    if !cs.status.success() {
        return Err(format!("invalid code signature on update: {}", String::from_utf8_lossy(&cs.stderr).trim()));
    }
    let Some(expected_team) = expected_team else { return Ok(()) };
    if info.team.as_deref() != Some(expected_team) {
        return Err(format!("update signing team {:?} did not match {expected_team}", info.team));
    }
    if check_gatekeeper {
        let gk = Command::new("/usr/sbin/spctl").args(["--assess", "--type", "execute"]).arg(app).output().map_err(|e| format!("failed to run spctl: {e}"))?;
        if !gk.status.success() {
            return Err(format!("Gatekeeper rejected update: {}", String::from_utf8_lossy(&gk.stderr).trim()));
        }
    }
    Ok(())
}

/// Atomically swaps `staged` into `dest`, restoring `dest` if the final move fails.
fn swap(staged: &Path, dest: &Path) -> Result<(), String> {
    let parent = dest.parent().ok_or_else(|| "app has no parent directory".to_owned())?;
    let name = dest.file_name().ok_or_else(|| "app has no file name".to_owned())?.to_string_lossy();
    let incoming = parent.join(format!(".{name}.update"));
    let outgoing = parent.join(format!(".{name}.previous"));
    for leftover in [&incoming, &outgoing] {
        if leftover.exists() {
            std::fs::remove_dir_all(leftover).ok();
        }
    }
    move_bundle(staged, &incoming).map_err(|e| format!("could not stage update in {}: {e}", parent.display()))?;
    if let Err(e) = move_bundle(dest, &outgoing) {
        std::fs::remove_dir_all(&incoming).ok();
        return Err(format!("could not move current app aside: {e}"));
    }
    if let Err(e) = move_bundle(&incoming, dest) {
        move_bundle(&outgoing, dest).ok();
        std::fs::remove_dir_all(&incoming).ok();
        return Err(format!("could not move update into place: {e}"));
    }
    std::fs::remove_dir_all(&outgoing).ok();
    Ok(())
}

fn move_bundle(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            let st = Command::new("/usr/bin/ditto").arg(from).arg(to).status()?;
            if !st.success() {
                std::fs::remove_dir_all(to).ok();
                return Err(io::Error::other("ditto failed across volumes"));
            }
            std::fs::remove_dir_all(from)
        }
        other => other,
    }
}

/// Opens the swapped bundle as a new instance; the host then quits this one.
fn relaunch(app: &Path) -> Result<(), String> {
    let status = Command::new("/usr/bin/open").arg("-n").arg(app).status().map_err(|e| format!("could not relaunch {}: {e}", app.display()))?;
    if !status.success() {
        return Err(format!("open -n exited with {status}"));
    }
    Ok(())
}

fn sweep_old_stages(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("stage-") {
            std::fs::remove_dir_all(entry.path()).ok();
        }
    }
}
