//! Explicit per-tab visibility and frame-throttling state machine for `EngineManager`.
//!
//! Centralizes `was_hidden`, windowless FPS throttling (1 FPS hidden vs display-matched
//! `pane_fps()` awake), and Windows D3D11 / Linux dmabuf GPU frame ring release across
//! `SetVisibleTabs`, legacy `FocusTab`, `SetTabUnthrottled`, `open_tab`, and `ExtEffect::FocusTab`.

use std::sync::atomic::Ordering;

use cef::{ImplBrowserHost, PaintElementType};

use super::{EngineManager, ManagedTab};

/// Whether a managed tab is actively rendering (`Awake`) or throttled with released GPU frames (`Hidden`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TabAwakeState {
    #[default]
    Hidden,
    Awake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AwakeEffect {
    None,
    Wake,
    Sleep,
}

fn step_awake(state: &mut TabAwakeState, want_awake: bool) -> AwakeEffect {
    match (*state, want_awake) {
        (TabAwakeState::Hidden, true) => {
            *state = TabAwakeState::Awake;
            AwakeEffect::Wake
        }
        (TabAwakeState::Awake, false) => {
            *state = TabAwakeState::Hidden;
            AwakeEffect::Sleep
        }
        (TabAwakeState::Awake, true) | (TabAwakeState::Hidden, false) => AwakeEffect::None,
    }
}

fn apply_awake_effect(id: u64, t: &mut ManagedTab, visible: bool, effect: AwakeEffect) {
    #[cfg(target_os = "linux")]
    {
        t.shown = visible;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = visible;

    match effect {
        AwakeEffect::None => {}
        AwakeEffect::Wake => {
            let fps = if crate::input_fps::is_boosted(id) {
                crate::input_fps::AGENT_INPUT_FPS.max(crate::input_fps::pane_fps())
            } else {
                crate::input_fps::pane_fps()
            };
            t.host.set_windowless_frame_rate(fps);
            t.host.was_hidden(0);
            t.host.invalidate(PaintElementType::default());
        }
        AwakeEffect::Sleep => {
            crate::input_fps::clear(id);
            t.host.set_focus(0);
            t.host.was_hidden(1);
            t.host.set_windowless_frame_rate(1);
            #[cfg(windows)]
            if let Some(ring) = &t.frames {
                ring.lock().unwrap().release_slots();
            }
            #[cfg(target_os = "linux")]
            if let Some(ring) = &t.frames {
                let _ = ring.lock().unwrap().step(crate::dmabuf_frames::Event::Released);
            }
        }
    }
}

impl EngineManager {
    /// Whether any managed tab is currently awake (used on macOS to relax the idle run-loop pump).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn has_awake_tabs(&self) -> bool {
        self.tabs.values().any(|t| t.awake_state == TabAwakeState::Awake)
    }

    pub(super) fn handle_set_visible_tabs(&mut self, tab_ids: Vec<u64>) {
        self.client_uses_lifecycle = true;
        self.visible_tabs = tab_ids.into_iter().collect();
        self.sync_tab_visibility();
    }

    pub(super) fn handle_set_tab_unthrottled(&mut self, tab_id: u64, active: bool) {
        self.client_uses_lifecycle = true;
        if let Some(t) = self.tabs.get_mut(&tab_id) {
            t.rpc_unthrottled = active;
        }
        self.sync_tab_visibility();
    }

    pub(super) fn handle_focus_tab(&mut self, tab_id: u64) {
        let was_awake = self
            .tabs
            .get(&tab_id)
            .is_some_and(|t| t.awake_state == TabAwakeState::Awake);
        if !self.client_uses_lifecycle {
            self.visible_tabs.clear();
            self.visible_tabs.insert(tab_id);
        }
        for (&id, t) in &self.tabs {
            t.host.set_focus(i32::from(id == tab_id));
        }
        self.sync_tab_visibility();
        // If already awake, `sync_tab_visibility` emitted `None`; keep legacy repaint on explicit FocusTab.
        if was_awake
            && let Some(t) = self.tabs.get(&tab_id)
            && t.awake_state == TabAwakeState::Awake
        {
            t.host.invalidate(PaintElementType::default());
        }
        self.on_tab_focus_extensions(tab_id);
    }

    pub(super) fn handle_ext_focus_tab(&mut self, engine_tab_id: u64) {
        if !self.client_uses_lifecycle {
            self.visible_tabs.clear();
        }
        self.visible_tabs.insert(engine_tab_id);
        for (&id, t) in &self.tabs {
            t.host.set_focus(i32::from(id == engine_tab_id));
        }
        self.sync_tab_visibility();
    }

    pub(super) fn on_tab_opened(&mut self, tab_id: u64, focused: bool, is_popup: bool) {
        if is_popup {
            if let Some(t) = self.tabs.get(&tab_id) {
                t.host.was_hidden(0);
                t.host.set_focus(1);
            }
        } else if focused {
            if !self.client_uses_lifecycle {
                self.visible_tabs.clear();
            }
            self.visible_tabs.insert(tab_id);
            for (&id, existing) in &self.tabs {
                if !existing.is_popup {
                    existing.host.set_focus(i32::from(id == tab_id));
                }
            }
        } else {
            self.visible_tabs.remove(&tab_id);
        }
        self.sync_tab_visibility();
    }

    pub(super) fn sync_tab_visibility(&mut self) {
        let legacy = !self.client_uses_lifecycle;
        for (&id, t) in &mut self.tabs {
            let visible = self.visible_tabs.contains(&id);
            let agent = t.prompts.agent_driven.load(Ordering::Relaxed);
            let want_awake = t.is_popup || visible || t.rpc_unthrottled || (legacy && agent);
            let effect = step_awake(&mut t.awake_state, want_awake);
            apply_awake_effect(id, t, visible, effect);
        }
    }
}
