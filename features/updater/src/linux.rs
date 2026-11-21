//! Linux: in-app updates for AppImage, `.deb` and `.rpm`, matching official's `electron-updater`
//! (`AppImageUpdater`, `DebUpdater`, `RpmUpdater`), and disabled on Snap, Flatpak and distro
//! packages (`main.ts:17`, `AppImageUpdater.js:17-26`).
//!
//! - **AppImage** (`$APPIMAGE` set, valid absolute path): downloads `linux-<arch>-appimage`,
//!   verifies size and SHA-256, `chmod 0755`, stages beside `$APPIMAGE` and atomically replaces it
//!   with `rename(2)`, then spawns the new AppImage (`APPIMAGE_SILENT_INSTALL=true`,
//!   `RUNE_RELAUNCH_AFTER=<pid>`) while this process quits cleanly.
//! - **`.deb`** (`/usr/bin/opencode-rune` owned by dpkg): downloads `linux-<arch>-deb`, verifies
//!   size and SHA-256 inside a private `0700` directory, installs via `dpkg -i` (with
//!   `apt-get install -f -y` fallback) or `apt`, elevated through `pkexec` when not root, then
//!   spawns `/usr/bin/opencode-rune` (`RUNE_RELAUNCH_AFTER=<pid>`).
//! - **`.rpm`** (`/usr/bin/opencode-rune` owned by rpm): downloads `linux-<arch>-rpm`, verifies
//!   size and SHA-256 inside a private `0700` directory, installs via `zypper` / `dnf` / `yum` /
//!   `rpm` elevated through `pkexec` when not root, then spawns `/usr/bin/opencode-rune`.
//! - **Distro packages, Snap, Flatpak, bare binaries**: updater returns `None` (`Disabled`:
//!   "This build is updated outside OpenCode, for example by your package manager.").

use std::fs::{self, Permissions};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::engine::{self, Engine, Facts};
use super::feed;
use super::machine::Target;
use super::platform::Platform;
use super::protocol::DEFAULT_SERVICE_URL;

/// What the `.deb` and `.rpm` install as (`apps/desktop/scripts/package-linux.sh`).
const PACKAGE: &str = "opencode-rune";
const PACKAGED_EXE: &str = "/usr/bin/opencode-rune";
const RELAUNCH_AFTER: &str = "RUNE_RELAUNCH_AFTER";
const INSTALLED_RECEIPT: &str = "installed-bundle.json";

static STAGE_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
enum InstallKind {
    AppImage(PathBuf),
    Deb,
    Rpm,
}

impl InstallKind {
    fn suffix(&self) -> &'static str {
        match self {
            InstallKind::AppImage(_) => "appimage",
            InstallKind::Deb => "deb",
            InstallKind::Rpm => "rpm",
        }
    }

    fn extension(&self) -> &'static str {
        match self {
            InstallKind::AppImage(_) => "AppImage",
            InstallKind::Deb => "deb",
            InstallKind::Rpm => "rpm",
        }
    }

    fn installed_path(&self) -> &Path {
        match self {
            InstallKind::AppImage(path) => path.as_path(),
            InstallKind::Deb | InstallKind::Rpm => Path::new(PACKAGED_EXE),
        }
    }
}

struct Staged {
    version: String,
    sha256: String,
    size: u64,
    dir: PathBuf,
    file: PathBuf,
    /// Original archive filename from the manifest target URL (for versioned AppImage updates).
    filename: String,
}

pub struct Linux {
    engine: Engine,
    version: String,
    kind: InstallKind,
    /// The release target this install follows (`linux-x64-appimage`, `linux-x64-deb`, ...).
    target: String,
    staged: Mutex<Option<Staged>>,
    args: Vec<String>,
}

/// The updater for an AppImage, `.deb` or `.rpm` install on a release channel (or with
/// `OPENCODE_RUNE_UPDATE_FEED` / `OPENCODE_RUNE_RELEASE_URL` set). Snap, Flatpak, distro
/// packages (such as AUR) and bare binaries return `None` (`Disabled`).
pub fn platform(exe: &Path, version: &str, build_channel: &str, updates: PathBuf) -> Option<Linux> {
    if feed::env("SNAP").is_some() || feed::env("FLATPAK_ID").is_some() {
        eprintln!("updater: linux packaged under Snap/Flatpak; updater disabled");
        return None;
    }
    let service = match feed::env("OPENCODE_RUNE_UPDATE_FEED").or_else(|| feed::env("OPENCODE_RUNE_RELEASE_URL")) {
        Some(url) => url,
        None if build_channel != "local" => DEFAULT_SERVICE_URL.to_owned(),
        None => return None,
    };
    let kind = detect_install(exe)?;
    let target = format!("{}-{}", feed::target_name(), kind.suffix());
    eprintln!("updater: linux {kind:?} install, following {target}");
    sweep_old_stages(&updates);
    sync_installed_receipt(&updates, kind.installed_path());
    let facts = Facts { version: version.to_owned(), channel: feed::feed_channel(build_channel), service, updates };
    let args = std::env::args().skip(1).collect();
    Some(Linux { engine: Engine::new(facts), version: version.to_owned(), kind, target, staged: Mutex::new(None), args })
}

fn detect_install(exe: &Path) -> Option<InstallKind> {
    if let Some(raw) = feed::env("APPIMAGE") {
        let path = PathBuf::from(&raw);
        if path.is_absolute() && !raw.contains('\0') && path.is_file() {
            return Some(InstallKind::AppImage(path));
        }
        eprintln!("updater: ignoring invalid APPIMAGE path: {raw:?}");
        return None;
    }
    if exe != Path::new(PACKAGED_EXE) {
        return None;
    }
    if Path::new(&format!("/var/lib/dpkg/info/{PACKAGE}.list")).is_file() {
        return Some(InstallKind::Deb);
    }
    let rpm = Command::new("rpm").args(["-q", "--quiet", PACKAGE]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    if rpm.is_ok_and(|s| s.success()) {
        return Some(InstallKind::Rpm);
    }
    None
}

impl Platform for Linux {
    fn version(&self) -> &str {
        &self.version
    }

    fn check(&self) -> Result<Option<Target>, String> {
        if let Some(r) = read_installed_receipt(&self.engine.facts.updates)
            && file_id(self.kind.installed_path()) == Some((r.dev, r.ino))
            && r.version != self.version
        {
            return Ok(Some(Target::Installed(r.version)));
        }
        Ok(self.engine.check_target(&self.target)?.map(|found| Target::Restart(found.version)))
    }

    fn stage(&self, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
        let found = self.engine.found().ok_or_else(|| "No update to download".to_owned())?;
        if let Ok(staged) = self.staged.lock()
            && let Some(st) = staged.as_ref()
            && st.sha256 == found.target.sha256
            && st.file.is_file()
        {
            return Ok(());
        }
        if let InstallKind::AppImage(ref path) = self.kind {
            check_writable_parent(path)?;
        }
        let root = &self.engine.facts.updates;
        fs::create_dir_all(root).map_err(|e| format!("could not create update dir: {e}"))?;
        let dir = root.join(format!("stage-{}-{}", std::process::id(), STAGE_SEQ.fetch_add(1, Ordering::Relaxed)));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).map_err(|e| format!("could not create stage dir: {e}"))?;
        fs::set_permissions(&dir, Permissions::from_mode(0o700)).map_err(|e| format!("could not lock stage dir permissions: {e}"))?;
        let filename = archive_filename(&found.target.url, &found.version, self.kind.extension());
        let file = dir.join(&filename);
        let result = (|| {
            let found = self.engine.download(&file, progress)?;
            if matches!(self.kind, InstallKind::AppImage(_)) {
                fs::set_permissions(&file, Permissions::from_mode(0o755)).map_err(|e| format!("could not mark AppImage executable: {e}"))?;
            }
            Ok(Staged {
                version: found.version,
                sha256: found.target.sha256,
                size: found.target.size,
                dir: dir.clone(),
                file,
                filename,
            })
        })();
        match result {
            Ok(staged) => {
                if let Ok(mut slot) = self.staged.lock()
                    && let Some(prev) = slot.replace(staged)
                {
                    fs::remove_dir_all(prev.dir).ok();
                }
                Ok(())
            }
            Err(e) => {
                fs::remove_dir_all(&dir).ok();
                Err(e)
            }
        }
    }

    fn install(&self) -> Result<(), String> {
        let staged = self.staged.lock().ok().and_then(|mut s| s.take()).ok_or_else(|| "No staged update ready to install.".to_owned())?;
        let outcome = (|| -> Result<(), String> {
            verify_staged(&staged)?;
            match &self.kind {
                InstallKind::AppImage(appimage) => self.install_appimage(appimage, &staged),
                InstallKind::Deb => self.install_deb(&staged),
                InstallKind::Rpm => self.install_rpm(&staged),
            }
        })();
        if outcome.is_ok() {
            fs::remove_dir_all(&staged.dir).ok();
        } else if let Ok(mut slot) = self.staged.lock() {
            *slot = Some(staged);
        }
        outcome
    }
}

impl Linux {
    /// Atomically replaces the AppImage on disk, preserves custom unversioned filenames
    /// (`AppImageUpdater.js:91-97`), records the receipt, and launches the new AppImage waiting on
    /// our PID via `RUNE_RELAUNCH_AFTER`.
    fn install_appimage(&self, appimage: &Path, staged: &Staged) -> Result<(), String> {
        let parent = appimage.parent().ok_or_else(|| "AppImage has no parent directory.".to_owned())?;
        let existing_name = appimage.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let dest = if staged.filename == existing_name || !has_semver(existing_name) {
            appimage.to_path_buf()
        } else {
            parent.join(&staged.filename)
        };
        let dest_name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("opencode-rune.AppImage");
        let incoming = parent.join(format!(".{dest_name}.update-{}", std::process::id()));
        fs::remove_file(&incoming).ok();
        place_file(&staged.file, &incoming, 0o755)?;
        if let Err(e) = fs::rename(&incoming, &dest) {
            fs::remove_file(&incoming).ok();
            return Err(format!("could not replace AppImage at {}: {e}", dest.display()));
        }
        if dest != appimage {
            fs::remove_file(appimage).ok();
        }
        write_installed_receipt(&self.engine.facts.updates, &dest, &staged.version, &staged.sha256);
        engine::record_installed(&staged.version, &staged.sha256);
        spawn_relaunched(&dest, &self.args, true)
    }

    fn install_deb(&self, staged: &Staged) -> Result<(), String> {
        let pm = detect_package_manager(&["dpkg", "apt"]).ok_or_else(|| "Neither dpkg nor apt command found. Cannot install .deb package.".to_owned())?;
        let pkg = staged.file.to_str().ok_or_else(|| "Staged package path is not valid UTF-8.".to_owned())?;
        match pm.as_str() {
            "dpkg" => {
                if let Err(err) = run_elevated(&["dpkg", "-i", pkg]) {
                    eprintln!("updater: dpkg -i failed ({err}); running apt-get install -f -y");
                    run_elevated(&["apt-get", "install", "-f", "-y"]).map_err(|fix_err| format!("dpkg install failed ({err}) and dependency fix failed ({fix_err})"))?;
                }
            }
            "apt" => {
                run_elevated(&["apt", "install", "-y", "--allow-unauthenticated", "--allow-downgrades", "--allow-change-held-packages", pkg])?;
            }
            other => return Err(format!("Package manager {other} not supported")),
        }
        let exe = Path::new(PACKAGED_EXE);
        write_installed_receipt(&self.engine.facts.updates, exe, &staged.version, &staged.sha256);
        engine::record_installed(&staged.version, &staged.sha256);
        spawn_relaunched(exe, &self.args, false)
    }

    fn install_rpm(&self, staged: &Staged) -> Result<(), String> {
        let pm = detect_package_manager(&["zypper", "dnf", "yum", "rpm"]).ok_or_else(|| "No supported RPM package manager (zypper, dnf, yum, rpm) found.".to_owned())?;
        let pkg = staged.file.to_str().ok_or_else(|| "Staged package path is not valid UTF-8.".to_owned())?;
        match pm.as_str() {
            "zypper" => run_elevated(&["zypper", "--non-interactive", "--no-refresh", "install", "--allow-unsigned-rpm", "-f", pkg])?,
            "dnf" => run_elevated(&["dnf", "install", "--nogpgcheck", "-y", pkg])?,
            "yum" => run_elevated(&["yum", "install", "--nogpgcheck", "-y", pkg])?,
            "rpm" => run_elevated(&["rpm", "-Uvh", "--replacepkgs", "--replacefiles", "--nodeps", pkg])?,
            other => return Err(format!("Package manager {other} not supported")),
        }
        let exe = Path::new(PACKAGED_EXE);
        write_installed_receipt(&self.engine.facts.updates, exe, &staged.version, &staged.sha256);
        engine::record_installed(&staged.version, &staged.sha256);
        spawn_relaunched(exe, &self.args, false)
    }
}

fn spawn_relaunched(exe: &Path, args: &[String], appimage: bool) -> Result<(), String> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env(RELAUNCH_AFTER, std::process::id().to_string())
        .env_remove("RUNE_INSTANCE_HANDED")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if appimage {
        cmd.env("APPIMAGE_SILENT_INSTALL", "true");
    }
    if let Ok(dir) = std::env::current_dir() {
        cmd.current_dir(dir);
    }
    cmd.spawn().map(drop).map_err(|e| format!("could not launch updated app {}: {e}", exe.display()))
}

fn verify_staged(staged: &Staged) -> Result<(), String> {
    let mut file = fs::File::open(&staged.file).map_err(|e| format!("staged update disappeared: {e}"))?;
    let meta = file.metadata().map_err(|e| format!("could not stat staged update: {e}"))?;
    if meta.len() != staged.size {
        return Err(format!("staged update size {} did not match expected {}", meta.len(), staged.size));
    }
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("could not verify staged update: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != staged.sha256 {
        return Err("staged update SHA-256 mismatch".into());
    }
    Ok(())
}

fn place_file(src: &Path, dst: &Path, mode: u32) -> Result<(), String> {
    if fs::rename(src, dst).is_ok() {
        fs::set_permissions(dst, Permissions::from_mode(mode)).map_err(|e| format!("could not set permissions on {}: {e}", dst.display()))?;
        return Ok(());
    }
    fs::copy(src, dst).map_err(|e| format!("could not copy update beside AppImage: {e}"))?;
    fs::set_permissions(dst, Permissions::from_mode(mode)).map_err(|e| format!("could not set permissions on {}: {e}", dst.display()))?;
    if let Ok(f) = fs::File::open(dst) {
        f.sync_all().ok();
    }
    Ok(())
}

fn check_writable_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "AppImage has no parent directory.".to_owned())?;
    let probe = parent.join(format!(".opencode-rune-update-probe-{}", std::process::id()));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map(drop)
        .and_then(|()| fs::remove_file(&probe))
        .map_err(|_| format!("{} is not writable.", parent.display()))
}

fn is_root() -> bool {
    fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

fn has_command(cmd: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(cmd);
        fs::metadata(&candidate).is_ok_and(|m| m.is_file() && (m.mode() & 0o111) != 0)
    })
}

fn detect_package_manager(priority: &[&str]) -> Option<String> {
    if let Some(override_pm) = feed::env("ELECTRON_BUILDER_LINUX_PACKAGE_MANAGER") {
        let safe = override_pm.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if safe && has_command(&override_pm) {
            return Some(override_pm);
        }
    }
    priority.iter().copied().find(|pm| has_command(pm)).map(str::to_owned)
}

/// Runs `argv` directly as root or elevated via `pkexec` (argv-only, no shell interpolation).
fn run_elevated(argv: &[&str]) -> Result<(), String> {
    let (program, args): (&str, Vec<&str>) = if is_root() {
        (argv[0], argv[1..].to_vec())
    } else if has_command("pkexec") {
        let mut a = Vec::with_capacity(argv.len());
        a.extend_from_slice(argv);
        ("pkexec", a)
    } else if has_command("sudo") {
        let mut a = Vec::with_capacity(argv.len() + 1);
        a.push("-n");
        a.extend_from_slice(argv);
        ("sudo", a)
    } else {
        return Err("No privilege elevation helper (pkexec or sudo) available to install package.".into());
    };
    let out = Command::new(program).args(&args).stdin(Stdio::null()).output().map_err(|e| format!("failed to run {program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    let detail = if stderr.is_empty() { format!("exited with {}", out.status) } else { stderr };
    Err(format!("{program} {}: {detail}", argv[0]))
}

fn archive_filename(url: &str, version: &str, ext: &str) -> String {
    let last = url.rsplit('/').next().unwrap_or_default();
    let clean = last.split('?').next().unwrap_or_default();
    if !clean.is_empty() && !clean.contains("..") && !clean.contains('/') && !clean.contains('\0') {
        clean.to_owned()
    } else {
        format!("OpenCode-Rune-{version}.{ext}")
    }
}

/// True when `name` contains a `digits.digits.digits` version substring (`AppImageUpdater.js:91`).
fn has_semver(name: &str) -> bool {
    let bytes = name.as_bytes();
    for start in 0..bytes.len() {
        let mut i = start;
        let mut groups = 0;
        while groups < 3 {
            let digits_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == digits_start {
                break;
            }
            groups += 1;
            if groups < 3 {
                if i < bytes.len() && bytes[i] == b'.' {
                    i += 1;
                } else {
                    break;
                }
            }
        }
        if groups == 3 {
            return true;
        }
    }
    false
}

fn sweep_old_stages(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("stage-") {
            fs::remove_dir_all(entry.path()).ok();
        }
    }
}

#[derive(Deserialize, Serialize)]
struct InstalledBundle {
    version: String,
    sha256: String,
    dev: u64,
    ino: u64,
}

fn file_id(path: &Path) -> Option<(u64, u64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

fn read_installed_receipt(updates: &Path) -> Option<InstalledBundle> {
    let text = fs::read_to_string(updates.join(INSTALLED_RECEIPT)).ok()?;
    let r: InstalledBundle = serde_json::from_str(&text).ok()?;
    (r.sha256.len() == 64).then_some(r)
}

fn sync_installed_receipt(updates: &Path, installed_path: &Path) {
    let Some(r) = read_installed_receipt(updates) else { return };
    if file_id(installed_path) == Some((r.dev, r.ino)) {
        engine::record_installed(&r.version, &r.sha256);
    } else {
        fs::remove_file(updates.join(INSTALLED_RECEIPT)).ok();
        if engine::read_installed_sha(&r.version).as_deref() == Some(r.sha256.as_str()) {
            opencode_platform::storage::write_json(engine::NAMESPACE, "rune.installedRelease", &serde_json::Value::Null);
        }
    }
}

fn write_installed_receipt(updates: &Path, placed: &Path, version: &str, sha256: &str) {
    let Some((dev, ino)) = file_id(placed) else { return };
    if fs::create_dir_all(updates).is_err() {
        return;
    }
    let tmp = updates.join(format!("installed-bundle.tmp-{}", std::process::id()));
    let Ok(json) = serde_json::to_vec(&InstalledBundle { version: version.to_owned(), sha256: sha256.to_owned(), dev, ino }) else { return };
    if fs::write(&tmp, json).is_ok() {
        let _ = fs::rename(&tmp, updates.join(INSTALLED_RECEIPT));
    } else {
        fs::remove_file(&tmp).ok();
    }
}
