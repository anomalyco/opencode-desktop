//! Where updates come from and what this build is: the release feed, its channel, the target
//! this build installs, and the overrides a local test uses.
//!
//! | Variable | Builds | What it does |
//! | --- | --- | --- |
//! | `OPENCODE_RUNE_UPDATE_FEED` | debug (Windows) | The feed's base URL, such as a local feed served by `bun`; turns the updater on in a development build, and lets an unsigned update from a loopback feed (`http://127.0.0.1:`, `localhost`, `[::1]`) install. Any other URL gets the release's publisher check. |
//! | `OPENCODE_RUNE_UPDATE_FEED` | all (Linux) | The feed's base URL; turns the check on in a development build. |
//! | `OPENCODE_RUNE_RELEASE_URL` | all (macOS, Linux) | The feed's base URL. |
//! | `OPENCODE_RUNE_UPDATE_CHANNEL` | all | The feed channel (`dev`, `stable`). |
//! | `OPENCODE_RUNE_UPDATE_INITIAL_DELAY_MS` | all | How long after launch the first check waits (10 s). |
//! | `OPENCODE_RUNE_AUTO_INSTALL_ON_READY` | all (macOS) | `1`: install as soon as an update is ready. |
//! | `OPENCODE_RUNE_SKIP_GATEKEEPER` | all (macOS) | `1`, with a loopback feed and an unsigned running bundle: skip the signing team and Gatekeeper checks. |

use std::time::Duration;

#[cfg_attr(not(any(target_os = "macos", windows)), allow(unused_imports))]
pub use super::protocol::DEFAULT_SERVICE_URL;

/// The feed Windows builds read: the release workflow publishes each signed build's install
/// folder there as target `windows-x64` / `windows-arm64`. Only a build that the release's
/// publisher signed reads it (`windows::EXPECTED_PUBLISHER`); a debug build reaches a local feed
/// through `OPENCODE_RUNE_UPDATE_FEED` instead.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WINDOWS_SERVICE_URL: &str = DEFAULT_SERVICE_URL;

/// The first check waits this long after launch, so it never competes with the first frames.
pub const INITIAL_DELAY: Duration = Duration::from_secs(10);
/// Then every ten minutes (Desktop's `machine.ts`, `Effect.sleep("10 minutes")`).
pub const INTERVAL: Duration = Duration::from_secs(600);

/// The release target this build installs.
pub fn target_name() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        super::protocol::TARGET_MACOS_ARM64
    } else if cfg!(target_os = "macos") {
        "macos-x64"
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        "windows-arm64"
    } else if cfg!(windows) {
        "windows-x64"
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-x64"
    }
}

/// The feed channel for build channel `build`: stable builds follow stable, every other build
/// follows dev.
pub fn feed_channel(build: &str) -> String {
    env("OPENCODE_RUNE_UPDATE_CHANNEL").unwrap_or_else(|| if build == "stable" { "stable" } else { "dev" }.to_owned())
}

pub fn initial_delay() -> Duration {
    env("OPENCODE_RUNE_UPDATE_INITIAL_DELAY_MS").and_then(|v| v.parse::<u64>().ok()).map_or(INITIAL_DELAY, Duration::from_millis)
}

/// The local test feed, honoured only by debug builds.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn debug_feed() -> Option<String> {
    if cfg!(debug_assertions) { env("OPENCODE_RUNE_UPDATE_FEED") } else { None }
}

/// A variable's trimmed value, when set and not empty.
#[allow(clippy::disallowed_methods)]
pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// True for an `http://` feed on this machine.
pub fn loopback(url: &str) -> bool {
    ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"].iter().any(|p| url.starts_with(p))
}
