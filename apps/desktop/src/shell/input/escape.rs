//! Escape closes the topmost layer and nothing else, as Desktop's layer stacks do: its popover
//! stack dismisses only the top entry on `escape.pressed` (`ui/src/components/popover-machine.ts`
//! `transitionPopoverStack`) and its dialog stack only the top dialog on `dismiss`
//! (`ui/src/context/dialog-machine.ts`). With no layer open Escape does nothing: it never
//! leaves a session or takes focus out of the composer.
//!
//! Menus, pickers, the palette and Rune modals take Escape in their own keys first; dismissible
//! page layers register themselves into `opencode_kernel::OverlayDispatcher` with explicit
//! priorities from each feature's `Feature::register` (and `kernel.rs` for core shell layers).

use rune::{KeyEvent, Tx};

use crate::App;

/// Escape in the window: true when an overlay or registered page layer took it.
pub(crate) fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.kernel.overlays.top().is_some_and(|top| top.dismiss_on_escape) {
        let effects = s.kernel.step_overlay(opencode_kernel::OverlayEvent::Escape);
        if !effects.is_empty() {
            let dispatcher = s.kernel.overlay_dispatcher.clone();
            dispatcher.apply_effects(&effects, s, tx);
            return true;
        }
    }
    let dispatcher = s.kernel.overlay_dispatcher.clone();
    if dispatcher.handle_escape(s, ev, tx) {
        return true;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if !(ev.mods.ctrl || ev.mods.cmd || ev.mods.alt || ev.mods.shift) {
        if s.workspace_panes.browser.host.popup_tab_id().is_some() {
            s.workspace_panes.browser.host.close_extension_popup();
            tx.instant();
            return true;
        }
        if let Some(id) = s.active_browser_tab_id() {
            if s.workspace_panes.browser.host.tab(id).is_some_and(|t| t.inspecting) {
                s.workspace_panes.browser.host.toggle_inspect(id);
                tx.instant();
                return true;
            }
            if s.browser_overlay_active(id) {
                s.close_browser_overlay();
                tx.instant();
                return true;
            }
        }
    }
    false
}
