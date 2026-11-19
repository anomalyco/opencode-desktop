//! Desktop host adapter for `opencode-feature-session-menu` (`features/session-menu`).

pub use opencode_feature_session_menu::*;

use opencode_kernel::{CommandEntry, Feature, KernelBuilder, OverlayRegistration, Phase, Slot};
use opencode_kit::client::{Server, Write};
use opencode_kit::model::{SessionInfo, Transcript};
use opencode_kit::servers::ServerKey;
use opencode_kit::wire_error::WireError;
use opencode_ui::popup::{Menu, PopupAnchor};
use rune::{El, FileDialogOptions, Tx};
use serde_json::Value;
use session_view::palette::Pal;

use crate::keymap::Bind;
use crate::{App, Route, Source};

/// A right-click on `e` opens `t`'s menu (specialized to [`App`]).
pub fn on(e: El, t: Target) -> El {
    opencode_feature_session_menu::on::<App>(e, t)
}

/// [`on`] for a project sidebar's session row where Rename opens the Rename dialog.
pub fn on_dialog(e: El, t: Target) -> El {
    opencode_feature_session_menu::on_dialog::<App>(e, t)
}

pub(crate) static MENUS: Slot<Menus> = Slot::new();

impl SessionMenuHost for App {
    fn menus(&self) -> &Menus {
        self.kernel.slots.get(&MENUS)
    }

    fn menus_mut(&mut self) -> &mut Menus {
        self.kernel.slots.get_mut(&MENUS)
    }

    fn sessions(&self) -> &[SessionInfo] {
        &self.sessions
    }

    fn sessions_mut(&mut self) -> Option<&mut Vec<SessionInfo>> {
        Some(&mut self.sessions)
    }

    fn session(&self, id: &str) -> Option<&SessionInfo> {
        App::session(self, id)
    }

    fn transcript(&self, id: &str) -> Option<&Transcript> {
        self.transcripts.get(id)
    }

    fn root_of<'a>(&'a self, id: &'a str) -> &'a str {
        App::root_of(self, id)
    }

    fn session_marks(&self, root_id: &str) -> SessionMarks {
        let m = self.marks(root_id);
        SessionMarks { attention: m.attention, busy: m.busy, unread: m.unread }
    }

    fn tabs(&self) -> Vec<TabSummary> {
        self.tab_summaries()
    }

    fn layout_has_sidebar(&self) -> bool {
        self.tab_layout().has_sidebar()
    }

    fn project_navigation(&self) -> bool {
        App::project_navigation(self)
    }

    fn side_focused(&self) -> bool {
        self.kernel.slots.get(&crate::panels::SIDE).focus.is_focused()
    }

    fn stage_active(&self) -> bool {
        self.fixture_run.stage.is_some()
    }

    fn active_server(&self) -> ServerKey {
        crate::servers::runtime::active(self).clone()
    }

    fn boot_ready(&self) -> bool {
        self.checkout.boot == crate::servers::runtime::Boot::Ready
    }

    fn live_server(&self) -> Option<&Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server),
            Source::Fixture => None,
        }
    }

    fn checkout_known(&self, server: &ServerKey, id: &str) -> bool {
        self.checkout.known(server, id)
    }

    fn checkout_read_menu(&self, server: &ServerKey, id: &str) -> Option<bool> {
        self.checkout.read_menu(server, id)
    }

    fn with_server(&mut self, server: &ServerKey, f: &mut dyn FnMut(&mut dyn SessionMenuHost)) -> bool {
        crate::servers::runtime::with_server(self, server, |s| f(s)).is_some()
    }

    fn write_server(&mut self, write: Write) -> bool {
        App::write(self, write)
    }

    fn server_label(&self, server: &ServerKey) -> String {
        crate::servers::runtime::label(server)
    }

    fn format_wire_error(&self, what: &str, error: &WireError) -> String {
        crate::errors::said(self, what, error)
    }

    fn open_popup(&mut self, menu: Menu, anchor: PopupAnchor, tx: &mut Tx, on_pick: MenuPick) {
        crate::popup::open(self, menu, anchor, tx, move |s: &mut App, picked, tx| on_pick(s, picked, tx));
    }

    fn popup_surface(&self, p: &Pal, menu: &Menu, active: Option<usize>) -> El {
        crate::popup::surface(p, menu, active)
    }

    fn toast_fail(&mut self, title: &str, description: &str) {
        crate::status_toast::fail(self, title, description);
    }

    fn toast_succeed(&mut self, title: &str, description: &str) {
        crate::status_toast::succeed(self, title, description);
    }

    fn toast_wire_error(&mut self, error: WireError) {
        crate::status_toast::from_wire(self, error);
    }

    fn offer_delete_undo(&mut self, title: &str, server: ServerKey, session: String, hidden: Vec<SessionInfo>) {
        crate::undo::offer(self, title, crate::undo::Undoable::Deleted { server, session, hidden });
    }

    fn storage_ready(&self) -> bool {
        crate::storage::is_ready()
    }

    fn storage_read_json(&self, namespace: &str, key: &str) -> Option<Value> {
        crate::storage::json(namespace, key)
    }

    fn storage_write_json(&mut self, namespace: &str, key: &str, value: &Value) {
        crate::storage::set_json(namespace, key, value);
    }

    fn touch_peers(&mut self) {
        crate::windows::peer::touch(self);
    }

    fn on_rename_focus_taken(&mut self) {
        self.composer.focused = false;
        self.search.focused = false;
    }

    fn save_tabs(&mut self) {
        App::save_tabs(self);
    }

    fn open_session(&mut self, server: &ServerKey, id: &str, tx: &mut Tx) {
        crate::servers::runtime::with_server(self, server, |s| s.open(id, tx));
    }

    fn close_tab(&mut self, uid: &str, tx: &mut Tx) {
        App::close_tab(self, uid, tx);
    }

    fn is_session_pinned(&self, server: &ServerKey, id: &str) -> bool {
        let key = crate::view::sidebar::state::key(server.as_str(), id);
        self.project_state.recent.pins.contains(&key)
    }

    fn toggle_session_pin(&mut self, server: &ServerKey, id: &str) {
        let key = crate::view::sidebar::state::key(server.as_str(), id);
        let recent = &mut self.project_state.recent;
        if recent.pins.contains(&key) {
            recent.unpin(&key);
        } else {
            recent.pin(key);
        }
    }

    fn archive_session(&mut self, server: &ServerKey, id: &str, tx: &mut Tx) {
        crate::servers::runtime::with_server(self, server, |s| App::archive_session(s, id, tx));
    }

    fn unarchive_session(&mut self, server: &ServerKey, id: &str) {
        crate::servers::runtime::with_server(self, server, |s| App::unarchive_session(s, id));
    }

    fn on_sessions_removed(&mut self, server: &ServerKey, gone: &[String]) {
        self.forget_history(|_, x| gone.iter().any(|g| g == x));
        let open_gone = matches!(&*self.route, Route::Session { id: open, server: on } if on == server && gone.contains(open));
        self.tabs.retain(|t| t.is_draft() || t.server != *server || !gone.contains(&t.id));
        self.gestures.closed.remove(|id| gone.iter().any(|g| g == id));
        if open_gone {
            *self.route = Route::Home;
        }
        App::save_tabs(self);
    }

    fn on_unarchive_replied(&mut self, session: String, ok: bool, why: &str, tx: &mut Tx) {
        self.switcher_restore_replied(session, ok, why, tx);
    }

    fn deliverer(&self) -> Option<HostDeliverer> {
        let deliver = App::deliverer(self);
        Some(Box::new(move |action: HostAction| {
            deliver(Box::new(move |s: &mut App, tx: &mut Tx| action(s, tx)));
        }))
    }

    fn save_file(&mut self, options: FileDialogOptions, tx: &mut Tx, on_done: SaveFileDone) {
        #[cfg(not(target_arch = "wasm32"))]
        tx.save_file(options, move |s: &mut App, answer, tx| {
            on_done(s, answer.map_err(|e| e.0), tx);
        });
        #[cfg(target_arch = "wasm32")]
        let _ = (options, tx, on_done);
    }
}

impl App {
    pub fn begin_rename(&mut self, t: &Target) {
        begin_rename(self, t);
    }

    pub fn finish_rename(&mut self, save: bool) {
        finish_rename(self, save);
    }

    #[allow(dead_code)]
    pub fn rename_session(&mut self, id: &str, title: &str) -> bool {
        rename_session(self, id, title)
    }

    #[allow(dead_code)]
    pub fn poll_rename(&mut self, now: f64, tx: &mut Tx) -> bool {
        poll_rename(self, now, tx)
    }

    pub fn open_rename_dialog(&mut self, t: &Target) {
        open_rename_dialog(self, t);
    }

    pub(crate) fn session_write_replied(&mut self, data: &Value, error: Option<WireError>, tx: &mut Tx) {
        session_write_replied(self, data, error, tx);
    }

    pub fn ask_delete(&mut self, id: &str) {
        ask_delete(self, id);
    }

    pub fn ask_delete_on(&mut self, server: &ServerKey, id: &str) {
        ask_delete_on(self, server, id);
    }

    pub fn cancel_delete(&mut self, tx: &mut Tx) {
        cancel_delete(self, tx);
    }

    #[allow(dead_code)]
    pub fn delete_session(&mut self, tx: &mut Tx) {
        delete_session(self, tx);
    }

    #[allow(dead_code)]
    pub fn poll_delete(&mut self, now: f64, tx: &mut Tx) -> bool {
        poll_delete(self, now, tx)
    }

    pub(crate) fn send_delete(&mut self, server: ServerKey, session: String, hidden: Vec<SessionInfo>, tx: &mut Tx) {
        send_delete(self, server, session, hidden, tx);
    }

    pub(crate) fn undelete(&mut self, server: &ServerKey, hidden: Vec<SessionInfo>) {
        undelete(self, server, hidden);
    }

    pub(crate) fn exporting(&self, server: &ServerKey, id: &str) -> bool {
        exporting(self, server, id)
    }

    pub(crate) fn export_session_on(&mut self, server: &ServerKey, id: &str) {
        export_session_on(self, server, id);
    }
}

/// Kernel feature registration for session context menus, rename, delete, unread, and preferences.
pub struct SessionMenuFeature;

impl Feature<App> for SessionMenuFeature {
    fn id(&self) -> &'static str {
        "feature.session_menu"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&MENUS);
        builder.add_command(
            CommandEntry::<App>::from_key(
                "rename",
                opencode_i18n::key!("command.session.rename"),
                crate::commands::order::RENAME,
                |s, tx| {
                    let Some(id) = s.open_root_session() else { return };
                    tx.instant();
                    s.begin_rename(&Target::active(s, &id, Spot::Header));
                },
            )
            .bind_id(Bind::Rename.id())
            .when(App::in_root_session),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "export",
                opencode_i18n::key!("command.session.export"),
                crate::commands::order::EXPORT,
                |s, _| {
                    let Some(id) = s.open_root_session() else { return };
                    s.export_session_on(&crate::servers::runtime::current(), &id);
                },
            )
            .describe(opencode_i18n::key!("command.session.export.description"))
            .slash("export")
            .section("session")
            .when(|s| s.open_root_session().is_some_and(|id| !s.exporting(crate::servers::runtime::active(s), &id))),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "copy-session",
                opencode_i18n::key!("command.session.copyID"),
                crate::commands::order::COPY_SESSION,
                |s, tx| {
                    let id = s.active_session_id().map(str::to_owned);
                    s.copy_id(id, tx);
                },
            )
            .section("session")
            .when(|s| s.active_session_id().is_some()),
        );
        // Delete session (`message-timeline.tsx:268-277`): the open root session's confirmation.
        builder.add_command(
            CommandEntry::<App>::from_key(
                "session.delete",
                opencode_i18n::key!("command.session.delete"),
                crate::commands::order::SESSION_DELETE,
                |s, tx| {
                    let Some(id) = s.open_root_session() else { return };
                    tx.instant();
                    s.ask_delete(&id);
                },
            )
            .when(App::in_root_session),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new(DELETE_OVERLAY_ID)
                .with_z(200)
                .with_key_priority(220)
                .with_render(|s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    view::confirm(s, p, cx)
                })
                .with_key(|s, ev, tx| key(s, ev, tx))
                .with_dismiss(|s, tx| cancel_delete(s, tx)),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new(RENAME_OVERLAY_ID)
                .with_z(210)
                .with_render(|s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    rename_dialog::view(s, p, cx)
                })
                .with_dismiss(|s, tx| close_rename_dialog(s, tx)),
        );
        builder.add_system_ordered(
            Phase::Ingest,
            crate::tick::order::ingest::SESSION_MENU_UNREAD,
            "session_menu.unread",
            |s, _, _| unread::poll(s),
        );
        builder.add_system_ordered(
            Phase::Ingest,
            crate::tick::order::ingest::SESSION_MENU_PREFERENCES,
            "session_menu.preferences",
            |s, _, _| preferences::poll(s),
        );
        builder.add_system_ordered(
            Phase::Derive,
            crate::tick::order::derive::SESSION_MENU_RENAME,
            "session_menu.poll_rename",
            |s, now, tx| poll_rename(s, now, tx),
        );
        builder.add_system_ordered(
            Phase::Derive,
            crate::tick::order::derive::SESSION_MENU_DELETE,
            "session_menu.poll_delete",
            |s, now, tx| poll_delete(s, now, tx),
        );
    }
}
