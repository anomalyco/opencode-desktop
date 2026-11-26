//! Memory reclamation across active and parked servers: runs on `App::rest`, immediately on
//! tab close / tab switch, and on a quiet 30-second periodic sweep so active animations (such as
//! the Thinking spinner or blinking caret) never block memory reclamation indefinitely.

use std::collections::HashSet;
use std::time::Duration;

use opencode_kit::servers::ServerKey;

use crate::{App, Route, files, windows};

/// Periodic quiet sweep cadence while animations prevent `App::on_rest` from firing.
const SWEEP_INTERVAL_MS: i64 = 30_000;
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Explicit state machine for the periodic quiet memory sweep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MemorySweep {
    #[default]
    Idle,
    Scheduled { due_ms: i64 },
}

enum SweepEvent {
    Poll { now_ms: i64 },
    Ran { now_ms: i64 },
}

enum SweepEffect {
    None,
    Schedule,
    RunAndSchedule,
}

fn step_sweep(state: &mut MemorySweep, event: SweepEvent) -> SweepEffect {
    match (*state, event) {
        (MemorySweep::Idle, SweepEvent::Poll { now_ms }) => {
            *state = MemorySweep::Scheduled { due_ms: now_ms.saturating_add(SWEEP_INTERVAL_MS) };
            SweepEffect::Schedule
        }
        (MemorySweep::Scheduled { due_ms }, SweepEvent::Poll { now_ms }) if now_ms >= due_ms => {
            *state = MemorySweep::Scheduled { due_ms: now_ms.saturating_add(SWEEP_INTERVAL_MS) };
            SweepEffect::RunAndSchedule
        }
        (MemorySweep::Scheduled { .. }, SweepEvent::Poll { .. }) => SweepEffect::None,
        (_, SweepEvent::Ran { now_ms }) => {
            *state = MemorySweep::Scheduled { due_ms: now_ms.saturating_add(SWEEP_INTERVAL_MS) };
            SweepEffect::Schedule
        }
    }
}

pub(crate) static SWEEP: opencode_kernel::Slot<MemorySweep> = opencode_kernel::Slot::new();

/// Quiet periodic sweep called from `tick::poll`: reclaims closed sessions and enforces decoded
/// image budgets across active and parked servers without mutating clean `Tracked` cells, forcing
/// a redraw, or disrupting `rune`'s `on_rest` idle timer.
pub(crate) fn poll(s: &mut App, tx: &mut rune::Tx) {
    let now_ms = *s.now_ms;
    let effect = step_sweep(s.kernel.slots.get_or_default(&SWEEP), SweepEvent::Poll { now_ms });
    match effect {
        SweepEffect::None => {}
        SweepEffect::Schedule => tx.wake_after(SWEEP_INTERVAL),
        SweepEffect::RunAndSchedule => {
            sweep_all(s, false);
            tx.wake_after(SWEEP_INTERVAL);
        }
    }
}

/// `App::on_rest`: full rest across the active server and every parked server, plus registered
/// rest hooks and `mimalloc` purging.
pub(crate) fn rest_all(s: &mut App, _: &mut rune::Tx) -> bool {
    let now_ms = *s.now_ms;
    let _ = step_sweep(s.kernel.slots.get_or_default(&SWEEP), SweepEvent::Ran { now_ms });
    sweep_all(s, true);
    false
}

/// Immediate cleanup when switching to another tab: closes any open full-resolution lightbox and
/// trims background decoded image pixels down to rest budgets.
pub(crate) fn on_tab_switched(s: &mut App) {
    s.composer_ext.attach.store.on_tab_switch();
}

/// Immediate cleanup when closing a session tab: unloads its transcript, page cache, review diffs,
/// open file previews, turn diffs, context window, and decoded pixels if no other tab, window, or
/// running job still needs it, without waiting for 10 s of zero animation frames.
pub(crate) fn on_tab_closed(s: &mut App, server: &ServerKey, id: &str) {
    let id = id.to_owned();
    let other_unreported = windows::any_other_window_unreported(&s.window.id);
    let mut dropped_ids = Vec::new();
    let _ = crate::servers::runtime::with_server(s, server, |s| {
        s.composer_ext.attach.store.on_tab_switch();
        let active_server = crate::servers::runtime::active(s).clone();
        let other_shown = windows::other_windows_shown(&s.window.id, &active_server);
        let kept_here = |sid: &str| {
            s.tabs.iter().any(|t| !t.is_draft() && t.server == active_server && t.id == sid)
                || matches!(&*s.route, Route::Session { server: srv, id: rid } if *srv == active_server && rid == sid)
                || s.running.iter().any(|r| r == sid)
                || other_shown.iter().any(|r| r == sid)
        };
        if kept_here(&id) {
            return;
        }
        let subagents: Vec<String> = s
            .transcripts
            .keys()
            .filter(|sid| *sid != &id && s.ancestors(sid).any(|a| a == id) && !kept_here(sid))
            .cloned()
            .collect();
        for sid in std::iter::once(id.clone()).chain(subagents) {
            s.pages.forget(&sid);
            if !other_unreported {
                s.transcripts.remove(&sid);
            }
            s.context_windows.forget(active_server.as_str(), &sid);
            dropped_ids.push(sid);
        }
        s.composer_ext.attach.store.rest();
    });
    // Drop server-agnostic per-window slots only for sessions not open under another server key.
    for sid in &dropped_ids {
        let open_elsewhere = s.tabs.iter().any(|t| !t.is_draft() && t.id == *sid)
            || matches!(&*s.route, Route::Session { id: rid, .. } if rid == sid);
        if open_elsewhere {
            continue;
        }
        s.kernel.slots.get_mut(&crate::review::REVIEW).sessions.remove(sid);
        {
            let f = s.kernel.slots.get_mut(&files::FILES);
            f.sessions.remove(sid);
            f.reads.forget_session(sid);
            files::watch::forget_session(s, sid);
        }
        s.turn_diffs.forget(sid);
        s.timeline_fx.ends.forget(sid);
        s.kernel.slots.get_mut(&crate::panels::SIDE).sessions.remove(sid);
    }
    if !other_unreported && !dropped_ids.is_empty() {
        retain_global_user_messages(s);
    }
    #[cfg(not(target_arch = "wasm32"))]
    crate::purge_free_pages();
}

fn sweep_all(s: &mut App, full_rest: bool) {
    let scheduler = s.kernel.scheduler.clone();
    let has_rest_hooks = scheduler.rest_names().next().is_some();
    let other_unreported = windows::any_other_window_unreported(&s.window.id);

    // Sweep every server half this window holds (active + parked) using per-server tab visibility.
    let initial_active = crate::servers::runtime::active(s).clone();
    let keys = s.checkout.keys();
    let mut all_kept_ids: HashSet<String> = HashSet::new();
    let mut all_keep_dirs: HashSet<String> = HashSet::new();
    let mut removed_any = false;
    for key in &keys {
        let is_initial = *key == initial_active;
        let _ = crate::servers::runtime::with_server(s, key, |s| {
            let (kept_ids, keep_dirs, removed) = sweep_current_server(s, other_unreported, full_rest, is_initial);
            all_kept_ids.extend(kept_ids);
            all_keep_dirs.extend(keep_dirs);
            removed_any |= removed;
        });
    }

    // Prune per-window caches only when there are actually stale entries (avoids dirtying Tracked
    // cells during quiet 30 s sweeps).
    let review_gone: Vec<String> = s
        .kernel
        .slots
        .get(&crate::review::REVIEW)
        .sessions
        .keys()
        .filter(|id| !all_kept_ids.contains(*id))
        .cloned()
        .collect();
    if full_rest || !review_gone.is_empty() {
        let shown_refs: HashSet<&str> = all_kept_ids.iter().map(String::as_str).collect();
        let review = s.kernel.slots.get_mut(&crate::review::REVIEW);
        for id in &review_gone {
            review.sessions.remove(id);
        }
        review.prune_cache(&shown_refs, &all_keep_dirs);
    }
    let files_gone: Vec<String> = s
        .kernel
        .slots
        .get(&files::FILES)
        .sessions
        .keys()
        .filter(|id| !all_kept_ids.contains(*id))
        .cloned()
        .collect();
    if full_rest || !files_gone.is_empty() {
        s.kernel.slots.get_mut(&files::FILES).retain_reply_links(|id| all_kept_ids.contains(id));
        for id in &files_gone {
            let f = s.kernel.slots.get_mut(&files::FILES);
            f.sessions.remove(id);
            f.reads.forget_session(id);
            files::watch::forget_session(s, id);
        }
    }
    if full_rest || removed_any {
        s.turn_diffs.retain(&|id| all_kept_ids.contains(id));
        s.timeline_fx.ends.retain(&|id| all_kept_ids.contains(id));
        if s.kernel.slots.get(&crate::panels::SIDE).sessions.keys().any(|id| !all_kept_ids.contains(id)) {
            s.kernel.slots.get_mut(&crate::panels::SIDE).sessions.retain(|id, _| all_kept_ids.contains(id));
        }
    }

    if !other_unreported && (full_rest || removed_any) {
        retain_global_user_messages(s);
    }

    if full_rest && s.tabs.is_empty() && !matches!(&*s.route, Route::Session { .. } | Route::Draft(_)) {
        s.timeline_fx.diagram.rest();
        s.kernel.slots.get(&crate::app::MATH).rest();
    }

    if has_rest_hooks && (full_rest || removed_any) {
        scheduler.run_rest(s, &|id| all_kept_ids.contains(id));
    }

    #[cfg(not(target_arch = "wasm32"))]
    crate::purge_free_pages();
}

fn sweep_current_server(s: &mut App, other_unreported: bool, full_rest: bool, is_initial: bool) -> (HashSet<String>, HashSet<String>, bool) {
    let active_server = crate::servers::runtime::active(s).clone();
    let other_shown = windows::other_windows_shown(&s.window.id, &active_server);
    let route_session = match &*s.route {
        Route::Session { server, id } if *server == active_server => Some(id.as_str()),
        _ => None,
    };
    let any_tab_here = s.tabs.iter().any(|t| t.server == active_server);
    let draft_route_here = is_initial && matches!(&*s.route, Route::Draft(_));
    let session_tabs_here: Vec<&str> = s
        .tabs
        .iter()
        .filter(|t| !t.is_draft() && t.server == active_server)
        .map(|t| t.id.as_str())
        .collect();
    let shown: HashSet<&str> = session_tabs_here
        .iter()
        .copied()
        .chain(route_session)
        .chain(s.running.iter().map(String::as_str))
        .chain(other_shown.iter().map(String::as_str))
        .collect();
    let kept = |id: &str| s.ancestors(id).any(|a| shown.contains(a));
    let gone: Vec<String> = if other_unreported {
        Vec::new()
    } else {
        s.transcripts.keys().filter(|id| !kept(id)).cloned().collect()
    };
    let kept_ids: HashSet<String> = shown
        .iter()
        .map(|id| (*id).to_owned())
        .chain(s.sessions.iter().filter(|sess| kept(&sess.id)).map(|sess| sess.id.clone()))
        .collect();
    let removed = !gone.is_empty();
    if full_rest || removed {
        s.pages.rest(&kept);
        s.context_windows.retain(active_server.as_str(), &kept);
    }
    if removed {
        let transcripts = &mut *s.transcripts;
        for id in &gone {
            transcripts.remove(id);
        }
    }
    let keep_dirs: HashSet<String> = s
        .sessions
        .iter()
        .filter(|sess| kept_ids.contains(&sess.id))
        .map(|sess| crate::review::dir_key(&sess.directory))
        .filter(|d| !d.is_empty())
        .collect();

    // Active quiet sweep keeps full active viewport pixel budget on servers with open tabs;
    // idle full rest (or servers with no open tabs) trims down to rest budgets / drops all pixels.
    if full_rest {
        s.composer_ext.attach.store.rest();
        if !any_tab_here && route_session.is_none() && !draft_route_here {
            s.composer_ext.attach.store.rest_pixels();
        }
    } else if !any_tab_here && route_session.is_none() && !draft_route_here {
        s.composer_ext.attach.store.rest_pixels();
    } else {
        s.composer_ext.attach.store.trim_active();
    }
    (kept_ids, keep_dirs, removed)
}

fn retain_global_user_messages(s: &App) {
    let active_server = crate::servers::runtime::active(s).clone();
    let cached = s.pages.user_message_ids();
    let parked = crate::servers::runtime::parked_user_message_ids(s);
    let other_cached = windows::other_windows_cached_user_messages(&s.window.id);
    let other_peer_msgs = crate::servers::runtime::other_peer_user_message_ids(&active_server);
    let inbox_ids: Vec<&str> = s.inboxes.values().flat_map(|i| i.items.iter().map(|it| it.id.as_str())).collect();
    let messages: HashSet<&str> = s
        .transcripts
        .values()
        .flat_map(|t| t.messages.iter())
        .filter_map(|m| match m {
            opencode_kit::model::Message::User { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .chain(cached.iter().map(String::as_str))
        .chain(parked.iter().map(String::as_str))
        .chain(other_cached.iter().map(String::as_str))
        .chain(other_peer_msgs.iter().map(String::as_str))
        .chain(inbox_ids)
        .collect();
    opencode_kit::model::retain_user_messages(&|id| messages.contains(id));
}
