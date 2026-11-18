//! Object-safe host boundary and job/menu types for the Files panel feature.

use opencode_code_view::KeyMatch;
use opencode_kit::client::Server;
use opencode_kit::wire_error::WireError;
use opencode_ui::keymap::Bind;
use opencode_ui::popup::Menu;
use rune::{DragItem, El, KeyEvent, PopupAnchor, Tx};
use rune_kit::diff::FileStatus;
use serde_json::Value;
use session_view::palette::Pal;

use crate::FilesPane;

/// Keybind matcher for in-file Find and Copy selection shortcuts via `opencode_ui::keymap::Bind`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FilesKeyMatch;

impl KeyMatch for FilesKeyMatch {
    fn is_find(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        Bind::Find.matches_tx(ev, tx)
    }

    fn is_copy_selection(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        Bind::CopySelection.matches_tx(ev, tx)
    }
}

/// Callback invoked when a popup/context menu closes with the picked item ID (or `None` when dismissed).
pub type MenuPick = Box<dyn FnOnce(&mut dyn FilesHost, Option<&str>, &mut Tx)>;

/// One external application entry offered by the host's "Open in" menus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenInMenuOption {
    /// Action identifier (`"app:<id>"`) passed back to [`FilesHost::open_in_pick`].
    pub item_id: String,
    pub label: &'static str,
    /// App icon identifier for `rune_theme_opencode::app_icons::named`.
    pub icon: &'static str,
    pub checked: bool,
}

/// Summary of the host's currently remembered external application for toolbar split buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenInCurrent {
    pub label: &'static str,
    /// App icon identifier for `rune_theme_opencode::app_icons::named`.
    pub icon: &'static str,
    pub busy: bool,
}

/// Off-thread server work scheduled through [`FilesHost::spawn_job`].
#[derive(Clone, Debug)]
pub enum FilesJob {
    /// Checks whether workspace path `target` exists inside `directory` by listing its parent folder.
    CheckCodePath {
        session_id: String,
        doc_path: String,
        code: String,
        directory: String,
        target: String,
    },
    /// Resolves a clicked relative inline-code path against `/api/fs/find` in `directory`.
    ResolveCodeLink {
        session_id: String,
        directory: String,
        path: String,
        code: String,
        lines: Option<(u32, u32)>,
    },
}

/// Resolved target of [`FilesJob::ResolveCodeLink`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedCodeLink {
    File(String),
    Picker(String),
}

/// Result of a [`FilesJob`], applied back on the UI thread via [`crate::apply_reply`].
#[derive(Clone, Debug)]
pub enum FilesReply {
    CodePathChecked {
        session_id: String,
        doc_path: String,
        code: String,
        exists: bool,
    },
    CodeLinkResolved {
        session_id: String,
        code: String,
        lines: Option<(u32, u32)>,
        target: Option<ResolvedCodeLink>,
    },
}

/// Object-safe host boundary for the Files panel engine, workspace tree, link resolution,
/// live watching, previews, and media stages.
///
/// Every state transition, watcher, loader, link resolver, key handler, and poll entrypoint in
/// `opencode-feature-files` operates on `&dyn FilesHost` / `&mut dyn FilesHost`. Generic
/// `H: FilesHost + 'static` bounds appear only at `rune::El` view/callback boundaries.
pub trait FilesHost: opencode_kernel::AppEnv + opencode_kernel::AppFeedback {
    /// Files pane state (tracked read).
    fn files(&self) -> &FilesPane;
    /// Files pane state without subscribing the caller's tracked region (used by view-time
    /// background checks such as [`crate::reply_code_links`] and [`crate::link_tip::prepare`]).
    fn files_untracked(&self) -> &FilesPane {
        self.files()
    }
    /// Files pane state (tracked write — callers only invoke when mutating).
    fn files_mut(&mut self) -> &mut FilesPane;
    /// Marks files pane state dirty when interior-mutable state (reply link answers) changed.
    fn touch_files(&self) {}

    /// Working directory of session `sid`, if loaded.
    fn session_directory(&self, sid: &str) -> Option<&str>;
    /// Project root directory for session `sid` (defaults to [`Self::session_directory`]).
    fn session_project_root(&self, sid: &str) -> Option<&str> {
        self.session_directory(sid)
    }
    /// Display title for project `project_id` (basename of its directory), if known.
    fn project_title(&self, project_id: &str) -> Option<String> {
        let _ = project_id;
        None
    }
    /// Working directory of the active draft when no session directory is available.
    fn draft_directory(&self) -> &str {
        ""
    }
    /// User home directory used to expand `~` and format out-of-workspace paths.
    fn home_directory(&self) -> &str {
        ""
    }

    /// Live OpenCode server handle when connected, or `None` in fixture mode.
    fn live_server(&self) -> Option<&Server>;
    /// Fixture JSON root used when [`Self::live_server`] is `None`.
    fn fixture_root(&self) -> Option<&Value> {
        None
    }

    /// Human-readable label of the active server for error descriptions.
    fn server_label(&self) -> String {
        String::new()
    }
    /// Formats a failed file read into a user-facing description.
    fn format_wire_error(&self, what: &str, error: &WireError) -> String {
        let server = self.server_label();
        if server.is_empty() {
            format!("{what}: {}", error.kind)
        } else {
            format!("{what} ({server}): {}", error.kind)
        }
    }

    /// Reads a JSON preference from host persistence.
    fn read_json(&self, ns: &str, key: &str) -> Option<Value> {
        let _ = (ns, key);
        None
    }
    /// Writes a JSON preference to host persistence.
    fn write_json(&self, ns: &str, key: &str, value: &Value) {
        let _ = (ns, key, value);
    }

    /// Opens an external web or app URL (`https:`, `mailto:`, etc.).
    fn open_external(&mut self, url: &str, tx: &mut Tx) {
        drop(tx.open_url(url));
    }

    /// Whether opening files in external local apps is available on this host.
    fn can_open_in(&self) -> bool {
        false
    }
    /// Current remembered external application summary for the toolbar split button.
    fn open_in_current(&self) -> Option<OpenInCurrent> {
        None
    }
    /// Localized display label of the platform file manager (e.g. "Finder", "File Explorer").
    fn open_in_finder_label(&self) -> &'static str {
        ""
    }
    /// Resolves workspace-relative `rel` to a full filesystem path for copying/opening.
    fn open_in_path(&self, rel: &str) -> Option<String> {
        let _ = rel;
        None
    }
    /// Builds a native file drag payload for workspace-relative `rel` when available locally.
    fn open_in_drag_item(&self, rel: &str) -> Option<DragItem> {
        let _ = rel;
        None
    }
    /// External application entries for "Open in" menus.
    fn open_in_menu_options(&self, with_current: bool) -> Vec<OpenInMenuOption> {
        let _ = with_current;
        Vec::new()
    }
    /// Handles picking an item (`"copy"` or `"app:<id>"`) from an "Open in" menu.
    fn open_in_pick(&mut self, rel: &str, id: &str, reveal: bool, tx: &mut Tx) {
        let _ = (rel, id, reveal, tx);
    }
    /// Launches the remembered external app on `rel`.
    fn open_in_launch_current(&mut self, rel: &str, remember: bool, reveal: bool) {
        let _ = (rel, remember, reveal);
    }
    /// Reveals `rel` in the host's file manager without changing the remembered app.
    fn open_in_reveal(&mut self, rel: &str) {
        let _ = rel;
    }
    /// Opens an absolute path outside the workspace in its default OS application.
    fn open_in_default(&mut self, abs_path: &str) {
        let _ = abs_path;
    }

    /// Whether the side panel region is currently open.
    fn side_open(&self) -> bool {
        false
    }
    /// Whether the session composer currently holds keyboard focus.
    fn is_composer_focused(&self) -> bool {
        false
    }
    /// Whether the side panel region currently holds keyboard focus.
    fn is_side_focused(&self) -> bool {
        false
    }
    /// Returns `(session_id, Some(file_path) | None /* file:open */)` when a `file:*` tab is active & visible.
    fn active_file_tab(&self) -> Option<(String, Option<String>)> {
        None
    }
    /// Returns `(opened_tab_ids, active_tab_id)` of concrete `file:` tabs (excluding `file:open`)
    /// in session `sid`'s side panel strip, for the narrow Files view.
    fn session_open_file_tabs(&self, sid: &str) -> (Vec<String>, Option<String>) {
        let _ = sid;
        (Vec::new(), None)
    }
    /// Returns `(session_id, file_path)` for every open concrete `file:` tab across all sessions.
    fn all_open_file_tabs(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// Opens `tab_key` in session `sid`'s side pane. When `remove_open_tab` is true, drops the
    /// transient `file:open` placeholder tab first; when `steal_focus` is false, preserves
    /// composer and side-panel focus.
    fn open_side_tab(
        &mut self,
        sid: &str,
        tab_key: &str,
        preview: bool,
        steal_focus: bool,
        remove_open_tab: bool,
        tx: &mut Tx,
    ) {
        let _ = (sid, tab_key, preview, steal_focus, remove_open_tab, tx);
    }
    /// Closes side tab `tab_key` in the active session's panel strip.
    fn close_side_tab(&mut self, tab_key: &str, tx: &mut Tx) {
        let _ = (tab_key, tx);
    }
    /// Clears the side panel's quiet hover/focus state when the user interacts with the file filter.
    fn side_quiet_clear(&mut self) {}
    /// Forwards tab-switching key chords from the file filter field to the host's panels engine.
    fn handle_panels_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let _ = (ev, tx);
        false
    }

    /// Opens a popup/context menu at `anchor` and invokes `on_pick` when it closes.
    fn open_context_menu(
        &mut self,
        menu: Menu,
        anchor: PopupAnchor,
        tx: &mut Tx,
        on_pick: MenuPick,
    ) {
        drop((menu, anchor, tx, on_pick));
    }

    /// Gives host plugins first chance to handle a clicked link; returns `true` if handled.
    fn plugin_open_link(&mut self, href: &str, tx: &mut Tx) -> bool {
        let _ = (href, tx);
        false
    }
    /// Returns a plugin-contributed custom file viewer element for `(sid, directory, path)`, if any.
    fn plugin_file_viewer(&self, p: &Pal, sid: &str, directory: &str, path: &str) -> Option<El> {
        let _ = (p, sid, directory, path);
        None
    }

    /// Whether workspace-relative `path` should open as a live page in the embedded browser pane.
    fn opens_as_page(&self, path: &str) -> bool {
        let _ = path;
        false
    }
    /// Opens a `file://` URL in a new embedded browser tab for session `sid`.
    fn open_page_tab(&mut self, sid: &str, url: &str) {
        let _ = (sid, url);
    }

    /// Ensures session `sid`'s review diff metadata is loaded (called when opening a file link).
    fn ensure_session_review(&mut self, sid: &str) {
        let _ = sid;
    }
    /// Returns the review change status for `path` in session `sid` when no explicit `kinds` map is provided.
    fn review_file_status(&self, sid: &str, path: &str) -> Option<FileStatus> {
        let _ = (sid, path);
        None
    }

    /// Schedules an off-thread [`FilesJob`] against the live server; execute with [`crate::execute_job`]
    /// and deliver the result on the UI thread with [`crate::apply_reply`].
    fn spawn_job(&mut self, job: FilesJob) {
        let _ = job;
    }
}
