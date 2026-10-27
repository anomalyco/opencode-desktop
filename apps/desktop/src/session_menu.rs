//! Right-click menus on sessions and renaming them in place, as Desktop has them (`tab-nav.tsx`,
//! `home/sessions/view.tsx`, `message-timeline.tsx`). The menus are native; the actions go
//! through the same calls Desktop's do (`PATCH /api/session/:id` with `{ title }`, `DELETE`,
//! the Export JSON).
//!
//! | Where | Items |
//! | --- | --- |
//! | Sidebar tab | Rename, Close tab, —, Delete session… |
//! | Home row, session header | Rename, Export…, —, Delete… |
//!
//! Rename edits the title where it stands: ⏎ saves, Esc cancels, a click elsewhere saves (the
//! Home row cancels, as Desktop's does). Double-clicking a tab and clicking a root session's
//! header title start it, and ⌃R does for the open session. Delete asks first.

use rune::{Key, KeyEvent, PopupAnchor, PopupItem, PopupMenu, TextEdit, Tx};

use opencode_kit::client::Write;

use crate::{App, Route};

/// Where a session's title is drawn: each has its own menu and its own blur rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    Tab,
    Row,
    Header,
}

/// What a menu is about.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub id: String,
    pub spot: Spot,
}

impl Target {
    pub fn new(id: &str, spot: Spot) -> Target {
        Target { id: id.to_owned(), spot }
    }
}

#[derive(Default)]
pub struct Menus {
    /// The menu showing now, so its row draws marked.
    pub on: Option<Target>,
    /// The title being edited, and its text.
    pub rename: Option<Target>,
    pub edit: TextEdit,
    /// The field has had focus: losing it afterwards is a click away.
    pub seen: bool,
    /// A session waiting on Delete's confirmation.
    pub confirm: Option<String>,
    /// The storybook's drawn menu lights this item.
    pub hot: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: &'static str,
    pub title: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Item(Item),
    Sep,
}

fn item(id: &'static str, title: &'static str, enabled: bool) -> Line {
    Line::Item(Item { id, title, enabled })
}

/// The menu for `t`, as Desktop lists it. A tab's Rename and Delete wait for its session to
/// load; a subagent's header has no menu.
pub fn lines(s: &App, t: &Target) -> Vec<Line> {
    let known = s.session(&t.id);
    match t.spot {
        Spot::Tab => vec![
            item("rename", "Rename", known.is_some()),
            item("close", "Close tab", true),
            Line::Sep,
            item("delete", "Delete session…", known.is_some()),
        ],
        Spot::Row | Spot::Header if known.is_none_or(|i| i.parent.is_some() && t.spot == Spot::Header) => vec![],
        Spot::Row | Spot::Header => vec![item("rename", "Rename", true), item("export", "Export…", true), Line::Sep, item("delete", "Delete…", true)],
    }
}

fn native(lines: &[Line]) -> PopupMenu {
    lines.iter().fold(PopupMenu::new(), |m, l| match l {
        Line::Item(i) => m.item(PopupItem::new(i.id, i.title).enabled(i.enabled)),
        Line::Sep => m.separator(),
    })
}

/// Opens `t`'s menu at `at`, marking its row until it closes. A Home row being renamed keeps
/// the plain field, as Desktop keeps the native menu there.
pub fn open(s: &mut App, t: Target, at: impl Into<PopupAnchor>, tx: &mut Tx) {
    if s.menus.rename.as_ref().is_some_and(|r| r.spot == Spot::Row) {
        return;
    }
    let lines = lines(s, &t);
    if lines.is_empty() {
        return;
    }
    s.finish_rename(true);
    s.menus.on = Some(t.clone());
    tx.instant();
    tx.popup_menu(native(&lines), at, move |s: &mut App, picked, tx| {
        s.menus.on = None;
        tx.instant();
        if let Some(id) = picked {
            run(s, &t, id, tx);
        }
    });
}

/// A right-click on `e` opens `t`'s menu.
pub fn on(e: rune::El, t: Target) -> rune::El {
    e.on_secondary_press(move |s: &mut App, press: rune::PointerPress, tx: &mut Tx| open(s, t.clone(), press, tx))
}

/// Runs the menu item `id` on `t`.
pub fn run(s: &mut App, t: &Target, id: &str, tx: &mut Tx) {
    match id {
        "rename" => s.begin_rename(t),
        "close" => s.close_tab(&t.id, tx),
        "export" => {
            s.write(Write::Export(t.id.clone()));
        }
        "delete" => s.menus.confirm = Some(t.id.clone()),
        _ => {}
    }
}

impl App {
    /// Starts editing `t`'s title in place, all of it selected.
    pub fn begin_rename(&mut self, t: &Target) {
        let Some(title) = self.session(&t.id).map(|i| i.title.clone()) else { return };
        self.finish_rename(true);
        let mut edit = TextEdit::new(title);
        // The caret sits at the start, so a long title shows from its first word.
        edit.anchor = edit.text.len();
        edit.focus = 0;
        edit.focused = true;
        self.menus.edit = edit;
        self.menus.rename = Some(t.clone());
        self.menus.seen = false;
        self.composer.focused = false;
        self.search.focused = false;
    }

    /// Ends the edit: with `save`, a changed, non-empty title is sent. Nothing when none is open.
    pub fn finish_rename(&mut self, save: bool) {
        let Some(t) = self.menus.rename.take() else { return };
        self.menus.edit.focused = false;
        if save {
            let title = self.menus.edit.text.clone();
            self.rename_session(&t.id, &title);
        }
    }

    /// Desktop's rename: the trimmed title, unless it's empty or the same, goes to
    /// `session.update`. The list and the tab show it at once; the server's event confirms it.
    pub fn rename_session(&mut self, id: &str, title: &str) -> bool {
        let title = title.trim();
        let Some(info) = self.sessions.iter_mut().find(|s| s.id == id) else { return false };
        if title.is_empty() || title == info.title {
            return false;
        }
        info.title = title.to_owned();
        self.save_tabs();
        self.write(Write::Rename { session: id.to_owned(), title: title.to_owned() });
        true
    }

    /// Whether the field lost focus since it had it: a click elsewhere. Home's row cancels,
    /// the tab and the header save (Desktop's blur handlers).
    pub fn poll_rename(&mut self) -> bool {
        let Some(t) = self.menus.rename.clone() else { return false };
        if self.menus.edit.focused {
            self.menus.seen = true;
            return false;
        }
        if !self.menus.seen {
            return false;
        }
        self.finish_rename(t.spot != Spot::Row);
        true
    }

    /// Deletes a session and its subagents: they leave the list and the tabs now, and the open
    /// page goes Home if it was theirs.
    pub fn delete_session(&mut self, id: &str, tx: &mut Tx) {
        self.menus.confirm = None;
        let mut gone = vec![id.to_owned()];
        let mut at = 0;
        while at < gone.len() {
            let parent = gone[at].clone();
            gone.extend(self.sessions.iter().filter(|s| s.parent.as_deref() == Some(&parent)).map(|s| s.id.clone()));
            at += 1;
        }
        tx.instant();
        self.forget_history(|_, x| gone.iter().any(|g| g == x));
        let open_gone = matches!(&self.route, Route::Session(open) if gone.contains(open));
        self.sessions.retain(|s| !gone.contains(&s.id));
        self.tabs.retain(|t| !gone.contains(&t.id));
        if open_gone {
            self.route = Route::Home;
        }
        self.save_tabs();
        self.write(Write::Delete(id.to_owned()));
    }
}

/// ⌃R renames the open session; Esc and ⏎ end an edit; Esc closes Delete's confirmation.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.menus.confirm.is_some() {
        if ev.key == Key::Escape {
            tx.instant();
            s.menus.confirm = None;
            return true;
        }
        return false;
    }
    if s.menus.rename.is_some() {
        if s.menus.edit.preedit.is_some() {
            return false;
        }
        match ev.key {
            Key::Escape => s.finish_rename(false),
            Key::Enter if !ev.mods.shift => s.finish_rename(true),
            _ => return false,
        }
        tx.instant();
        return true;
    }
    let ctrl_r = ev.mods.ctrl && !ev.mods.alt && matches!(&ev.key, Key::Command('r') | Key::Text(_)) && match &ev.key {
        Key::Text(t) => t.eq_ignore_ascii_case("r") || t == "\u{12}",
        _ => true,
    };
    if !ctrl_r || s.story.is_some() {
        return false;
    }
    let Route::Session(id) = s.route.clone() else { return false };
    if s.session(&id).is_none_or(|i| i.parent.is_some()) {
        return false;
    }
    tx.instant();
    s.begin_rename(&Target::new(&id, Spot::Header));
    true
}
