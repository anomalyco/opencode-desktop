use super::*;

pub(crate) fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if knobs::key(s, ev, tx) || stories::key(s, ev, tx) || s.switcher_key(ev, tx) || s.settings_key(ev, tx) || (s.opening.is_none() && s.question_key(ev, tx)) || s.add_key(ev, tx) || s.shortcut(ev, tx) || s.picker_key(ev, tx) { return true; }
    match &ev.key {
        Key::Command('e') => {
            s.edge = (s.edge + 1) % edge::ALL.len();
            true
        }
        Key::Command(c) if c.eq_ignore_ascii_case(&'b') && ev.mods.shift => {
            s.sidebar_open = !s.sidebar_open;
            true
        }
        Key::Command('b') => {
            s.opening = None;
            s.route = if s.route == Route::Home { s.tabs.last().map_or(Route::Home, |t| Route::Session(t.id.clone())) } else { Route::Home };
            tx.settle();
            true
        }
        Key::Escape if s.opening.is_some() => {
            s.cancel_open(tx);
            true
        }
        Key::Escape if s.composer.focused && matches!(&s.route, Route::Session(id) if s.busy(id)) => {
            s.stop();
            true
        }
        Key::Escape if s.activity_open => {
            tx.instant();
            s.activity_open = false;
            true
        }
        Key::Escape if s.story.is_some() => {
            stories::close(s, tx);
            true
        }
        // A subagent's session has no composer: Esc goes back to the session that started it.
        Key::Escape if s.open_parent(tx) => true,
        Key::Escape if s.route != Route::Home && !s.composer.focused => {
            s.route = Route::Home;
            tx.settle();
            true
        }
        _ => false,
    }
}
