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

use rune::machine::Seq;
use rune::{Key, KeyEvent, PopupAnchor, TextEdit, Tx};

use opencode_kit::client::Write;

use crate::popup::{self, Menu};
use crate::{App, Route};

mod delete;
mod rename;

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
    /// The title being edited (`rename`), its field's text, and its save.
    rename: rename::Edit,
    pub edit: TextEdit,
    save: rename::Save,
    seq: Seq,
    /// Delete's confirmation.
    delete: delete::Delete,
    /// The storybook's drawn menu lights this item.
    pub hot: Option<String>,
}

impl Menus {
    /// The title being edited.
    pub fn renaming(&self) -> Option<Target> {
        match &self.rename {
            rename::Edit::Arming { target, .. } | rename::Edit::Editing { target, .. } => Some(target.clone()),
            rename::Edit::Idle => None,
        }
    }

    /// Whether `t`'s title is being edited.
    pub fn renames(&self, t: &Target) -> bool {
        matches!(&self.rename, rename::Edit::Arming { target, .. } | rename::Edit::Editing { target, .. } if target == t)
    }

    /// The session Delete's confirmation is for, while it shows.
    pub fn confirming(&self) -> Option<String> {
        match &self.delete {
            delete::Delete::Confirming { session } | delete::Delete::Deleting { session, .. } => Some(session.clone()),
            delete::Delete::Closed => None,
        }
    }

    /// The delete is on its way: the dialog's buttons are off (`selectSessionDeleteView`).
    pub fn deleting(&self) -> bool {
        matches!(self.delete, delete::Delete::Deleting { .. })
    }
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
    if s.menus.renaming().is_some_and(|r| r.spot == Spot::Row) {
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
        "delete" => {
            crate::popup::hand_off(s);
            s.ask_delete(&t.id);
        }
        _ => {}
    }
}

impl App {
    /// Starts editing `t`'s title in place, all of it selected; one open elsewhere ends first.
    pub fn begin_rename(&mut self, t: &Target) {
        let Some(title) = self.session(&t.id).map(|i| i.title.clone()) else { return };
        self.finish_rename(true);
        self.rename_event(rename::Event::Start { target: t.clone(), title }, "");
    }

    /// Ends the edit: with `save`, a changed, non-empty title is sent. Nothing when none is open.
    pub fn finish_rename(&mut self, save: bool) {
        let draft = self.menus.edit.text.clone();
        let event = if save { rename::Event::Submit { draft: &draft, now: self.clock } } else { rename::Event::Cancel };
        self.rename_event(event, "");
    }

    /// Runs `event` through the rename machine and carries out what it asks.
    fn rename_event(&mut self, event: rename::Event<'_>, why: &str) {
        let m = &mut self.menus;
        for effect in rename::step(&mut m.seq, &mut m.rename, &mut m.save, event) {
            match effect {
                rename::Effect::Arm(title) => {
                    let mut edit = TextEdit::new(title);
                    // The caret sits at the start, so a long title shows from its first word.
                    edit.anchor = edit.text.len();
                    edit.focus = 0;
                    edit.focused = true;
                    self.menus.edit = edit;
                    self.composer.focused = false;
                    self.search.focused = false;
                }
                rename::Effect::Disarm => self.menus.edit.focused = false,
                rename::Effect::Remember { session, title } => {
                    if let Some(info) = self.sessions.iter_mut().find(|s| s.id == session) {
                        info.title = title;
                        self.save_tabs();
                    }
                }
                rename::Effect::Save { session, title } => {
                    // A story's write goes nowhere and a build that can't write sends none: each
                    // answers at once.
                    let sent = self.write(Write::Rename { session: session.clone(), title: title.clone() });
                    if self.stage.is_some() || !sent {
                        let current = self.session(&session).map(|i| i.title.clone());
                        return self.rename_event(rename::Event::Replied { session: &session, title: &title, ok: sent, current: current.as_deref() }, why);
                    }
                }
                // `poll_rename` keeps the save's deadline.
                rename::Effect::WakeAt(_) => {}
                rename::Effect::Failed => crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), if why.is_empty() { opencode_i18n::t!("common.requestFailed") } else { why }),
            }
        }
    }

    /// Desktop's rename from a dialog or a palette command: the trimmed title, unless it's empty
    /// or the same, goes to `session.update`. The list and the tab show it at once; the server's
    /// event confirms it.
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

    /// The field's focus and the save's deadline: focus arms the edit, and losing it afterwards
    /// is a click away (Home's row cancels, the tab and the header save, as Desktop's blur
    /// handlers do).
    pub fn poll_rename(&mut self, now: f64, tx: &mut Tx) -> bool {
        let before = self.menus.renaming();
        let focused = self.menus.edit.focused;
        let arming = matches!(self.menus.rename, rename::Edit::Arming { .. });
        let editing = matches!(self.menus.rename, rename::Edit::Editing { .. });
        if arming && focused {
            self.rename_event(rename::Event::Armed, "");
        } else if editing && !focused {
            let draft = self.menus.edit.text.clone();
            self.rename_event(rename::Event::Blurred { draft: &draft, now }, "");
        }
        let current = |s: &App, id: &str| s.session(id).map(|i| i.title.clone());
        if let rename::Save::Saving { session, due, .. } = &self.menus.save {
            let (session, due) = (session.clone(), *due);
            let title = current(self, &session);
            self.rename_event(rename::Event::Tick { now, current: title.as_deref() }, "");
            if matches!(self.menus.save, rename::Save::Saving { .. }) {
                tx.wake_at(due);
            }
        }
        before != self.menus.renaming()
    }

    /// The server answered a rename, a delete or a restore this window sent (`client.session.write`).
    pub(crate) fn session_write_replied(&mut self, data: &serde_json::Value, tx: &mut Tx) {
        let session = data["sessionID"].as_str().unwrap_or_default().to_owned();
        let ok = data["ok"].as_bool() == Some(true);
        let why = data["error"].as_str().map(crate::status_toast::plain).unwrap_or_default();
        let why = why.as_str();
        match data["kind"].as_str() {
            Some("rename") => {
                let title = data["title"].as_str().unwrap_or_default().to_owned();
                let current = self.session(&session).map(|i| i.title.clone());
                let ours = matches!(&self.menus.save, rename::Save::Saving { session: s, optimistic, .. } if *s == session && *optimistic == title);
                // A rename this edit didn't send (the rename dialog's) says so as any write does.
                if !ours && !ok {
                    crate::status_toast::from_server(self, why.to_owned());
                }
                self.rename_event(rename::Event::Replied { session: &session, title: &title, ok, current: current.as_deref() }, why);
            }
            Some("delete") => self.delete_event(delete::Event::Replied { session, ok }, why, tx),
            Some("unarchive") => self.switcher_restore_replied(session, ok, why, tx),
            _ => {}
        }
        if !ok && let Some(e) = data["error"].as_str() {
            crate::status_toast::from_server(self, e.to_owned());
        }
    }

    /// Asks before deleting `id`.
    pub fn ask_delete(&mut self, id: &str) {
        self.delete_event(delete::Event::Ask(id.to_owned()), "", &mut Tx::default());
    }

    /// Cancel, Escape or a press outside Delete's confirmation; nothing while the delete runs.
    pub fn cancel_delete(&mut self, tx: &mut Tx) {
        self.delete_event(delete::Event::Cancel, "", tx);
    }

    /// Delete: the session and its subagents leave the list, and the server's answer decides.
    pub fn delete_session(&mut self, id: &str, tx: &mut Tx) {
        let mut gone = vec![id.to_owned()];
        let mut at = 0;
        while at < gone.len() {
            let parent = gone[at].clone();
            gone.extend(self.sessions.iter().filter(|s| s.parent.as_deref() == Some(&parent)).map(|s| s.id.clone()));
            at += 1;
        }
        let tree = gone.iter().filter_map(|g| self.session(g).cloned()).collect();
        self.delete_event(delete::Event::Confirm { tree, now: tx.clock() }, "", tx);
    }

    /// The delete's deadline.
    pub fn poll_delete(&mut self, now: f64, tx: &mut Tx) -> bool {
        let before = self.menus.confirming();
        self.delete_event(delete::Event::Tick { now }, "", tx);
        if let delete::Delete::Deleting { due, .. } = self.menus.delete {
            tx.wake_at(due);
        }
        before != self.menus.confirming()
    }

    /// Runs `event` through Delete's machine and carries out what it asks.
    fn delete_event(&mut self, event: delete::Event, why: &str, tx: &mut Tx) {
        for effect in delete::step(&mut self.menus.delete, event) {
            tx.instant();
            match effect {
                delete::Effect::Hide(ids) => self.sessions.retain(|s| !ids.contains(&s.id)),
                delete::Effect::Run(id) => {
                    // A story's write goes nowhere and a build that can't write sends none: each
                    // answers at once.
                    let sent = self.write(Write::Delete(id.clone()));
                    if self.stage.is_some() || !sent {
                        return self.delete_event(delete::Event::Replied { session: id, ok: sent }, why, tx);
                    }
                }
                delete::Effect::WakeAt(due) => tx.wake_at(due),
                delete::Effect::Restore(hidden) => {
                    for s in hidden {
                        if self.session(&s.id).is_none() {
                            self.sessions.push(s);
                        }
                    }
                }
                delete::Effect::Removed(gone) => self.session_removed(&gone),
                delete::Effect::Failed => crate::status_toast::fail(self, opencode_i18n::t!("session.delete.failed.title"), why),
            }
        }
    }

    /// The server deleted these sessions: their tabs and history go, and the open page goes
    /// Home if it was theirs (`notifySessionTabsRemoved`).
    fn session_removed(&mut self, gone: &[String]) {
        self.forget_history(|_, x| gone.iter().any(|g| g == x));
        let open_gone = matches!(&self.route, Route::Session { id: open, .. } if gone.contains(open));
        self.sessions.retain(|s| !gone.contains(&s.id));
        self.tabs.retain(|t| !gone.contains(&t.id));
        self.closed.remove(|id| gone.iter().any(|g| g == id));
        if open_gone {
            self.route = Route::Home;
        }
        self.save_tabs();
    }
}

/// ⌃R renames the open session; Esc and ⏎ end an edit; Esc closes Delete's confirmation.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.menus.confirming().is_some() {
        if ev.key == Key::Escape {
            s.cancel_delete(tx);
            return true;
        }
        return false;
    }
    if s.menus.renaming().is_some() {
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
    let Route::Session { id, .. } = s.route.clone() else { return false };
    if s.session(&id).is_none_or(|i| i.parent.is_some()) {
        return false;
    }
    tx.instant();
    s.begin_rename(&Target::new(&id, Spot::Header));
    true
}
