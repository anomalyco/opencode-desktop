//! Linux: checks the release feed and offers the download, never installs.
//!
//! The release ships an AppImage, a `.deb` and an `.rpm` (`scripts/package-linux.sh`), none of
//! which can safely replace itself, so a newer release comes back as Desktop's external update
//! (`Target::External`, the `download-required` state): Download opens the channel's latest
//! link for the package this build was installed from,
//! `https://anoma.ly/release/opencode/desktop-rune/<channel>/linux-<arch>-<format>`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::engine::{Engine, Facts};
use super::feed;
use super::machine::Target;
use super::platform::Platform;
use super::protocol::{APP_NAME, ARTIFACT_NAME, DEFAULT_SERVICE_URL};

/// What the `.deb` and `.rpm` install as (`package-linux.sh`).
const PACKAGE: &str = "opencode-rune";
const PACKAGED_EXE: &str = "/usr/bin/opencode-rune";

/// How this build was installed, as the release names its Linux targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    AppImage,
    Deb,
    Rpm,
}

impl Format {
    fn suffix(self) -> &'static str {
        match self {
            Format::AppImage => "appimage",
            Format::Deb => "deb",
            Format::Rpm => "rpm",
        }
    }
}

pub struct Linux {
    engine: Engine,
    version: String,
    /// The release target this install follows (`linux-x64-appimage`).
    target: String,
}

/// The updater for a release build (any channel but `local`), or for any build pointed at a
/// feed with `OPENCODE_RUNE_UPDATE_FEED` / `OPENCODE_RUNE_RELEASE_URL`. It only reads the feed
/// and opens a link, so no signature gates it.
pub fn platform(exe: &Path, version: &str, build_channel: &str, updates: PathBuf) -> Option<Linux> {
    let service = match feed::env("OPENCODE_RUNE_UPDATE_FEED").or_else(|| feed::env("OPENCODE_RUNE_RELEASE_URL")) {
        Some(url) => url,
        None if build_channel != "local" => DEFAULT_SERVICE_URL.to_owned(),
        None => return None,
    };
    let format = format(exe);
    let target = format!("{}-{}", feed::target_name(), format.suffix());
    eprintln!("updater: linux {format:?} install, following {target}");
    let facts = Facts { version: version.to_owned(), channel: feed::feed_channel(build_channel), service, updates };
    Some(Linux { engine: Engine::new(facts), version: version.to_owned(), target })
}

/// An AppImage runtime sets `$APPIMAGE`; the packaged executable belongs to whichever package
/// manager owns it; anything else (a copied binary) gets the AppImage, which runs anywhere.
fn format(exe: &Path) -> Format {
    if feed::env("APPIMAGE").is_some() {
        return Format::AppImage;
    }
    if exe != Path::new(PACKAGED_EXE) {
        return Format::AppImage;
    }
    if Path::new(&format!("/var/lib/dpkg/info/{PACKAGE}.list")).is_file() {
        return Format::Deb;
    }
    let rpm = Command::new("rpm").args(["-q", "--quiet", PACKAGE]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    if rpm.is_ok_and(|s| s.success()) {
        return Format::Rpm;
    }
    Format::AppImage
}

impl Linux {
    /// The channel's latest link for this install's target.
    fn latest(&self) -> String {
        let facts = &self.engine.facts;
        format!("{}/{APP_NAME}/{ARTIFACT_NAME}/{}/{}", facts.service.trim_end_matches('/'), facts.channel, self.target)
    }
}

impl Platform for Linux {
    fn version(&self) -> &str {
        &self.version
    }

    fn check(&self) -> Result<Option<Target>, String> {
        Ok(self.engine.check_target(&self.target)?.map(|found| Target::External { version: found.version, url: self.latest() }))
    }

    fn stage(&self, _progress: &dyn Fn(u64, u64)) -> Result<(), String> {
        Err("Linux builds update from a download".into())
    }

    fn install(&self) -> Result<(), String> {
        Err("Linux builds update from a download".into())
    }

    fn external(&self) -> bool {
        true
    }

    // On the updater's worker thread, so waiting for `xdg-open` holds nothing up.
    fn external_install(&self, url: &str) -> Result<(), String> {
        let status = Command::new("xdg-open").arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(format!("Could not open the download (xdg-open {s})")),
            Err(e) => Err(format!("Could not open the download: {e}")),
        }
    }
}
