//! Right-click menus on sessions and renaming them in place, as Desktop has them (`tab-nav.tsx`,
//! `home/sessions/view.tsx`, `message-timeline.tsx`). The menus are native on macOS and drawn
//! elsewhere (`opencode_ui::popup`); the actions go through the same calls Desktop's do
//! (`PATCH /api/session/:id` with `{ title }`, `DELETE`, the Export JSON).
//!
//! | Where | Items |
//! | --- | --- |
//! | Sidebar tab | Rename, Close tab, Close other tabs, Close tabs to the right, —, Delete session… |
//! | Home row, session header | Rename, Export…, Make project, —, Delete… |
//! | Recent row | Pin (or Unpin, Move up, Move down), Mute or Unmute, Hide, —, Rename, Archive, —, Delete… |
//! | Inbox card, project sidebar row | Rename (a dialog: `rename_dialog`), Export…, Mark as read or unread, Archive (the inbox), —, Delete… |
//!
//! Rename edits the title where it stands: ⏎ saves, Esc cancels, a click elsewhere saves (the
//! Home row cancels, as Desktop's does). Double-clicking a tab and clicking a root session's
//! header title start it, and ⌃R does for the open session. Delete asks first.

#![forbid(unsafe_code)]

pub mod delete;
pub mod export;
pub mod host;
pub mod preferences;
pub mod rename;
pub mod rename_dialog;
pub mod unread;
pub mod view;

use opencode_kit::model::SessionInfo;
use opencode_kit::servers::ServerKey;
use opencode_kit::wire_error::WireError;
use opencode_ui::keymap::{BROWSER_RELOAD_PAGE, Bind};
use opencode_ui::popup::{self, Menu};
use rune::machine::Seq;
use rune::{El, Key, KeyEvent, PointerPress, PopupAnchor, TextEdit, Tx};

pub use delete::{ask_delete, ask_delete_on, cancel_delete, delete_session, poll_delete, send_delete, undelete};
pub use export::{export_session_on, exporting};
pub use host::{HostAction, HostDeliverer, MenuPick, SaveFileDone, SessionMarks, SessionMenuHost, TabSummary};
pub use rename::{begin_rename, finish_rename, poll_rename, rename_session};
pub use rename_dialog::{cancel as close_rename_dialog, open as open_rename_dialog};

use host::on_server;

/// Overlay stack identifier for the session rename dialog (`opencode_kernel::OverlayStack`).
pub const RENAME_OVERLAY_ID: &str = "dialog.session_rename";

/// Overlay stack identifier for the session delete confirmation dialog (`opencode_kernel::OverlayStack`).
pub const DELETE_OVERLAY_ID: &str = "dialog.session_delete";

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
    /// `id` on `host`'s currently checked-out server.
    pub fn active(host: &dyn SessionMenuHost, id: &str, spot: Spot) -> Target {
        Target::on(&host.active_server(), id, spot)
    }

    /// `id` on `server`.
    pub fn on(server: &ServerKey, id: &str, spot: Spot) -> Target {
        Target { id: id.to_owned(), server: server.clone(), spot }
    }

    /// Whether `tab` is the tab this target names.
    pub fn matches_tab(&self, tab: &TabSummary) -> bool {
        tab.id == self.id && (tab.is_draft || tab.server == self.server)
    }

    /// The tab a tab's target names.
    pub fn tab(&self, host: &dyn SessionMenuHost) -> Option<TabSummary> {
        host.tabs().into_iter().find(|t| self.matches_tab(t))
    }
}

/// Display title for `s`: its title, or Desktop's localized untitled fallback when empty.
pub fn title_of(s: &SessionInfo) -> String {
    if s.title.is_empty() {
        opencode_i18n::t!("rune.session.untitled").into()
    } else {
        s.title.clone()
    }
}

#[derive(Default)]
pub struct Menus {
    pub(crate) exports: export::Exports,
    pub(crate) unread_generation: u64,
    pub(crate) preferences_generation: u64,
    /// The menu showing now, so its row draws marked.
    pub on: Option<Target>,
    /// The title being edited (`rename`), its field's text, and its save.
    pub(crate) rename: rename::Edit,
    pub edit: TextEdit,
    pub(crate) save: rename::Save,
    /// The save this window stopped waiting for and already said so (session, title): its late
    /// failure says nothing more.
    pub(crate) gave_up: Option<(String, String)>,
    pub(crate) seq: Seq,
    /// The inbox's and the project sidebar's Rename dialog, and its field.
    pub(crate) dialog: rename_dialog::State,
    pub dialog_edit: TextEdit,
    /// Delete's confirmation.
    pub(crate) delete: delete::Delete,
    /// Deletes sent (their undo toast went), until the server answers.
    pub(crate) sent: delete::Sent,
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
    pub fn asked(&self) -> Option<&delete::Asked> {
        match &self.delete {
            delete::Delete::Confirming { asked } => Some(asked),
            delete::Delete::Closed => None,
        }
    }

    /// Whether the Rename dialog is currently open (editing or saving).
    pub fn rename_dialog_open(&self) -> bool {
        !matches!(self.dialog, rename_dialog::State::Closed)
    }

    /// Whether an export is currently running for `(server, id)`.
    pub fn exporting(&self, server: &ServerKey, id: &str) -> bool {
        self.exports.is_exporting(server, id)
    }

    /// Current manual-unread hydration generation observed by this window.
    pub fn unread_generation(&self) -> u64 {
        self.unread_generation
    }

    /// Current per-session preferences hydration generation observed by this window.
    pub fn preferences_generation(&self) -> u64 {
        self.preferences_generation
    }

    /// Whether any sent deletes are still awaiting a server reply.
    pub fn has_pending_deletes(&self) -> bool {
        !self.sent.is_empty()
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
    /// Host-provided item row (`pin_menu_extra` / `tab_menu_extra`).
    Extra(popup::Item),
    /// Host-provided submenu row (`Add to group`).
    Sub { id: String, title: String, menu: Menu },
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
pub fn lines(host: &dyn SessionMenuHost, t: &Target) -> Vec<Line> {
    // The session is known once its own server has it: live on the server the window works
    // against, as of its last checkout on another (`props.session` from the tab's server).
    let tabs = if t.spot == Spot::Tab { host.tabs() } else { Vec::new() };
    let tab = (t.spot == Spot::Tab).then(|| tabs.iter().find(|x| t.matches_tab(x))).flatten();
    let here = t.server == host.active_server();
    let known = here.then(|| host.session(&t.id)).flatten();
    let known_elsewhere = !here && host.checkout_known(&t.server, &t.id);
    let draft = tab.is_some_and(|x| x.is_draft);
    let read_item = if host.menus().unread_generation == 0 || t.spot == Spot::Header {
        None
    } else if let Some(info) = known {
        let root = host.root_of(&info.id);
        let marks = host.session_marks(root);
        (!marks.busy && !marks.attention && (marks.unread || info.idle.is_some())).then_some(marks.unread)
    } else {
        host.checkout_read_menu(&t.server, &t.id)
    }
    .map(|read| {
        if read {
            item("read", opencode_i18n::t!("session.markRead"), true)
        } else {
            item("unread", opencode_i18n::t!("session.markUnread"), true)
        }
    });
    match t.spot {
        Spot::Tab => {
            let uid = tab.map(|x| x.uid.as_str()).unwrap_or_default();
            let at = tabs.iter().position(|x| x.uid == uid);
            let known = known.is_some() || known_elsewhere;
            let close = [
                item("close", opencode_i18n::t!("common.closeTab"), true),
                item("close-others", opencode_i18n::t!("common.closeOtherTabs"), tabs.iter().any(|x| x.uid != uid)),
                // Side tabs run down the sidebar (`TabCloseMenuItems`).
                item(
                    "close-right",
                    if host.layout_has_sidebar() {
                        opencode_i18n::t!("common.closeTabsBelow")
                    } else {
                        opencode_i18n::t!("common.closeTabsToRight")
                    },
                    at.is_some_and(|i| i + 1 < tabs.len()),
                ),
            ];
            let extra = host.tab_menu_extra(uid);
            // A draft has no session to rename or delete: only the close items (`DraftTabItem`).
            if draft {
                let sep = (!extra.is_empty()).then_some(Line::Sep);
                return extra.into_iter().chain(sep).chain(close).collect();
            }
            let sep = (!extra.is_empty()).then_some(Line::Sep);
            std::iter::once(item("rename", opencode_i18n::t!("common.rename"), known))
                .chain([item("export", asks(opencode_i18n::t!("common.export")), known && !exporting(host, &t.server, &t.id))])
                .chain(read_item)
                .chain(sep)
                .chain(extra)
                .chain([Line::Sep])
                .chain(close)
                .chain([Line::Sep, item("delete", asks(opencode_i18n::t!("common.delete")), known)])
                .collect()
        }
        Spot::Row | Spot::Header if known.is_none_or(|i| i.parent.is_some() && t.spot == Spot::Header) => vec![],
        Spot::Row | Spot::Header => [
            item("rename", opencode_i18n::t!("common.rename"), true),
            item("export", asks(opencode_i18n::t!("common.export")), !exporting(host, &t.server, &t.id)),
        ]
        .into_iter()
        .chain(read_item)
        .chain([Line::Sep, item("delete", asks(opencode_i18n::t!("common.delete")), true)])
        .collect(),
    }
}

/// `lines` as the menu that shows them.
pub fn menu(lines: &[Line]) -> Menu {
    lines.iter().fold(Menu::new(opencode_i18n::t!("command.category.session")), |m, l| match l {
        Line::Item(i) => m.item(popup::Item::new(i.id, i.title.clone()).enabled(i.enabled).danger(i.danger)),
        Line::Sep => m.separator(),
        Line::Extra(i) => m.item(i.clone()),
        Line::Sub { id, title, menu } => m.submenu(id.clone(), title.clone(), menu.clone()),
    })
}

/// Opens `t`'s menu at `at`, marking its row until it closes. A Home row being renamed keeps
/// the plain field, as Desktop keeps the native menu there.
pub fn open(host: &mut dyn SessionMenuHost, t: Target, at: impl Into<PopupAnchor>, tx: &mut Tx) {
    open_at(host, t, at.into(), false, tx);
}

/// [`open`], its Rename opening the Rename dialog (`RenameDialog`) where Desktop's does.
pub fn open_dialog(host: &mut dyn SessionMenuHost, t: Target, at: impl Into<PopupAnchor>, tx: &mut Tx) {
    open_at(host, t, at.into(), true, tx);
}

/// Non-generic core behind [`open`] and [`open_dialog`].
pub fn open_at(host: &mut dyn SessionMenuHost, t: Target, at: PopupAnchor, dialog: bool, tx: &mut Tx) {
    if host.menus().renaming().is_some_and(|r| r.spot == Spot::Row) {
        return;
    }
    let mut items = lines(host, &t);
    let is_draft = t.tab(host).is_some_and(|x| x.is_draft);
    if !is_draft && (dialog || t.spot == Spot::Tab) {
        if host.navigation_prototype() {
            let mut lead = host.pin_menu_extra(&t.server, &t.id);
            if !lead.is_empty() {
                lead.push(Line::Sep);
                items.splice(0..0, lead);
            }
        } else if host.project_navigation() {
            let pinned = host.is_session_pinned(&t.server, &t.id);
            let muted = preferences::muted(&t.server, &t.id);
            items.splice(
                0..0,
                [
                    item("pin-toggle", if pinned { opencode_i18n::t!("sidebar.recent.unpin") } else { opencode_i18n::t!("sidebar.recent.pin") }, true),
                    item("mute-toggle", if muted { opencode_i18n::t!("sidebar.recent.unmute") } else { opencode_i18n::t!("sidebar.recent.mute") }, true),
                    Line::Sep,
                ],
            );
        }
    }
    if items.is_empty() {
        return;
    }
    finish_rename(host, true);
    host.menus_mut().on = Some(t.clone());
    tx.instant();
    // The header's "…" opens 6 under it (`message-timeline.tsx`: `gutter={6}`).
    let dots = t.spot == Spot::Header && matches!(at, PopupAnchor::Key(_));
    let built = if dots {
        menu(&items).gap(6.0)
    } else if t.spot == Spot::Row && !dialog {
        // home/sessions/view.tsx: Menu.Trigger is a size-px span placed at the press.
        menu(&items).pointer_trigger()
    } else {
        menu(&items)
    };
    host.open_popup(
        built,
        at,
        tx,
        Box::new(move |h, picked, tx| {
            h.menus_mut().on = None;
            tx.instant();
            match picked {
                Some("rename") if dialog => rename_dialog::open(h, &t),
                Some(id) => run(h, &t, id, tx),
                None => {}
            }
        }),
    );
}

/// A right-click on `e` opens `t`'s menu.
pub fn on<H: SessionMenuHost + 'static>(e: El, t: Target) -> El {
    e.on_secondary_press(move |s: &mut H, press: PointerPress, tx: &mut Tx| open(s, t.clone(), press, tx))
}

/// [`on`] for a project sidebar's session row (`project-session-row.tsx:185-197`): Rename opens
/// the dialog.
pub fn on_dialog<H: SessionMenuHost + 'static>(e: El, t: Target) -> El {
    e.on_secondary_press(move |s: &mut H, press: PointerPress, tx: &mut Tx| open_dialog(s, t.clone(), press, tx))
}

/// Runs the menu item `id` on `t`.
pub fn run(host: &mut dyn SessionMenuHost, t: &Target, id: &str, tx: &mut Tx) {
    if host.run_pin_menu_extra(&t.server, &t.id, id, tx) {
        return;
    }
    if t.spot == Spot::Tab
        && let Some(tab) = t.tab(host)
        && host.run_tab_menu_extra(&tab.uid, id, tx)
    {
        return;
    }
    match id {
        "pin-toggle" => host.toggle_session_pin(&t.server, &t.id),
        "mute-toggle" => preferences::update(host, preferences::Kind::Muted, &t.server, &t.id, !preferences::muted(&t.server, &t.id)),
        "rename" => begin_rename(host, t),
        "close" => {
            if let Some(tab) = t.tab(host) {
                host.close_tab(&tab.uid, tx);
            }
        }
        "close-others" | "close-right" => {
            let tabs = host.tabs();
            let at = tabs.iter().position(|x| t.matches_tab(x)).unwrap_or(0);
            let keep = tabs.get(at).map(|x| x.uid.as_str());
            let gone: Vec<String> = tabs
                .iter()
                .enumerate()
                .filter(|(i, x)| Some(x.uid.as_str()) != keep && (id == "close-others" || *i > at))
                .map(|(_, x)| x.uid.clone())
                .collect();
            for g in gone {
                host.close_tab(&g, tx);
            }
        }
        "export" => export::export_session_on(host, &t.server, &t.id),
        "archive" => host.archive_session(&t.server, &t.id, tx),
        "unarchive" => host.unarchive_session(&t.server, &t.id),
        "duplicate" => host.duplicate_session(&t.server, &t.id, tx),
        "read" | "unread" => {
            let _ = on_server(host, &t.server, |h| {
                if id == "read" {
                    unread::read(h, &t.id);
                } else {
                    let root = h.root_of(&t.id).to_owned();
                    unread::set(h, &t.server, &root, true);
                }
            });
        }
        "delete" => ask_delete_on(host, &t.server, &t.id),
        _ => {}
    }
}

/// The server answered a rename, a delete or a restore this window sent (`client.session.write`).
pub fn session_write_replied(host: &mut dyn SessionMenuHost, data: &serde_json::Value, error: Option<WireError>, tx: &mut Tx) {
    let session = data["sessionID"].as_str().unwrap_or_default().to_owned();
    let ok = data["ok"].as_bool() == Some(true);
    // What the machines below keep of a failure (an inline note); the toast is said from the
    // typed error itself, once, at the end.
    let why = error.as_ref().map(|e| host.format_wire_error("", e)).unwrap_or_default();
    let why = why.as_str();
    match data["kind"].as_str() {
        Some("rename") => {
            let title = data["title"].as_str().unwrap_or_default().to_owned();
            // The reply comes in on its own server, checked out for it (`servers::runtime::poll`).
            let server = host.active_server();
            let current = host.session(&session).map(|i| i.title.clone());
            // A save this window already gave up on (and said so) says nothing more when it fails late.
            if host.menus().gave_up.as_ref().is_some_and(|(s, t)| *s == session && *t == title) {
                host.menus_mut().gave_up = None;
                if !ok {
                    rename::dispatch(
                        host,
                        rename::Event::Replied { session: &session, server: &server, title: &title, ok, current: current.as_deref() },
                        why,
                    );
                    return;
                }
            }
            rename::dispatch(
                host,
                rename::Event::Replied { session: &session, server: &server, title: &title, ok, current: current.as_deref() },
                why,
            );
            let draft = host.menus().dialog_edit.text.clone();
            rename_dialog::dispatch(
                host,
                rename_dialog::Event::Replied { session: &session, server: &server, title: &title, ok, draft: &draft },
                why,
            );
        }
        Some("delete") => {
            let server = host.active_server();
            delete::sent_event(host, delete::SentEvent::Replied { session, server, ok }, why, tx);
        }
        Some("unarchive") => host.on_unarchive_replied(session, ok, why, tx),
        _ => {}
    }
    if let Some(e) = error {
        host.toast_wire_error(e);
    }
}

/// ⌃R renames the open session; Esc and ⏎ end an edit; Esc closes Delete's confirmation or the
/// Rename dialog.
pub fn key(host: &mut dyn SessionMenuHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if host.menus().confirming().is_some() {
        if ev.key == Key::Escape {
            cancel_delete(host, tx);
            return true;
        }
        return false;
    }
    if host.menus().rename_dialog_open() {
        if host.menus().dialog_edit.is_composing() {
            return false;
        }
        if ev.key == Key::Escape {
            rename_dialog::cancel(host, tx);
            return true;
        }
        return false;
    }
    if host.menus().renaming().is_some() {
        if host.menus().edit.is_composing() {
            return false;
        }
        match ev.key {
            Key::Escape => finish_rename(host, false),
            Key::Enter if !ev.mods.shift => finish_rename(host, true),
            _ => return false,
        }
        tx.instant();
        return true;
    }
    // Off macOS ⌃R is Ctrl+R, the Browser's reload; a focused pane keeps it, as Desktop's
    // `session.rename` skips plugin panes.
    let pane_owns = host.side_focused() && BROWSER_RELOAD_PAGE.matches(ev);
    if !Bind::Rename.matches_tx(ev, tx) || pane_owns || host.stage_active() {
        return false;
    }
    let Some(id) = host.active_session_id().map(str::to_owned) else {
        return false;
    };
    if host.session(&id).is_none_or(|i| i.parent.is_some()) {
        return false;
    }
    tx.instant();
    let server = host.active_server();
    begin_rename(host, &Target::on(&server, &id, Spot::Header));
    true
}
