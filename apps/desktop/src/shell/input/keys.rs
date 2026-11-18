use crate::*;
use crate::keymap::Bind;

pub(crate) fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    // A key before the after-frame turn still finds the dormant built-ins' shortcuts.
    crate::plugin_host::doze_owed(s);
    crate::status_toast::hotkey(s, ev, tx);
    // A key from inside the side region (something there has the keyboard) wakes its tab's label.
    if s.side.focus.is_quiet() && tx.focused().is_some() && !s.composer.focused {
        s.side.focus.wake_on_key();
    }
    let overlays = s.kernel.overlay_dispatcher.clone();
    if crate::popup::key(s, ev, tx)
        || crate::narrow::key(s, ev, tx)
        || overlays.handle_key_band(s, ev, tx, 0, 160)
        || crate::glitch::key(s, ev, tx)
        || crate::demo::key(s, ev, tx)
        || overlays.handle_key_band(s, ev, tx, 190, 299)
        || crate::dictation::key(s, ev, tx)
        || knobs::key(s, ev, tx)
        || overlays.handle_key_band(s, ev, tx, 300, 499)
        || s.panels_key(ev, tx)
        || (s.opening.is_none() && s.question_key(ev, tx))
        || s.add_key(ev, tx)
        || s.draft_location_key(ev, tx)
        || crate::app_menu::accelerator(s, ev, tx)
        || s.shortcut(ev, tx)
        || s.picker_key(ev, tx)
    {
        return true;
    }
    if global(s, ev, tx) {
        return true;
    }
    if ev.key == Key::Escape {
        return crate::escape::key(s, ev, tx);
    }
    match &ev.key {
        // Page Up and Page Down page the timeline wherever the pointer and focus are, as a chat
        // pages its messages from the composer; Rune's own fallback pages the first scroll view.
        Key::PageUp | Key::PageDown if !(ev.mods.cmd || ev.mods.alt || ev.mods.ctrl) => match &*s.route {
            Route::Session { id, .. } => {
                tx.scroll_page(format!("scroll-{id}"), if ev.key == Key::PageDown { 1 } else { -1 });
                true
            }
            _ => false,
        },
        _ => crate::type_to_compose::key(s, ev, tx),
    }
}

/// The window's commands that a focused field doesn't keep for itself, as Desktop matches its
/// command keybinds on the document before the field (`command.tsx` `handleKeyDown`, capture):
/// the composer asks for them first too, so Alt+Home goes Home from it.
pub(crate) fn global(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    // Top tabs have no sidebar: the keys pass through, as Desktop registers no command there.
    if s.shows_sidebar() && Bind::Sidebar.matches_tx(ev, tx) {
        return s.toggle_sidebar(tx);
    }
    if Bind::Home.matches_tx(ev, tx) {
        if s.toggle_home(tx) {
            s.focus_home_search(tx);
        }
        return true;
    }
    false
}
