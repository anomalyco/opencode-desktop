//! Desktop host adapter for `opencode-feature-switcher` (`features/switcher`).

pub use opencode_feature_switcher::*;

use std::hash::{DefaultHasher, Hash, Hasher};

use opencode_kernel::{CommandEntry, Feature, KernelBuilder, OverlayRegistration, Slot};
use opencode_kit::client::Server;
use opencode_kit::model::{Project, SessionInfo};
use opencode_kit::servers::ServerKey;
use rune::{El, KeyEvent, Tx, Waker};
use session_view::palette::Pal;

use crate::keymap::Bind;
use crate::pickers::PickerHost as _;
use crate::{App, Route, Source};

pub(crate) static SWITCHER: Slot<Switcher> = Slot::new();

impl SwitcherHost for App {
    fn switcher(&self) -> &Switcher {
        self.kernel.slots.get(&SWITCHER)
    }

    fn switcher_mut(&mut self) -> &mut Switcher {
        self.kernel.slots.get_mut(&SWITCHER)
    }

    fn session_search(&self) -> &SessionSearch {
        &self.session_search
    }

    fn session_search_mut(&mut self) -> &mut SessionSearch {
        &mut self.session_search
    }

    fn sessions(&self) -> &[SessionInfo] {
        &self.sessions
    }

    fn session(&self, id: &str) -> Option<&SessionInfo> {
        App::session(self, id)
    }

    fn push_session(&mut self, session: SessionInfo) {
        self.sessions.push(session);
    }

    fn project(&self, id: &str) -> Option<&Project> {
        App::project(self, id)
    }

    fn project_for_session(&self, session: &SessionInfo) -> Option<&Project> {
        App::project_for_session(self, session)
    }

    fn sessions_version(&self) -> (u32, u64) {
        (rune::Tracked::id(&self.sessions), rune::Tracked::version(&self.sessions))
    }

    fn projects_version(&self) -> (u32, u64) {
        (rune::Tracked::id(&self.projects), rune::Tracked::version(&self.projects))
    }

    fn is_home(&self) -> bool {
        *self.route == Route::Home
    }

    fn is_session_open(&self, id: &str) -> bool {
        *self.route == Route::here(id.to_owned())
    }

    fn is_tab_open(&self, server: Option<&ServerKey>, id: &str) -> bool {
        match server {
            Some(key) => self.tabs.iter().any(|t| t.is(key, id)),
            None => self.tabs.iter().any(|t| t.id == id),
        }
    }

    fn settings_open(&self) -> bool {
        App::settings_open(self)
    }

    fn is_stage(&self) -> bool {
        self.fixture_run.stage.is_some()
    }

    fn home_search_active(&self) -> bool {
        matches!(*self.route, Route::Home) && (self.search.focused || !self.search.text.is_empty())
    }

    fn view_runs(&self) -> u64 {
        App::view_runs(self)
    }

    fn peer_waker(&self) -> Option<Waker> {
        crate::windows::peer::waker(self)
    }

    fn narrow_width(&self) -> f32 {
        self.layout.narrow.width()
    }

    fn live_server(&self) -> Option<Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server.clone()),
            Source::Fixture => None,
        }
    }

    fn active_server(&self) -> ServerKey {
        crate::servers::runtime::active(self).clone()
    }

    fn server_label(&self, key: &ServerKey) -> String {
        crate::servers::registry::label_of(self, key)
    }

    fn server_health_failed(&self, key: &ServerKey) -> bool {
        opencode_servers::health::is_failed(crate::servers::health::get(key).as_ref())
    }

    fn remote_server_seeds(&mut self, _archived: bool) -> Vec<RemoteSeed> {
        let visible = crate::servers::registry::visible(self);
        let active = crate::servers::runtime::active(self).clone();
        let usable: Vec<ServerKey> = visible.iter().map(|e| e.key()).collect();
        let count = usable.len();
        let mut out = Vec::new();
        for key in usable.into_iter().filter(|k| *k != active) {
            let tag = (count >= 2 && !key.is_sidecar()).then(|| crate::servers::registry::label_of(self, &key));
            let read = |s: &mut App| {
                let conn = match &*s.source {
                    Source::Live { server, .. } => Some(server.clone()),
                    Source::Fixture => None,
                };
                let known: Vec<SessionInfo> = s.sessions.iter().filter(|i| i.parent.is_none()).cloned().collect();
                (conn, known, (*s.projects).clone())
            };
            let Some((Some(conn), known, projects)) = crate::servers::runtime::with_server(self, &key, read) else {
                continue;
            };
            out.push(RemoteSeed { server: key, tag, conn, known, projects });
        }
        out
    }

    fn switch_server(&mut self, server: &ServerKey) {
        crate::servers::runtime::switch(self, server);
    }

    fn unarchive_session(&mut self, id: String) -> bool {
        self.write(opencode_kit::client::Write::Unarchive(id))
    }

    fn unarchive_session_locally(&mut self, id: &str) {
        App::unarchive_session_locally(self, id);
    }

    fn palette_commands(&self) -> Vec<PaletteCommand> {
        App::palette_commands(self)
            .into_iter()
            .map(|c| PaletteCommand {
                id: c.id,
                title: c.title.into(),
                description: c.description.into(),
                hint: c.keys,
                chord: c.keys.is_none().then(|| c.effective_chord()).flatten(),
                featured: c.featured,
            })
            .collect()
    }

    fn run_command(&mut self, id: &'static str, tx: &mut Tx) {
        App::run_command(self, id, tx);
    }

    fn palette_settings(&self, query: &str) -> Vec<PaletteSetting> {
        crate::settings::search::palette(self, query, usize::MAX)
            .into_iter()
            .filter(|e| !e.entity && !e.id.starts_with("shortcut:"))
            .map(|e| PaletteSetting { id: e.id, title: e.title, where_: e.where_ })
            .collect()
    }

    fn reveal_setting(&mut self, id: &str, tx: &mut Tx) {
        if let Some(e) = crate::settings::search::index(self).into_iter().find(|e| e.id == id) {
            crate::settings::search::reveal(self, &e, tx);
        }
    }

    fn side_tab_keys(&self, session_id: &str) -> Option<(Option<&str>, &[String])> {
        let tabs = &self.kernel.slots.get(&crate::panels::SIDE).sessions.get(session_id)?.tab_state.tabs;
        Some((tabs.active.as_deref(), &tabs.all))
    }

    fn local_workspace_files(&self, session_id: &str, query: &str, limit: usize) -> Vec<String> {
        self.kernel.slots.get(&crate::files::FILES).sessions.get(session_id).map(|w| w.local_files(query, limit)).unwrap_or_default()
    }

    fn fixture_find(&self) -> Option<(Vec<String>, Option<f64>)> {
        let extra = self.fixture_run.fixture["find_files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str().map(str::to_owned))
            .collect();
        Some((extra, self.fixture_run.fixture["find_ms"].as_f64()))
    }

    fn ensure_workspace_files(&mut self, session_id: &str) {
        App::ensure_workspace_files(self, session_id);
    }

    fn open_workspace_file(&mut self, path: &str, preview: bool, line: Option<u32>, tx: &mut Tx) {
        App::open_workspace_file(self, path, preview, line, tx);
    }

    fn palette_fulltext_rows(&self) -> Vec<FullTextRow> {
        self.fulltext_rows(crate::fulltext::Surface::Palette)
            .iter()
            .map(|r| FullTextRow {
                session: r.session.clone(),
                kind: r.kind,
                message: r.message.clone(),
                snippet: r.snippet.clone(),
                more: r.more,
                fallback: r.fallback.clone(),
            })
            .collect()
    }

    fn palette_indexing_history(&self) -> bool {
        self.indexing_history(crate::fulltext::Surface::Palette)
    }

    fn before_switcher_open(&mut self) -> bool {
        // The palette lists the commands a plugin offers as their `enabled` and `hidden` say.
        crate::plugin_host::dormant::wake_kind(self, "commands", &["enabled", "hidden"]);
        let restore = self.composer.focused || self.kernel.slots.get(&crate::project_switcher::PROJECT_SWITCHER).restore_composer();
        self.close_picker(false);
        self.close_project_switcher(false);
        self.project_state.home_menu_open = false;
        self.kernel.slots.get_mut(&crate::session_menu::MENUS).edit.focused = false;
        self.composer.focused = false;
        self.search.focused = false;
        self.kernel.slots.get_mut(&crate::find::FIND).input.focused = false;
        restore
    }

    fn after_switcher_close(&mut self, restore_composer: bool) {
        self.composer.focused = restore_composer;
    }

    fn prefetch_session(&self, id: &str) {
        App::prefetch_session(self, id);
    }

    fn open_session(&mut self, id: &str, find: Option<(&str, &str)>, tx: &mut Tx) {
        self.open(id, tx);
        // The palette is gone: show the chosen shell, not the previous transcript,
        // while the existing page request finishes. Keep its retry/back state.
        if let Some(open) = &mut *self.opening {
            open.step(crate::session_pages::OpenEvent::Expired);
        }
        if let Some((query, message)) = find {
            self.open_find_at(query, message, tx);
        }
    }

    fn open_inbox_session(&mut self, id: &str, tx: &mut Tx) {
        App::open_inbox_session(self, id, tx);
    }

    fn handle_shortcut(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.shortcut(ev, tx)
    }

    fn is_session_hidden(&self, server: &ServerKey, id: &str) -> bool {
        crate::session_menu::preferences::hidden(server, id)
    }

    fn unhide_session(&mut self, server: &ServerKey, id: &str) {
        crate::session_menu::preferences::update(self, crate::session_menu::preferences::Kind::Hidden, server, id, false);
    }

    fn session_marks(&self, id: &str) -> SessionMarks {
        let m = self.marks(id);
        SessionMarks { attention: m.attention, busy: m.busy, unread: m.unread }
    }

    fn session_avatar(&self, p: &Pal, project: Option<&Project>, name: &str, open: bool, unread: bool) -> El {
        crate::icons::session_avatar(self, p, project, name, open, unread)
    }

    fn avatar_memo_version(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (crate::icons::finish(self), rune::Tracked::version(&self.appearance_state.icons), rune::Tracked::version(&self.projects)).hash(&mut h);
        h.finish()
    }
}

impl App {
    pub fn open_switcher(&mut self) {
        open(self);
    }

    pub fn open_session_search(&mut self, tx: &mut Tx) {
        open_session_search(self, tx);
    }

    pub fn open_command_search(&mut self, tx: &mut Tx) {
        open_command_search(self, tx);
    }

    pub fn open_file_search(&mut self, tx: &mut Tx) {
        open_file_search(self, tx);
    }

    pub fn open_archived_search(&mut self, tx: &mut Tx) {
        open_archived_search(self, tx);
    }

    pub fn close_switcher(&mut self, restore: bool) {
        close(self, restore);
    }

    pub fn switcher_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        key(self, ev, tx)
    }

    pub fn switcher_nav(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        nav(self, ev, tx)
    }

    pub fn switcher_entries(&self) -> Vec<Entry> {
        entries(self)
    }

    pub fn switcher_active(&self, rows: &[Entry]) -> Option<String> {
        active(self, rows)
    }

    pub(crate) fn switcher_restore_replied(&mut self, id: String, ok: bool, why: &str, tx: &mut Tx) {
        restore_replied(self, id, ok, why, tx);
    }

    pub fn poll_switcher(&mut self, tx: &mut Tx, local_answers: bool) -> bool {
        poll(self, tx, local_answers)
    }

    pub(crate) fn poll_session_search(&mut self, now: f64, tx: &mut Tx) -> bool {
        poll_session_search(self, now, tx)
    }

    pub(crate) fn rest_session_search(&mut self) {
        rest_session_search(self);
    }

    pub(crate) fn find_sessions(&self, scope: Scope, q: &str, limit: usize) -> Vec<(i32, i64, Box<str>)> {
        find_sessions(self, scope, q, limit)
    }

    pub(crate) fn search_session(&self, id: &str) -> Option<&SessionInfo> {
        search_session(self, id)
    }

    pub(crate) fn session_search_building(&self) -> bool {
        session_search_building(self)
    }

    pub(crate) fn merged_roots<'a>(&'a self, extra: &'a [SessionInfo]) -> Vec<&'a SessionInfo> {
        merged_roots_for(self, extra)
    }

    pub(crate) fn project_label(&self, s: &SessionInfo) -> String {
        project_label(self, s)
    }
}

/// Kernel feature registration for the command/file/session palette and project switcher.
pub struct SwitcherFeature;

impl Feature<App> for SwitcherFeature {
    fn id(&self) -> &'static str {
        "feature.switcher"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_command(
            CommandEntry::<App>::from_key(
                "select-project",
                opencode_i18n::key!("session.new.project.search"),
                crate::commands::order::SELECT_PROJECT,
                |s, tx| s.toggle_project_switcher(tx),
            )
            .bind_id(Bind::SelectProject.id()),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "sessions",
                opencode_i18n::key!("command.session.search"),
                crate::commands::order::SESSIONS,
                |s, tx| s.open_session_search(tx),
            )
            .bind_id(Bind::SessionSearch.id()),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                crate::commands::SEARCH,
                opencode_i18n::key!("command.commands.search"),
                crate::commands::order::COMMANDS,
                |s, tx| s.open_command_search(tx),
            )
            .bind_id(Bind::CommandSearch.id()),
        );
        // Desktop's `file.open` (`use-session-commands.tsx:551-558`): Ctrl+P's search, as `/open`.
        builder.add_command(
            CommandEntry::<App>::from_key(
                "file.open",
                opencode_i18n::key!("command.file.open"),
                crate::commands::order::FILE_OPEN,
                |s, tx| s.open_file_search(tx),
            )
            .describe(opencode_i18n::key!("palette.search.placeholder"))
            .bind_id(Bind::FileOpen.id())
            .slash("open")
            .when(App::in_root_session),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new(OVERLAY_ID)
                .with_z(100)
                .with_key_priority(300)
                .with_render(|s, cx| {
                    if s.kernel.slots.get(&SWITCHER).is_open() {
                        let p = crate::palette::pal(s.dark(cx));
                        view(s, p, cx).into()
                    } else {
                        Vec::new()
                    }
                })
                .with_key(|s, ev, tx| s.switcher_key(ev, tx))
                .with_dismiss(|s, tx| {
                    tx.instant();
                    close(s, true);
                }),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new(PROJECT_OVERLAY_ID)
                .with_z(110)
                .with_key_priority(310)
                .with_render(|s, cx| {
                    if s.kernel.slots.get(&crate::project_switcher::PROJECT_SWITCHER).is_open() {
                        let p = crate::palette::pal(s.dark(cx));
                        project::view(s, p, cx).into()
                    } else {
                        Vec::new()
                    }
                })
                .with_key(|s, ev, tx| s.project_switcher_key(ev, tx))
                .with_dismiss(|s, tx| {
                    tx.instant();
                    project::close(s, true);
                }),
        );
        builder.add_setup("switcher.waker", |s, w| {
            s.kernel.slots.get_mut(&SWITCHER).set_waker(w.clone());
            s.session_search.set_waker(w);
        });
        builder.add_rest("switcher.rest_search", |s, _| s.rest_session_search());
        builder.register_slot(&SWITCHER);
        builder.register_slot(&crate::project_switcher::PROJECT_SWITCHER);
        // Precaution: reads listed server projects/worktrees; `projects.home` can open a project dir.
        builder
            .schedule(crate::tick::stage::INGEST_SERVERS, "project_switcher.poll", |s, _, _| {
                crate::project_switcher::poll(s)
            })
            .after("projects.home");
        // Ingest local answers before settling the list: its rows, highlight and empty state must
        // all see the same sources. Previously a full-text landing changed the rows after this poll.
        // Known: `poll_fulltext` reads `home_search` updated by `workspace_tools.home_search`.
        builder
            .schedule(crate::tick::stage::DERIVE_SEARCH, "switcher.poll", |s, now, tx| {
                let local_answers = s.poll_session_search(now, tx) | s.poll_fulltext();
                s.poll_switcher(tx, local_answers)
            })
            .after("workspace_tools.home_search");
    }
}
