//! Readies the browser engine in the background (Settings › Download browser in the background),
//! so the first page opens without waiting for its download: once the app has sat idle for `IDLE`
//! (no input and nothing animating but ambient loops, see `AMBIENT`), `BrowserHost::prefetch`
//! starts the throttled, resumable download. The engine's state (`EngineStatus`) is what the pane
//! and Settings read.

use std::time::{Duration, Instant};

use browser_pane::{ENGINE_DOWNLOAD_MB, EngineStatus};
use rune::Tx;

use crate::App;
use opencode_i18n::t;

/// How long the app sits idle before the download starts.
const IDLE: Duration = Duration::from_secs(30);
/// Polls this close together are frames: something animates, so the app isn't idle.
const FRAME_GAP: Duration = Duration::from_millis(100);
/// A gap this long between polls ends a stretch of motion (a spinner may skip a beat or two).
const STILL_GAP: Duration = Duration::from_secs(5);
/// Motion that has run this long without a break is ambient (a spinner on a session that waits
/// for an answer, say), not a transition: it no longer holds the download back, which runs at
/// background priority and throttled anyway.
const AMBIENT: Duration = Duration::from_secs(60);

/// When the background download may start. Rune polls on every frame and sleeps between them,
/// so polls coming at frame rate mean motion; between, one wake waits for the idle deadline.
#[derive(Default)]
pub struct IdleGate {
    /// The download was asked for this launch (or the gate is off until the setting changes).
    started: bool,
    armed_at: Option<Instant>,
    last_poll: Option<Instant>,
    /// When the current stretch of motion began, and its latest frame.
    moving_since: Option<Instant>,
    last_busy: Option<Instant>,
    wake_at: Option<Instant>,
}

impl IdleGate {
    /// Waits for the next idle stretch again (the setting went back on).
    pub fn rearm(&mut self) {
        *self = Self::default();
    }
}

impl App {
    /// Starts the background download once the app has been idle for `IDLE`, if it is wanted:
    /// a live window with the setting on and no engine on disk.
    pub(crate) fn poll_browser_prefetch(&mut self, tx: &mut Tx) {
        let wanted = self.persists()
            && !self.use_mock_browser()
            && self.settings.prefetch_browser
            && self.browser.host.status() == EngineStatus::NotInstalled;
        let gate = &mut self.browser.prefetch;
        if !wanted || gate.started {
            return;
        }
        let now = Instant::now();
        let gap = gate.last_poll.map(|t| now.duration_since(t));
        if gap.is_some_and(|g| g < FRAME_GAP) {
            gate.moving_since.get_or_insert(now);
            gate.last_busy = Some(now);
        } else if gap.is_none_or(|g| g >= STILL_GAP) {
            gate.moving_since = None;
        }
        gate.last_poll = Some(now);
        let armed = *gate.armed_at.get_or_insert(now);
        let transient = gate.moving_since.is_none_or(|t| now.duration_since(t) < AMBIENT);
        let motion = gate.last_busy.filter(|_| transient);
        let since = [Some(armed), rune::last_input(), motion].into_iter().flatten().max().unwrap_or(now);
        let due = since + IDLE;
        if now >= due {
            gate.started = true;
            self.browser.host.prefetch();
            return;
        }
        if gate.wake_at.is_none_or(|at| at <= now) {
            gate.wake_at = Some(due);
            tx.wake_after(due - now);
        }
    }

    pub fn set_prefetch_browser(&mut self, on: bool) {
        self.change_settings(|s| s.prefetch_browser = on);
        self.browser.prefetch.rearm();
        if !on {
            self.browser.host.stop_prefetch();
        }
    }
}

/// Settings' line under the toggle: what the background download is doing.
pub fn describe(status: &EngineStatus) -> String {
    let mb = |b: u64| (b as f64 / 1_000_000.0).round();
    match status {
        EngineStatus::Unsupported | EngineStatus::NotInstalled => t!("rune.browser.prefetch.idle", size = ENGINE_DOWNLOAD_MB),
        EngineStatus::Downloading { downloaded_bytes, total_bytes: Some(total) } if *total > 0 => {
            t!("rune.browser.prefetch.downloadingOf", done = mb(*downloaded_bytes), total = mb(*total))
        }
        EngineStatus::Downloading { downloaded_bytes, .. } => t!("rune.browser.prefetch.downloading", done = mb(*downloaded_bytes)),
        EngineStatus::Verifying => t!("rune.browser.prefetch.verifying").into(),
        EngineStatus::Paused(reason) => t!("rune.browser.prefetch.paused", reason = paused(*reason)),
        EngineStatus::Installed | EngineStatus::Connecting | EngineStatus::Ready => t!("rune.browser.prefetch.ready").into(),
        EngineStatus::Failed(f) if f.retry_at.is_some() => t!("rune.browser.prefetch.failedRetrying").into(),
        EngineStatus::Failed(_) => t!("rune.browser.prefetch.failed").into(),
    }
}

/// Why a download waits, in the current language.
fn paused(reason: browser_pane::PauseReason) -> &'static str {
    match reason {
        browser_pane::PauseReason::Metered => t!("rune.browser.prefetch.paused.metered"),
        browser_pane::PauseReason::OnBattery => t!("rune.browser.prefetch.paused.battery"),
    }
}
