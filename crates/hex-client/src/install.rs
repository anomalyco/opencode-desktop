//! Whether Hex is on this computer, starting it, and the fixes a problem offers: Hex's download,
//! Hex's window, or the system's microphone privacy page.
//!
//! On Windows Hex's installer puts the app in `versions\<version>\hex.exe` under its data folder
//! (`%APPDATA%\hex`), and `current-version` names the one to run. macOS and Linux find Hex only
//! while it runs, through its endpoint file.

use std::path::{Path, PathBuf};

/// Hex's download page, which picks the build for the computer it's opened on.
pub const DOWNLOAD: &str = "https://anoma.ly/hex/";

/// Whether the mic shows while Hex isn't installed, so a press offers Hex's download. Windows
/// shows it; macOS keeps it hidden until Hex runs.
pub const MIC_WITHOUT_HEX: bool = cfg!(windows);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// No Hex on this computer (or, off Windows, none running).
    Missing,
    /// Hex is installed but isn't running: a dictation starts it first.
    Stopped,
    /// Hex is running: its endpoint file is there.
    Running,
}

/// Where Hex is, from its endpoint file `file` (`discovery`) and its data folder `support`.
pub fn presence(file: &Path, support: &Path) -> Presence {
    if file.exists() {
        return Presence::Running;
    }
    if app(support).is_some() { Presence::Stopped } else { Presence::Missing }
}

/// The Hex app the installer laid out under `support`.
#[cfg(windows)]
pub fn app(support: &Path) -> Option<PathBuf> {
    let version = std::fs::read_to_string(support.join("current-version")).ok()?;
    let exe = support.join("versions").join(version.trim()).join("hex.exe");
    exe.is_file().then_some(exe)
}

#[cfg(not(windows))]
pub fn app(_support: &Path) -> Option<PathBuf> {
    None
}

/// Starts Hex: in the background, as a login launch does, or with its window shown (which
/// shows a running Hex's window too).
#[cfg(windows)]
pub fn launch(support: &Path, show: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if recorded_launch(show) {
        return Ok(());
    }
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let exe = app(support).ok_or("Hex isn't installed")?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("app");
    if !show {
        cmd.arg("--hidden");
    }
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    // Hex outlives the app, so it keeps none of the app's handles.
    let null = std::process::Stdio::null;
    cmd.stdin(null()).stdout(null()).stderr(null());
    cmd.creation_flags(DETACHED_PROCESS).spawn().map(drop).map_err(|e| format!("Couldn't open Hex: {e}"))
}

#[cfg(target_os = "macos")]
pub fn launch(_support: &Path, show: bool) -> Result<(), String> {
    if recorded_launch(show) {
        return Ok(());
    }
    let mut cmd = std::process::Command::new("open");
    if !show {
        cmd.arg("-g");
    }
    cmd.args(["-a", "Hex"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(drop).map_err(|e| format!("Couldn't open Hex: {e}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn launch(_support: &Path, show: bool) -> Result<(), String> {
    if recorded_launch(show) {
        return Ok(());
    }
    std::process::Command::new("hex").arg("start").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(drop).map_err(|e| format!("Couldn't open Hex: {e}"))
}

/// A proof run that records external opens records Hex's start instead of making it.
fn recorded_launch(show: bool) -> bool {
    harness_log::record("launchApp", &[("app", "hex"), ("show", if show { "true" } else { "false" })])
}

/// Opens the system's microphone privacy page, where the app's access is turned on.
#[cfg(windows)]
pub fn open_mic_settings() {
    if harness_log::record("openSettings", &[("url", "ms-settings:privacy-microphone")]) {
        return;
    }
    // `ms-settings:` links open through the shell; Explorer hands them to Settings.
    std::process::Command::new("explorer.exe").arg("ms-settings:privacy-microphone").spawn().ok();
}

#[cfg(target_os = "macos")]
pub fn open_mic_settings() {
    const PAGE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
    if harness_log::record("openSettings", &[("url", PAGE)]) {
        return;
    }
    std::process::Command::new("open").arg(PAGE).spawn().ok();
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn open_mic_settings() {}

/// What a problem offers to do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Open Hex's download page.
    Install,
    /// Open Hex: start it, or show its window to finish setup or turn it on.
    OpenHex,
    /// Open the system's microphone privacy page.
    MicSettings,
}

impl Fix {
    pub fn label(self) -> &'static str {
        match self {
            Fix::Install => "Get Hex",
            Fix::OpenHex => "Open Hex",
            Fix::MicSettings => "Microphone settings",
        }
    }
}
