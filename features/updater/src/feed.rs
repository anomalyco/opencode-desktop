//! Where updates come from and what this build is: the release feed, its channel, the target
//! this build installs, and the overrides a local test uses.
//!
//! | Variable | Builds | What it does |
//! | --- | --- | --- |
//! | `OPENCODE_RUNE_UPDATE_FEED` | debug | The feed's base URL, such as a local loopback feed served by `bun`; turns the updater on in a development build, and on Windows lets an unsigned update from a loopback feed (`http://127.0.0.1:`, `localhost`, `[::1]`) install. Non-loopback `http://` is always refused. |
//! | `OPENCODE_RUNE_RELEASE_URL` | debug (macOS, Linux) | Legacy alias for `OPENCODE_RUNE_UPDATE_FEED`; ignored in release builds. |
//! | `OPENCODE_RUNE_UPDATE_INITIAL_DELAY_MS` | all | How long after launch the first check waits (10 s). |
//! | `OPENCODE_RUNE_AUTO_INSTALL_ON_READY` | all (macOS) | `1`: install as soon as an update is ready. |
//! | `OPENCODE_RUNE_ALLOW_HARNESS_UPDATES` | all | `1`: opt in to running the updater in an isolated `RUNE_DIRS_ROOT` / `--harness-writes` window (needed on signed release/dev builds, where `OPENCODE_RUNE_UPDATE_FEED` is ignored). Has no effect on normal or headless runs, and never weakens feed URL or signature checks. |
//! | `OPENCODE_RUNE_SKIP_GATEKEEPER` | debug (macOS) | `1`, with a loopback feed and an unsigned running bundle: skip the signing team and Gatekeeper checks. Ignored in release builds. |

use std::time::Duration;

#[cfg_attr(not(any(target_os = "macos", target_os = "linux", windows)), allow(unused_imports))]
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

/// The one channel this app ships on: `https://anoma.ly/release/opencode/desktop-rune/dev.json`.
/// There is no stable channel, so no build switches channels by itself.
pub const CHANNEL: &str = "dev";

/// The feed channel every build follows.
pub fn feed_channel() -> String {
    CHANNEL.to_owned()
}

pub fn initial_delay() -> Duration {
    env("OPENCODE_RUNE_UPDATE_INITIAL_DELAY_MS").and_then(|v| v.parse::<u64>().ok()).map_or(INITIAL_DELAY, Duration::from_millis)
}

/// The local test feed (`OPENCODE_RUNE_UPDATE_FEED`), honoured only by debug builds.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn debug_feed() -> Option<String> {
    if cfg!(debug_assertions) { env("OPENCODE_RUNE_UPDATE_FEED") } else { None }
}

/// The local test feed (`OPENCODE_RUNE_UPDATE_FEED` or legacy `OPENCODE_RUNE_RELEASE_URL`),
/// honoured only by debug builds. Release builds always use [`DEFAULT_SERVICE_URL`].
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub fn debug_release_url() -> Option<String> {
    if cfg!(debug_assertions) {
        env("OPENCODE_RUNE_UPDATE_FEED").or_else(|| env("OPENCODE_RUNE_RELEASE_URL"))
    } else {
        None
    }
}

/// Skips macOS signing-team and Gatekeeper checks for an unsigned loopback test build; debug only.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn debug_skip_gatekeeper() -> bool {
    cfg!(debug_assertions) && env("OPENCODE_RUNE_SKIP_GATEKEEPER").as_deref() == Some("1")
}

/// Whether an isolated scratch-root / harness window explicitly opted in to running the updater.
pub fn harness_opt_in() -> bool {
    let update_feed = env("OPENCODE_RUNE_UPDATE_FEED");
    let release_url = env("OPENCODE_RUNE_RELEASE_URL");
    let allow = env("OPENCODE_RUNE_ALLOW_HARNESS_UPDATES");
    super::policy::opt_in_active(
        cfg!(debug_assertions),
        cfg!(any(target_os = "macos", target_os = "linux")),
        update_feed.as_deref(),
        release_url.as_deref(),
        allow.as_deref(),
    )
}

/// A variable's trimmed value, when set and not empty.
#[allow(clippy::disallowed_methods)]
pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// True for an `http://` URL whose authority targets loopback (`127.0.0.1`, `localhost`, `[::1]`)
/// with no userinfo.
pub fn loopback(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let host = if let Some(after) = authority.strip_prefix('[') {
        let Some((h, tail)) = after.split_once(']') else {
            return false;
        };
        if !tail.is_empty() && !tail.starts_with(':') {
            return false;
        }
        h
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// True when `url` is permitted for update manifest or payload fetches: `https://` everywhere,
/// plus loopback `http://` in debug builds only. Non-loopback `http://` is always refused.
pub fn allowed_update_url(url: &str) -> bool {
    url.starts_with("https://") || (cfg!(debug_assertions) && loopback(url))
}

/// Whether the HTTP transport must enforce HTTPS-only (including across redirects).
pub fn requires_https(url: &str) -> bool {
    !(cfg!(debug_assertions) && loopback(url))
}
