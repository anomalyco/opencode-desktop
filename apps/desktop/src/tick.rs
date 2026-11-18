//! Four-phase per-frame tick (`poll`), one-time window `setup`, and `visibility` lifecycle hooks.
//!
//! Decoupled feature systems run through `s.kernel.scheduler` in explicit phases (`Ingest`,
//! `Derive`, `React`, `Persist`) so new features never edit this file.

use opencode_kernel::Phase;
use opencode_kit::model::Delivery;
use rune::{Tx, Waker};

use crate::{
    App, Route, agent_rows, appearance, background, copied, crash_notice, deep_links, demo,
    diagram, disclosure, dropzone, fork, glitch, headless, inbox_model,
    incoming, jump_list, language, launch, layout_state, line_comments, line_reveal, message_nav,
    model_picker, narrow, notifications, plugin_host, popup, project_switcher, projects,
    reconnect, rune_mcp, servers, session_menu, session_state, session_status, settings, speech,
    status_toast, stop, storage, submit, telemetry, text_rendering, todos,
    turn_errors, view, wall_ms, windows, working,
};

#[derive(Default)]
struct PolledRuns(u64);

pub(crate) fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    session_view::scroller::set_device_scale(windows::view_scale(s));
    *s.clock = now;
    s.sync_history();
    *s.now_ms = if s.live() { wall_ms() } else { *s.base_ms + (now * 1000.0) as i64 };
    demo::poll(s);
    // The morph loop is for watching: the composer it lands on doesn't take the keys.
    if s.stage.is_some_and(|st| st.watch()) {
        s.composer.focused = false;
    }
    // The view ran since the last poll: state the polls below derive from other state may have moved.
    let runs = s.view_runs.get();
    let polled = s.kernel.slots.slot::<PolledRuns>();
    let viewed = runs != polled.0;
    polled.0 = runs;
    let scheduler = s.kernel.scheduler.clone();

    // ── Phase 1: Ingest ──────────────────────────────────────────────────────────────────────────
    // Pull external inputs, worker results, network events, timers, and registered Ingest systems
    // (`office::poll`, `updater::poll`, `dictation::poll`, `open_in::poll`).
    // Another window changed the session data this one shows.
    let mut changed = windows::peer::behind(s);
    changed |= session_menu::unread::poll(s);
    changed |= session_menu::preferences::poll(s);
    changed |= s.poll_places(now, tx);
    s.poll_undo(now, tx);
    reconnect::audit(s);
    popup::poll(s, tx);
    // The servers the window isn't checked out on: their launches, replies and streams.
    changed |= servers::runtime::poll(s, tx);
    changed |= servers::health::poll(s, tx);
    changed |= projects::home::poll(s, viewed || changed, tx);
    changed |= project_switcher::poll(s);
    // A project's image the icon worker found (or lost): the tiles that read the icons draw again.
    if rune::Tracked::untracked(&s.icons).take_found() {
        rune::Tracked::touch(&s.icons);
        changed = true;
    }
    changed |= launch::poll(s, tx);
    launch::trace::poll(tx);
    changed |= scheduler.run_phase(Phase::Ingest, s, now, tx);
    changed |= glitch::poll(s, tx);
    changed |= appearance::poll(s);
    text_rendering::poll(s, tx);
    changed |= language::poll(s, tx);
    changed |= settings::search::poll(s, tx);
    settings::pages::servers::unavailable::poll(s, tx);
    changed |= notifications::poll(s, tx);
    changed |= speech::poll(s, tx);
    // A shortcut from the composer that moved the keyboard into the side region (its command
    // may run a frame later) leaves the selected tab's label muted (`SideState::quiet`).
    let composing = s.composer.focused;
    if s.side.was_composing && !composing && s.side.focused && !s.side.pressed && s.terminal_focus().is_none() {
        s.side.quiet = true;
    }
    // Written only when it changes: a write to the side state rebuilds every region reading it.
    if (s.side.was_composing, s.side.pressed) != (composing, false) {
        (s.side.was_composing, s.side.pressed) = (composing, false);
    }
    changed |= deep_links::drain(s, tx);
    changed |= message_nav::poll(s, tx);
    changed |= turn_errors::poll(s, tx);
    changed |= dropzone::poll(s, tx);
    changed |= s.poll_pages(tx);
    while let Some(why) = s.output_errors.as_ref().and_then(|(_, rx)| rx.try_recv().ok()) {
        status_toast::fail(s, opencode_i18n::t!("common.requestFailed"), why);
        changed = true;
    }
    changed |= s.poll_context_window();
    changed |= s.poll_sends();
    changed |= s.tick_outbox(tx);
    changed |= s.poll_jobs(tx);
    let asked = s.dock_face();
    changed |= incoming::poll(s, tx);
    if s.playback.front().is_some_and(|e| e.0 <= now + 1e-9) {
        while s.playback.front().is_some_and(|e| e.0 <= now + 1e-9) {
            let (at, ty, mut data) = s.playback.pop_front().unwrap();
            if ty.starts_with("session.execution.") || ty == "session.archived" {
                data["eventCreated"] = (*s.base_ms + (at * 1000.0) as i64).into();
            }
            // A scripted delivery names the oldest prompt of its kind, which has no id until sent.
            if let Some(kind) = Delivery::parse(&data["next"])
                && let Some(item) = s.inboxes.get(data["sessionID"].as_str().unwrap_or("")).and_then(|i| i.items.iter().find(|i| i.delivery == kind))
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
            if let Some(sid) = data["sessionID"].as_str().filter(|sid| ty == "session.step.started" && s.route == Route::here(sid.to_string())) {
                s.refresh_fixture_page(sid);
            }
            s.live_event(ty, data);
            changed = true;
        }
    }
    // The next playback event is due by the clock: the window frames then too (not only when the
    // playback thread's wake arrives, which may be a frame later), as a timer asks.
    if let Some(&(at, _, _)) = s.playback.front() { tx.wake_at(at); }
    changed |= stop::poll(s, now, tx);
    changed |= s.revert_timeouts(tx);
    changed |= fork::poll(s, tx);
    changed |= message_nav::jump::poll(s, tx);
    s.queue_timers(tx);
    s.start_timers(tx);
    changed |= submit::poll(s, tx);

    // ── Phase 2: Derive ──────────────────────────────────────────────────────────────────────────
    // Recompute derived views, indices, and projections from ingested state.
    changed |= scheduler.run_phase(Phase::Derive, s, now, tx);
    changed |= working::sync(s);
    changed |= todos::sync(s, now, tx) | settings::pages::worktrees::poll(s, tx);
    if s.dock_face() != asked {
        s.morph(tx);
    }
    changed |= disclosure::poll(s, now, tx);
    changed |= session_view::timeline::projection_poll(s, tx);
    changed |= line_reveal::poll(s, now, tx);
    changed |= copied::poll(s, now, tx);
    changed |= diagram::poll(s, now, tx);
    changed |= session_view::images::poll(s, now, tx);
    changed |= model_picker::hover::poll(s, now, tx);
    changed |= agent_rows::poll(s, tx);
    changed |= background::poll(s, tx);
    changed |= s.poll_home_search(tx);
    // Ingest local answers before settling the list: its rows, highlight and empty state must
    // all see the same sources. Previously a full-text landing changed the rows after this poll.
    let local_answers = s.poll_session_search(now, tx) | s.poll_fulltext();
    changed |= s.poll_switcher(tx, local_answers);
    changed |= s.poll_mentions(tx);
    changed |= s.poll_commands(tx);
    changed |= s.poll_suggestions(tx);

    // ── Phase 3: React ───────────────────────────────────────────────────────────────────────────
    // Trigger follow-up actions, animations, focus shifts, UI reactions, and registered React
    // systems (`egg::poll`).
    changed |= scheduler.run_phase(Phase::React, s, now, tx);
    changed |= s.poll_rename(now, tx);
    changed |= s.poll_branch_picker(now, tx);
    changed |= s.poll_delete(now, tx);
    changed |= s.poll_uploads(tx);
    if s.attach.poll() {
        rune::Tracked::touch(&s.attach);
        changed = true;
    }
    changed |= s.poll_panels(viewed || changed, tx);
    changed |= rune_mcp::poll(s, tx);
    changed |= s.poll_turn_diffs();
    changed |= settings::pages::project::poll(s, tx);
    changed |= settings::pages::about::poll(s, now, tx);
    changed |= settings::pages::projects::poll(s);
    changed |= reconnect::poll(s, tx);
    changed |= session_state::poll(s, tx);
    changed |= status_toast::poll(s, now, tx);
    changed |= layout_state::poll(s, now, tx);
    changed |= narrow::poll(s, now, tx);
    changed |= inbox_model::poll(s, now, tx);
    changed |= view::tab_preview::poll(s, now, tx);
    changed |= session_status::poll(s, now, tx);
    // Each draft keeps its own composer in any window, saved or not (`restore::show_draft`).
    changed |= s.show_draft();

    // ── Phase 4: Persist ─────────────────────────────────────────────────────────────────────────
    // Flush dirty persistence state, window geometry, background caches, and registered Persist
    // systems (`updater::poll_owed`).
    changed |= scheduler.run_phase(Phase::Persist, s, now, tx);
    if s.persists() {
        changed |= s.tell_storage_notice();
        changed |= s.hydrate_restore();
        changed |= crash_notice::poll(s);
        s.poll_restore();
        s.poll_preview(false);
        changed |= s.poll_copies(tx);
        changed |= line_comments::poll_saved(s);
        windows::reopen::poll(s);
        windows::reopen::poll_asked(s);
        s.prompts.hydrate((*s.waker).clone());
        s.poll_window();
        s.poll_taskbar();
        jump_list::poll(s);
        // A window with its own id saves its frame itself (`windows`), as AppKit's one autosave
        // name can't tell windows apart; a hot build's one window isn't among them.
        if !*s.framed && (s.window.id.is_empty() || cfg!(feature = "hot")) {
            *s.framed = rune_mac::remember_frame("OpenCodeRuneMain");
        }
    }
    s.finish_tail_change(tx);
    changed
}

pub(crate) fn setup(s: &mut App, waker: Waker) {
    *s.waker = Some(waker.clone());
    storage::on_ready(waker.clone());
    // The outbox was opened before the window had a waker: its stored prompts wake it when they're in.
    s.listen_outbox();
    windows::peer::set_waker(s, waker.clone());
    s.attach.set_waker(waker.clone());
    s.icons.set_waker(waker.clone());
    s.home_search.set_waker(waker.clone());
    s.add.slash.set_waker(waker.clone());
    s.turn_diffs.set_waker(waker.clone());
    s.icons.set_sync(headless());
    s.setup_browser(waker.clone());
    rune::marks::mark("browser.setup");
    // Only the plugins this launch's first screen shows start before its first frame (`first_screen`).
    telemetry::startup("launch.plugins", || plugin_host::setup(s, waker.clone()));
    rune::marks::mark("plugins.started");
    s.setup_rune_mcp(waker.clone());
    rune::marks::mark("mcp.setup");
    let scheduler = s.kernel.scheduler.clone();
    scheduler.run_setup(s, waker);
    if !s.playback.is_empty() {
        s.wake_at_playback();
    }
    launch::setup(s);
}

/// A window shown or hidden: hidden, what it changed lands at once, without waiting for it
/// (Desktop flushes its stores on `visibilitychange` to hidden, `renderer/platform/storage.ts`).
pub(crate) fn visibility(s: &mut App, shown: bool, tx: &mut Tx) {
    if shown { s.reload_recent_on_focus(); }
    status_toast::visibility(s, shown, tx);
    let scheduler = s.kernel.scheduler.clone();
    scheduler.run_visibility(s, shown, tx);
    if !shown && s.persists() {
        s.poll_restore();
        s.poll_preview(true);
        storage::flush(opencode_storage::FlushReason::Hide, false);
    }
}
