//! Desktop host adapter for `opencode-feature-restore` (`features/restore`).

pub use opencode_feature_restore::*;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, LazyLock};

use opencode_kernel::{Feature, KernelBuilder};
use opencode_kit::client::PathAttachment;
use opencode_kit::model::{Attachment, PromptComment, Transcript};
use opencode_kit::servers::tabs::{ClosedTab, WindowDoc};
use opencode_kit::servers::{ServerKey, TabKey};
use opencode_storage::{FlushReason, Lookup, Storage};
use rune::{TextEdit, Tx};
use serde_json::{Map, Value};
use session_view::composer::mode::Mode;
use session_view::disclosure::Disclosures;
use session_view::jump::Ends;

use crate::{App, Route};

static EMPTY_PLACES: LazyLock<places::Kept> = LazyLock::new(places::Kept::default);

/// The draft key for a desktop [`Route`]: a session's id, or a draft tab's.
pub fn draft_key(route: &Route) -> Option<String> {
    match route {
        Route::Session { id, .. } | Route::Draft(id) => Some(id.clone()),
        Route::Home => None,
    }
}

fn to_restore_route(route: &Route) -> RestoreRoute {
    match route {
        Route::Home => RestoreRoute::Home,
        Route::Draft(id) => RestoreRoute::Draft(id.clone()),
        Route::Session { server, id } => RestoreRoute::Session { server: server.clone(), id: id.clone() },
    }
}

fn from_restore_route(route: RestoreRoute) -> Route {
    match route {
        RestoreRoute::Home => Route::Home,
        RestoreRoute::Draft(id) => Route::Draft(id),
        RestoreRoute::Session { server, id } => Route::Session { server, id },
    }
}

impl RestoreHost for App {
    fn restore(&self) -> &Restore {
        &self.restore
    }

    fn restore_mut(&mut self) -> &mut Restore {
        &mut self.restore
    }

    fn kept_places(&self) -> &places::Kept {
        self.kernel.slots.get::<places::Kept>().unwrap_or(&EMPTY_PLACES)
    }

    fn kept_places_mut(&mut self) -> &mut places::Kept {
        self.kernel.slots.slot::<places::Kept>()
    }

    fn window_id(&self) -> &str {
        &self.window.id
    }

    fn stage_active(&self) -> bool {
        self.stage.is_some()
    }

    fn run_background(&self, job: Box<dyn FnOnce() + Send>) {
        crate::plugin_host::tasks::run(job);
    }

    fn route(&self) -> RestoreRoute {
        to_restore_route(&self.route)
    }

    fn set_route(&mut self, route: RestoreRoute) {
        *self.route = from_restore_route(route);
    }

    fn tabs(&self) -> Vec<RestoreTab> {
        self.tabs
            .iter()
            .map(|t| RestoreTab {
                uid: t.uid(),
                id: t.id.clone(),
                server: t.server.clone(),
                title: t.title.clone(),
                is_draft: t.is_draft(),
            })
            .collect()
    }

    fn current_tab_uid(&self) -> Option<String> {
        App::current_tab(self)
    }

    fn home_return_tab(&self) -> Option<&str> {
        self.home_return_tab.as_deref()
    }

    fn set_home_return_tab(&mut self, uid: Option<String>) {
        *self.home_return_tab = uid;
    }

    fn tab_key_by_uid(&self, uid: &str) -> Option<TabKey> {
        self.tab(uid).map(crate::tabs::Tab::key)
    }

    fn tab_key_by_id(&self, id: &str) -> Option<TabKey> {
        self.tabs.iter().find(|t| t.id == id).map(crate::tabs::Tab::key)
    }

    fn session_tab_ids(&self) -> Vec<String> {
        self.tabs.iter().filter(|t| !t.is_draft()).map(|t| t.id.clone()).collect()
    }

    fn restore_closed_tabs(&mut self, closed: &[ClosedTab]) {
        *self.closed = crate::tabs::Closed::from_doc(closed);
    }

    fn hydrate_closed_tabs(&mut self, closed_doc: &[ClosedTab], closed_drafts: &Value, now_ms: i64) -> Vec<String> {
        let mut closed = crate::tabs::Closed::from_doc(closed_doc);
        let expired = closed.take_drafts_json(closed_drafts, now_ms);
        for e in self.closed.entries().cloned().collect::<Vec<_>>() {
            if e.tab.is_draft() {
                closed.push_draft(e, now_ms);
            } else {
                closed.push(e);
            }
        }
        *self.closed = closed;
        expired
    }

    fn closed_tabs_doc(&self) -> Vec<ClosedTab> {
        self.closed.to_doc()
    }

    fn closed_drafts_json(&self) -> Value {
        self.closed.drafts_json()
    }

    fn closed_tabs_stamp(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.closed.hash(&mut h);
        h.finish()
    }

    fn composer(&self) -> &TextEdit {
        &self.composer
    }

    fn composer_mut(&mut self) -> &mut TextEdit {
        &mut self.composer
    }

    fn composer_version(&self) -> u64 {
        rune::Tracked::version(&self.composer)
    }

    fn composer_mode(&self) -> Mode {
        self.add.mode
    }

    fn park_composer_mode(&mut self, key: String, mode: Mode) {
        self.add.park(key, mode);
    }

    fn swap_composer_mode_draft(&mut self, from: Option<&str>, to: Option<&str>) {
        self.add.swap_draft(from, to);
    }

    fn parked_composer_modes(&self) -> Vec<(String, Mode)> {
        self.add.parked().map(|(k, m)| (k.to_owned(), m)).collect()
    }

    fn attach_files(&self) -> &[Attachment] {
        &self.attach.files
    }

    fn attach_paths(&self) -> &[PathAttachment] {
        &self.attach.paths
    }

    fn attach_context(&self) -> &[ContextFile] {
        &self.attach.context
    }

    fn push_attach_file(&mut self, file: Attachment) {
        self.attach.files.push(file);
    }

    fn add_attach_path(&mut self, path: PathAttachment) {
        self.attach.add_path(path);
    }

    fn take_attach_files(&mut self) -> Vec<Attachment> {
        std::mem::take(&mut self.attach.files)
    }

    fn take_attach_paths(&mut self) -> Vec<PathAttachment> {
        std::mem::take(&mut self.attach.paths)
    }

    fn take_attach_context(&mut self) -> Vec<ContextFile> {
        std::mem::take(&mut self.attach.context)
    }

    fn set_attach_files(&mut self, files: Vec<Attachment>) {
        self.attach.files = files;
    }

    fn set_attach_paths(&mut self, paths: Vec<PathAttachment>) {
        self.attach.paths = paths;
    }

    fn set_attach_context(&mut self, context: Vec<ContextFile>) {
        self.attach.context = context;
    }

    fn quote_any(&self) -> bool {
        self.quote.any()
    }

    fn quote_revision(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.quote.revision().hash(&mut h);
        h.finish()
    }

    fn quote_comments(&self) -> Vec<&PromptComment> {
        self.quote.items.iter().map(|q| &q.comment).collect()
    }

    fn take_quotes(&mut self) -> Vec<PromptComment> {
        self.quote.take()
    }

    fn push_quote(&mut self, comment: PromptComment) {
        self.quote.push(comment);
    }

    fn leave_queue_edits(&mut self) {
        App::leave_queue_edits(self);
    }

    fn suggest_drop(&mut self) {
        App::suggest_drop(self);
    }

    fn last_project(&self) -> Option<(&ServerKey, &str)> {
        self.home_state.last_project.as_ref().map(|(s, d)| (s, d.as_str()))
    }

    fn set_last_project(&mut self, project: Option<(ServerKey, String)>) {
        self.home_state.last_project = project;
    }

    fn sidebar_open(&self) -> bool {
        *self.sidebar_open
    }

    fn set_sidebar_open(&mut self, open: bool) {
        *self.sidebar_open = open;
    }

    fn sidebar_width(&self) -> f32 {
        *self.sidebar_w
    }

    fn set_sidebar_width(&mut self, width: f32) {
        *self.sidebar_w = width;
    }

    fn organize_collapsed(&self) -> bool {
        self.project_state.organize_collapsed
    }

    fn organize_expanded(&self) -> Option<&HashSet<String>> {
        Some(&self.project_state.organize_expanded)
    }

    fn organize_order(&self) -> Option<&BTreeMap<String, Vec<String>>> {
        Some(&self.project_state.organize_order)
    }

    fn apply_organize(&mut self, state: OrganizeState) {
        let st = &mut *self.project_state;
        st.organize_collapsed = state.collapsed;
        st.organize_expanded = state.expanded;
        st.organize_order = state.order;
    }

    fn freeze_organize_order(&mut self) {
        App::freeze_organize_order(self);
    }

    fn restore_sidebar_view(&mut self, value: &Value) {
        self.project_state.customize.restore(value);
    }

    fn sidebar_view_json(&self) -> Value {
        self.project_state.customize.to_json()
    }

    fn sidebar_view_stamp(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.project_state.customize.stamp(&mut h);
        h.finish()
    }

    fn restore_sidebar_recent(&mut self, value: &Value) {
        self.project_state.recent.restore(value);
    }

    fn sidebar_recent_json(&self) -> Value {
        self.project_state.recent.to_json()
    }

    fn sidebar_recent_pins_stamp(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.project_state.recent.pins.hash(&mut h);
        h.finish()
    }

    fn settings_open(&self) -> bool {
        self.settings.open
    }

    fn settings_page_id(&self) -> &str {
        self.settings_pages.page.id()
    }

    fn open_restored_settings(&mut self, tab: &str) {
        self.settings.open = true;
        self.settings_pages.page = crate::settings::pages::Page::parse(tab);
    }

    fn reopen_settings_page(&mut self, tx: &mut Tx) {
        let page = std::mem::take(&mut self.settings_pages.page);
        self.open_settings_page(page, tx);
    }

    fn active_server(&self) -> ServerKey {
        crate::servers::runtime::active(self).clone()
    }

    fn current_server(&self) -> ServerKey {
        crate::servers::runtime::current()
    }

    fn sidecar_launch_waiting(&self) -> bool {
        self.launch.phase(ServerKey::SIDECAR).is_some_and(crate::launch::Phase::waiting)
    }

    fn is_cold_transcript(&self, id: &str) -> bool {
        self.launch.cold_transcript.as_deref() == Some(id)
    }

    fn set_cold_transcript(&mut self, id: Option<String>) {
        self.launch.cold_transcript = id;
    }

    fn has_session(&self, id: &str) -> bool {
        self.sessions.iter().any(|s| s.id == id)
    }

    fn session_title(&self, id: &str) -> Option<&str> {
        self.sessions.iter().find(|s| s.id == id).map(|s| s.title.as_str())
    }

    fn session_updated(&self, id: &str) -> Option<i64> {
        self.session(id).map(|s| s.updated)
    }

    fn transcript(&self, id: &str) -> Option<&Transcript> {
        self.transcripts.get(id)
    }

    fn insert_transcript(&mut self, id: String, transcript: Transcript) {
        self.transcripts.insert(id, transcript);
    }

    fn expanded(&self) -> &Disclosures {
        &self.expanded
    }

    fn expanded_mut(&mut self) -> &mut Disclosures {
        &mut self.expanded
    }

    fn ends(&self) -> &Ends {
        &self.ends
    }

    fn ends_mut(&mut self) -> &mut Ends {
        &mut self.ends
    }

    fn timeline_has_row(&self, id: &str, row: &str) -> bool {
        self.transcripts.get(id).is_some_and(|t| session_view::timeline::has_row(self, t, row))
    }

    fn timeline_row_message(&self, id: &str, row: &str) -> Option<usize> {
        let t = self.transcripts.get(id)?;
        session_view::timeline::row_message(self, t, row)
    }

    fn settle_timeline_jump(&mut self, tx: &mut Tx) {
        session_view::jump::settle(self, tx);
    }

    fn read_toward_place(&mut self, id: &str, tx: &mut Tx) {
        App::read_toward_place(self, id, tx);
    }

    fn has_pages_gap(&self, id: &str) -> bool {
        self.pages.gap(id)
    }

    fn on_copy_received(&mut self, id: &str) {
        if let Some(o) = &mut *self.opening
            && o.id == id
        {
            o.step(crate::session_pages::OpenEvent::Received);
        }
    }

    fn restore_side_json(&mut self, value: &Value) {
        self.side.restore_json(value);
    }

    fn restore_side_rest(&mut self, value: &Value) {
        self.side.restore_rest(value);
    }

    fn side_json_where(&self, pred: &dyn Fn(&str) -> bool) -> Value {
        self.side.to_json_where(pred)
    }

    fn side_stamp(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.side.stamp_hash(&mut h);
        h.finish()
    }

    fn load_persisted_terminals(&mut self, server: &str) {
        App::load_persisted_terminals(self, server);
    }

    fn flush_persisted_terminals(&mut self) {
        self.queue_terminal_save();
        crate::terminal::persist::flush();
    }

    fn storage_is_ready(&self) -> bool {
        crate::storage::is_ready()
    }

    fn storage_json(&self, namespace: &str, key: &str) -> Option<Value> {
        crate::storage::json(namespace, key)
    }

    fn storage_set_json(&mut self, namespace: &str, key: &str, value: &Value) {
        crate::storage::set_json(namespace, key, value);
    }

    fn storage_delete(&mut self, namespace: &str, key: &str) {
        crate::storage::delete(namespace, key);
    }

    fn storage_update_json(&mut self, namespace: &str, key: &str, edit: &mut dyn FnMut(&mut Map<String, Value>)) {
        crate::storage::update(namespace, key, edit);
    }

    fn storage_window_rows(&self, window_id: &str) -> BTreeMap<String, Arc<str>> {
        crate::storage::window_rows(window_id)
    }

    fn storage_doc(&self, key: &str) -> Lookup {
        crate::storage::doc(key)
    }

    fn storage_set_doc(&mut self, key: &str, value: Option<String>) {
        crate::storage::set_doc(key, value);
    }

    fn storage_doc_keys(&self, prefix: &str) -> Vec<String> {
        crate::storage::doc_keys(prefix)
    }

    fn storage_load_docs(&mut self, keys: &[&str], done: Box<dyn FnOnce() + Send>) {
        crate::storage::load_docs(keys, done);
    }

    fn storage_boot_preview(&self, server: &str, session: &str) -> Option<(i64, Vec<u8>)> {
        crate::storage::boot_preview(server, session)
    }

    fn storage_set_preview(&mut self, server: &str, session: &str, value: Option<(i64, Arc<[u8]>)>) {
        crate::storage::set_preview(server, session, value);
    }

    fn with_storage(&self, op: Box<dyn FnOnce(&Storage) + Send>) {
        crate::storage::with_storage(move |s| op(s));
    }

    fn storage_flush(&mut self, reason: FlushReason, wait: bool) {
        crate::storage::flush(reason, wait);
    }

    fn take_storage_notice(&mut self) -> Option<StorageNotice> {
        use crate::storage::Notice;
        crate::storage::take_notice().map(|n| match n {
            Notice::Memory(why) => StorageNotice::Memory(why),
            Notice::InUse => StorageNotice::InUse,
            Notice::Waiting => StorageNotice::Waiting,
            Notice::Damaged(kept) => StorageNotice::Damaged(kept),
            Notice::Checkpoint => StorageNotice::Checkpoint,
            Notice::FirstRun => StorageNotice::FirstRun,
            Notice::Saving => StorageNotice::Saving,
            Notice::NotSaving { full, why } => StorageNotice::NotSaving { full, why },
        })
    }

    fn take_skip_restore(&mut self, session_id: &str) -> bool {
        crate::launch::last_exit::take_skip_restore(session_id)
    }

    fn note_restoring_session(&mut self, session_id: &str) {
        crate::launch::last_exit::note_restoring_session(session_id);
    }

    fn set_active_exit_session(&mut self, session_id: Option<&str>) {
        crate::launch::last_exit::set_active_session(session_id);
    }

    fn note_skipped_restore(&mut self, server: ServerKey, session_id: String, title: String) {
        crate::crash_notice::note_skipped(
            self.kernel.slots.slot::<crate::crash_notice::State>(),
            server,
            session_id,
            title,
        );
    }

    fn commit_pending_undos(&mut self) {
        crate::undo::commit_all(self);
    }

    fn before_close(&mut self) {
        self.tear_back();
        self.close_rune_mcp();
    }

    fn after_close(&mut self) {
        self.window_closed();
        crate::servers::runtime::leave_all(self);
    }

    fn notify_quitting(&mut self) {
        crate::notifications::quitting(self);
    }

    fn after_quit(&mut self) {
        self.window_quitting();
        crate::windows::quit_saved(&self.window.id);
    }

    fn toast_fail(&mut self, title: &str, description: &str) {
        crate::status_toast::fail(self, title, description);
    }

    fn toast_succeed(&mut self, title: &str, description: &str) {
        crate::status_toast::succeed(self, title, description);
    }
}

impl App {
    pub fn restore_ui(&mut self, doc: &WindowDoc, server: &str) {
        restore_ui(self, doc, server);
    }

    pub fn tell_storage_notice(&mut self) -> bool {
        tell_storage_notice(self)
    }

    pub fn hydrate_restore(&mut self) -> bool {
        hydrate_restore(self)
    }

    #[allow(dead_code)]
    pub fn hydrated(&self) -> bool {
        hydrated(self)
    }

    pub fn resume_settings_page(&mut self) {
        resume_settings_page(self);
    }

    pub fn save_on_close(&mut self) {
        save_on_close(self);
    }

    pub fn save_on_quit(&mut self) {
        save_on_quit(self);
    }

    pub(crate) fn show_draft(&mut self) -> bool {
        show_draft(self)
    }

    pub(crate) fn draft_documents(&self) -> HashMap<String, String> {
        draft_documents(self)
    }

    pub(crate) fn draft_document_of(&self, k: &str) -> Option<String> {
        draft_document_of(self, k)
    }

    pub fn poll_restore(&mut self) {
        poll_restore(self);
    }

    pub(crate) fn composer_holds(&self, key: &str) -> bool {
        composer_holds(self, key)
    }

    pub(crate) fn draft_parked(&self, key: &str) -> Option<Parked> {
        draft_parked(self, key)
    }

    pub(crate) fn park_draft(&mut self, key: String, text: String, parked: Parked) {
        park_draft(self, key, text, parked);
    }

    pub(crate) fn draft_has_content(&self, key: &str) -> bool {
        draft_has_content(self, key)
    }

    pub(crate) fn park_shown_draft(&mut self) {
        park_shown_draft(self);
    }

    pub(crate) fn drop_shown_draft(&mut self) {
        drop_shown_draft(self);
    }

    pub(crate) fn land_upload(&mut self, key: Option<&str>, r: PathAttachment) {
        land_upload(self, key, r);
    }

    #[allow(dead_code)]
    pub(crate) fn restore_places(&mut self) {
        restore_places(self);
    }

    pub(crate) fn land_place(&mut self, id: &str, tx: &mut Tx) -> bool {
        land_place(self, id, tx)
    }

    pub(crate) fn poll_places(&mut self, now: f64, tx: &mut Tx) -> bool {
        poll_places(self, now, tx)
    }

    #[allow(dead_code)]
    pub(crate) fn ask_copy(&mut self, server: &ServerKey, id: &str) {
        ask_copy(self, server, id);
    }

    pub(crate) fn poll_copies(&mut self, tx: &mut Tx) -> bool {
        poll_copies(self, tx)
    }

    pub(crate) fn poll_preview(&mut self, now: bool) {
        poll_preview(self, now);
    }

    pub(crate) fn poll_undo(&mut self, now: f64, tx: &mut Tx) {
        poll_undo(self, now, tx);
    }
}

/// Kernel feature registration for window state restoration, drafts, places, previews, and copies.
pub struct RestoreFeature;

impl Feature<App> for RestoreFeature {
    fn id(&self) -> &'static str {
        "feature.restore"
    }

    fn register(&self, _builder: &mut KernelBuilder<App>) {}
}
