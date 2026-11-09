//! Right-click menus on sessions and renaming them in place, as Desktop has them (`tab-nav.tsx`,
//! `home/sessions/view.tsx`, `message-timeline.tsx`). The menus are native on macOS and drawn
//! elsewhere (`popup`); the actions go through the same calls Desktop's do
//! (`PATCH /api/session/:id` with `{ title }`, `DELETE`, the Export JSON).
//!
//! | Where | Items |
//! | --- | --- |
//! | Sidebar tab | Rename, Close tab, Close other tabs, Close tabs to the right, —, Delete session… |
//! | Home row, session header | Rename, Export…, Make project, —, Delete… |
//! | Inbox task / project line | Close (archive) / Stop being a project, —, Delete… |
//!
//! Rename edits the title where it stands: ⏎ saves, Esc cancels, a click elsewhere saves (the
//! Home row cancels, as Desktop's does). Double-clicking a tab and clicking a root session's
//! header title start it, and ⌃R does for the open session. Delete asks first.

use rune::{Key, KeyEvent, PopupAnchor, TextEdit, Tx};

use opencode_kit::client::Write;

use crate::popup::{self, Menu};
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
    pub title: String,
    pub enabled: bool,
    /// Destructive: Desktop's critical ink.
    pub danger: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Item(Item),
    Sep,
}

fn item(id: &'static str, title: impl Into<String>, enabled: bool) -> Line {
    Line::Item(Item { id, title: title.into(), enabled, danger: false })
}

/// A word with Desktop's trailing ellipsis for an item that asks first ("Delete…").
fn asks(word: &str) -> String {
    format!("{word}\u{2026}")
}

/// The menu for `t`, as Desktop lists it. A tab's Rename and Delete wait for its session to
/// load, and its Close other tabs and Close tabs to the right for tabs to close; a subagent's
/// header has no menu.
pub fn lines(s: &App, t: &Target) -> Vec<Line> {
    let known = s.session(&t.id);
    let draft = s.tabs.iter().any(|tab| tab.id == t.id && tab.is_draft());
    match t.spot {
        Spot::Tab => {
            let at = s.tabs.iter().position(|x| x.id == t.id);
            let close = [
                item("close", opencode_i18n::t!("common.closeTab"), true),
                item("close-others", opencode_i18n::t!("common.closeOtherTabs"), s.tabs.iter().any(|x| x.id != t.id)),
                // Side tabs run down the sidebar (`TabCloseMenuItems`).
                item("close-right", if s.tab_layout().has_sidebar() { opencode_i18n::t!("common.closeTabsBelow") } else { opencode_i18n::t!("common.closeTabsToRight") }, at.is_some_and(|i| i + 1 < s.tabs.len())),
            ];
            // A draft has no session to rename or delete: only the close items (`DraftTabItem`).
            if draft {
                return close.into();
            }
            std::iter::once(item("rename", opencode_i18n::t!("common.rename"), known.is_some()))
                .chain(close)
                .chain([Line::Sep, Line::Item(Item { id: "delete", title: asks(opencode_i18n::t!("session.delete.title")), enabled: known.is_some(), danger: true })])
                .collect()
        }
        Spot::Row | Spot::Header if known.is_none_or(|i| i.parent.is_some() && t.spot == Spot::Header) => vec![],
        Spot::Row | Spot::Header => vec![item("rename", opencode_i18n::t!("common.rename"), true), item("export", asks(opencode_i18n::t!("common.export")), true), Line::Sep, item("delete", asks(opencode_i18n::t!("common.delete")), true)],
    }
}

/// `lines` as the menu that shows them.
pub fn menu(lines: &[Line]) -> Menu {
    lines.iter().fold(Menu::new(opencode_i18n::t!("command.category.session")), |m, l| match l {
        Line::Item(i) => m.item(popup::Item::new(i.id, i.title.clone()).enabled(i.enabled).danger(i.danger)),
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
    popup::open(s, menu(&lines), at, tx, move |s: &mut App, picked, tx| {
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
        "close-others" | "close-right" => {
            let at = s.tabs.iter().position(|x| x.id == t.id).unwrap_or(0);
            let gone: Vec<String> = s.tabs.iter().enumerate()
                .filter(|(i, x)| x.id != t.id && (id == "close-others" || *i > at))
                .map(|(_, x)| x.id.clone())
                .collect();
            for g in gone {
                s.close_tab(&g, tx);
            }
        }
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
        self.closed.remove(|id| gone.iter().any(|g| g == id));
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
        if s.menus.edit.is_composing() {
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
    // Off macOS ⌃R is Ctrl+R, the Browser's reload; a focused pane keeps it, as Desktop's
    // `session.rename` skips plugin panes.
    let pane_owns = s.side.focused && crate::keymap::Bind::BrowserReload.matches_tx(ev, tx);
    if !crate::keymap::Bind::Rename.matches_tx(ev, tx) || pane_owns || s.stage.is_some() {
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
