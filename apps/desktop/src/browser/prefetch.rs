//! Readies the browser engine in the background (Settings › Download browser in the background),
//! so the first page opens without waiting for its download: once the app has sat idle for `IDLE`
//! (no input and nothing animating but ambient loops, see `AMBIENT`), `BrowserHost::prefetch`
//! starts the throttled, resumable download. The engine's state (`EngineStatus`) is what the pane
//! and Settings read.

use std::time::Duration;

use browser_pane::{ENGINE_DOWNLOAD_MB, EngineStatus};
use rune::Tx;

use crate::App;
use opencode_i18n::t;

/// How long the app sits idle before the download starts, in seconds.
const IDLE: f64 = 30.0;
/// Polls this close together are frames: something animates, so the app isn't idle.
const FRAME_GAP: f64 = 0.1;
/// A gap this long between polls ends a stretch of motion (a spinner may skip a beat or two).
const STILL_GAP: f64 = 5.0;
/// Motion that has run this long without a break is ambient (a spinner on a session that waits
/// for an answer, say), not a transition: it no longer holds the download back, which runs at
/// background priority and throttled anyway.
const AMBIENT: f64 = 60.0;

/// When the background download may start. Rune polls on every frame and sleeps between them,
/// so polls coming at frame rate mean motion; between, one wake waits for the idle deadline.
/// Times are the app's clock, in seconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum IdleGate {
    /// Not watched yet this launch, or again since the setting came back on.
    #[default]
    Unarmed,
    /// Watching for an idle stretch since `armed`.
    Watching { armed: f64, last_poll: f64, motion: Motion, wake_at: f64 },
    /// The download was asked for this launch.
    Started,
}

/// What the polls say moves on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// Nothing has moved since the gate armed.
    None,
    /// Frames come close together: a stretch that began at `since`, its latest at `last`.
    Moving { since: f64, last: f64 },
    /// The last stretch of motion ended; its latest frame was at `last`.
    Settled { last: f64 },
}

pub enum GateEvent {
    /// A poll at `now`, with the latest input at `input` and whether the download is wanted
    /// (a live window, the setting on, no engine on disk).
    Poll { now: f64, input: Option<f64>, wanted: bool },
    /// The setting went back on: watch for the next idle stretch.
    Rearm,
}

#[derive(Debug, PartialEq)]
pub enum GateEffect {
    Prefetch,
    WakeAfter(f64),
}

impl Motion {
    fn step(self, now: f64, gap: f64) -> Motion {
        match self {
            Motion::Moving { since, .. } if gap < FRAME_GAP => Motion::Moving { since, last: now },
            _ if gap < FRAME_GAP => Motion::Moving { since: now, last: now },
            Motion::Moving { last, .. } if gap >= STILL_GAP => Motion::Settled { last },
            other => other,
        }
    }

    /// When the motion that still holds the download back last moved: an ambient loop doesn't.
    fn holds(self, now: f64) -> Option<f64> {
        match self {
            Motion::None => None,
            Motion::Moving { since, last } => (now - since < AMBIENT).then_some(last),
            Motion::Settled { last } => Some(last),
        }
    }
}

impl IdleGate {
    pub fn step(self, event: GateEvent) -> (IdleGate, Vec<GateEffect>) {
        let (now, input, wanted) = match event {
            GateEvent::Rearm => return (IdleGate::Unarmed, Vec::new()),
            GateEvent::Poll { now, input, wanted } => (now, input, wanted),
        };
        let (armed, motion, wake_at) = match self {
            IdleGate::Started => return (IdleGate::Started, Vec::new()),
            _ if !wanted => return (self, Vec::new()),
            IdleGate::Unarmed => (now, Motion::None, f64::NEG_INFINITY),
            IdleGate::Watching { armed, last_poll, motion, wake_at } => (armed, motion.step(now, now - last_poll), wake_at),
        };
        let due = [Some(armed), input, motion.holds(now)].into_iter().flatten().fold(armed, f64::max) + IDLE;
        if now >= due {
            return (IdleGate::Started, vec![GateEffect::Prefetch]);
        }
        // One wake for the deadline; a new one only once it passed (an input or motion moved it).
        let (wake_at, effects) = if wake_at <= now { (due, vec![GateEffect::WakeAfter(due - now)]) } else { (wake_at, Vec::new()) };
        (IdleGate::Watching { armed, last_poll: now, motion, wake_at }, effects)
    }
}

impl App {
    /// Starts the background download once the app has been idle for `IDLE`, if it is wanted:
    /// a live window with the setting on and no engine on disk.
    pub(crate) fn poll_browser_prefetch(&mut self, tx: &mut Tx) {
        let wanted = self.persists() && !self.use_mock_browser() && self.settings.prefetch_browser && self.browser.host.status() == EngineStatus::NotInstalled;
        if !wanted && self.browser.prefetch == IdleGate::Unarmed {
            return;
        }
        let now = self.clock;
        let input = rune::last_input().map(|at| now - at.elapsed().as_secs_f64());
        let (gate, effects) = std::mem::take(&mut self.browser.prefetch).step(GateEvent::Poll { now, input, wanted });
        self.browser.prefetch = gate;
        for effect in effects {
            match effect {
                GateEffect::Prefetch => self.browser.host.prefetch(),
                GateEffect::WakeAfter(secs) => tx.wake_after(Duration::from_secs_f64(secs.max(0.0))),
            }
        }
    }

    pub fn set_prefetch_browser(&mut self, on: bool) {
        self.change_settings(|s| s.prefetch_browser = on);
        let (gate, _) = std::mem::take(&mut self.browser.prefetch).step(GateEvent::Rearm);
        self.browser.prefetch = gate;
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
