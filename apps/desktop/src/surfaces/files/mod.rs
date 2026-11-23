//! Desktop host adapter for `opencode-feature-files` (`features/files`).

pub use opencode_feature_files::*;

pub(crate) mod reply;
mod link_tip_stories;
mod markdown_stories;
mod stories;

use std::collections::HashSet;
use std::rc::Rc;

use opencode_kernel::{Feature, KernelBuilder, Slot};
use opencode_kit::client::Server;
use opencode_kit::wire_error::WireError;
use opencode_ui::popup::Menu;
use rune::{DragItem, El, KeyEvent, PopupAnchor, Tx};
use rune_kit::diff::FileStatus;
use serde_json::Value;
use session_view::palette::Pal;

use crate::jobs::Apply;
use crate::panels::SessionPane;
use crate::{App, Source};

pub(crate) static FILES: Slot<FilesPane> = Slot::new();

impl FilesHost for App {
    fn files(&self) -> &FilesPane {
        self.kernel.slots.get(&FILES)
    }

    fn files_untracked(&self) -> &FilesPane {
        self.kernel.slots.untracked(&FILES)
    }

    fn files_mut(&mut self) -> &mut FilesPane {
        self.kernel.slots.get_mut(&FILES)
    }

    fn touch_files(&self) {
        self.kernel.slots.touch(&FILES);
    }

    fn session_directory(&self, sid: &str) -> Option<&str> {
        self.session(sid).map(|s| s.directory.as_str())
    }

    fn session_physical_directory(&self, sid: &str) -> Option<String> {
        let dir = self.session_directory(sid).filter(|d| !d.is_empty()).unwrap_or_else(|| self.draft_dir());
        if dir.is_empty() {
            return None;
        }
        if self.can_reveal_outside() {
            self.pinned_local_root(dir)
        } else {
            Some(dir.to_owned())
        }
    }

    fn session_project_root(&self, sid: &str) -> Option<&str> {
        let info = self.session(sid)?;
        Some(self.project_for_session(info).map_or(info.directory.as_str(), |p| p.directory.as_str()))
    }

    fn project_title(&self, project_id: &str) -> Option<String> {
        self.project(project_id)
            .map(|pr| opencode_kit::path::basename(&pr.directory).to_owned())
    }

    fn draft_directory(&self) -> &str {
        self.draft_dir()
    }

    fn draft_physical_directory(&self) -> Option<String> {
        let dir = self.draft_dir();
        if dir.is_empty() {
            return None;
        }
        if self.can_reveal_outside() {
            self.pinned_local_root(dir)
        } else {
            Some(dir.to_owned())
        }
    }

    /// This machine's home for the app's own server (the fixture's, in a fixture run); another
    /// server's as Desktop infers it from where that server runs (`createPathHelpers`' `home`,
    /// `sync.data.path.home`), as `App::tilde` reads it.
    fn home_directory(&self) -> std::borrow::Cow<'_, str> {
        let server = crate::servers::runtime::active(self);
        if server.is_sidecar() {
            return self.home.as_str().into();
        }
        crate::servers::health::home(server).unwrap_or_default().into()
    }

    fn live_server(&self) -> Option<&Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server),
            Source::Fixture => None,
        }
    }

    fn fixture_root(&self) -> Option<&Value> {
        Some(&self.fixture_run.fixture)
    }

    fn server_label(&self) -> String {
        crate::servers::runtime::label(crate::servers::runtime::active(self))
    }

    fn format_wire_error(&self, what: &str, error: &WireError) -> String {
        let server = self.server_label();
        crate::errors::UserError::from_wire(what, &server, error).description()
    }

    fn read_json(&self, ns: &str, key: &str) -> Option<Value> {
        crate::storage::json(ns, key)
    }

    fn write_json(&self, ns: &str, key: &str, value: &Value) {
        crate::storage::set_json(ns, key, value);
    }

    fn open_external(&mut self, url: &str, tx: &mut Tx) {
        crate::open_in::open_external(tx, url);
    }

    fn route_web_link(&mut self, sid: Option<&str>, url: &str, from_file: bool, tx: &mut Tx) {
        let source = if from_file {
            crate::surfaces::browser::links::LinkSource::FileViewer
        } else {
            crate::surfaces::browser::links::LinkSource::Chat
        };
        self.route_web_or_app_link(sid, url, source, None, tx);
    }

    fn can_open_in(&self) -> bool {
        App::can_open_in(self)
    }

    fn can_reveal_outside(&self) -> bool {
        App::can_reveal_outside(self)
    }

    fn open_in_current(&self) -> Option<OpenInCurrent> {
        let (label, icon, busy) = crate::open_in::current_info(self)?;
        Some(OpenInCurrent { label, icon, busy })
    }

    fn open_in_finder_label(&self) -> &'static str {
        crate::open_in::finder_label(self)
    }

    fn open_in_path(&self, rel: &str) -> Option<String> {
        App::open_in_path(self, rel)
    }

    fn open_in_drag_item(&self, rel: &str) -> Option<DragItem> {
        crate::open_in::drag_item(self, rel)
    }

    fn open_in_menu_options(&self, with_current: bool) -> Vec<OpenInMenuOption> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::open_in::menu_options(self, with_current)
                .into_iter()
                .map(|opt| OpenInMenuOption {
                    item_id: opt.item_id,
                    label: opt.label,
                    icon: opt.icon,
                    checked: opt.checked,
                })
                .collect()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = with_current;
            Vec::new()
        }
    }

    fn open_in_pick(&mut self, rel: &str, id: &str, reveal: bool, tx: &mut Tx) {
        crate::open_in::pick(self, rel, id, reveal, tx);
    }

    fn open_in_launch_current(&mut self, rel: &str, remember: bool, reveal: bool) {
        crate::open_in::open_current(self, rel, remember, reveal);
    }

    fn open_in_reveal(&mut self, rel: &str) {
        self.open_in_app_as(crate::open_in::FINDER, rel, false, true);
    }

    fn open_in_reveal_abs(&mut self, abs_path: &str) {
        crate::open_in::reveal_abs(self, abs_path.to_owned());
    }

    fn side_open(&self) -> bool {
        App::side_open(self)
    }

    fn is_composer_focused(&self) -> bool {
        self.composer.focused
    }

    fn is_side_focused(&self) -> bool {
        self.kernel.slots.get(&crate::panels::SIDE).focus.is_focused()
    }

    fn active_file_tab(&self) -> Option<(String, Option<String>)> {
        App::active_file_tab(self)
    }

    fn session_open_file_tabs(&self, sid: &str) -> (Vec<String>, Option<String>) {
        let side = self.kernel.slots.get(&crate::panels::SIDE);
        let opened = side
            .sessions
            .get(sid)
            .map(|st| {
                st.tab_state
                    .tabs
                    .all
                    .iter()
                    .filter_map(|k| k.strip_prefix("file:"))
                    .filter(|id| *id != data::OPEN_TAB_ID)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let active = side.sessions.get(sid).and_then(|st| {
            st.tab_state
                .tabs
                .active
                .as_deref()?
                .strip_prefix("file:")
                .map(str::to_owned)
        });
        (opened, active)
    }

    fn all_open_file_tabs(&self) -> Vec<(String, String)> {
        let shown = self.active_file_tab().and_then(|(sid, path)| Some((sid, path?)));
        let mut tabs: Vec<(String, String)> = shown.iter().cloned().collect();
        for (sid, st) in self.kernel.slots.get(&crate::panels::SIDE).sessions.iter() {
            for key in &st.tab_state.tabs.all {
                if let Some(path) = key.strip_prefix("file:").and_then(data::file_tab_path)
                    && shown.as_ref() != Some(&(sid.clone(), path.clone()))
                {
                    tabs.push((sid.clone(), path));
                }
            }
        }
        tabs
    }

    fn open_side_tab(
        &mut self,
        sid: &str,
        tab_key: &str,
        preview: bool,
        steal_focus: bool,
        remove_open_tab: bool,
        tx: &mut Tx,
    ) {
        let saved_focus = (self.composer.focused, self.kernel.slots.get(&crate::panels::SIDE).focus.mode());
        if remove_open_tab && let Some(st) = self.kernel.slots.get_mut(&crate::panels::SIDE).sessions.get_mut(sid) {
            st.tab_state.tabs.all.retain(|k| k != data::OPEN_TAB_KEY);
        }
        self.show_pane_tab(tx, |s| {
            s.open_side_tab_for_session(sid, tab_key, Some(SessionPane::Side), preview, false);
        });
        if !steal_focus {
            self.composer.focused = saved_focus.0;
            self.kernel.slots.get_mut(&crate::panels::SIDE).focus.restore(saved_focus.1);
        }
    }

    fn close_side_tab(&mut self, tab_key: &str, tx: &mut Tx) {
        App::close_side_tab(self, tab_key, tx);
    }

    fn side_quiet_clear(&mut self) {
        if self.kernel.slots.get(&crate::panels::SIDE).focus.is_quiet() {
            self.kernel.slots.get_mut(&crate::panels::SIDE).focus.wake();
        }
    }

    fn handle_panels_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.panels_key(ev, tx)
    }

    fn open_context_menu(
        &mut self,
        menu: Menu,
        anchor: PopupAnchor,
        tx: &mut Tx,
        on_pick: MenuPick,
    ) {
        crate::popup::open(self, menu, anchor, tx, move |s: &mut App, picked: Option<&str>, tx: &mut Tx| {
            on_pick(s, picked, tx);
        });
    }

    fn plugin_open_link(&mut self, href: &str, tx: &mut Tx) -> bool {
        crate::plugin_host::open_link(self, href, tx)
    }

    fn plugin_file_viewer(&self, p: &Pal, sid: &str, directory: &str, path: &str) -> Option<El> {
        crate::plugin_host::file_viewer(self, p, sid, directory, path)
    }

    fn opens_as_page(&self, path: &str) -> bool {
        App::opens_as_page(self, path)
    }

    fn open_workspace_preview(&mut self, sid: &str, rel_path: &str, tx: &mut Tx) {
        self.show_pane_tab(tx, |s| {
            s.open_workspace_preview_tab(sid, rel_path, None, true);
        });
    }

    fn try_open_artifact_link(&mut self, sid: &str, href: &str) -> bool {
        App::try_open_artifact_link(self, sid, href)
    }

    fn open_page_tab(&mut self, sid: &str, url: &str) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.browser_unsupported() {
            return;
        }
        self.open_new_browser_tab_for_session(Some(sid), url, None, true);
    }

    fn ensure_session_review(&mut self, sid: &str) {
        App::ensure_session_review(self, sid);
    }

    fn review_file_status(&self, sid: &str, path: &str) -> Option<FileStatus> {
        self.kernel
            .slots
            .get(&crate::review::REVIEW)
            .sessions
            .get(sid)
            .and_then(|sr| sr.meta.iter().find(|m| m.path == path))
            .map(|m| m.status)
    }

    fn spawn_job(&mut self, job: FilesJob) {
        self.job(move |srv| {
            let reply = execute_job(srv, job);
            Box::new(move |s: &mut App, tx: &mut Tx| apply_reply(s, reply, tx)) as Apply
        });
    }
}

impl App {
    /// Returns `(session_id, Some(file_path) | None /* file:open */)` when a `file:*` tab is active & visible.
    pub fn active_file_tab(&self) -> Option<(String, Option<String>)> {
        if !(self.side_open() || self.fixture_run.stage.is_some()) {
            return None;
        }
        let reg = self.resolve_side_region()?;
        let act = reg.active.as_deref()?;
        let id = act.strip_prefix("file:")?;
        if id == data::OPEN_TAB_ID {
            Some((reg.session_id.clone(), None))
        } else {
            data::file_tab_path(id).map(|p| (reg.session_id.clone(), Some(p)))
        }
    }

    pub fn ensure_workspace_files(&mut self, session_id: &str) {
        ensure_workspace_files(self, session_id);
    }

    pub fn toggle_workspace_dir(&mut self, dir_path: &str) {
        toggle_workspace_dir(self, dir_path);
    }

    pub fn ensure_file_loaded(&mut self, session_id: &str, path: &str) {
        ensure_file_loaded(self, session_id, path);
    }

    pub fn open_workspace_file(
        &mut self,
        path: &str,
        preview: bool,
        line: Option<u32>,
        tx: &mut Tx,
    ) {
        open_workspace_file(self, path, preview, line, tx);
    }

    pub fn open_workspace_file_at(
        &mut self,
        path: &str,
        preview: bool,
        lines: Option<(u32, u32)>,
        tx: &mut Tx,
    ) {
        open_workspace_file_at(self, path, preview, lines, tx);
    }

    pub fn open_workspace_file_for_session(
        &mut self,
        sid: &str,
        path: &str,
        preview: bool,
        lines: Option<(u32, u32)>,
        steal_focus: bool,
        tx: &mut Tx,
    ) {
        open_workspace_file_for_session(self, sid, path, preview, lines, steal_focus, tx);
    }

    pub fn open_workspace_file_source(
        &mut self,
        sid: &str,
        path: &str,
        preview: bool,
        lines: Option<(u32, u32)>,
        steal_focus: bool,
        tx: &mut Tx,
    ) {
        open_workspace_file_source(self, sid, path, preview, lines, steal_focus, tx);
    }

    pub fn filtered_sidebar_files(&self, session_id: &str) -> Vec<String> {
        filtered_sidebar_files(self, session_id)
    }

    pub fn filtered_files(&self, session_id: &str, query: &str) -> Vec<String> {
        filtered_files(self, session_id, query)
    }

    pub fn submit_files_filter(&mut self, tx: &mut Tx) {
        submit_files_filter(self, tx);
    }

    pub fn request_sidebar_file_search(&mut self) {
        request_sidebar_file_search(self);
    }

    pub fn request_file_search(&mut self, session_id: &str, query: &str) {
        request_file_search(self, session_id, query);
    }

    pub fn link_directory(&self, sid: &str) -> String {
        link_directory(self, sid)
    }

    pub fn open_reply_link(&mut self, href: &str, tx: &mut Tx) {
        open_reply_link(self, href, tx);
    }

    /// Handles `Links:Open` from a third-party plugin or any tool-scoped call: refuses raw UNC,
    /// `file://`, `oc-preview://`, `workspace://`, and `artifact://` links; opens relative
    /// workspace files strictly in the Files source viewer (never minting an `oc-preview://`
    /// grant); ignores outside local files; and routes regular web links safely (refusing
    /// same-server artifact URLs and marking tool-opened tabs `agent_driven`).
    pub fn open_restricted_plugin_link(
        &mut self,
        sid: Option<&str>,
        href: &str,
        agent: bool,
        tx: &mut Tx,
    ) {
        let trimmed = href.trim();
        if trimmed.is_empty()
            || browser_policy::is_raw_unc_href(trimmed)
            || browser_policy::is_file_scheme(trimmed)
            || browser_policy::preview::is_preview_scheme(trimmed)
            || browser_policy::preview::is_workspace_scheme(trimmed)
            || browser_policy::preview::is_artifact_scheme(trimmed)
        {
            return;
        }
        let Some(sid) = sid.map(str::to_owned) else { return };
        let dir = link_directory(self, &sid);
        let home = FilesHost::home_directory(self).into_owned();
        match crate::files::links::classify(trimmed, "", &dir, &home) {
            crate::files::links::Link::Inside { path, lines, .. } => {
                self.open_workspace_file_source(&sid, &path, false, lines, true, tx);
            }
            crate::files::links::Link::Web(url) => {
                // A plugin's link reaches the OS only as `http(s)`: `mailto:` and app schemes are
                // copied, never opened and never offered on the review bar.
                let is_http = url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://"))
                    || url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"));
                if !is_http {
                    if !agent && browser_policy::classify_app_link(&url, browser_policy::AppSchemeSource::Conversation).is_some() {
                        tx.copy(url.clone());
                        crate::status_toast::say(self, opencode_i18n::t!("common.copied").to_owned(), String::new());
                    }
                    return;
                }
                self.route_plugin_web_or_app_link(
                    Some(&sid),
                    &url,
                    crate::browser::links::LinkSource::Chat,
                    agent,
                    tx,
                );
            }
            crate::files::links::Link::Heading(_) | crate::files::links::Link::Outside { .. } => {}
        }
    }

    pub fn open_link(&mut self, sid: &str, from: Option<&str>, href: &str, tx: &mut Tx) {
        open_link(self, sid, from, href, tx);
    }

    pub fn reply_code_links(&self, body: &str, live: bool) -> Option<Rc<HashSet<String>>> {
        reply_code_links(self, body, live)
    }

    pub fn toggle_rendered(&mut self, sid: &str, path: &str, tx: &mut Tx) {
        toggle_rendered(self, sid, path, tx);
    }

    pub fn set_file_wrap(&mut self, sid: &str, path: &str, wrap: bool) {
        set_file_wrap(self, sid, path, wrap);
    }

    pub fn sync_rendered_find(&mut self, sid: &str, path: &str, tx: &mut Tx) -> bool {
        sync_rendered_find(self, sid, path, tx)
    }

    pub fn file_find_step(&mut self, sid: &str, path: &str, forward: bool, tx: &mut Tx) {
        file_find_step(self, sid, path, forward, tx);
    }

    pub fn scroll_to_heading(&mut self, sid: &str, path: &str, anchor: &str, tx: &mut Tx) {
        scroll_to_heading(self, sid, path, anchor, tx);
    }
}

/// Kernel feature registration for the Files panel engine.
pub struct FilesFeature;

impl Feature<App> for FilesFeature {
    fn id(&self) -> &'static str {
        "feature.files"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&FILES);
        builder.add_setup("files.waker", |s, w| s.kernel.slots.get_mut(&FILES).set_waker(w));
        // Precaution: reads `active_session_id()`, which `jobs.poll` closures may mutate.
        builder
            .schedule(crate::tick::stage::INGEST_PAGES, "files.reply_links", |s, _, tx| {
                poll_reply_links(s, tx)
            })
            .after("jobs.poll");
        // Known/precaution: `files.poll` (`finish_file_link`) can open a side tab mutating
        // `panels::SIDE`, while `review.poll` reads `side_open()` (`should_refresh_on_poll`).
        builder
            .schedule(crate::tick::stage::REACT_PANELS, "files.poll", |s, _, tx| {
                crate::plugin_host::native::files::poll(s, tx)
            })
            .after("review.poll");
    }
}
