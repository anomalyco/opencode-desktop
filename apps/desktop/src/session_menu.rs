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
use opencode_kit::model::SessionInfo;
use opencode_kit::servers::ServerKey;

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

/// What a menu is about: a session (a draft, for a draft's tab) on its server. Its actions run
/// on that server whichever one the window works against, as Desktop's run on the tab's or
/// row's own server context (`tab-nav.tsx:103-104`).
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub id: String,
    pub server: ServerKey,
    pub spot: Spot,
}

impl Target {
    /// `id` on the server the running turn works against.
    pub fn new(id: &str, spot: Spot) -> Target {
        Target::on(&crate::servers::runtime::current(), id, spot)
    }

    pub fn on(server: &ServerKey, id: &str, spot: Spot) -> Target {
        Target { id: id.to_owned(), server: server.clone(), spot }
    }

    /// The tab a tab's target names.
    pub fn tab<'a>(&self, s: &'a App) -> Option<&'a crate::tabs::Tab> {
        s.tabs.iter().find(|x| x.id == self.id && (x.is_draft() || x.server == self.server))
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
    /// The save this window stopped waiting for and already said so (session, title): its late
    /// failure says nothing more.
    gave_up: Option<(String, String)>,
    seq: Seq,
    /// Delete's confirmation.
    delete: delete::Delete,
    /// Deletes sent (their undo toast went), until the server answers.
    sent: delete::Sent,
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
        self.asked().map(|a| a.session.clone())
    }

    /// What Delete's confirmation asks about, while it shows: the session, its server, its title.
    pub(crate) fn asked(&self) -> Option<&delete::Asked> {
        match &self.delete {
            delete::Delete::Confirming { asked } => Some(asked),
            delete::Delete::Closed => None,
        }
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
    // The session is known once its own server has it: live on the server the window works
    // against, as of its last checkout on another (`props.session` from the tab's server).
    let tab = (t.spot == Spot::Tab).then(|| t.tab(s)).flatten();
    let here = t.server == *crate::servers::runtime::active(s);
    let known = here.then(|| s.session(&t.id)).flatten();
    let known_elsewhere = !here && s.checkout.known(&t.server, &t.id);
    let draft = tab.is_some_and(crate::tabs::Tab::is_draft);
    match t.spot {
        Spot::Tab => {
            let uid = tab.map(crate::tabs::Tab::uid).unwrap_or_default();
            let at = s.tab_at(&uid);
            let known = known.is_some() || known_elsewhere;
            let close = [
                item("close", opencode_i18n::t!("common.closeTab"), true),
                item("close-others", opencode_i18n::t!("common.closeOtherTabs"), s.tabs.iter().any(|x| x.uid() != uid)),
                // Side tabs run down the sidebar (`TabCloseMenuItems`).
                item("close-right", if s.tab_layout().has_sidebar() { opencode_i18n::t!("common.closeTabsBelow") } else { opencode_i18n::t!("common.closeTabsToRight") }, at.is_some_and(|i| i + 1 < s.tabs.len())),
            ];
            // A draft has no session to rename or delete: only the close items (`DraftTabItem`).
            if draft {
                return close.into();
            }
            std::iter::once(item("rename", opencode_i18n::t!("common.rename"), known))
                .chain(close)
                .chain([Line::Sep, Line::Item(Item { id: "delete", title: asks(opencode_i18n::t!("session.delete.title")), enabled: known, danger: true })])
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
        "close" => {
            if let Some(uid) = t.tab(s).map(crate::tabs::Tab::uid) {
                s.close_tab(&uid, tx);
            }
        }
        "close-others" | "close-right" => {
            let at = t.tab(s).and_then(|x| s.tab_at(&x.uid())).unwrap_or(0);
            let keep = s.tabs.get(at).map(crate::tabs::Tab::uid);
            let gone: Vec<String> = s.tabs.iter().enumerate()
                .filter(|(i, x)| Some(x.uid()) != keep && (id == "close-others" || *i > at))
                .map(|(_, x)| x.uid())
                .collect();
            for g in gone {
                s.close_tab(&g, tx);
            }
        }
        "export" => {
            crate::servers::runtime::with_server(s, &t.server, |s| s.write(Write::Export(t.id.clone())));
        }
        "delete" => s.ask_delete_on(&t.server, &t.id),
        _ => {}
    }
}

impl App {
    /// Starts editing `t`'s title in place, all of it selected; one open elsewhere ends first.
    pub fn begin_rename(&mut self, t: &Target) {
        let Some(title) = crate::servers::runtime::with_server(self, &t.server, |s| s.session(&t.id).map(|i| i.title.clone())).flatten() else { return };
        self.finish_rename(true);
        self.rename_event(rename::Event::Start { target: t.clone(), title }, "");
    }

    /// Ends the edit: with `save`, a changed, non-empty title is sent. Nothing when none is open.
    pub fn finish_rename(&mut self, save: bool) {
        let draft = self.menus.edit.text.clone();
        let event = if save { rename::Event::Submit { draft: &draft, now: *self.clock } } else { rename::Event::Cancel };
        self.rename_event(event, "");
    }

    /// Runs `event` through the rename machine and carries out what it asks.
    fn rename_event(&mut self, event: rename::Event<'_>, why: &str) {
        let m = &mut *self.menus;
        let waiting = match &m.save {
            rename::Save::Saving { session, optimistic, .. } => Some((session.clone(), optimistic.clone())),
            _ => None,
        };
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
                rename::Effect::Remember { session, server, title } => {
                    crate::servers::runtime::with_server(self, &server, |s| {
                        if let Some(info) = s.sessions.iter_mut().find(|s| s.id == session) {
                            info.title = title;
                            s.save_tabs();
                        }
                    });
                }
                rename::Effect::Save { session, server, title } => {
                    // A story's write goes nowhere and a build that can't write sends none: each
                    // answers at once.
                    let (sent, current) = crate::servers::runtime::with_server(self, &server, |s| {
                        let sent = s.write(Write::Rename { session: session.clone(), title: title.clone() });
                        (sent, s.session(&session).map(|i| i.title.clone()))
                    })
                    .unwrap_or((false, None));
                    if self.stage.is_some() || !sent {
                        let why = if sent { why } else { opencode_i18n::t!("rune.send.disconnected") };
                        return self.rename_event(rename::Event::Replied { session: &session, server: &server, title: &title, ok: sent, current: current.as_deref() }, why);
                    }
                }
                // `poll_rename` keeps the save's deadline.
                rename::Effect::WakeAt(_) => {}
                // A refusal in the server's words has its own toast (`from_server`, in
                // `session_write_replied`); this says the rest: not sent, or no answer in time.
                rename::Effect::Failed if why.is_empty() => {
                    self.menus.gave_up = waiting.clone();
                    crate::status_toast::fail(self, opencode_i18n::t!("rune.toast.renameFailed"), opencode_i18n::t!("rune.toast.renameNoAnswer"));
                }
                rename::Effect::Failed if why == opencode_i18n::t!("rune.send.disconnected") => crate::status_toast::fail(self, opencode_i18n::t!("rune.toast.renameFailed"), why),
                rename::Effect::Failed => {}
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
        if let rename::Save::Saving { session, server, due, .. } = &self.menus.save {
            let (session, server, due) = (session.clone(), server.clone(), *due);
            // Only the deadline needs the title: another server is checked out just for it.
            let title = if now + 1e-9 >= due { crate::servers::runtime::with_server(self, &server, |s| s.session(&session).map(|i| i.title.clone())).flatten() } else { None };
            self.rename_event(rename::Event::Tick { now, current: title.as_deref() }, "");
            if matches!(self.menus.save, rename::Save::Saving { .. }) {
                tx.wake_at(due);
            }
        }
        before != self.menus.renaming()
    }

    /// The server answered a rename, a delete or a restore this window sent (`client.session.write`).
    pub(crate) fn session_write_replied(&mut self, data: &serde_json::Value, error: Option<opencode_kit::wire_error::WireError>, tx: &mut Tx) {
        let session = data["sessionID"].as_str().unwrap_or_default().to_owned();
        let ok = data["ok"].as_bool() == Some(true);
        // What the machines below keep of a failure (an inline note); the toast is said from the
        // typed error itself, once, at the end.
        let why = error.as_ref().map(|e| crate::errors::said(self, "", e)).unwrap_or_default();
        let why = why.as_str();
        match data["kind"].as_str() {
            Some("rename") => {
                let title = data["title"].as_str().unwrap_or_default().to_owned();
                // The reply comes in on its own server, checked out for it (`servers::runtime::poll`).
                let server = crate::servers::runtime::active(self).clone();
                let current = self.session(&session).map(|i| i.title.clone());
                // A save this window already gave up on (and said so) says nothing more when it fails late.
                if self.menus.gave_up.as_ref().is_some_and(|(s, t)| *s == session && *t == title) {
                    self.menus.gave_up = None;
                    if !ok {
                        self.rename_event(rename::Event::Replied { session: &session, server: &server, title: &title, ok, current: current.as_deref() }, why);
                        return;
                    }
                }
                self.rename_event(rename::Event::Replied { session: &session, server: &server, title: &title, ok, current: current.as_deref() }, why);
            }
            Some("delete") => {
                let server = crate::servers::runtime::active(self).clone();
                self.sent_event(delete::SentEvent::Replied { session, server, ok }, why, tx);
            }
            Some("unarchive") => self.switcher_restore_replied(session, ok, why, tx),
            _ => {}
        }
        if let Some(e) = error {
            crate::status_toast::from_wire(self, e);
        }
    }

    /// Asks before deleting `id` on the server the window works against.
    pub fn ask_delete(&mut self, id: &str) {
        let server = crate::servers::runtime::active(self).clone();
        self.ask_delete_on(&server, id);
    }

    /// Asks before deleting `id` on `server`, named by its title there.
    pub fn ask_delete_on(&mut self, server: &ServerKey, id: &str) {
        let title = crate::servers::runtime::with_server(self, server, |s| s.session(id).map(crate::view::title_of)).flatten().unwrap_or_else(|| opencode_i18n::t!("command.session.new").to_owned());
        let asked = delete::Asked { session: id.to_owned(), server: server.clone(), title };
        self.delete_event(delete::Event::Ask(asked), &mut Tx::default());
    }

    /// Cancel, Escape or a press outside Delete's confirmation.
    pub fn cancel_delete(&mut self, tx: &mut Tx) {
        self.delete_event(delete::Event::Cancel, tx);
    }

    /// Delete: the session the confirmation asks about and its subagents leave its server's
    /// list, and the toast that offers them back sends the delete when it goes (`delete`).
    pub fn delete_session(&mut self, tx: &mut Tx) {
        let Some(asked) = self.menus.asked().cloned() else { return };
        let tree = crate::servers::runtime::with_server(self, &asked.server, |s| {
            let mut gone = vec![asked.session.clone()];
            let mut at = 0;
            while at < gone.len() {
                let parent = gone[at].clone();
                gone.extend(s.sessions.iter().filter(|s| s.parent.as_deref() == Some(&parent)).map(|s| s.id.clone()));
                at += 1;
            }
            gone.iter().filter_map(|g| s.session(g).cloned()).collect()
        })
        .unwrap_or_default();
        self.delete_event(delete::Event::Confirm { tree }, tx);
    }

    /// The deadline of the deletes sent.
    pub fn poll_delete(&mut self, now: f64, tx: &mut Tx) -> bool {
        // Only a delete in flight hears the clock: polls leave the menus untouched otherwise, so
        // regions that read them stay reused.
        let Some(due) = self.menus.sent.next_due() else { return false };
        if now + 1e-9 < due {
            tx.wake_at(due);
            return false;
        }
        self.sent_event(delete::SentEvent::Tick { now }, "", tx);
        true
    }

    /// Runs `event` through Delete's confirmation and carries out what it asks.
    fn delete_event(&mut self, event: delete::Event, tx: &mut Tx) {
        for effect in delete::step(&mut self.menus.delete, event) {
            tx.instant();
            match effect {
                delete::Effect::Hide(server, ids) => {
                    crate::servers::runtime::with_server(self, &server, |s| s.sessions.retain(|s| !ids.contains(&s.id)));
                }
                delete::Effect::Defer { server, session, hidden } => {
                    crate::undo::offer(self, opencode_i18n::t!("rune.undo.sessionDeleted"), crate::undo::Undoable::Deleted { server, session, hidden });
                }
            }
        }
    }

    /// The delete of `session` on `server` goes now: its undo toast went, or the app quits.
    pub(crate) fn send_delete(&mut self, server: ServerKey, session: String, hidden: Vec<SessionInfo>, tx: &mut Tx) {
        let now = *self.clock;
        self.sent_event(delete::SentEvent::Sent { server: server.clone(), session: session.clone(), hidden, now }, "", tx);
        // A story's write goes nowhere and a build that can't write sends none: each answers at once.
        let sent = crate::servers::runtime::with_server(self, &server, |s| s.write(Write::Delete(session.clone()))).unwrap_or(false);
        if self.stage.is_some() || !sent {
            self.sent_event(delete::SentEvent::Replied { session, server, ok: sent }, "", tx);
        }
    }

    /// Undo on a delete's toast: its sessions are listed again, nothing sent.
    pub(crate) fn undelete(&mut self, server: &ServerKey, hidden: Vec<SessionInfo>) {
        crate::servers::runtime::with_server(self, server, |s| s.unhide(hidden));
    }

    fn unhide(&mut self, hidden: Vec<SessionInfo>) {
        for h in hidden {
            if self.session(&h.id).is_none() {
                self.sessions.push(h);
            }
        }
    }

    /// Runs `event` through the deletes sent and carries out what it asks.
    fn sent_event(&mut self, event: delete::SentEvent, why: &str, tx: &mut Tx) {
        for effect in self.menus.sent.step(event) {
            match effect {
                delete::SentEffect::WakeAt(due) => tx.wake_at(due),
                delete::SentEffect::Removed(server, gone) => {
                    tx.instant();
                    crate::servers::runtime::with_server(self, &server, |s| s.session_removed(&gone));
                }
                delete::SentEffect::Failed(server, hidden) => {
                    tx.instant();
                    crate::servers::runtime::with_server(self, &server, |s| s.unhide(hidden));
                    // A refusal with the server's words has its own toast (`from_server`, in
                    // `session_write_replied`).
                    if why.is_empty() {
                        crate::status_toast::fail(self, opencode_i18n::t!("session.delete.failed.title"), "");
                    }
                }
            }
        }
    }

    /// The server the window works against deleted these sessions: their tabs and history go,
    /// and the open page goes Home if it was theirs (`notifySessionTabsRemoved`).
    fn session_removed(&mut self, gone: &[String]) {
        let server = crate::servers::runtime::active(self).clone();
        self.forget_history(|_, x| gone.iter().any(|g| g == x));
        let open_gone = matches!(&*self.route, Route::Session { id: open, server: on } if *on == server && gone.contains(open));
        self.sessions.retain(|s| !gone.contains(&s.id));
        self.tabs.retain(|t| t.is_draft() || t.server != server || !gone.contains(&t.id));
        self.closed.remove(|id| gone.iter().any(|g| g == id));
        if open_gone {
            *self.route = Route::Home;
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
    let pane_owns = s.side.focused && crate::keymap::BROWSER_RELOAD_PAGE.matches(ev);
    if !crate::keymap::Bind::Rename.matches_tx(ev, tx) || pane_owns || s.stage.is_some() {
        return false;
    }
    let Route::Session { id, .. } = (*s.route).clone() else { return false };
    if s.session(&id).is_none_or(|i| i.parent.is_some()) {
        return false;
    }
    tx.instant();
    s.begin_rename(&Target::new(&id, Spot::Header));
    true
}
