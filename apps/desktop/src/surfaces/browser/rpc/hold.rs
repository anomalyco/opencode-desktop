//! Explicit per-tab agent RPC unthrottle hold state machine with 30 s cooldown
//! (matching official `packages/gui-plugins/src/browser/agent-rendering.ts`).
//!
//! Decouples background rendering unthrottle from security-sticky `agent_driven` so idle
//! agent tabs return to 1 FPS + `was_hidden(1)` + released GPU textures once work finishes.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use opencode_kit::servers::ServerKey;
use rune::time::Instant;

use super::session::SessionAttachment;
use crate::App;

/// Official agent rendering cooldown (`packages/gui-plugins/src/browser/agent-rendering.ts`).
pub const COOLDOWN: Duration = Duration::from_secs(30);

/// Explicit hold lifecycle for one browser tab driven by agent RPC.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HoldState {
    #[default]
    Idle,
    Active,
    Cooldown { until: Instant },
}

/// Input event driving [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldEvent {
    Sync { active: bool, now: Instant },
}

/// Pure effects emitted by [`step`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HoldEffects {
    pub set_unthrottled: Option<bool>,
    pub wake_at: Option<Instant>,
}

/// Pure transition function for [`HoldState`].
pub fn step(state: &mut HoldState, event: HoldEvent) -> HoldEffects {
    let HoldEvent::Sync { active, now } = event;
    match (*state, active) {
        (HoldState::Idle, true) => {
            *state = HoldState::Active;
            HoldEffects { set_unthrottled: Some(true), wake_at: None }
        }
        (HoldState::Idle, false) | (HoldState::Active, true) => HoldEffects::default(),
        (HoldState::Active, false) => {
            let until = now + COOLDOWN;
            *state = HoldState::Cooldown { until };
            HoldEffects { set_unthrottled: None, wake_at: Some(until) }
        }
        (HoldState::Cooldown { .. }, true) => {
            *state = HoldState::Active;
            HoldEffects::default()
        }
        (HoldState::Cooldown { until }, false) if now >= until => {
            *state = HoldState::Idle;
            HoldEffects { set_unthrottled: Some(false), wake_at: None }
        }
        (HoldState::Cooldown { until }, false) => {
            HoldEffects { set_unthrottled: None, wake_at: Some(until) }
        }
    }
}

/// Tracks per-tab [`HoldState`] entries across all attached RPC sessions.
#[derive(Default)]
pub(super) struct RpcHoldManager {
    states: BTreeMap<u64, HoldState>,
}

impl RpcHoldManager {
    /// Recomputes active RPC holds across all sessions, steps each tab's [`HoldState`],
    /// forwards transitions to `BrowserHost::set_tab_unthrottled`, and returns the earliest
    /// cooldown deadline to schedule on `tx.wake_after`.
    pub fn sync(
        &mut self,
        app: &mut App,
        sessions: &mut BTreeMap<(ServerKey, String), SessionAttachment>,
        now: Instant,
    ) -> Option<Instant> {
        let mut active_tabs: HashSet<u64> = HashSet::new();
        for att in sessions.values_mut() {
            for hid in att.hold_touched.drain(..) {
                if app.workspace_panes.browser.host.tab(hid).is_some() {
                    active_tabs.insert(hid);
                }
            }
            for cmd in &att.active_commands {
                active_tabs.insert(cmd.target_host_tab());
            }
            if let Some(rec) = &att.trace_recording
                && rec.auto_stopped.is_none()
            {
                active_tabs.insert(rec.host_id);
            }
            for (&hid, rec) in &att.cpu_started {
                if rec.auto_stopped.is_none() {
                    active_tabs.insert(hid);
                }
            }
            for &hid in att.heap_chunks.keys() {
                active_tabs.insert(hid);
            }
            if app.session_busy(&att.session_id) {
                for &hid in att.host_to_wire.keys() {
                    if app
                        .workspace_panes
                        .browser
                        .host
                        .tab(hid)
                        .is_some_and(|t| t.prompts.agent_driven)
                    {
                        active_tabs.insert(hid);
                    }
                }
            }
        }

        // Drop state for closed tabs; `BrowserHost::close_tab` / `TabClosed` already cleaned up.
        self.states
            .retain(|&hid, _| app.workspace_panes.browser.host.tab(hid).is_some());
        active_tabs.retain(|&hid| app.workspace_panes.browser.host.tab(hid).is_some());

        let mut all_tabs: Vec<u64> = self.states.keys().copied().collect();
        for &hid in &active_tabs {
            if !self.states.contains_key(&hid) {
                all_tabs.push(hid);
            }
        }

        let mut earliest_wake: Option<Instant> = None;
        for hid in all_tabs {
            let active = active_tabs.contains(&hid);
            let state = self.states.entry(hid).or_default();
            let eff = step(state, HoldEvent::Sync { active, now });
            if let Some(unthrottled) = eff.set_unthrottled {
                app.workspace_panes.browser.host.set_tab_unthrottled(hid, unthrottled);
            }
            if let Some(w) = eff.wake_at {
                earliest_wake = Some(earliest_wake.map_or(w, |cur| cur.min(w)));
            }
            if *state == HoldState::Idle {
                self.states.remove(&hid);
            }
        }
        earliest_wake
    }
}
