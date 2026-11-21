//! A drawn menu's keys, as Kobalte's menus take them from the item that holds focus.

use rune::{Key, KeyEvent, Tx};

use super::menu::{Menu, row_key};
use super::state::{choose_at, close, focus_lit, notify_highlight, open_sub_at, refresh};
use super::{PopupHost, focus};

/// The drawn menu's keys: Up/Down/Home/End light a row, a letter lights the next one it starts,
/// Right or Enter opens a submenu and Left closes it, Enter or Space picks, Esc and Tab dismiss.
/// It takes every key while it's open.
pub fn key<S: PopupHost + 'static>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.popup().shown.as_ref().is_some_and(|m| m.search.is_some()) {
        refresh(s);
        // Typing and moving in the search field are the field's; the rest move in the menu.
        // An editing field keeps Return, which a search field gives to the lit row.
        let editing = s.popup().shown.as_ref().is_some_and(|m| m.search.as_ref().is_some_and(|x| x.field.is_some()));
        let field = !matches!(ev.key, Key::Escape | Key::Tab | Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Enter) || (editing && ev.key == Key::Enter);
        if field {
            return false;
        }
    }
    let Some(shown) = s.popup().shown.as_ref() else { return false };
    // The keys move in the deepest open menu with a lit row.
    let level = shown.key_level();
    let lit = shown.active_at(level);
    let opens_lit = lit.is_some_and(|n| shown.menu_at(level).is_some_and(|m| m.sub_at(n).is_some()));
    // Submenus open toward the end of the line: to the left in a right-to-left window, where Left
    // opens one and Right goes back.
    let key = match (&ev.key, opencode_i18n::direction() == opencode_i18n::Direction::Rtl) {
        (Key::Left, true) => Key::Right,
        (Key::Right, true) => Key::Left,
        (k, _) => k.clone(),
    };
    match &key {
        Key::Escape | Key::Tab => {
            close(s, None, focus::Reason::Keys, tx);
            return true;
        }
        Key::Left if level > 0 => {
            if let Some(shown) = s.popup_mut().shown.as_mut() {
                shown.subs.truncate(level - 1);
            }
            tx.instant();
            focus_lit(s, level - 1, true, tx);
            return true;
        }
        Key::Right => {
            if let Some(n) = lit.filter(|_| opens_lit) {
                open_sub_at(s, level, n, true, tx);
                tx.instant();
                focus_lit(s, level + 1, true, tx);
            }
            return true;
        }
        Key::Enter | Key::Text(_) if is_space_or_enter(&ev.key) => {
            match lit {
                Some(n) if opens_lit => {
                    open_sub_at(s, level, n, true, tx);
                    tx.instant();
                    focus_lit(s, level + 1, true, tx);
                }
                Some(n) => choose_at(s, level, n, tx),
                None => {}
            }
            return true;
        }
        _ => {}
    }
    let next_move = s.popup().shown.as_ref().and_then(|shown| {
        let menu = shown.menu_at(level)?;
        let next = step(menu, lit, &ev.key)?;
        let id = menu.item_at(next).map(|i| i.id.clone());
        Some((next, id))
    });
    let Some((next, id)) = next_move else { return true };
    if let Some(shown) = s.popup_mut().shown.as_mut() {
        shown.set_active(level, next);
        // The keys leave a submenu the pointer opened as they move off its row.
        if shown.subs.get(level).is_some_and(|o| o.at != next) {
            shown.subs.truncate(level);
        }
    }
    if let Some(id) = id {
        if level == 0 && lit != Some(next) {
            notify_highlight(s, &id);
        }
        tx.scroll_into_view(row_key(&id));
    }
    tx.instant();
    focus_lit(s, level, true, tx);
    true
}

/// The row `key` lights in `menu`, from row `active`: arrows step and wrap, Home and End go to
/// the ends, a letter goes to the next row it starts. None for any other key.
fn step(menu: &Menu, active: Option<usize>, key: &Key) -> Option<usize> {
    let offered = menu.offered();
    let at = active.and_then(|a| offered.iter().position(|&n| n == a));
    let last = offered.len().saturating_sub(1);
    let i = match key {
        Key::Down => at.map_or(0, |i| if i >= last { 0 } else { i + 1 }),
        Key::Up => at.map_or(last, |i| if i == 0 { last } else { i - 1 }),
        Key::Home | Key::PageUp => 0,
        Key::End | Key::PageDown => last,
        Key::Text(t) => {
            let c = t.chars().next()?.to_lowercase().to_string();
            let starts = |&&n: &&usize| menu.title_at(n).is_some_and(|title| title.to_lowercase().starts_with(&c));
            let after = at.map_or(0, |i| i + 1);
            let n = offered.iter().skip(after).find(starts).or_else(|| offered.iter().find(starts))?;
            offered.iter().position(|o| o == n)?
        }
        _ => return None,
    };
    offered.get(i).copied()
}

fn is_space_or_enter(key: &Key) -> bool {
    matches!(key, Key::Enter) || matches!(key, Key::Text(t) if t == " ")
}
