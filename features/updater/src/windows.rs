//! Windows: a per-user install that replaces itself folder by folder.
//!
//! The install model, which the Windows installer lays out:
//!
//! ```text
//! %LOCALAPPDATA%\Programs\OpenCode Rune\            the install root: Start menu shortcut,
//!   opencode-desktop.exe                            taskbar pin, toast activator and
//!                                                   uninstall entry all point in here
//! %LOCALAPPDATA%\Programs\.OpenCode Rune.staged\    the next version, verified, beside it
//! %LOCALAPPDATA%\Programs\.OpenCode Rune.old-<pid>\ the previous one while the swap runs
//! %LOCALAPPDATA%\rune-opencode\Cache\Updates\       downloads, the helper's copy and its log
//! %LOCALAPPDATA%\rune-opencode\Cache\Updates\ready  the version staged, so a restart knows it
//! ```
//!
//! Why: a per-user folder needs no administrator to replace, as Hex's and VS Code's user
//! installs; Windows locks a running program's files, so the swap waits for the app to exit;
//! renaming a folder is atomic within a volume, and staging beside the root keeps it on the same
//! volume; the root's path never changes, so nothing that points into it needs updating; and
//! the previous folder stays until the new one is in place, so a failed swap puts it back.
//!
//! A check reads the feed's `windows-x64` (or `windows-arm64`) target: a zip of the install
//! folder. The download's size and SHA-256 must match the manifest's, every program and library
//! in it must carry a trusted Authenticode signature from [`EXPECTED_PUBLISHER`], and it unpacks
//! beside the root. Clicking Update / Install and restart copies the staged version's own
//! executable into the updates folder and starts it as the helper (`--apply-update`, see
//! `apply.rs`), which waits for this process to exit, swaps the folders and starts the new
//! version with the same arguments. Windows never installs on plain quit (`platform.ts:30`).

pub(super) mod bundle;
pub(super) mod receipt;

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::engine::{Engine, Facts};
use super::feed;
use super::platform::Platform;

pub const EXE: &str = "opencode-desktop.exe";

/// Who signs every program and library of a release: the signing certificate's subject name as
/// `CertGetNameStringW` reads it (`CERT_NAME_SIMPLE_DISPLAY_TYPE`: the CN), the identity Azure
/// Artifact Signing gives OpenCode's and Hex's Windows builds too. The release workflow's
/// `verify-windows` job reads it from here.
pub const EXPECTED_PUBLISHER: &str = "Anomaly Innovations, Inc https://anoma.ly/";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

static SEQ: AtomicU64 = AtomicU64::new(1);

struct Staged {
    version: String,
    sha256: String,
    dir: PathBuf,
}

/// Whose builds an update may carry.
enum Trust {
    /// Programs and libraries Windows trusts, signed by [`EXPECTED_PUBLISHER`]: the release feed,
    /// and a debug build's feed anywhere but this machine.
    Publisher,
    /// Anything, signed or not: a debug build's loopback feed, since local builds aren't signed.
    Anything,
}

pub struct Windows {
    engine: Engine,
    version: String,
    root: PathBuf,
    trust: Trust,
    staged: Mutex<Option<Staged>>,
    /// The arguments to start the new version with.
    args: Vec<String>,
}

/// The updater for the install `exe` runs from: on with the release feed in a build that
/// [`EXPECTED_PUBLISHER`] signed, or with `OPENCODE_RUNE_UPDATE_FEED` in a debug build. Runs on
/// the updater's worker thread: it reads the exe's signature and sweeps the install's siblings.
pub fn platform(exe: &Path, version: &str, build_channel: &str, updates: PathBuf) -> Option<Windows> {
    let (service, trust) = match feed::debug_feed() {
        Some(url) if feed::loopback(&url) => (url, Trust::Anything),
        Some(url) => (url, Trust::Publisher),
        None if release_signed(exe, build_channel) => (feed::WINDOWS_SERVICE_URL.to_owned(), Trust::Publisher),
        None => return None,
    };
    if !feed::allowed_update_url(&service) {
        log(&format!("updater: off: refusing insecure feed URL {service}"));
        return None;
    }
    let root = exe.parent()?.to_path_buf();
    sweep(&root, &updates);
    let _ = receipt::at(&root);
    let facts = Facts { version: version.to_owned(), channel: feed::feed_channel(build_channel), service, updates };
    let args = std::env::args().skip(1).collect();
    Some(Windows { engine: Engine::new(facts), version: version.to_owned(), root, trust, staged: Mutex::new(None), args })
}

/// Only a build the release's publisher signed reads the release feed. A development build
/// (channel `local`), a pull request's and any other unsigned build never do, so none of them
/// replaces itself with a release by surprise.
fn release_signed(exe: &Path, build_channel: &str) -> bool {
    if build_channel == "local" {
        return false;
    }
    let started = rune::time::Instant::now();
    let signer = authenticode::embedded_signer(exe);
    if signer.as_deref() == Some(EXPECTED_PUBLISHER) {
        return true;
    }
    let found = signer.map_or_else(|| "no trusted signature".to_owned(), |s| format!("signed by {s}"));
    log(&format!("updater: off: this build isn't signed by {EXPECTED_PUBLISHER} ({found}, read in {} µs)", started.elapsed().as_micros()));
    false
}

impl Platform for Windows {
    fn version(&self) -> &str {
        &self.version
    }

    fn check(&self) -> Result<Option<super::machine::Target>, String> {
        // A racing launch can own the workspace on the old image. That process must never
        // replace the newer tree it is no longer running from, even on an offline check.
        if let Some(installed) = receipt::at(&self.root)
            && installed.version != self.version
        {
            return Ok(Some(super::machine::Target::Installed(installed.version)));
        }
        Ok(self.engine.check()?.map(super::machine::Target::Restart))
    }

    fn stage(&self, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
        let found = self.engine.found().ok_or_else(|| "No update to download".to_owned())?;
        if let Some(staged_dir) = receipt::staged_dir_if_matches(&self.root, &found.version, &found.target.sha256) {
            self.verify_quick(&staged_dir)?;
            *self.staged.lock().unwrap_or_else(|e| e.into_inner()) = Some(Staged { version: found.version, sha256: found.target.sha256, dir: staged_dir });
            return Ok(());
        }
        let staged_dir = beside(&self.root, &format!("ready-{}", found.target.sha256));
        if let Ok(staged) = self.staged.lock()
            && let Some(st) = staged.as_ref()
            && st.sha256 == found.target.sha256
            && st.dir.join(EXE).is_file()
        {
            return Ok(());
        }
        check_replaceable(&self.root)?;
        let updates = &self.engine.facts.updates;
        std::fs::create_dir_all(updates).map_err(|e| format!("could not create update dir: {e}"))?;
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let download = updates.join(format!("download-{}-{seq}", std::process::id()));
        let unpack = beside(&self.root, &format!("staging-{}-{seq}", std::process::id()));
        let result: Result<Staged, String> = (|| {
            std::fs::create_dir_all(&download).map_err(|e| format!("could not create download dir: {e}"))?;
            let zip = download.join("update.zip");
            let found = self.engine.download(&zip, progress)?;
            log(&format!("updater: downloaded {} ({} bytes, SHA-256 {} matches)", found.version, found.target.size, found.target.sha256));
            std::fs::create_dir_all(&unpack).map_err(|e| format!("could not create unpack dir: {e}"))?;
            unpack_zip(&zip, &unpack)?;
            let app = locate(&unpack)?;
            self.verify(&app)?;
            let dest = match std::fs::symlink_metadata(&staged_dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => staged_dir.clone(),
                Ok(_) => match bundle::retire(&staged_dir) {
                    Ok(_) => staged_dir.clone(),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => staged_dir.clone(),
                    Err(why) => {
                        // A stale dir with a file held open by another process or a planted
                        // junction/reparse point is left untouched (preserving any link target),
                        // while we route around to a unique sibling directory so the update completes.
                        log(&format!("updater: leaving obstructed stage {} untouched ({why}); routing around", staged_dir.display()));
                        let alt = beside(&self.root, &format!("ready-{}-{}-{seq}", found.target.sha256, std::process::id()));
                        if std::fs::symlink_metadata(&alt).is_ok() {
                            return Err(format!("could not stage the update: fallback path {} already exists", alt.display()));
                        }
                        alt
                    }
                },
                Err(e) => return Err(format!("could not inspect staged path: {e}")),
            };
            std::fs::rename(&app, &dest).map_err(|e| format!("could not stage the update: {e}"))?;
            receipt::stage(&dest, &found.version, &found.target.sha256)?;
            Ok(Staged { version: found.version, sha256: found.target.sha256, dir: dest })
        })();
        std::fs::remove_dir_all(&download).ok();
        std::fs::remove_dir_all(&unpack).ok();
        let staged = result?;
        log(&format!("updater: {} staged in {}", staged.version, staged.dir.display()));
        if let Ok(mut slot) = self.staged.lock() {
            *slot = Some(staged);
        }
        Ok(())
    }

    fn install(&self) -> Result<(), String> {
        self.hand_off(true)
    }

    fn take_last_error(&self) -> Option<String> {
        let installed = receipt::at(&self.root).map(|r| r.version);
        receipt::take_apply_failure(&self.engine.facts.updates, &self.version, installed.as_deref())
    }
}

impl Windows {
    /// Every program and library in `app` must be signed by the release's publisher (or, from a
    /// local feed in a debug build, may be unsigned).
    fn verify(&self, app: &Path) -> Result<(), String> {
        if !app.join(EXE).is_file() {
            return Err(format!("the update has no {EXE}"));
        }
        let mut files = Vec::new();
        binaries(app, &mut files);
        for file in &files {
            let name = file.strip_prefix(app).unwrap_or(file).display().to_string();
            match (authenticode::signer(file), &self.trust) {
                (Ok(signer), _) if signer == EXPECTED_PUBLISHER => {}
                (Ok(signer), Trust::Anything) => log(&format!("updater: {name} is signed by {signer}; allowed from a local feed")),
                (Err(e), Trust::Anything) => log(&format!("updater: {name}: {e}; allowed from a local feed")),
                (Ok(signer), Trust::Publisher) => return Err(format!("{name} is signed by {signer}, not {EXPECTED_PUBLISHER}")),
                (Err(e), Trust::Publisher) => return Err(format!("the update couldn't be verified: {name}: {e}")),
            }
        }
        log(&format!("updater: checked the signatures of {} files", files.len()));
        Ok(())
    }

    /// Starts the staged version's executable as the helper, which swaps the folders once this
    /// process exits.
    fn hand_off(&self, relaunch: bool) -> Result<(), String> {
        let staged = self.staged.lock().ok().and_then(|s| s.as_ref().map(|s| (s.version.clone(), s.sha256.clone(), s.dir.clone()))).ok_or_else(|| "No staged update ready to install.".to_owned())?;
        let (version, sha256, dir) = staged;
        if receipt::at(&self.root).is_some_and(|r| r.version == version && r.sha256 == sha256) {
            return Ok(());
        }
        self.verify_quick(&dir)?;
        let updates = &self.engine.facts.updates;
        let helper = updates.join(format!("opencode-update-helper-{}.exe", std::process::id()));
        std::fs::copy(dir.join(EXE), &helper).map_err(|e| format!("could not prepare the update: {e}"))?;
        let mut cmd = Command::new(&helper);
        cmd.arg("--apply-update")
            .arg("--wait")
            .arg(std::process::id().to_string())
            .arg("--root")
            .arg(&self.root)
            .arg("--staged")
            .arg(&dir)
            .arg("--exe")
            .arg(EXE)
            .arg("--version")
            .arg(&version)
            .arg("--sha256")
            .arg(&sha256)
            .arg("--updates")
            .arg(updates)
            .arg("--cwd")
            .arg(std::env::current_dir().unwrap_or_else(|_| self.root.clone()));
        if relaunch {
            cmd.arg("--relaunch");
        }
        // A working folder inside the install would hold it open against the swap.
        cmd.arg("--").args(&self.args).current_dir(updates).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        // Out of any job the app runs in, so the helper outlives it.
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
        let child = cmd.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB).spawn().or_else(|_| cmd.creation_flags(flags).spawn()).map_err(|e| format!("could not start the update: {e}"))?;
        log(&format!("updater: helper {} (pid {}) installs {version} after this process exits{}", helper.display(), child.id(), if relaunch { ", then restarts" } else { "" }));
        Ok(())
    }

    /// The staged executable, which runs as the helper, is still there and still trusted.
    fn verify_quick(&self, dir: &Path) -> Result<(), String> {
        let exe = dir.join(EXE);
        if !exe.is_file() {
            return Err(format!("the staged update lost {}", exe.display()));
        }
        match (authenticode::signer(&exe), &self.trust) {
            (Ok(signer), Trust::Publisher) if signer != EXPECTED_PUBLISHER => Err(format!("the staged update is signed by {signer}, not {EXPECTED_PUBLISHER}")),
            (Err(e), Trust::Publisher) => Err(format!("the staged update couldn't be verified: {e}")),
            _ => Ok(()),
        }
    }
}

/// `<root's parent>\.<root's name>.<what>`: beside the install, on its volume.
pub fn beside(root: &Path, what: &str) -> PathBuf {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    root.parent().unwrap_or(root).join(format!(".{name}.{what}"))
}

fn check_replaceable(root: &Path) -> Result<(), String> {
    let parent = root.parent().ok_or_else(|| "The install folder has no parent folder.".to_owned())?;
    let probe = parent.join(format!(".opencode-rune-update-probe-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map(drop)
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|_| format!("{} is not writable, so OpenCode can't update itself there.", parent.display()))
}

/// Windows' own bsdtar reads zip archives (a GNU tar earlier on PATH doesn't).
fn unpack_zip(zip: &Path, into: &Path) -> Result<(), String> {
    let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let out = Command::new(system.join("System32").join("tar.exe"))
        .arg("-xf")
        .arg(zip)
        .arg("-C")
        .arg(into)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("failed to run tar: {e}"))?;
    if !out.status.success() {
        return Err(format!("could not unpack update archive: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// The unpacked install folder: the archive's root, or its one folder.
fn locate(unpacked: &Path) -> Result<PathBuf, String> {
    if unpacked.join(EXE).is_file() {
        return Ok(unpacked.to_path_buf());
    }
    let dirs: Vec<PathBuf> = std::fs::read_dir(unpacked).map_err(|e| format!("could not read unpacked dir: {e}"))?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    match dirs.as_slice() {
        [one] if one.join(EXE).is_file() => Ok(one.clone()),
        _ => Err(format!("the update archive has no {EXE}")),
    }
}

/// Every `.exe` and `.dll` under `dir`.
fn binaries(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            binaries(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe") || x.eq_ignore_ascii_case("dll")) {
            out.push(p);
        }
    }
}

/// Clears what an earlier update left: the previous version's folder, unfinished unpacks and
/// downloads, and the helper's copies.
fn sweep(root: &Path, updates: &Path) {
    repair_interrupted(root);
    let root_intact = root.join(EXE).is_file();
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let keep_ready = receipt::staged()
        .and_then(|st| st.staged_path(root).file_name().map(|n| n.to_string_lossy().into_owned()));
    if let Some(parent) = root.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        let old_prefix = format!(".{name}.old-");
        let staging_prefix = format!(".{name}.staging-");
        let ready_prefix = format!(".{name}.ready-");
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if root_intact && n.strip_prefix(&old_prefix).and_then(|pid| pid.parse::<u32>().ok()).is_some() {
                // A locked image does not protect the assets beside it. Never remove even one
                // entry on a refusal; another launch tries again. Stages belong to their helper.
                bundle::retire(&e.path()).ok();
            } else if let Some(pid) = n.strip_prefix(&staging_prefix).and_then(|s| s.split_once('-')).and_then(|(p, _)| p.parse::<u32>().ok())
                && bundle::process_gone(pid)
            {
                bundle::retire(&e.path()).ok();
            } else if root_intact
                && let Some(rest) = n.strip_prefix(&ready_prefix)
                && keep_ready.as_deref().is_none_or(|k| !k.eq_ignore_ascii_case(&n))
            {
                // Reclaim orphaned `.ready-<older_sha>` folders from skipped releases (matching
                // official `installer.nsh:152-215` `removeStagedUpdates`), keeping any routed-around
                // stage owned by a still-living process.
                let live_owner = rest
                    .split_once('-')
                    .and_then(|(_, tail)| tail.split_once('-'))
                    .and_then(|(pid, _)| pid.parse::<u32>().ok())
                    .is_some_and(|pid| pid == std::process::id() || !bundle::process_gone(pid));
                if !live_owner {
                    bundle::retire(&e.path()).ok();
                }
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(updates) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(pid) = n.strip_prefix("download-").and_then(|s| s.split_once('-')).and_then(|(p, _)| p.parse::<u32>().ok())
                && bundle::process_gone(pid)
            {
                std::fs::remove_dir_all(e.path()).ok();
            } else if n.starts_with("opencode-update-helper-") {
                std::fs::remove_file(e.path()).ok();
            }
        }
    }
}

/// If a non-transacted helper was killed in the window after moving `root` to `.old-<pid>` and
/// before placing the verified stage, restore `root` from the verified stage (or `.old-<pid>`)
/// once that helper PID is gone, before any `.old-*` retirement can run.
fn repair_interrupted(root: &Path) {
    if root.exists() {
        return;
    }
    let Some(parent) = root.parent() else { return };
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    let mut dead_old: Option<PathBuf> = None;
    for e in entries.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if let Some(pid) = n.strip_prefix(&format!(".{name}.old-")).and_then(|p| p.parse::<u32>().ok())
            && bundle::process_gone(pid)
            && e.path().join(EXE).is_file()
        {
            dead_old = Some(e.path());
            break;
        }
    }
    let Some(old) = dead_old else { return };
    if let Some(st) = receipt::staged() {
        let staged_dir = st.staged_path(root);
        if bundle::matches_id(&staged_dir, &st.bundle) && std::fs::rename(&staged_dir, root).is_ok() {
            log(&format!("updater: completed interrupted swap of {} into {}", st.version, root.display()));
            return;
        }
    }
    if std::fs::rename(&old, root).is_ok() {
        log(&format!("updater: restored {} from {}", root.display(), old.display()));
    }
}

fn log(message: &str) {
    eprintln!("{message}");
}
