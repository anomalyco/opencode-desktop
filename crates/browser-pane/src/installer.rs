//! The engine's availability state, its failure messages, and, with the `engine` feature, where
//! installed engines live (`browser-engine\<version>\OpenCode Browser Engine`), which one runs,
//! and the socket the app talks to it on. `source` says where a new one comes from and which
//! ones this build accepts, `download` and `unpack` fetch and install it, `prefetch` runs the job.

#[cfg(feature = "engine")]
use std::path::{Path, PathBuf};
use rune::time::Instant;

#[cfg(feature = "engine")]
use sha2::{Digest, Sha256};

/// The engine programs this build speaks to: an engine's version is CEF's (`156.0.26`) and this
/// revision (`156.0.26.1`). Bump it when the app needs an engine built from newer engine code (a
/// protocol change); a release of a newer CEF with the same revision is an update for every app
/// that accepts it.
pub const ENGINE_REVISION: u32 = 1;
#[cfg(not(windows))]
pub const ENGINE_BUNDLE_NAME: &str = "OpenCode Browser Engine.app";
#[cfg(not(windows))]
pub const ENGINE_BIN_REL: &str = "Contents/MacOS/opencode-browser-engine";
#[cfg(windows)]
pub const ENGINE_BUNDLE_NAME: &str = "OpenCode Browser Engine";
#[cfg(windows)]
pub const ENGINE_BIN_REL: &str = "opencode-browser-engine.exe";
/// Approximate archive size in decimal megabytes, shown before the first download (the index
/// gives the exact one once it's read): the 156.0.14.1 macOS zip is `128_330_085` bytes.
#[cfg(not(windows))]
pub const ENGINE_DOWNLOAD_MB: u64 = 128;
/// Approximate archive size in decimal megabytes: `package-windows.ps1`'s x64 archive of
/// 156.0.26.1 is `141_657_795` bytes.
#[cfg(windows)]
pub const ENGINE_DOWNLOAD_MB: u64 = 142;

pub const ERR_DOWNLOAD: &str = "Couldn't download the browser. Check your connection and try again.";
pub(crate) const ERR_VERIFY: &str = "The browser download was damaged, so it wasn't installed. Try again.";
pub(crate) const ERR_INSTALL: &str = "Couldn't install the browser. Check available disk space and try again.";
pub(crate) const ERR_DISK_FULL: &str = "There isn't enough disk space to install the browser. Free some space and try again.";
pub(crate) const ERR_NO_SOURCE: &str = "Browser isn't available for this platform yet.";
pub const ERR_STOPPED: &str = "The browser stopped unexpectedly.";
/// The engine runs but hasn't answered a request in time (Retry ends it and starts a new one).
pub const ERR_HUNG: &str = "The browser isn't responding.";

/// Not enough room for the download and what it unpacks to, said before it starts.
pub(crate) fn err_disk_space(need: u64, free: u64) -> String {
    let mb = |b: u64| b.div_ceil(1_000_000);
    format!("The browser needs {} MB of disk space and {} MB are free. Free some space and try again.", mb(need), mb(free))
}

/// The download's server or the network in between refused it, with its HTTP status.
pub(crate) fn err_blocked(status: u16) -> String {
    format!("The browser download was blocked (HTTP {status}). Your network or administrator may not allow it.")
}

/// The archive isn't one this build trusts: `why` names the rule it failed.
pub(crate) fn err_untrusted(why: &str) -> String {
    format!("The browser download couldn't be verified ({why}), so it wasn't installed.")
}

/// The engine's availability, one state machine that the pane and Settings read:
///
/// ```text
/// NotInstalled ─► Downloading ─► Verifying ─► Installed ─► Connecting ─► Ready
///                  │   ▲    │                     ▲            │
///                  ▼   │    ▼                     └─ found on disk at launch
///             Paused(reason)  Failed(retry_at) ─► Downloading (at retry_at, or Retry)
///                                  ▲
///                                  └─ the engine process went away, or stopped answering
///                                     (Retry starts it again; a late answer takes it back)
/// ```
///
/// `Downloading` is the download a page waits for (full speed, one try). An update downloads
/// beside the running engine, once a run after its first page opens, and never shows here. Nothing
/// of this runs before a page opens: no request, timer or thread.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineStatus {
    /// This build has no engine for its platform (it is built without the `engine` feature, or
    /// for a platform without a Unix-socket engine), so the pane never downloads or starts one.
    Unsupported,
    /// Not looked for yet: a launch touches nothing of the engine (its files, its socket, its
    /// source) until a page opens (or BrowserHost::prepare starts it), which then finds out.
    Unchecked,

    NotInstalled,
    Downloading { downloaded_bytes: u64, total_bytes: Option<u64> },
    /// The archive is in; its hash and signatures are checked and it moves into place.
    Verifying,
    /// The background download waits for the OS to allow it. Opening the pane goes ahead anyway.
    Paused(PauseReason),
    /// On disk, not running.
    Installed,
    Connecting,
    Ready,
    Failed(Failure),
}

/// Why a download or start failed, and when the background download tries again (`None`: it
/// waits for Retry).
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub message: String,
    pub retry_at: Option<Instant>,
}

impl EngineStatus {
    /// The engine runs but didn't answer a request in time.
    pub fn is_hung(&self) -> bool {
        matches!(self, Self::Failed(f) if f.message == ERR_HUNG)
    }
}

impl Failure {
    pub fn retrying(message: impl Into<String>, at: Instant) -> Self {
        Self { message: message.into(), retry_at: Some(at) }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { message, retry_at: None }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Why the background download is paused, as the OS reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseReason {
    /// The connection is metered (or roaming, or over its data limit).
    Metered,
    /// The computer runs on its battery.
    OnBattery,
}

impl PauseReason {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Metered => "Paused on a metered connection",
            Self::OnBattery => "Paused on battery power",
        }
    }
}

/// An engine's version: CEF's three numbers, then the engine revision (`ENGINE_REVISION`).
/// Written `156.0.26.1` (its folder) or `156.0.26-1` (the index's release version).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub cef: [u32; 3],
    pub revision: u32,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.trim().split(['.', '-']).map(|p| p.parse::<u32>().ok());
        let v = Self { cef: [parts.next()??, parts.next()??, parts.next()??], revision: parts.next()?? };
        parts.next().is_none().then_some(v)
    }

    /// Whether this build speaks to an engine of this version.
    pub fn accepted(self) -> bool {
        self.revision == ENGINE_REVISION
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c] = self.cef;
        write!(f, "{a}.{b}.{c}.{}", self.revision)
    }
}

/// Where everything of the engine lives, the third input beside the source and the trust rule:
/// its state folder holds the installed versions and each app's browser profile, its cache
/// folder the sockets. The app passes the folders its own data root decides (`set_home`), so a
/// scratch profile gets a scratch engine and a real install the real one; without it they are
/// `rune::dirs`' for `co.anomaly.opencode`, shared by every app that shows a browser pane
/// (`~/Library/Application Support/co.anomaly.opencode` on macOS, `%LOCALAPPDATA%\co.anomaly.opencode`
/// on Windows).
#[cfg(feature = "engine")]
static HOME: std::sync::OnceLock<rune::dirs::AppDirs> = std::sync::OnceLock::new();

/// Sets the engine's folders, once, before any page opens.
#[cfg(feature = "engine")]
pub fn set_home(dirs: rune::dirs::AppDirs) {
    let _ = HOME.set(dirs);
}

#[cfg(feature = "engine")]
pub(crate) fn engine_dirs() -> Option<rune::dirs::AppDirs> {
    HOME.get().cloned().or_else(|| rune::dirs::AppDirs::new("co.anomaly.opencode"))
}

/// The folder holding one folder per installed engine version.
#[cfg(feature = "engine")]
pub fn engines_root() -> PathBuf {
    engine_dirs()
        .map_or_else(|| std::env::temp_dir().join("co.anomaly.opencode"), |d| d.state().to_owned())
        .join("browser-engine")
}

/// An installed engine: its version and its executable.
#[cfg(feature = "engine")]
#[derive(Clone, Debug)]
pub struct Installed {
    pub version: Option<Version>,
    pub bin: PathBuf,
}

/// The engine to start: `OPENCODE_BROWSER_ENGINE_BIN` (an executable) or
/// `OPENCODE_BROWSER_ENGINE_APP` (its folder) when set, a development build's own, and otherwise
/// the newest installed version this build accepts. Reads one folder; call it off the UI thread.
#[cfg(feature = "engine")]
pub fn installed() -> Option<Installed> {
    if let Some(bin) = std::env::var_os("OPENCODE_BROWSER_ENGINE_BIN").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(Installed { version: None, bin });
    }
    if let Some(bin) = std::env::var_os("OPENCODE_BROWSER_ENGINE_APP").map(|app| PathBuf::from(app).join(ENGINE_BIN_REL)).filter(|p| p.is_file()) {
        return Some(Installed { version: None, bin });
    }
    installed_versions(&engines_root())
        .into_iter()
        .filter(|v| v.accepted())
        .max()
        .map(|version| Installed { version: Some(version), bin: bin_in(&engines_root().join(version.to_string())) })
        .filter(|i| i.bin.is_file())
}

/// The versions with a folder in `root`, accepted by this build or not.
#[cfg(feature = "engine")]
pub(crate) fn installed_versions(root: &Path) -> Vec<Version> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    entries.flatten().filter_map(|e| e.file_name().to_str().and_then(Version::parse)).collect()
}

/// The executable inside a version's folder.
#[cfg(feature = "engine")]
pub(crate) fn bin_in(version_dir: &Path) -> PathBuf {
    version_dir.join(ENGINE_BUNDLE_NAME).join(ENGINE_BIN_REL)
}

/// Removes every version folder but `keep` and the newest that each other app revision would
/// start, and unpacking folders left by an app that quit mid-install (their process is gone, so
/// another app's install in progress keeps its own). A folder an engine still runs from stays:
/// it moves aside first, which Windows refuses while its files are open, so nothing in use is
/// ever half deleted. Reads one folder; call it off the UI thread.
#[cfg(feature = "engine")]
pub(crate) fn remove_old(root: &Path, keep: Version) {
    let versions = installed_versions(root);
    let newest_of = |revision: u32| versions.iter().filter(|v| v.revision == revision).max().copied();
    for v in &versions {
        if *v == keep || (v.revision != keep.revision && newest_of(v.revision) == Some(*v)) || *v > keep {
            continue;
        }
        let dir = root.join(v.to_string());
        let trash = root.join(format!(".{v}.old-{}", std::process::id()));
        if std::fs::rename(&dir, &trash).is_ok() {
            let _ = std::fs::remove_dir_all(&trash);
        }
    }
    sweep_unpacking(root);
}

/// Removes the unpacking and removal folders a process left behind when it quit (or was killed)
/// mid-way; each one is as large as an engine.
#[cfg(feature = "engine")]
pub(crate) fn sweep_unpacking(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // `.<version>.staging-<pid>-<n>` and `.<version>.old-<pid>`: the process that made it is gone.
        let pid = name.strip_prefix('.').and_then(|n| n.split_once(".staging-").or_else(|| n.split_once(".old-"))).and_then(|(_, rest)| rest.split('-').next()?.parse::<u32>().ok());
        if pid.is_some_and(|pid| pid != std::process::id() && !crate::background::pid_alive(pid)) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(feature = "engine")]
pub fn socket_path() -> PathBuf {
    if let Some(custom) = std::env::var_os("OPENCODE_BROWSER_SOCKET") {
        return PathBuf::from(custom);
    }
    // Windows: the folder under `%LOCALAPPDATA%` is already the user's own.
    #[cfg(unix)]
    let uid = unsafe { libc::geteuid() };
    #[cfg(windows)]
    let uid = 0;
    let dir = engine_dirs().map_or_else(|| PathBuf::from("/tmp"), |d| d.cache().to_owned());
    let _ = create_private_dir(&dir);
    let exe = std::env::current_exe().unwrap_or_default();
    dir.join(socket_name_for(&exe, uid))
}

/// The engine serves one app at a time: the last to connect takes every frame and tab update,
/// and every app numbers its tabs from 1. So each app (by executable path) runs its own engine.
#[cfg(feature = "engine")]
pub fn socket_name_for(exe: &Path, uid: u32) -> String {
    format!("browser-{uid}-{}.sock", exe_tag(exe))
}

/// Eight hex digits of the executable path's hash, pairing an app's socket with its profile.
#[cfg(feature = "engine")]
pub(crate) fn exe_tag(exe: &Path) -> String {
    let digest = Sha256::digest(exe.as_os_str().as_encoded_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(feature = "engine")]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// A failed write's message: no space left, or another install failure.
#[cfg(feature = "engine")]
pub(crate) fn err_write(err: &std::io::Error) -> String {
    // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL on Windows; ENOSPC, EDQUOT elsewhere.
    #[cfg(windows)]
    let full = matches!(err.raw_os_error(), Some(39 | 112));
    #[cfg(unix)]
    let full = matches!(err.raw_os_error(), Some(libc::ENOSPC | libc::EDQUOT));
    if full || err.kind() == std::io::ErrorKind::StorageFull { ERR_DISK_FULL.into() } else { ERR_INSTALL.into() }
}
