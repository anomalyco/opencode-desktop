//! Pure process-level policy deciding whether the self-updater may start.
//!
//! Kept free of external crate dependencies so external harness tables can compile and verify it
//! directly without adding test code to the repository.

/// Process-level inputs deciding whether the self-updater may start and run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartInputs {
    /// True when Rune is running headless (`rune::is_headless()`).
    pub headless: bool,
    /// True when `RUNE_DIRS_ROOT` redirects all app folders into an isolated scratch root.
    pub rooted: bool,
    /// True when `--harness-writes` is present in argv (honoured only under `rooted`).
    pub harness_writes: bool,
    /// True when the run explicitly opts in to running the updater under a scratch root or
    /// harness window:
    /// - Debug builds: `OPENCODE_RUNE_UPDATE_FEED` (or legacy `OPENCODE_RUNE_RELEASE_URL` on
    ///   macOS/Linux), which updater loopback proofs already set;
    /// - Any build (including signed release/dev builds): `OPENCODE_RUNE_ALLOW_HARNESS_UPDATES=1`.
    pub opt_in: bool,
}

/// Why the self-updater is enabled or suppressed for this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Normal user run (neither headless nor rooted): updater runs unchanged.
    Normal,
    /// Headless run (`rune::is_headless()`): updater never starts, even if an opt-in env var is
    /// set (matches existing headless behaviour).
    Headless,
    /// Windowed harness run (`--harness-writes` + `RUNE_DIRS_ROOT`) without explicit opt-in.
    HarnessWindowSuppressed,
    /// Isolated scratch-root run (`RUNE_DIRS_ROOT` without `--harness-writes`) without opt-in.
    /// Suppressed because windowed proof and perf scripts (`rune-launch.ts --live`, `harness/perf`,
    /// `behaviour-suite.ts`) run under `RUNE_DIRS_ROOT` without `--harness-writes`; leaving the
    /// updater on would let a publisher-signed copy self-replace its folder on quit or show native
    /// OS update dialogs on the owner's desktop.
    ScratchRootSuppressed,
    /// Scratch-root or harness-window run that explicitly opted in to running the updater.
    OptedIn,
}

impl Decision {
    /// True when the updater service may start, check, download, install, or show dialogs.
    pub const fn allowed(self) -> bool {
        matches!(self, Self::Normal | Self::OptedIn)
    }
}

/// Pure decision function for whether the self-updater may run in this process.
pub const fn decide(input: StartInputs) -> Decision {
    if input.headless {
        return Decision::Headless;
    }
    if !input.rooted {
        return Decision::Normal;
    }
    if input.opt_in {
        return Decision::OptedIn;
    }
    if input.harness_writes {
        return Decision::HarnessWindowSuppressed;
    }
    Decision::ScratchRootSuppressed
}

fn non_empty(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|s| !s.is_empty())
}

/// Pure helper evaluating whether the environment explicitly opts in to running the updater inside
/// a scratch-root / harness window.
///
/// - `OPENCODE_RUNE_ALLOW_HARNESS_UPDATES=1` opts in on any build (debug or signed release/dev),
///   without altering feed URLs or signature verification.
/// - `OPENCODE_RUNE_UPDATE_FEED` (and `OPENCODE_RUNE_RELEASE_URL` on macOS/Linux) opt in only on
///   debug builds, matching `feed::debug_feed` / `feed::debug_release_url`.
pub fn opt_in_active(
    debug_assertions: bool,
    accepts_legacy_release_url: bool,
    update_feed: Option<&str>,
    release_url: Option<&str>,
    allow_harness_updates: Option<&str>,
) -> bool {
    if non_empty(allow_harness_updates) == Some("1") {
        return true;
    }
    if !debug_assertions {
        return false;
    }
    non_empty(update_feed).is_some()
        || (accepts_legacy_release_url && non_empty(release_url).is_some())
}
