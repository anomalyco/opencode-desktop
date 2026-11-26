//! The sections' right-click menus. On a tab among two or more selected: what to do with all of
//! them. On any other tab: its section items, then the tab's own menu (the selection goes). On a
//! header: rename, fold, remove the section (its tabs stay open), close its tabs.

use super::*;
use crate::popup::{self, Item, Menu};
use crate::session_menu::{self, Line};
use crate::tabs::sections as sec;
use opencode_i18n::{plural, t};
use rune::{PointerPress, PopupAnchor};

/// ⌘G, as the menus show it.
fn group_keys() -> Vec<String> {
    crate::keymap::Bind::FindNext.keycaps()
}

/// Move to section: every section but `except`.
fn move_menu(s: &App, except: Option<&str>) -> Option<Menu> {
    let doc = &sec::state(s).doc;
    let others: Vec<&sec::Section> = doc.sections.iter().filter(|x| Some(x.id.as_str()) != except).collect();
    if others.is_empty() {
        return None;
    }
    Some(others.into_iter().fold(Menu::new(t!("rune.sections.moveTo")).min_width(160.0), |m, x| m.item(Item::new(format!("sec:in:{}", x.id), x.name.clone()))))
}

/// A right-click on tab `uid`.
pub(super) fn tab(s: &mut App, uid: &str, press: PointerPress, tx: &mut Tx) {
    let sel = sec::state(s).selection.uids().to_vec();
    if sel.len() >= 2 && sel.iter().any(|u| u == uid) {
        return batch(s, sel, press.into(), tx);
    }
    sec::clear_selection(s);
    single(s, uid, press.into(), tx);
}

fn batch(s: &mut App, uids: Vec<String>, at: PopupAnchor, tx: &mut Tx) {
    let n = uids.len();
    let sessions = sec::session_count(s, &uids);
    let doc = &sec::state(s).doc;
    let grouped = uids.iter().any(|u| doc.section_of(u).is_some());
    let mut menu = Menu::new(t!("rune.sections.selected", count = n)).min_width(208.0).item(Item::new("sec:new", t!("rune.sections.newWith", count = n)).keycaps(group_keys()));
    if let Some(m) = move_menu(s, None) {
        menu = menu.submenu("sec:move", t!("rune.sections.moveTo"), m);
    }
    if grouped {
        menu = menu.item(Item::new("sec:out", t!("rune.sections.removeFrom")));
    }
    menu = menu.separator().item(Item::new("sec:close", t!("rune.sections.close", count = n)));
    if sessions > 0 {
        menu = menu
            .item(Item::new("sec:archive", t!("rune.sections.archive", count = sessions)))
            .item(Item::new("sec:delete", t!("rune.sections.delete", count = sessions)).danger(true));
    }
    menu = menu.separator().item(Item::new("sec:clear", t!("rune.sections.clearSelection")));
    tx.instant();
    let again = at.clone();
    popup::open(s, menu, at, tx, move |s: &mut App, picked, tx| {
        tx.instant();
        if let Some(id) = picked {
            run(s, &uids, id, again, tx);
        }
    });
}

fn single(s: &mut App, uid: &str, at: PopupAnchor, tx: &mut Tx) {
    let Some(t) = s.tab(uid).cloned() else { return };
    let target = crate::view::tab_strip::target(s, &t);
    let own = sec::state(s).doc.section_of(uid).map(|x| x.id.clone());
    let mut lines = vec![Line::Extra(Item::new("sec:new", t!("rune.sections.new")).keycaps(group_keys()))];
    if let Some(m) = move_menu(s, own.as_deref()) {
        lines.push(Line::Sub { id: "sec:move".into(), title: t!("rune.sections.moveTo").into(), menu: m });
    }
    if own.is_some() {
        lines.push(Line::Extra(Item::new("sec:out", t!("rune.sections.removeFrom"))));
    }
    let mut rest = session_menu::lines(s, &target);
    // The session's own menu has no Archive on a tab; with several at hand it's one press away.
    if !t.is_draft() && matches!(rest.last(), Some(Line::Item(i)) if i.id == "delete") {
        rest.insert(rest.len() - 2, Line::Extra(Item::new("archive", t!("common.archive"))));
    }
    if !rest.is_empty() {
        lines.push(Line::Sep);
        lines.extend(rest);
    }
    s.finish_rename(true);
    s.kernel.slots.get_mut(&session_menu::MENUS).on = Some(target.clone());
    tx.instant();
    let uids = vec![uid.to_owned()];
    let again = at.clone();
    popup::open(s, session_menu::menu(&lines), at, tx, move |s: &mut App, picked, tx| {
        s.kernel.slots.get_mut(&session_menu::MENUS).on = None;
        tx.instant();
        match picked {
            Some(id) if id.starts_with("sec:") => run(s, &uids, id, again, tx),
            Some(id) => session_menu::run(s, &target, id, tx),
            None => {}
        }
    });
}

fn run(s: &mut App, uids: &[String], id: &str, at: PopupAnchor, tx: &mut Tx) {
    match id {
        "sec:new" => sec::create(s, uids, tx),
        "sec:out" => sec::ungroup(s, uids),
        "sec:close" => sec::close_tabs(s, uids, tx),
        "sec:archive" => sec::archive(s, uids, tx),
        "sec:delete" => confirm_delete(s, uids.to_vec(), at, tx),
        "sec:clear" => sec::clear_selection(s),
        _ => {
            if let Some(section) = id.strip_prefix("sec:in:") {
                sec::move_to(s, uids, section);
            }
        }
    }
}

/// Deleting several sessions asks once more, in place: a second, deliberate press.
fn confirm_delete(s: &mut App, uids: Vec<String>, at: PopupAnchor, tx: &mut Tx) {
    let n = sec::session_count(s, &uids);
    let menu = Menu::new(t!("rune.sections.deleteTitle", count = n))
        .min_width(208.0)
        .group(t!("rune.sections.deleteTitle", count = n))
        .item(Item::new("confirm", t!("rune.sections.deleteConfirm", count = n)).danger(true))
        .item(Item::new("cancel", t!("common.cancel")));
    popup::open(s, menu, at, tx, move |s: &mut App, picked, tx| {
        tx.instant();
        if picked == Some("confirm") {
            sec::delete(s, &uids);
        }
    });
}

/// A right-click on section `id`'s header.
pub(super) fn header(s: &mut App, id: &str, at: PopupAnchor, tx: &mut Tx) {
    let Some(section) = sec::state(s).doc.get(id).cloned() else { return };
    let open: Vec<String> = section.members.iter().filter(|u| s.tab(u).is_some()).cloned().collect();
    let menu = Menu::new(section.name.clone())
        .min_width(192.0)
        .item(Item::new("rename", t!("rune.sections.rename")))
        .item(Item::new("toggle", if section.collapsed { t!("rune.sections.expand") } else { t!("rune.sections.collapse") }))
        .item(Item::new("remove", t!("rune.sections.remove")))
        .separator()
        .item(Item::new("close", plural!("rune.sections.closeAll", open.len())).enabled(!open.is_empty()));
    tx.instant();
    let id = id.to_owned();
    popup::open(s, menu, at, tx, move |s: &mut App, picked, tx| {
        tx.instant();
        match picked {
            Some("rename") => sec::start_rename(s, &id, tx),
            Some("toggle") => sec::toggle(s, &id, tx),
            Some("remove") => sec::remove_section(s, &id, tx),
            Some("close") => sec::close_tabs(s, &open, tx),
            _ => {}
        }
    });
}
