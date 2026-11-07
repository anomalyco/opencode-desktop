use super::*;
use crate::keymap::Bind;

pub(crate) fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if crate::popup::key(s, ev, tx) || crate::plugin_host::popover_key(s, ev, tx) || crate::plugin_host::dialog_key(s, ev, tx) || crate::connect::key(s, ev, tx) || crate::mcp::key(s, ev, tx) || crate::glitch::key(s, ev, tx) || crate::demo::key(s, ev, tx) || crate::diagram::key(s, ev, tx) || s.fork_key(ev, tx) || crate::session_menu::key(s, ev, tx) || crate::dictation::key(s, ev, tx) || knobs::key(s, ev, tx) || s.switcher_key(ev, tx) || s.project_switcher_key(ev, tx) || s.settings_key(ev, tx) || s.panels_key(ev, tx) || (s.opening.is_none() && s.question_key(ev, tx)) || s.add_key(ev, tx) || s.draft_location_key(ev, tx) || crate::app_menu::accelerator(ev, tx) || s.shortcut(ev, tx) || s.picker_key(ev, tx) { return true; }
    match &ev.key {
        // Top tabs have no sidebar: the keys pass through, as Desktop registers no command there.
        _ if s.tab_layout().has_sidebar() && Bind::Sidebar.matches_tx(ev, tx) => s.toggle_sidebar(tx),
        _ if Bind::Home.matches_tx(ev, tx) => {
            if s.toggle_home(tx) {
                s.focus_home_search(tx);
            }
            true
        }
        Key::Escape if !s.sidebar_open && s.layout.peek.shown() => {
            s.queue_peek(crate::layout_state::PeekEvent::Escape, tx);
            true
        }
        Key::Escape if crate::quote::dismiss(s) => {
            tx.instant();
            true
        }
        Key::Escape if s.opening.is_some() => {
            s.cancel_open(tx);
            true
        }
        Key::Escape if crate::stop::key(s, ev, tx) => true,
        Key::Escape if s.project_state.home_menu_open => {
            tx.instant();
            s.project_state.home_menu_open = false;
            true
        }
        Key::Escape if s.activity_open => {
            tx.instant();
            s.activity_open = false;
            true
        }
        Key::Escape if s.todo_open => {
            tx.instant();
            s.todo_open = false;
            true
        }
        Key::Escape if crate::find::dismiss_key(s, ev, tx) => true,
        Key::Escape if crate::status_toast::key(s, ev, tx) => true,
        Key::Escape if s.dismiss_tool_step(tx) => true,
        // On a subagent's page with nothing typed, Esc goes back to the session that started it.
        Key::Escape if crate::subagents::reply::composer_empty(s) && s.open_parent(tx) => true,
        Key::Escape if s.route != Route::Home && !s.composer.focused => {
            s.go_home(tx);
            true
        }
        // Page Up and Page Down page the timeline wherever the pointer and focus are, as a chat
        // pages its messages from the composer; Rune's own fallback pages the first scroll view.
        Key::PageUp | Key::PageDown if !(ev.mods.cmd || ev.mods.alt || ev.mods.ctrl) => match &s.route {
            Route::Session(id) => {
                tx.scroll_page(format!("scroll-{id}"), if ev.key == Key::PageDown { 1 } else { -1 });
                true
            }
            _ => false,
        },
        _ => false,
    }
}
