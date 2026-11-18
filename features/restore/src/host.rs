//! Object-safe host boundary for `opencode-feature-restore`.
//!
//! Every restoration, persistence, draft-swapping, preview, offline-copy, timeline-place, and
//! draft-undo entrypoint in this crate operates on non-generic `&dyn RestoreHost` /
//! `&mut dyn RestoreHost`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use opencode_kit::client::PathAttachment;
use opencode_kit::model::{Attachment, PromptComment, Transcript};
use opencode_kit::servers::tabs::ClosedTab;
use opencode_kit::servers::{ServerKey, TabKey};
use opencode_storage::{FlushReason, Lookup, Storage};
use rune::{TextEdit, Tx};
use serde_json::{Map, Value};
use session_view::composer::mode::Mode;
use session_view::disclosure::Disclosures;
use session_view::jump::Ends;

use crate::{ContextFile, OrganizeState, Restore, RestoreRoute, RestoreTab, StorageNotice, places};

/// Object-safe host boundary for window restoration, composer draft parking and blob persistence,
/// timeline places, boot previews, offline transcript copies, and draft undo history.
pub trait RestoreHost: opencode_kernel::AppEnv {
    // ── Feature state ───────────────────────────────────────────────────────────────────────────

    /// Per-window restoration state (tracked read).
    fn restore(&self) -> &Restore;
    /// Per-window restoration state (tracked write — callers only invoke when mutating).
    fn restore_mut(&mut self) -> &mut Restore;

    /// Timeline places persistence state (stored in `opencode_kernel::StateSlots` in desktop).
    fn kept_places(&self) -> &places::Kept;
    /// Mutable timeline places persistence state.
    fn kept_places_mut(&mut self) -> &mut places::Kept;

    // ── Window identity, runtime & environment ──────────────────────────────────────────────────

    /// Window identifier (`""` for ephemeral or single-window browser runs).
    fn window_id(&self) -> &str { "" }
    /// Whether a Storybook stage is currently mounted over the window.
    fn stage_active(&self) -> bool { false }
    /// Runs off-thread background work (such as serializing large undo histories).
    fn run_background(&self, job: Box<dyn FnOnce() + Send>) {
        if let Err(e) = rune::task::spawn("restore-worker", job) {
            eprintln!("restore worker dropped: {e}");
        }
    }

    // ── Route & navigation ──────────────────────────────────────────────────────────────────────

    /// Current window route.
    fn route(&self) -> RestoreRoute;
    /// Sets the active window route during restoration.
    fn set_route(&mut self, route: RestoreRoute);

    // ── Open & closed tabs ──────────────────────────────────────────────────────────────────────

    /// Open window tabs in strip order.
    fn tabs(&self) -> Vec<RestoreTab> { Vec::new() }
    /// UID of the currently active tab, if any.
    fn current_tab_uid(&self) -> Option<String> { None }
    /// UID of the tab Home returns to when toggled back.
    fn home_return_tab(&self) -> Option<&str> { None }
    /// Sets the tab UID Home returns to (`tabs.recent`).
    fn set_home_return_tab(&mut self, _uid: Option<String>) {}
    /// Looks up a tab's [`TabKey`] by its unique tab UID.
    fn tab_key_by_uid(&self, uid: &str) -> Option<TabKey> {
        self.tabs().into_iter().find(|t| t.uid == uid).map(|t| t.key())
    }
    /// Looks up a tab's [`TabKey`] by its session or draft ID.
    fn tab_key_by_id(&self, id: &str) -> Option<TabKey> {
        self.tabs().into_iter().find(|t| t.id == id).map(|t| t.key())
    }
    /// Session IDs of all non-draft open tabs (for pruning persisted timeline places).
    fn session_tab_ids(&self) -> Vec<String> {
        self.tabs().into_iter().filter(|t| !t.is_draft).map(|t| t.id).collect()
    }

    /// Replaces closed tabs from a loaded window document (`doc.closed`).
    fn restore_closed_tabs(&mut self, _closed: &[ClosedTab]) {}
    /// Hydrates closed tabs and closed drafts from disk, preserving tabs closed since launch as
    /// newest, and returns draft IDs expired past the retention window.
    fn hydrate_closed_tabs(&mut self, _closed: &[ClosedTab], _closed_drafts: &Value, _now_ms: i64) -> Vec<String> {
        Vec::new()
    }
    /// Serializes closed session tabs in `WindowDoc` shape (`tabs.closed`).
    fn closed_tabs_doc(&self) -> Vec<ClosedTab> { Vec::new() }
    /// Serializes closed draft tabs (`rune.closedDrafts`).
    fn closed_drafts_json(&self) -> Value { Value::Array(Vec::new()) }
    /// Hash/stamp of the window's closed-tabs state for change detection in `poll_restore`.
    fn closed_tabs_stamp(&self) -> u64 { 0 }

    // ── Composer, attachments, quotes & mode ────────────────────────────────────────────────────

    /// Active composer editor (read).
    fn composer(&self) -> &TextEdit;
    /// Active composer editor (write).
    fn composer_mut(&mut self) -> &mut TextEdit;
    /// Tracked version of the composer editor (skips undo-print comparisons on idle frames).
    fn composer_version(&self) -> u64 { 0 }

    /// Current composer mode (`Normal` or `Shell`).
    fn composer_mode(&self) -> Mode { Mode::Normal }
    /// Parks composer `mode` for draft `key` (`add.park`).
    fn park_composer_mode(&mut self, _key: String, _mode: Mode) {}
    /// Swaps parked composer mode when switching drafts (`add.swap_draft`).
    fn swap_composer_mode_draft(&mut self, _from: Option<&str>, _to: Option<&str>) {}
    /// All parked `(draft_key, Mode)` entries (`add.parked`).
    fn parked_composer_modes(&self) -> Vec<(String, Mode)> { Vec::new() }

    /// Inline file/image attachments currently in the composer.
    fn attach_files(&self) -> &[Attachment] { &[] }
    /// Server-path attachments currently in the composer.
    fn attach_paths(&self) -> &[PathAttachment] { &[] }
    /// Selected-file context cards currently in the composer.
    fn attach_context(&self) -> &[ContextFile] { &[] }
    /// Appends a restored inline attachment to the active composer.
    fn push_attach_file(&mut self, _file: Attachment) {}
    /// Adds a finished upload path to the active composer (deduplicated by path).
    fn add_attach_path(&mut self, _path: PathAttachment) {}
    /// Moves inline attachments out of the composer.
    fn take_attach_files(&mut self) -> Vec<Attachment> { Vec::new() }
    /// Moves path attachments out of the composer.
    fn take_attach_paths(&mut self) -> Vec<PathAttachment> { Vec::new() }
    /// Moves context cards out of the composer.
    fn take_attach_context(&mut self) -> Vec<ContextFile> { Vec::new() }
    /// Replaces inline attachments in the composer.
    fn set_attach_files(&mut self, _files: Vec<Attachment>) {}
    /// Replaces path attachments in the composer.
    fn set_attach_paths(&mut self, _paths: Vec<PathAttachment>) {}
    /// Replaces context cards in the composer.
    fn set_attach_context(&mut self, _context: Vec<ContextFile>) {}

    /// Whether the composer holds any quote/comment cards.
    fn quote_any(&self) -> bool { false }
    /// Revision counter bumped whenever the composer's quote cards change.
    fn quote_revision(&self) -> u64 { 0 }
    /// Borrowed references to the composer's current quote comments.
    fn quote_comments(&self) -> Vec<&PromptComment> { Vec::new() }
    /// Takes all quote comments out of the composer.
    fn take_quotes(&mut self) -> Vec<PromptComment> { Vec::new() }
    /// Pushes a restored quote comment into the composer.
    fn push_quote(&mut self, _comment: PromptComment) {}

    /// Leaves any active queued-prompt edit before switching drafts (`leave_queue_edits`).
    fn leave_queue_edits(&mut self) {}
    /// Resets composer suggestion popovers and prompt recall when switching drafts (`suggest_drop`).
    fn suggest_drop(&mut self) {}

    // ── Sidebar, projects & settings ────────────────────────────────────────────────────────────

    /// Last selected Home project `(server, directory)` (`tabs.recent.target`).
    fn last_project(&self) -> Option<(&ServerKey, &str)> { None }
    /// Sets the restored Home `last_project`.
    fn set_last_project(&mut self, _project: Option<(ServerKey, String)>) {}

    /// Whether the left sidebar is open.
    fn sidebar_open(&self) -> bool { false }
    /// Sets whether the left sidebar is open.
    fn set_sidebar_open(&mut self, _open: bool) {}
    /// Current left sidebar width in points.
    fn sidebar_width(&self) -> f32 { 240.0 }
    /// Sets the left sidebar width in points (already clamped to `[SIDEBAR_MIN, SIDEBAR_MAX]`).
    fn set_sidebar_width(&mut self, _width: f32) {}

    /// Whether the organized sidebar's Projects section is collapsed.
    fn organize_collapsed(&self) -> bool { false }
    /// Expanded `<server>:<project>` keys in the organized sidebar.
    fn organize_expanded(&self) -> Option<&HashSet<String>> { None }
    /// Per-server project order in the organized sidebar.
    fn organize_order(&self) -> Option<&BTreeMap<String, Vec<String>>> { None }
    /// Applies restored organized-sidebar state.
    fn apply_organize(&mut self, _state: OrganizeState) {}
    /// Freezes initial organized project order once server projects arrive (`freeze_organize_order`).
    fn freeze_organize_order(&mut self) {}

    /// Restores Customize sidebar preferences (`sidebar.view`).
    fn restore_sidebar_view(&mut self, _value: &Value) {}
    /// Serializes Customize sidebar preferences (`sidebar.view`).
    fn sidebar_view_json(&self) -> Value { Value::Null }
    /// Hash/stamp of Customize sidebar preferences.
    fn sidebar_view_stamp(&self) -> u64 { 0 }

    /// Restores Recent sidebar pins and limits (`sidebar.recent`).
    fn restore_sidebar_recent(&mut self, _value: &Value) {}
    /// Serializes Recent sidebar pins and limits (`sidebar.recent`).
    fn sidebar_recent_json(&self) -> Value { Value::Null }
    /// Hash/stamp of Recent sidebar pins.
    fn sidebar_recent_pins_stamp(&self) -> u64 { 0 }

    /// Whether the Settings overlay/page is currently open.
    fn settings_open(&self) -> bool { false }
    /// Active Settings page ID (e.g. `"general"`, `"providers"`).
    fn settings_page_id(&self) -> &str { "" }
    /// Restores Settings open on `tab` from `lastActiveUrl` (`/settings?tab=...`).
    fn open_restored_settings(&mut self, _tab: &str) {}
    /// Reloads the open Settings page once the live server is bound (`resume_settings_page`).
    fn reopen_settings_page(&mut self, _tx: &mut Tx) {}

    // ── Servers, sessions, transcripts & timeline places ────────────────────────────────────────

    /// Server the window is currently checked out on (`servers::runtime::active`).
    fn active_server(&self) -> ServerKey { ServerKey::sidecar() }
    /// Server the running turn works against (`servers::runtime::current`).
    fn current_server(&self) -> ServerKey { self.active_server() }
    /// Whether the app's own local sidecar server is still in its waiting launch phase.
    fn sidecar_launch_waiting(&self) -> bool { false }
    /// Whether `id` is currently loaded only from the cold boot preview (`launch.cold_transcript`).
    fn is_cold_transcript(&self, _id: &str) -> bool { false }
    /// Records the session ID whose transcript was seeded from the cold boot preview.
    fn set_cold_transcript(&mut self, _id: Option<String>) {}

    /// Whether session `id` exists in the active server's loaded session list.
    fn has_session(&self, _id: &str) -> bool { false }
    /// Title of session `id` in the loaded session list, if present.
    fn session_title(&self, _id: &str) -> Option<&str> { None }
    /// Last-updated timestamp (`SessionInfo::updated`) of session `id`, if loaded.
    fn session_updated(&self, _id: &str) -> Option<i64> { None }

    /// Loaded transcript for session `id`, if in memory.
    fn transcript(&self, _id: &str) -> Option<&Transcript> { None }
    /// Whether session `id`'s transcript is already in memory.
    fn has_transcript(&self, id: &str) -> bool { self.transcript(id).is_some() }
    /// Inserts a decoded boot preview or offline copy transcript into memory.
    fn insert_transcript(&mut self, _id: String, _transcript: Transcript) {}

    /// Timeline row disclosure state (`session_view::disclosure::Disclosures`).
    fn expanded(&self) -> &Disclosures;
    /// Mutable timeline row disclosure state.
    fn expanded_mut(&mut self) -> &mut Disclosures;
    /// Timeline scroll/place tracking (`session_view::jump::Ends`).
    fn ends(&self) -> &Ends;
    /// Mutable timeline scroll/place tracking.
    fn ends_mut(&mut self) -> &mut Ends;

    /// Whether session `id`'s projected timeline currently contains `row`
    /// (`session_view::timeline::has_row`).
    fn timeline_has_row(&self, _id: &str, _row: &str) -> bool { false }
    /// Message index in session `id`'s transcript for timeline `row`
    /// (`session_view::timeline::row_message`).
    fn timeline_row_message(&self, _id: &str, _row: &str) -> Option<usize> { None }
    /// Settles timeline jump positioning after an offline copy lands (`session_view::jump::settle`).
    fn settle_timeline_jump(&mut self, _tx: &mut Tx) {}
    /// Loads older transcript pages toward session `id`'s saved timeline place (`read_toward_place`).
    fn read_toward_place(&mut self, _id: &str, _tx: &mut Tx) {}
    /// Whether session `id`'s paged transcript currently has an unfilled gap below a place preview.
    fn has_pages_gap(&self, _id: &str) -> bool { false }
    /// Notifies an opening session page state machine that a cached copy arrived.
    fn on_copy_received(&mut self, _id: &str) {}

    // ── Side panels & terminal persistence ──────────────────────────────────────────────────────

    /// Restores active-tab side panel state (`side` key from boot record).
    fn restore_side_json(&mut self, _value: &Value) {}
    /// Restores remaining sessions' side panel state (`side.rest` key after hydration).
    fn restore_side_rest(&mut self, _value: &Value) {}
    /// Serializes side panel state for session keys matching `pred`.
    fn side_json_where(&self, _pred: &dyn Fn(&str) -> bool) -> Value { Value::Null }
    /// Hash/stamp of side panel state for change detection in `poll_restore`.
    fn side_stamp(&self) -> u64 { 0 }

    /// Loads persisted terminal workspaces for `server` during `restore_ui`.
    fn load_persisted_terminals(&mut self, _server: &str) {}
    /// Queues and flushes persisted terminal workspaces on window close or app quit.
    fn flush_persisted_terminals(&mut self) {}

    // ── Persistence / storage operations ────────────────────────────────────────────────────────

    /// Whether stage-B persistent storage is hydrated and ready.
    fn storage_is_ready(&self) -> bool { false }
    /// Reads a JSON value from `(namespace, key)`.
    fn storage_json(&self, _namespace: &str, _key: &str) -> Option<Value> { None }
    /// Persists a JSON value under `(namespace, key)`.
    fn storage_set_json(&mut self, _namespace: &str, _key: &str, _value: &Value) {}
    /// Deletes `(namespace, key)` from persistent storage.
    fn storage_delete(&mut self, _namespace: &str, _key: &str) {}
    /// Updates a shared JSON object `(namespace, key)` in place and persists only if changed.
    fn storage_update_json(&mut self, _namespace: &str, _key: &str, _edit: &mut dyn FnMut(&mut Map<String, Value>)) {}
    /// Returns every row in window `window_id`'s namespace (`opencode.window.<id>.dat`).
    fn storage_window_rows(&self, _window_id: &str) -> BTreeMap<String, Arc<str>> { BTreeMap::new() }
    /// Looks up a document by key (drafts, undo histories).
    fn storage_doc(&self, _key: &str) -> Lookup { Lookup::Ready(None) }
    /// Sets (`Some`) or removes (`None`) a document.
    fn storage_set_doc(&mut self, _key: &str, _value: Option<String>) {}
    /// Lists document keys stored under `prefix` (empty until storage is ready).
    fn storage_doc_keys(&self, _prefix: &str) -> Vec<String> { Vec::new() }
    /// Loads documents `keys` into memory on the storage thread, invoking `done` when ready.
    fn storage_load_docs(&mut self, _keys: &[&str], _done: Box<dyn FnOnce() + Send>) {}
    /// Reads a session's boot-packet preview `(updated_at, bytes)`.
    fn storage_boot_preview(&self, _server: &str, _session: &str) -> Option<(i64, Vec<u8>)> { None }
    /// Writes or clears a session's first-screen preview.
    fn storage_set_preview(&mut self, _server: &str, _session: &str, _value: Option<(i64, Arc<[u8]>)>) {}
    /// Runs `op` with the underlying [`Storage`] handle (immediately when ready, else once open).
    fn with_storage(&self, _op: Box<dyn FnOnce(&Storage) + Send>) {}
    /// Flushes queued writes for `reason` (blocking up to deadline when `wait` is true).
    fn storage_flush(&mut self, _reason: FlushReason, _wait: bool) {}
    /// Takes the next pending storage health notice, if any (`storage::take_notice`).
    fn take_storage_notice(&mut self) -> Option<StorageNotice> { None }

    // ── Crash notice, undo, notifications & window lifecycle hooks ──────────────────────────────

    /// Consumes a restore-loop escape sentinel for `session_id` (`last_exit::take_skip_restore`).
    fn take_skip_restore(&mut self, _session_id: &str) -> bool { false }
    /// Marks that this launch is restoring `session_id` (`last_exit::note_restoring_session`).
    fn note_restoring_session(&mut self, _session_id: &str) {}
    /// Updates the currently active session tracked for fatal exit records
    /// (`last_exit::set_active_session`).
    fn set_active_exit_session(&mut self, _session_id: Option<&str>) {}
    /// Records a skipped session for the next-launch recovery toast (`crash_notice::note_skipped`).
    fn note_skipped_restore(&mut self, _server: ServerKey, _session_id: String, _title: String) {}

    /// Commits any pending undoable toast actions before close or quit (`undo::commit_all`).
    fn commit_pending_undos(&mut self) {}
    /// Window pre-close cleanup before persisting (`tear_back`, `close_rune_mcp`).
    fn before_close(&mut self) {}
    /// Window post-persist close teardown (`window_closed`, `servers::runtime::leave_all`).
    fn after_close(&mut self) {}
    /// Notifies the notification subsystem that the app is quitting (`notifications::quitting`).
    fn notify_quitting(&mut self) {}
    /// Window post-persist quit teardown (`window_quitting`, `windows::quit_saved`).
    fn after_quit(&mut self) {}

    /// Shows a failure status toast (`status_toast::fail`).
    fn toast_fail(&mut self, _title: &str, _description: &str) {}
    /// Shows a success status toast (`status_toast::succeed`).
    fn toast_succeed(&mut self, _title: &str, _description: &str) {}
}
