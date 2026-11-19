//! Four-phase per-frame tick (`poll`), one-time window `setup`, and `visibility` lifecycle hooks.
//!
//! Decoupled feature and subsystem polls run through `s.kernel.scheduler` in explicit phases
//! (`Ingest`, `Derive`, `React`, `Persist`) registered in `kernel.rs` and each `Feature::register`,
//! keeping this file focused on frame timing, live/fixture stream dispatch, and window lifecycle.

use opencode_kernel::Phase;
use opencode_kit::model::Delivery;
use rune::{Tx, Waker};

use crate::{
    App, Route, demo, headless, incoming, launch, plugin_host, stop, storage, submit, telemetry,
    wall_ms, windows,
};

/// Per-frame tick handoff slot read by registered `LifecycleScheduler` systems so polls preserve
/// exact `viewed || changed` freshness and pre-ingest `dock_face` transitions.
#[derive(Default)]
struct TickFrame {
    polled_runs: u64,
    fresh: bool,
    asked_dock_face: Option<String>,
}

impl App {
    pub(crate) fn tick_fresh(&self) -> bool {
        self.kernel.slots.get::<TickFrame>().is_some_and(|f| f.fresh)
    }

    pub(crate) fn note_tick_change(&mut self, changed: bool) -> bool {
        if changed {
            self.kernel.slots.slot::<TickFrame>().fresh = true;
        }
        changed
    }

    pub(crate) fn take_asked_dock_face(&mut self) -> Option<String> {
        self.kernel.slots.slot::<TickFrame>().asked_dock_face.take()
    }
}

pub(crate) fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    session_view::scroller::set_device_scale(windows::view_scale(s));
    *s.clock = now;
    s.sync_history();
    *s.now_ms = if s.live() { wall_ms() } else { *s.fixture_run.base_ms + (now * 1000.0) as i64 };
    demo::poll(s);
    // The morph loop is for watching: the composer it lands on doesn't take the keys.
    if s.fixture_run.stage.is_some_and(|st| st.watch()) {
        s.composer.focused = false;
    }
    // The view ran since the last poll: state the polls below derive from other state may have moved.
    let runs = s.view_runs();
    let frame = s.kernel.slots.slot::<TickFrame>();
    let viewed = runs != frame.polled_runs;
    frame.polled_runs = runs;
    frame.fresh = viewed;
    let scheduler = s.kernel.scheduler.clone();

    // ── Phase 1: Ingest ──────────────────────────────────────────────────────────────────────────
    // Pull external inputs, worker results, timers, and registered Ingest systems, then dispatch
    // live SSE/WS events and scripted fixture playback.
    let mut changed = scheduler.run_phase(Phase::Ingest, s, now, tx);
    let asked = s.dock_face();
    s.kernel.slots.slot::<TickFrame>().asked_dock_face = asked;
    changed |= incoming::poll(s, tx);
    while s.fixture_run.playback.front().is_some_and(|e| e.0 <= now + 1e-9) {
        let Some((at, ty, mut data)) = s.fixture_run.playback.pop_front() else { break };
        if ty.starts_with("session.execution.") || ty == "session.archived" {
            data["eventCreated"] = (*s.fixture_run.base_ms + (at * 1000.0) as i64).into();
        }
        // A scripted delivery names the oldest prompt of its kind, which has no id until sent.
        if let Some(kind) = Delivery::parse(&data["next"])
            && let Some(item) = s
                .inboxes
                .get(data["sessionID"].as_str().unwrap_or(""))
                .and_then(|i| i.items.iter().find(|i| i.delivery == kind))
        {
            data["inboxID"] = item.id.clone().into();
        }
        if ty.starts_with("demo.") {
            demo::apply(s, &ty, &data, tx);
            changed = true;
            continue;
        }
        if ty == "fixture.page" {
            s.show_fixture_page(data["sessionID"].as_str().unwrap_or(""), tx);
            changed = true;
            continue;
        }
        if let Some(sid) = data["sessionID"]
            .as_str()
            .filter(|sid| ty == "session.step.started" && s.route == Route::here(sid.to_string()))
        {
            s.refresh_fixture_page(sid);
        }
        s.live_event(ty, data);
        changed = true;
    }
    // The next playback event is due by the clock: the window frames then too (not only when the
    // playback thread's wake arrives, which may be a frame later), as a timer asks.
    if let Some(&(at, _, _)) = s.fixture_run.playback.front() {
        tx.wake_at(at);
    }
    changed |= stop::poll(s, now, tx);
    s.queue_timers(tx);
    s.start_timers(tx);
    changed |= submit::poll(s, tx);

    // ── Phase 2: Derive ──────────────────────────────────────────────────────────────────────────
    changed |= scheduler.run_phase(Phase::Derive, s, now, tx);

    // ── Phase 3: React ───────────────────────────────────────────────────────────────────────────
    s.kernel.slots.slot::<TickFrame>().fresh = viewed || changed;
    changed |= scheduler.run_phase(Phase::React, s, now, tx);

    // ── Phase 4: Persist ─────────────────────────────────────────────────────────────────────────
    changed |= scheduler.run_phase(Phase::Persist, s, now, tx);
    s.finish_tail_change(tx);
    changed
}

pub(crate) fn setup(s: &mut App, waker: Waker) {
    *s.waker = Some(waker.clone());
    storage::on_ready(waker.clone());
    // The outbox was opened before the window had a waker: its stored prompts wake it when they're in.
    s.listen_outbox();
    windows::peer::set_waker(s, waker.clone());
    s.appearance_state.icons.set_waker(waker.clone());
    s.turn_diffs.set_waker(waker.clone());
    s.appearance_state.icons.set_sync(headless());
    s.setup_browser(waker.clone());
    rune::marks::mark("browser.setup");
    // Only the plugins this launch's first screen shows start before its first frame (`first_screen`).
    telemetry::startup("launch.plugins", || plugin_host::setup(s, waker.clone()));
    rune::marks::mark("plugins.started");
    let scheduler = s.kernel.scheduler.clone();
    scheduler.run_setup(s, waker);
    if !s.fixture_run.playback.is_empty() {
        s.wake_at_playback();
    }
    launch::setup(s);
}

/// A window shown or hidden: hidden, what it changed lands at once, without waiting for it
/// (Desktop flushes its stores on `visibilitychange` to hidden, `renderer/platform/storage.ts`).
pub(crate) fn visibility(s: &mut App, shown: bool, tx: &mut Tx) {
    if shown {
        s.reload_recent_on_focus();
    }
    let scheduler = s.kernel.scheduler.clone();
    scheduler.run_visibility(s, shown, tx);
    if !shown && s.persists() {
        s.poll_restore();
        s.poll_preview(true);
        storage::flush(opencode_storage::FlushReason::Hide, false);
    }
}
