//! Runtime transitions for an open popup menu: opening (native or drawn), dismissing,
//! searching, hovering, picking, submenu navigation, and native repost-window polling.

use std::rc::Rc;

use rune::{El, PopupAnchor, TextEdit, Tx};
use crate::palette::Pal;
use crate::popover;

use super::menu::{Entry, Look, Menu, key_at, row_key};
use super::{NATIVE, PopupHost, SURFACE, Search, Shown, SubOpen, focus, native};

/// Moves focus onto the lit row of the menu `level` deep, as Kobalte's items take focus; a
/// searchable menu keeps it in its field.
pub(super) fn focus_lit<S: PopupHost + 'static>(s: &S, level: usize, ring: bool, tx: &mut Tx) {
    let Some(shown) = &s.popup().shown else { return };
    if !shown.holds_focus() {
        return;
    }
    let key = shown.active_at(level).and_then(|n| key_at(shown.menu_at(level)?, n));
    if let Some(key) = key {
        tx.focus(key, ring);
    }
}

/// Settles [`super::Popup`]'s native AppKit repost window once it elapses.
pub fn poll<S: PopupHost + 'static>(s: &mut S, tx: &mut Tx) {
    // A submenu kept past the pointer crossing its menu closes, or the row it waited over lights.
    if s.popup().submenu != super::submenu::State::Steady {
        let now = s.clock();
        let effect = super::submenu::step(&mut s.popup_mut().submenu, super::submenu::Event::Tick { now });
        apply(s, effect, tx);
    }
    if !matches!(s.popup().native, native::State::JustClosed { .. } | native::State::Dismissing { .. }) {
        return;
    }
    let now = s.clock();
    if let native::Effect::WakeAfter(d) = native::step(&mut s.popup_mut().native, native::Event::Tick { now }) {
        tx.wake_after(d);
    }
}

/// Opens `menu` at `anchor`, an element's key or a point in window points. `pick` runs once it
/// closes, with the picked item's id or `None`.
pub fn open<S: PopupHost + 'static>(
    s: &mut S,
    menu: Menu,
    anchor: impl Into<PopupAnchor>,
    tx: &mut Tx,
    pick: impl FnOnce(&mut S, Option<&str>, &mut Tx) + 'static,
) {
    if NATIVE {
        let anchor = anchor.into();
        let keyed = match &anchor {
            PopupAnchor::Key(k) => Some(k.as_str()),
            PopupAnchor::Point(..) => None,
        };
        let by_keys = tx.by_keys();
        match native::step(&mut s.popup_mut().native, native::Event::OpenRequested { anchor: keyed, by_keys }) {
            native::Effect::SwallowReposted => {
                pick(s, None, tx);
            }
            native::Effect::Show | native::Effect::WakeAfter(_) | native::Effect::None => {
                tx.popup_menu(menu.native(), anchor, move |s: &mut S, picked: Option<&str>, tx: &mut Tx| {
                    if let native::Effect::WakeAfter(d) = native::step(&mut s.popup_mut().native, native::Event::MenuClosed { picked: picked.is_some() }) {
                        tx.wake_after(d);
                    }
                    pick(s, picked, tx);
                });
            }
        }
        return;
    }
    open_drawn(s, menu, anchor, tx, pick);
}

/// [`open`], drawn in the window on every platform.
pub fn open_drawn<S: PopupHost + 'static>(
    s: &mut S,
    menu: Menu,
    anchor: impl Into<PopupAnchor>,
    tx: &mut Tx,
    pick: impl FnOnce(&mut S, Option<&str>, &mut Tx) + 'static,
) {
    open_drawn_inner(s, menu, None, anchor.into(), tx, pick);
}

/// Opens a drawn menu with a live search field focused above its rows: `rows` rebuilds the menu
/// from the current query text on open and on each keystroke.
pub fn open_searchable<S: PopupHost + 'static>(
    s: &mut S,
    placeholder: impl Into<String>,
    rows: impl Fn(&S, &str) -> Menu + 'static,
    anchor: impl Into<PopupAnchor>,
    tx: &mut Tx,
    pick: impl FnOnce(&mut S, Option<&str>, &mut Tx) + 'static,
) {
    let initial = rows(s, "");
    let search = Search { placeholder: placeholder.into(), rows: Rc::new(rows), field: None };
    open_drawn_inner(s, initial, Some(search), anchor.into(), tx, pick);
}

/// A drawn menu under a field that edits a value as it's typed rather than filtering its rows,
/// as Desktop's group menu edits the group's name over its colours (`pinned-area.tsx`'s
/// `GroupEditor`). `value` fills the field, which takes the keys as the menu opens; `lead`
/// draws before its text and `changed` runs on every edit. `label` names it. The menu takes focus
/// as any menu does (a press on the field moves it there); in the field, Up and Down light the
/// rows, Escape closes, Return stays in it. Choosing a `Item::radio` row runs `radio` with its id
/// and leaves the menu open, its rows built again (the check moves).
#[allow(clippy::too_many_arguments)]
pub fn open_with_field<S: PopupHost + 'static>(
    s: &mut S,
    label: impl Into<String>,
    value: &str,
    lead: impl Fn(&S, &Pal) -> El + 'static,
    changed: impl Fn(&mut S, &str, &mut Tx) + 'static,
    radio: impl Fn(&mut S, &str, &mut Tx) + 'static,
    rows: impl Fn(&S) -> Menu + 'static,
    anchor: impl Into<PopupAnchor>,
    tx: &mut Tx,
    pick: impl FnOnce(&mut S, Option<&str>, &mut Tx) + 'static,
) {
    let initial = rows(s);
    let field = super::Field { lead: Rc::new(lead), changed: Rc::new(changed), radio: Rc::new(radio) };
    let search = Search { placeholder: label.into(), rows: Rc::new(move |s: &S, _: &str| rows(s)), field: Some(field) };
    open_drawn_inner(s, initial, Some(search), anchor.into(), tx, pick);
    s.popup_mut().query = TextEdit::new(value);
}

fn open_drawn_inner<S: PopupHost + 'static>(
    s: &mut S,
    menu: Menu,
    search: Option<Search<S>>,
    anchor: PopupAnchor,
    tx: &mut Tx,
    pick: impl FnOnce(&mut S, Option<&str>, &mut Tx) + 'static,
) {
    close(s, None, focus::Reason::Outside, tx);
    let has_search = search.is_some();
    // An editing field leaves focus to the menu, as Kobalte's menu takes it when it opens.
    let field_takes_keys = search.as_ref().is_some_and(|x| x.field.is_none());
    // A menu's max-content width comes from its rows, not the width left on its anchor's
    // end side. Measure once at open; placement can then shift the whole menu on screen.
    let menu = if menu.width.is_none() && menu.look == Look::Menu && !has_search { menu.hug() } else { menu };
    let origin = match &anchor {
        PopupAnchor::Key(key) => focus::Origin::Trigger(key.clone()),
        PopupAnchor::Point(..) => focus::Origin::Point { focused: tx.focused().filter(|k| !k.starts_with('#')).map(str::to_owned) },
    };
    // A menu opened while the keyboard drives (Return or Space on its trigger) lights its first
    // row and hands focus back with the ring.
    let modality = if tx.by_keys() { focus::Modality::Keyboard } else { focus::Modality::Pointer };
    let opened = focus::Opened { modality, origin };
    let selected = menu.selected.as_deref().and_then(|id| menu.entries.iter().position(|e| matches!(e, Entry::Item(i) if i.id == id)));
    let first = menu.offered().first().copied();
    let active = selected.or(first.filter(|_| modality == focus::Modality::Keyboard));
    if let Some(id) = menu.selected.clone() {
        tx.scroll_into_view(row_key(&id));
    }
    // The search field takes the keys while it's open, as Desktop's does once it opens; any
    // other menu takes focus itself.
    {
        let popup = s.popup_mut();
        popup.query = TextEdit::new("");
        popup.query.focused = field_takes_keys;
        popup.remembered = menu.remembers_focus.then(|| tx.focused().filter(|k| !k.starts_with('#')).map(str::to_owned)).flatten();
    }
    if field_takes_keys {
        s.on_focus_taken();
    } else {
        let lit = active.filter(|_| modality == focus::Modality::Keyboard).and_then(|n| key_at(&menu, n));
        let (key, ring) = focus::opening(&opened, SURFACE, lit);
        s.on_focus_taken();
        tx.focus(key, ring);
    }
    s.popup_mut().shown = Some(Shown { menu, anchor, active, subs: Vec::new(), opened, search, highlight: None, pick: Box::new(pick) });
    s.popup_mut().submenu = super::submenu::State::Steady;
    refresh(s);
    popover::motion::opening(s.knobs(), tx);
}

/// Attaches a callback invoked whenever a root item of the currently open drawn menu becomes
/// highlighted by pointer hover or keyboard navigation (e.g. live sound previews in Settings).
pub fn set_highlight<S: PopupHost + 'static>(s: &mut S, cb: impl Fn(&mut S, &str) + 'static) {
    if let Some(shown) = s.popup_mut().shown.as_mut() {
        shown.highlight = Some(Rc::new(cb));
    }
}

pub(super) fn notify_highlight<S: PopupHost + 'static>(s: &mut S, id: &str) {
    let cb = s.popup().shown.as_ref().and_then(|m| m.highlight.as_ref().map(Rc::clone));
    if let Some(cb) = cb {
        cb(s, id);
    }
}

/// Closes the drawn menu as a press outside it would. Nothing when none is open.
pub fn dismiss<S: PopupHost + 'static>(s: &mut S, tx: &mut Tx) {
    close(s, None, focus::Reason::Outside, tx);
}

/// A searchable menu's rows for what's typed now; when they changed, the first one it lists
/// lights (`list-machine.ts`: each result set opens on its first row).
pub(super) fn refresh<S: PopupHost + 'static>(s: &mut S) {
    let Some(search) = s.popup().shown.as_ref().and_then(|m| m.search.clone()) else { return };
    let query = s.popup().query.text.clone();
    let menu = (search.rows)(s, &query);
    if s.popup().shown.is_none() {
        return;
    }
    let Some(shown) = s.popup_mut().shown.as_mut() else { return };
    let changed = shown.menu.entries != menu.entries;
    shown.menu = menu;
    if changed {
        shown.active = shown.menu.offered().first().copied();
    }
}

/// The search field's text changed: its rows for the new query, the first lit
/// (`search-list.tsx:261-265`).
pub(super) fn query_changed<S: PopupHost + 'static>(s: &mut S) {
    refresh(s);
    if s.popup().shown.is_some()
        && let Some(shown) = s.popup_mut().shown.as_mut()
    {
        shown.active = shown.menu.offered().first().copied();
    }
}

/// Closes the drawn menu, answering `picked`, for `reason`; focus goes where the menu machine
/// says ([`focus`]). Nothing when none is open.
pub(super) fn close<S: PopupHost + 'static>(s: &mut S, picked: Option<&str>, reason: focus::Reason, tx: &mut Tx) {
    // Called with nothing shown too (every key): the popup is written only when one closes.
    if s.popup().shown.is_none() {
        return;
    }
    let Some(shown) = s.popup_mut().shown.take() else { return };
    s.popup_mut().submenu = super::submenu::State::Steady;
    s.popup_mut().query.focused = false;
    // A menu that never took focus leaves it where it is, but the keys still hand it to the trigger.
    let trigger = matches!(shown.opened.origin, focus::Origin::Trigger(_));
    let effect = match (shown.holds_focus(), reason, trigger) {
        (true, ..) | (false, focus::Reason::Keys, true) => focus::closing(shown.opened, reason, tx.by_keys()),
        _ => None,
    };
    popover::motion::closing(s.knobs(), tx);
    (shown.pick)(s, picked, tx);
    // A pick that opened another menu has focus where that put it, as Kobalte's restore finds
    // focus outside the menu and leaves it (menu.tsx:349-352); a field it focused through its
    // model wins over this anyway. A dialog it opened keeps focus too, and hands it back as a
    // web dialog opened from a menu item does (`Tx::return_focus`).
    if s.popup().shown.is_some() {
        return;
    }
    match effect {
        Some(focus::Effect::Focus { key, ring }) => tx.return_focus(key, ring),
        Some(focus::Effect::Blur) => tx.blur(),
        None => {}
    }
}

/// Picks row `n`, if it's offered.
pub(super) fn choose<S: PopupHost + 'static>(s: &mut S, n: usize, tx: &mut Tx) {
    refresh(s);
    let Some(item) = s.popup().shown.as_ref().and_then(|m| m.menu.item_at(n)).filter(|i| i.enabled).cloned() else { return };
    // A radio row under an editing field sets its value and leaves the menu open (`Menu.RadioItem`).
    let radio = s.popup().shown.as_ref().and_then(|m| m.search.as_ref()?.field.as_ref()).filter(|_| item.radio).map(|f| Rc::clone(&f.radio));
    if let Some(radio) = radio {
        radio(s, &item.id, tx);
        refresh(s);
        return;
    }
    close(s, Some(&item.id), focus::Reason::Picked, tx);
}

/// Picks row `m` of the first open submenu, if it's offered.
pub(super) fn choose_sub<S: PopupHost + 'static>(s: &mut S, m: usize, tx: &mut Tx) {
    choose_at(s, 1, m, tx);
}

/// Picks row `m` of the menu `level` deep, if it's offered.
pub(super) fn choose_at<S: PopupHost + 'static>(s: &mut S, level: usize, m: usize, tx: &mut Tx) {
    if level == 0 {
        return choose(s, m, tx);
    }
    let item = s.popup().shown.as_ref().and_then(|shown| shown.menu_at(level)?.item_at(m));
    let Some(id) = item.filter(|i| i.enabled).map(|i| i.id.clone()) else { return };
    close(s, Some(&id), focus::Reason::Picked, tx);
}

/// Opens the root's row `n`'s submenu, lighting its first row when the keys opened it.
pub(super) fn open_sub<S: PopupHost + 'static>(s: &mut S, n: usize, keys: bool, tx: &mut Tx) {
    open_sub_at(s, 0, n, keys, tx);
}

/// Opens row `n`'s submenu of the menu `level` deep, closing any open from its other rows.
pub(super) fn open_sub_at<S: PopupHost + 'static>(s: &mut S, level: usize, n: usize, keys: bool, tx: &mut Tx) {
    if s.popup().shown.is_none() {
        return;
    }
    let opened = {
        let Some(shown) = s.popup_mut().shown.as_mut() else { return };
        let Some(sub) = shown.menu_at(level).and_then(|m| m.sub_at(n)) else { return };
        let active = keys.then(|| sub.offered().first().copied()).flatten();
        shown.set_active(level, n);
        if shown.subs.get(level).is_none_or(|o| o.at != n) || keys {
            shown.subs.truncate(level);
            shown.subs.push(SubOpen { at: n, active });
            true
        } else {
            false
        }
    };
    if opened {
        s.popup_mut().submenu = super::submenu::State::Steady;
        popover::motion::opening(s.knobs(), tx);
    }
}

/// Chooses the offered item drawn under window point `at` (a press that dragged from its menu
/// button and let go here, as Kobalte's items select on a release from any press origin).
/// False when no item is there.
pub(super) fn pick_at<S: PopupHost + 'static>(s: &mut S, at: [f32; 2], tx: &mut Tx) -> bool {
    if s.popup().shown.is_none() {
        return false;
    }
    let inside = |r: &[f32; 4]| at[0] >= r[0] && at[0] <= r[0] + r[2] && at[1] >= r[1] && at[1] <= r[1] + r[3];
    let hit = s.popup().rows.borrow().iter().rev().find(|(_, r)| inside(r)).map(|(id, _)| id.clone());
    let Some(id) = hit else { return false };
    close(s, Some(&id), focus::Reason::Picked, tx);
    true
}

/// The pointer over the root's row `n`: it lights, and opens its submenu or closes the open one.
pub(super) fn hover<S: PopupHost + 'static>(s: &mut S, n: usize, tx: &mut Tx) {
    refresh(s);
    hover_at(s, 0, n, tx);
}

/// The pointer over row `n` of the menu `level` deep, which takes focus without a ring, as a
/// Kobalte item does on hover.
pub(super) fn hover_focus<S: PopupHost + 'static>(s: &mut S, level: usize, n: usize, tx: &mut Tx) {
    if level == 0 { hover(s, n, tx) } else { hover_at(s, level, n, tx) }
    focus_lit(s, level, false, tx);
}

/// The pointer over row `m` of the first open submenu.
pub(super) fn hover_sub<S: PopupHost + 'static>(s: &mut S, m: usize, tx: &mut Tx) {
    hover_at(s, 1, m, tx);
}

/// The pointer over row `n` of the menu `level` deep. Back on the open submenu's row or inside
/// the submenu, nothing waits any more. Over another row while a submenu is open, the
/// [`submenu`](super::submenu) machine decides whether the row lights yet (the pointer may be on
/// its way into the submenu) and keeps the submenu a moment after it does.
fn hover_at<S: PopupHost + 'static>(s: &mut S, level: usize, n: usize, tx: &mut Tx) {
    let Some(shown) = s.popup().shown.as_ref() else { return };
    if level > shown.subs.len() {
        return;
    }
    let open = shown.subs.get(level).map(|o| o.at);
    // The pointer only just left the submenu's row when that row is still the lit one.
    let left = open.filter(|&o| o != n && shown.active_at(level) == Some(o)).map(|_| s.popup().pointer.get()[0]);
    match open {
        Some(o) if o != n => {
            let now = s.clock();
            let event = super::submenu::Event::Crossed { level, row: n, at: s.popup().pointer.get()[1], left, target: s.popup().targets.borrow().get(level).copied(), now };
            let effect = super::submenu::step(&mut s.popup_mut().submenu, event);
            apply(s, effect, tx);
        }
        _ => {
            if s.popup().submenu != super::submenu::State::Steady {
                super::submenu::step(&mut s.popup_mut().submenu, super::submenu::Event::Returned);
            }
            light(s, level, n);
        }
    }
}

/// Row `n` of the menu `level` deep lights, opening its submenu in place of the open one; a row
/// without one leaves the open submenu to the machine's delay.
fn light<S: PopupHost + 'static>(s: &mut S, level: usize, n: usize) {
    let (opens, highlight_id) = {
        let Some(shown) = s.popup_mut().shown.as_mut() else { return };
        let changed = level == 0 && shown.active != Some(n);
        shown.set_active(level, n);
        let opens = shown.menu_at(level).is_some_and(|m| m.sub_at(n).is_some());
        match shown.subs.get(level) {
            Some(o) if o.at == n => {}
            Some(_) if !opens => {}
            _ => {
                shown.subs.truncate(level);
                if opens {
                    shown.subs.push(SubOpen { at: n, active: None });
                }
            }
        }
        let id = changed.then(|| shown.menu.item_at(n).filter(|i| i.enabled).map(|i| i.id.clone())).flatten();
        (opens, id)
    };
    if opens {
        super::submenu::step(&mut s.popup_mut().submenu, super::submenu::Event::Returned);
    }
    if let Some(id) = highlight_id {
        notify_highlight(s, &id);
    }
}

fn apply<S: PopupHost + 'static>(s: &mut S, effect: super::submenu::Effect, tx: &mut Tx) {
    use super::submenu::Effect;
    match effect {
        Effect::None => {}
        Effect::WakeAt(at) => tx.wake_after(std::time::Duration::from_secs_f64((at - s.clock()).max(0.0))),
        Effect::Light { level, row, wake } => {
            light(s, level, row);
            focus_lit(s, level, false, tx);
            tx.wake_after(std::time::Duration::from_secs_f64((wake - s.clock()).max(0.0)));
        }
        Effect::Close(level) => {
            if let Some(shown) = s.popup_mut().shown.as_mut() {
                shown.subs.truncate(level);
            }
        }
    }
}

/// The pointer moved over the drawn menu, to `at` in window points. Only a pointer on its way into
/// a submenu acts on it; otherwise nothing redraws.
pub(super) fn moved<S: PopupHost + 'static>(s: &mut S, at: [f32; 2], tx: &mut Tx) {
    let [_, last] = s.popup().pointer.get();
    s.popup().pointer.set([last, at]);
    let super::submenu::State::Aiming { level, .. } = s.popup().submenu else {
        tx.live_only();
        return;
    };
    let (target, now) = (s.popup().targets.borrow().get(level).copied(), s.clock());
    let event = super::submenu::Event::Moved { at, target, now };
    let effect = super::submenu::step(&mut s.popup_mut().submenu, event);
    apply(s, effect, tx);
}
