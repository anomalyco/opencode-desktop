//! Four-phase per-frame tick (`poll`), canonical system order (`order`), one-time window `setup`,
//! and `visibility` lifecycle hooks.
//!
//! Decoupled feature and subsystem polls run through `s.kernel.scheduler` in explicit phases
//! (`Ingest`, `Derive`, `React`, `Persist`) ordered by [`order`], matching the pre-refactor
//! `poll` sequence (`31a59a9:apps/desktop/src/main.rs`) independently of feature registration order.

use opencode_kit::model::Delivery;
use rune::{Tx, Waker};

use crate::{
    App, Route, demo, headless, incoming, launch, plugin_host, stop, storage, submit, telemetry,
    wall_ms, windows,
};

/// Canonical per-frame poll order within each scheduler phase, spaced by 10, matching the
/// pre-refactor `poll` sequence (`31a59a9:apps/desktop/src/main.rs`).
pub(crate) mod order {
    pub(crate) mod ingest {
        pub const WINDOWS_PEER_BEHIND: u16 = 10;
        pub const SESSION_MENU_UNREAD: u16 = 20;
        pub const SESSION_MENU_PREFERENCES: u16 = 30;
        pub const RESTORE_PLACES: u16 = 40;
        pub const RESTORE_UNDO: u16 = 50;
        pub const RECONNECT_AUDIT: u16 = 60;
        pub const POPUP: u16 = 70;
        pub const SERVERS_RUNTIME: u16 = 80;
        pub const SERVERS_HEALTH: u16 = 90;
        pub const PROJECTS_HOME: u16 = 100;
        pub const PROJECT_SWITCHER: u16 = 110;
        pub const APPEARANCE_ICONS: u16 = 120;
        pub const DICTATION: u16 = 130;
        pub const LAUNCH: u16 = 140;
        pub const LAUNCH_TRACE: u16 = 150;
        pub const UPDATER: u16 = 160;
        #[cfg(feature = "office")]
        pub const OFFICE: u16 = 170;
        pub const GLITCH: u16 = 180;
        pub const EGG: u16 = 190;
        pub const APPEARANCE: u16 = 200;
        pub const TEXT_RENDERING: u16 = 210;
        pub const LANGUAGE: u16 = 220;
        pub const SETTINGS_SEARCH: u16 = 230;
        pub const SETTINGS_SERVERS_UNAVAILABLE: u16 = 240;
        pub const NOTIFICATIONS: u16 = 250;
        pub const SPEECH: u16 = 260;
        pub const PANELS_SYNC_COMPOSER: u16 = 270;
        pub const DEEP_LINKS: u16 = 280;
        pub const QUOTE: u16 = 290;
        pub const LINE_COMMENTS: u16 = 300;
        pub const HISTORY_MESSAGE_NAV: u16 = 310;
        pub const TURN_ERRORS: u16 = 320;
        pub const COMPOSER_DROPZONE: u16 = 330;
        pub const SESSION_PAGES: u16 = 340;
        pub const APP_OUTPUT_ERRORS: u16 = 350;
        pub const CONTEXT_WINDOW: u16 = 360;
        pub const INBOX_SENDS: u16 = 370;
        pub const INBOX_OUTBOX: u16 = 380;
        pub const OPEN_IN: u16 = 390;
        pub const JOBS: u16 = 400;
        pub const FILES_REPLY_LINKS: u16 = 410;
        pub const TICK_STREAM: u16 = 420;
        pub const HISTORY_REWIND: u16 = 430;
        pub const HISTORY_FORK: u16 = 440;
        pub const HISTORY_JUMP: u16 = 450;
        pub const TICK_SUBMIT: u16 = 460;
    }

    pub(crate) mod derive {
        pub const WORKING_SYNC: u16 = 10;
        pub const SUBAGENTS_SHELLS: u16 = 20;
        pub const TIMELINE_TODOS: u16 = 30;
        pub const SETTINGS_WORKTREES: u16 = 40;
        pub const TIMELINE_DOCK_MORPH: u16 = 50;
        pub const DISCLOSURE: u16 = 60;
        pub const TIMELINE_PROJECTION: u16 = 70;
        pub const LINE_REVEAL: u16 = 80;
        pub const COPIED: u16 = 90;
        pub const DIAGRAM: u16 = 100;
        pub const TIMELINE_IMAGES: u16 = 110;
        pub const MODEL_PICKER_HOVER: u16 = 120;
        pub const TIMELINE_AGENT_ROWS: u16 = 130;
        pub const BACKGROUND: u16 = 140;
        pub const FIND: u16 = 150;
        pub const HOME_SEARCH: u16 = 160;
        pub const SWITCHER: u16 = 170;
        pub const MENTIONS: u16 = 180;
        pub const COMPOSER_COMMANDS: u16 = 190;
        pub const COMPOSER_SUGGESTIONS: u16 = 200;
        pub const SESSION_MENU_RENAME: u16 = 210;
        pub const BRANCH_PICKER: u16 = 220;
        pub const SESSION_MENU_DELETE: u16 = 230;
        pub const COMPOSER_UPLOADS: u16 = 240;
    }

    pub(crate) mod react {
        pub const COMPOSER_ATTACH: u16 = 10;
        pub const PANELS: u16 = 20;
        pub const RUNE_MCP: u16 = 30;
        pub const TURN_DIFFS: u16 = 40;
        pub const SETTINGS_PROJECT: u16 = 50;
        pub const SETTINGS_ABOUT: u16 = 60;
        pub const SETTINGS_PROJECTS: u16 = 70;
        pub const RECONNECT: u16 = 80;
        pub const SESSION_STATE: u16 = 90;
        pub const TIPS: u16 = 100;
        pub const STATUS_TOAST: u16 = 110;
        pub const LAYOUT_STATE: u16 = 120;
        pub const NARROW: u16 = 130;
        pub const INBOX_MODEL: u16 = 140;
        pub const TAB_PREVIEW: u16 = 150;
        pub const SESSION_STATUS: u16 = 160;
        pub const RESTORE_SHOW_DRAFT: u16 = 170;
    }

    pub(crate) mod persist {
        pub const UPDATER_OWED: u16 = 10;
        pub const RESTORE_PERSIST: u16 = 20;
        pub const ANNOTATIONS_SAVED: u16 = 30;
        pub const WINDOWS_PERSIST: u16 = 40;
    }
}

/// Per-frame tick handoff slot carrying the pre-stream `dock_face` snapshot from `tick.stream`
/// into Derive's `timeline.dock_morph`.
#[derive(Default)]
struct TickFrame {
    asked_dock_face: Option<String>,
}

impl App {
    pub(crate) fn take_asked_dock_face(&mut self) -> Option<String> {
        self.kernel.slots.slot::<TickFrame>().asked_dock_face.take()
    }
}

/// Snapshots `dock_face`, drains live SSE/WS events and due fixture playback events, and runs
/// `stop::poll` at the exact mid-ingest boundary (`31a59a9:apps/desktop/src/main.rs`).
pub(crate) fn poll_stream(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let asked = s.dock_face();
    s.kernel.slots.slot::<TickFrame>().asked_dock_face = asked;
    let mut changed = incoming::poll(s, tx);
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
    changed
}

/// Arms queue/start timers and runs `submit::poll` right after rewind/fork/jump polls.
pub(crate) fn poll_submit(s: &mut App, tx: &mut Tx) -> bool {
    s.queue_timers(tx);
    s.start_timers(tx);
    submit::poll(s, tx)
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
    let scheduler = s.kernel.scheduler.clone();
    let changed = scheduler.run_all(s, now, tx);
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
