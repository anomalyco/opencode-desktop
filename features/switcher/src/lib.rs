//! ⌘K, ⇧⌘P, ⌘P, ⇧⌘F, and ⇧⌘O — Desktop's "Search everything" command/file/session palette
//! (`shell/commands/{dialog,palette,search}.ts(x)`), shared root-session search index, and
//! project/worktree/branch location switcher (`new-session/location/dialog.tsx`).
//!
//! All crate entrypoints operate on non-generic `&dyn SwitcherHost` / `&mut dyn SwitcherHost`
//! (and `&dyn ProjectSwitcherHost` / `&mut dyn ProjectSwitcherHost` in [`project`]), keeping
//! thin `H: Host + 'static` generic wrappers strictly at `rune::El` view callback boundaries.

#![forbid(unsafe_code)]

mod band;
mod entries;
mod order;
pub mod poll;
pub mod project;
mod search;
pub mod search_index;
pub mod servers;
mod session_index;
pub mod shown;
pub mod view;

use std::cell::RefCell;

use opencode_kit::client::Server;
use opencode_kit::model::{Project, SessionInfo};
use opencode_kit::servers::ServerKey;
use opencode_search::file_rank::parse_path_and_line;
use opencode_search::fulltext::{Kind as FullTextKind, Snippet};
use opencode_search::query::{Highlight, HighlightEvent};
use opencode_ui::keymap::Bind;
use rune::machine::Seq;
use rune::{Chord, Direction, El, Key, KeyEvent, Latest, Phase, PointerEvent, TextEdit, Tx, Waker};
use session_view::palette::Pal;

pub use entries::{
    Action, COMMAND_SEARCH_ID, COMMON, COMMON_VIEW, Entry, HOME_COMMON, MATCHES, Move, RECENT, RECENT_SESSIONS, active, entries, matches, merged_roots, merged_roots_for, move_for, project_label,
    rank, target,
};
pub use poll::{fetch_live_find, poll};
pub use project::{ProjectSwitcher, ProjectSwitcherHost};
pub use search_index::{
    Scope, SessionSearch, find_sessions, is_building as session_search_building, poll as poll_session_search, rest as rest_session_search, search_session,
};
pub use servers::RemoteSeed;
pub use view::{dialog, highlighted, relative_time, snippet_line, snippet_line_parts, view};

/// Overlay stack identifier for the command/file/session switcher (`opencode_kernel::OverlayStack`).
pub const OVERLAY_ID: &str = "dialog.switcher";
/// Overlay stack identifier for the project/worktree/branch switcher (`opencode_kernel::OverlayStack`).
pub const PROJECT_OVERLAY_ID: &str = "dialog.project_switcher";

/// `SESSION_RESTORE_TIMEOUT_MS`: how long the dialog waits for a restore before it gives
/// control back (the request itself keeps running).
pub const RESTORE_TIMEOUT: f64 = 15.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwitcherMode {
    #[default]
    All,
    Sessions,
    /// Search commands: commands only.
    Commands,
    /// The Inbox's Archived button: archived sessions, each restored as it opens
    /// (`DialogSessionSearch archived`).
    Archived,
}

/// One command offered by the host catalog to the palette.
#[derive(Clone, Debug)]
pub struct PaletteCommand {
    pub id: &'static str,
    pub title: String,
    pub description: String,
    pub hint: Option<Bind>,
    pub chord: Option<Chord>,
    pub featured: bool,
}

/// One settings search entry offered by the host (non-entity, non-shortcut).
#[derive(Clone, Debug)]
pub struct PaletteSetting {
    pub id: String,
    pub title: String,
    /// Where the setting lives (e.g. `"Preferences / General"`, `"Appearance"`).
    pub where_: String,
}

/// One full-text search hit supplied by the host for the palette surface.
#[derive(Clone, Debug)]
pub struct FullTextRow {
    pub session: String,
    pub kind: FullTextKind,
    /// The message id that matched, when it belongs to the session transcript.
    pub message: Option<String>,
    pub snippet: Snippet,
    /// Additional matching rows in the same session omitted from the display.
    pub more: usize,
    /// Fallback session metadata when the session is absent from the loaded root list.
    pub fallback: Option<SessionInfo>,
}

/// A session's activity/attention/unread indicators for palette rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SessionMarks {
    pub attention: bool,
    pub busy: bool,
    pub unread: bool,
}

/// Object-safe host boundary for the command/file/session palette and shared session search index.
pub trait SwitcherHost {
    fn switcher(&self) -> &Switcher;
    fn switcher_mut(&mut self) -> &mut Switcher;
    fn session_search(&self) -> &SessionSearch;
    fn session_search_mut(&mut self) -> &mut SessionSearch;

    // Sessions & projects
    fn sessions(&self) -> &[SessionInfo];
    fn session(&self, id: &str) -> Option<&SessionInfo>;
    fn push_session(&mut self, session: SessionInfo);
    fn project(&self, id: &str) -> Option<&Project>;
    fn project_for_session(&self, session: &SessionInfo) -> Option<&Project>;
    /// Tracked `(id, version)` pair for the host's sessions list.
    fn sessions_version(&self) -> (u32, u64);
    /// Tracked `(id, version)` pair for the host's projects list.
    fn projects_version(&self) -> (u32, u64);

    // Active route, clock & layout
    fn is_home(&self) -> bool;
    fn active_session_id(&self) -> Option<&str>;
    /// Whether `id` is the session currently displayed in the active route.
    fn is_session_open(&self, id: &str) -> bool;
    /// Whether `id` is open as a tab (`server == None` checks the active server).
    fn is_tab_open(&self, server: Option<&ServerKey>, id: &str) -> bool;
    fn settings_open(&self) -> bool {
        false
    }
    fn is_stage(&self) -> bool {
        false
    }
    fn home_search_active(&self) -> bool {
        false
    }
    fn view_runs(&self) -> u64 {
        2
    }
    fn clock(&self) -> f64;
    fn now_ms(&self) -> i64 {
        0
    }
    fn waker(&self) -> Option<Waker> {
        None
    }
    /// Shared peer waker for multi-window session index builds (defaults to [`Self::waker`]).
    fn peer_waker(&self) -> Option<Waker> {
        self.waker()
    }
    fn narrow_width(&self) -> f32 {
        0.0
    }
    fn zoom(&self) -> f32 {
        1.0
    }
    fn spellcheck(&self) -> bool {
        false
    }

    // Live server & multi-server
    fn live_server(&self) -> Option<Server>;
    fn active_server(&self) -> ServerKey {
        ServerKey::sidecar()
    }
    fn server_label(&self, key: &ServerKey) -> String {
        key.as_str().to_owned()
    }
    fn server_health_failed(&self, _key: &ServerKey) -> bool {
        false
    }
    /// Snapshots of other visible servers when opening cross-server Sessions or Archived search.
    fn remote_server_seeds(&mut self, _archived: bool) -> Vec<RemoteSeed> {
        Vec::new()
    }
    fn switch_server(&mut self, _server: &ServerKey) {}
    /// Dispatches an unarchive write for `id`; returns `true` when sent to a live server.
    fn unarchive_session(&mut self, _id: String) -> bool {
        false
    }
    fn unarchive_session_locally(&mut self, _id: &str) {}

    // Commands & settings
    fn palette_commands(&self) -> Vec<PaletteCommand> {
        Vec::new()
    }
    fn run_command(&mut self, id: &'static str, tx: &mut Tx);
    /// Non-entity, non-shortcut settings search entries matching `query`.
    fn palette_settings(&self, _query: &str) -> Vec<PaletteSetting> {
        Vec::new()
    }
    fn reveal_setting(&mut self, _id: &str, _tx: &mut Tx) {}

    // Workspace files
    /// Active tab key and all open side-panel tab keys for `session_id`.
    fn side_tab_keys(&self, _session_id: &str) -> Option<(Option<&str>, &[String])> {
        None
    }
    fn local_workspace_files(&self, _session_id: &str, _query: &str, _limit: usize) -> Vec<String> {
        Vec::new()
    }
    /// Fixture-mode extra files (`find_files`) and optional simulated latency in ms (`find_ms`).
    fn fixture_find(&self) -> Option<(Vec<String>, Option<f64>)> {
        None
    }
    fn ensure_workspace_files(&mut self, _session_id: &str) {}
    fn open_workspace_file(&mut self, _path: &str, _preview: bool, _line: Option<u32>, _tx: &mut Tx) {}

    // Full-text search
    fn palette_fulltext_rows(&self) -> Vec<FullTextRow> {
        Vec::new()
    }
    fn palette_indexing_history(&self) -> bool {
        false
    }

    // Lifecycle, overlays, focus & actions
    /// Prepares the shell before opening the switcher (wakes dormant command plugins, closes
    /// conflicting pickers/menus/project switcher, blurs inputs) and returns whether composer
    /// focus should be restored when dismissed.
    fn before_switcher_open(&mut self) -> bool {
        false
    }
    /// Restores (or clears) composer focus after closing the switcher.
    fn after_switcher_close(&mut self, _restore_composer: bool) {}
    /// Registers an opened dialog overlay (`OVERLAY_ID`) with `OverlayStack`.
    fn on_overlay_opened(&mut self, id: &'static str, return_focus: Option<String>);
    /// Removes a closed dialog overlay from `OverlayStack`.
    fn on_overlay_closed(&mut self, id: &'static str);
    fn prefetch_session(&self, _id: &str) {}
    /// Opens session `id`, optionally opening Find at `(query, message_id)`.
    fn open_session(&mut self, id: &str, find: Option<(&str, &str)>, tx: &mut Tx);
    /// Opens a restored archived session via the Inbox path.
    fn open_inbox_session(&mut self, id: &str, tx: &mut Tx) {
        self.open_session(id, None, tx);
    }
    fn toast_fail(&mut self, title: String, description: String);
    fn handle_shortcut(&mut self, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
        false
    }
    fn is_session_hidden(&self, _server: &ServerKey, _id: &str) -> bool {
        false
    }
    fn unhide_session(&mut self, _server: &ServerKey, _id: &str) {}

    // Session row visuals
    fn session_marks(&self, _id: &str) -> SessionMarks {
        SessionMarks::default()
    }
    fn session_avatar(&self, p: &Pal, project: Option<&Project>, name: &str, open: bool, unread: bool) -> El;
    /// Memo invalidation token for session avatars (icon finish, icon cache version, projects version).
    fn avatar_memo_version(&self) -> u64 {
        0
    }
}

/// The session search's pick (`SessionSearchState`): an archived session restores before it opens.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Restore {
    #[default]
    Idle,
    Restoring { session: String, due: f64 },
}

/// A source's answer to the request that carried `attempt`; `Err` when it failed.
pub(crate) struct Reply<T> {
    pub attempt: rune::machine::Attempt,
    pub items: Result<Vec<T>, ()>,
}

pub struct Switcher {
    pub open: bool,
    pub mode: SwitcherMode,
    pub search: TextEdit,
    /// The lit row: the first until the keys or the pointer choose one, and the first again
    /// for each new query (`SearchList`).
    pub(crate) active: Highlight,
    /// The pointer's last point on a row: only a pointer that moved takes a row (`onMouseMove`'s
    /// movement check).
    pointer: Option<[f32; 2]>,
    restore_composer: bool,
    /// Sessions the server found while this was open, kept for later keystrokes
    /// (`SessionSourceState.loaded`).
    pub loaded: Vec<SessionInfo>,
    /// Which rows the dialog builds (`band`).
    pub(crate) band: band::Band,
    pub(crate) order: RefCell<order::Order>,
    pub(crate) seq: Seq,
    /// The server's sessions for the query, and the folder's files (`file.searchFiles`), each a
    /// source machine (`search.rs`).
    pub(crate) sessions: search::Source<()>,
    pub(crate) files: search::Source<String>,
    pub(crate) fetch: Latest<Reply<SessionInfo>>,
    pub(crate) files_fetch: Latest<Reply<String>>,
    /// The fixture's answer to the file query and when it lands (its `find_ms`, as a server's).
    pub(crate) fixture_find: Option<(f64, Reply<String>)>,
    /// What the list shows while its searches run (`shown.rs`).
    pub(crate) shown: shown::Shown,
    pub(crate) restore: Restore,
    /// The other servers' session sources while it searches sessions (`servers.rs`).
    pub(crate) remotes: Vec<servers::Remote>,
}

impl Default for Switcher {
    fn default() -> Self {
        Switcher {
            open: false,
            mode: SwitcherMode::All,
            search: TextEdit::new(""),
            active: Highlight::default(),
            pointer: None,
            restore_composer: false,
            loaded: Vec::new(),
            band: Default::default(),
            order: Default::default(),
            seq: Seq::default(),
            sessions: search::Source::default(),
            files: search::Source::default(),
            fetch: Latest::default(),
            files_fetch: Latest::default(),
            fixture_find: None,
            shown: shown::Shown::default(),
            restore: Restore::Idle,
            remotes: Vec::new(),
        }
    }
}

impl Switcher {
    pub fn set_waker(&mut self, waker: Waker) {
        self.fetch = Latest::new(waker.clone());
        self.files_fetch = Latest::new(waker);
    }

    /// The keys highlight row `key`.
    pub fn activate(&mut self, key: String) {
        self.active.set(key);
    }

    /// The pointer moved on row `key` to `at`, in window points (`SearchList`'s `onMouseMove`):
    /// only a moving pointer takes the row, so rows the keys, the wheel or late results carry
    /// under a still one leave the highlight alone. True when the highlight changed.
    pub fn point(&mut self, key: &str, at: [f32; 2]) -> bool {
        if self.pointer.replace(at) == Some(at) || self.active.pointed(key) {
            return false;
        }
        self.active.step(HighlightEvent::Pointer(key.to_owned()));
        true
    }

    /// The field's text changed: the list starts again from its first row.
    pub fn query_changed(&mut self) {
        self.active.step(HighlightEvent::QueryChanged);
    }

    pub fn query(&self) -> &str {
        self.search.text.trim()
    }

    pub fn restore_composer(&self) -> bool {
        self.restore_composer
    }

    pub fn loaded(&self) -> &[SessionInfo] {
        &self.loaded
    }

    /// The typed query's session search failed (`selectPaletteSearchFailed`).
    pub(crate) fn failed_now(&self) -> bool {
        self.sessions.failed() && self.sessions.query() == Some(self.query())
    }
}

/// Opens the switcher in [`SwitcherMode::All`] without an explicit transaction.
pub fn open(host: &mut dyn SwitcherHost) {
    open_mode(host, SwitcherMode::All, None);
}

/// Opens the switcher in `mode`, capturing the currently focused element from `tx` for overlay return focus.
pub fn open_with_tx(host: &mut dyn SwitcherHost, mode: SwitcherMode, tx: &mut Tx) {
    open_mode(host, mode, tx.focused().map(str::to_owned));
}

pub fn open_session_search(host: &mut dyn SwitcherHost, tx: &mut Tx) {
    tx.instant();
    open_with_tx(host, SwitcherMode::Sessions, tx);
}

/// Search commands: the palette on commands alone.
pub fn open_command_search(host: &mut dyn SwitcherHost, tx: &mut Tx) {
    tx.instant();
    open_with_tx(host, SwitcherMode::Commands, tx);
}

/// ⌘P, `file.open`: the same palette ⌘K opens over a session (`use-session-commands.tsx`).
pub fn open_file_search(host: &mut dyn SwitcherHost, tx: &mut Tx) {
    tx.instant();
    if let Some(sid) = host.active_session_id().map(str::to_owned) {
        host.ensure_workspace_files(&sid);
    }
    open_with_tx(host, SwitcherMode::All, tx);
}

/// The Inbox's Archived button: session search over archived sessions.
pub fn open_archived_search(host: &mut dyn SwitcherHost, tx: &mut Tx) {
    tx.instant();
    open_with_tx(host, SwitcherMode::Archived, tx);
}

/// Opens or switches the palette to `mode`, notifying `OverlayStack` when opening from closed.
pub fn open_mode(host: &mut dyn SwitcherHost, mode: SwitcherMode, return_focus: Option<String>) {
    let was_open = host.switcher().open;
    if was_open && host.switcher().mode == mode {
        return;
    }
    let prev_restore = host.switcher().restore_composer;
    let shell_restore = host.before_switcher_open();
    let restore = if was_open { prev_restore } else { shell_restore };
    let now = host.clock();
    {
        let sw = host.switcher_mut();
        sw.restore_composer = restore;
        sw.open = true;
        sw.mode = mode;
        *sw.order.get_mut() = Default::default();
        sw.search = TextEdit::new("");
        sw.search.focused = true;
        sw.active.clear();
        sw.pointer = None;
        sw.loaded.clear();
        sw.fetch.cancel();
        sw.files_fetch.cancel();
        sw.fixture_find = None;
        sw.sessions = search::Source::default();
        sw.files = search::Source::default();
        sw.restore = Restore::Idle;
    }
    let remotes = servers::open(host);
    {
        let sw = host.switcher_mut();
        sw.remotes = remotes;
        sw.shown.step(shown::Event::Opened { now });
    }
    if !was_open {
        host.on_overlay_opened(OVERLAY_ID, return_focus);
    }
}

/// Closes the switcher; `restore` gives the composer back its focus if it had it.
pub fn close(host: &mut dyn SwitcherHost, restore: bool) {
    if !host.switcher().open {
        return;
    }
    let should_restore = {
        let sw = host.switcher_mut();
        sw.open = false;
        sw.search.focused = false;
        sw.search.cancel_composition();
        sw.fetch.cancel();
        sw.files_fetch.cancel();
        sw.fixture_find = None;
        sw.sessions = search::Source::default();
        sw.files = search::Source::default();
        sw.restore = Restore::Idle;
        sw.remotes.clear();
        restore && sw.restore_composer
    };
    host.after_switcher_close(should_restore);
    host.on_overlay_closed(OVERLAY_ID);
}

/// Executes a selected palette entry action.
pub fn run_action(host: &mut dyn SwitcherHost, action: Action, tx: &mut Tx) {
    if !host.switcher().open {
        return;
    }
    // One pick at a time: Enter or a click while a session restores does nothing.
    if matches!(host.switcher().restore, Restore::Restoring { .. }) {
        return;
    }
    // Another server's session: the window works against its server from here on, and it
    // opens (or restores) there as one of its own.
    if let Action::RemoteSession { server, id } = action {
        let found = host.switcher().remotes.iter().find(|r| r.server == server).and_then(|r| r.session(&id).cloned());
        host.switch_server(&server);
        if let Some(info) = found.filter(|_| host.session(&id).is_none()) {
            host.switcher_mut().loaded.push(info);
        }
        return run_action(host, Action::Session(id), tx);
    }
    let restore = host.switcher().mode == SwitcherMode::Archived;
    let line = parse_path_and_line(host.switcher().query()).1;
    // An archived session is restored first, and opens once the server has
    // (`session-search-machine.ts`): the dialog waits for it.
    if let (true, Action::Session(id)) = (restore, &action) {
        let due = tx.clock() + RESTORE_TIMEOUT;
        host.switcher_mut().restore = Restore::Restoring { session: id.clone(), due };
        tx.wake_at(due);
        let stage = host.is_stage();
        if !host.unarchive_session(id.clone()) || stage {
            restore_replied(host, id.clone(), stage, "", tx);
        }
        return;
    }
    // A session only its messages matched opens with Find on the query, so what matched
    // lights in its timeline (a draft's words are in its composer).
    let q = host.switcher().query().trim().to_owned();
    let find = match &action {
        Action::Session(id) => found_message(host, id, &q),
        _ => None,
    };
    let back = host.switcher().restore_composer;
    close(host, false);
    if let Action::Session(id) = &action
        && !restore
        && host.session(id).is_none()
        && let Some(s) = host.switcher().loaded.iter().find(|s| s.id == *id).cloned()
    {
        host.push_session(s);
    }
    match action {
        Action::Session(_) if restore => {}
        Action::Session(id) => {
            host.open_session(&id, find.as_deref().map(|m| (q.as_str(), m)), tx);
        }
        Action::RemoteSession { .. } => {}
        Action::Command(id) => {
            host.run_command(id, tx);
            // A command that moves focus itself keeps it (`Tx::return_focus` yields to it);
            // otherwise the keyboard goes back to the prompt it came from, rather than to
            // nothing (in a browser, focus said nothing after Toggle review).
            if back {
                tx.return_focus("prompt", tx.focus_visible());
            }
        }
        // `path:line` (or `path#L12`) opens the file at that line.
        // Keyboard focus follows into the file, onto its line, so a reader lands in what
        // opened rather than on the window.
        Action::File(path) => {
            host.open_workspace_file(&path, false, line, tx);
            tx.focus_when_shown(format!("cl{}", line.unwrap_or(1).max(1)), tx.focus_visible());
        }
        Action::Setting(id) => {
            host.reveal_setting(&id, tx);
        }
    }
}

/// The server answered a restore of archived session `id` (or it timed out, `ok` false).
pub fn restore_replied(host: &mut dyn SwitcherHost, id: String, ok: bool, why: &str, tx: &mut Tx) {
    let ours = matches!(&host.switcher().restore, Restore::Restoring { session, .. } if *session == id);
    if !ours {
        // Restored after the dialog stopped waiting: it shows as restored, and no tab opens.
        if ok {
            host.unarchive_session_locally(&id);
        }
        return;
    }
    host.switcher_mut().restore = Restore::Idle;
    if !ok {
        let title = opencode_i18n::t!("common.requestFailed");
        host.toast_fail(title.to_owned(), (if why.is_empty() { title } else { why }).to_owned());
        return;
    }
    if host.session(&id).is_none()
        && let Some(s) = host.switcher().loaded.iter().find(|s| s.id == id).cloned()
    {
        host.push_session(s);
    }
    host.unarchive_session_locally(&id);
    close(host, false);
    host.open_inbox_session(&id, tx);
}

/// ⌘K and ⇧⌘P toggle "Search everything", ⇧⌘F "Search sessions" and ⌘P,
/// over a session, "Search everything" again; while open, the list's keys.
pub fn key(host: &mut dyn SwitcherHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if Bind::CommandSearch.matches_tx(ev, tx) {
        if host.switcher().open && host.switcher().mode == SwitcherMode::Commands {
            close(host, true);
        } else {
            open_command_search(host, tx);
        }
        return true;
    }
    if Bind::FileOpen.matches_tx(ev, tx) && host.active_session_id().is_some() && !host.settings_open() {
        if host.switcher().open && host.switcher().mode == SwitcherMode::All {
            tx.instant();
            close(host, true);
        } else {
            open_file_search(host, tx);
        }
        return true;
    }
    for (bind, mode) in [(Bind::Palette, SwitcherMode::All), (Bind::SessionSearch, SwitcherMode::Sessions)] {
        if !bind.matches_tx(ev, tx) {
            continue;
        }
        if host.switcher().open && host.switcher().mode == mode {
            close(host, true);
        } else if mode == SwitcherMode::All {
            open_file_search(host, tx);
        } else {
            tx.instant();
            open_with_tx(host, mode, tx);
        }
        return true;
    }
    host.switcher().open && nav(host, ev, tx)
}

/// The search field's keys: moves, Enter picks, Escape closes.
pub fn nav(host: &mut dyn SwitcherHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if host.switcher().search.is_composing() {
        return false;
    }
    if let Some(mv) = move_for(ev) {
        let rows = entries(host);
        let lit = active(host, &rows);
        let from = rows.iter().position(|e| Some(&e.key) == lit.as_ref()).unwrap_or(0);
        let sections: Vec<&str> = rows.iter().map(|e| e.group).collect();
        if let Some(i) = target(&sections, from, mv) {
            let key = rows[i].key.clone();
            tx.instant();
            if i == 0 {
                tx.scroll_to_y("switcher-scroll", 0.0);
            } else {
                tx.scroll_into_view_in("switcher-scroll", format!("sw-{key}"));
            }
            host.switcher_mut().activate(key);
            if let Action::Session(id) = &rows[i].action {
                host.prefetch_session(id);
            }
        }
        return true;
    }
    match ev.key {
        Key::Enter if !ev.mods.shift => {
            let rows = entries(host);
            if let Some(key) = active(host, &rows)
                && let Some(entry) = rows.into_iter().find(|e| e.key == key)
            {
                run_action(host, entry.action, tx);
            }
            true
        }
        Key::Escape => {
            tx.instant();
            close(host, true);
            true
        }
        _ => false,
    }
}

/// Percent-encodes a query parameter's value.
pub fn encode(s: &str) -> String {
    opencode_search::query::percent_encode(s)
}

/// The message to open session `id` at from palette full-text results for `query`.
fn found_message(host: &dyn SwitcherHost, id: &str, query: &str) -> Option<String> {
    let q = query.trim();
    let fold = opencode_search::SearchFold::for_query(q);
    let rows = host.palette_fulltext_rows();
    let row = rows.iter().find(|r| r.session == id)?;
    let titled = host.session(id).is_some_and(|s| fold.fold(&s.title).contains(&fold.fold(q)));
    row.message.clone().filter(|_| !titled)
}

// --- Thin non-generic callback helpers invoked by `view.rs` ---

pub(crate) fn nav_or_shortcut(host: &mut dyn SwitcherHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    nav(host, ev, tx) || host.handle_shortcut(ev, tx)
}

pub(crate) fn row_key(host: &mut dyn SwitcherHost, action: &Action, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let press = matches!(ev.key, Key::Enter) || ev.key == Key::Text(" ".into());
    if press && !ev.mods.cmd && !ev.mods.alt && !ev.mods.shift {
        run_action(host, action.clone(), tx);
    }
    press
}

pub(crate) fn row_pointer_move(host: &mut dyn SwitcherHost, key: &str, ev: PointerEvent, tx: &mut Tx) {
    if !host.switcher_mut().point(key, [ev.window_x, ev.window_y]) {
        return tx.live_only();
    }
    tx.instant();
    if let Some(id) = key.strip_prefix("session:") {
        host.prefetch_session(id);
    }
}

pub(crate) fn row_drag(host: &mut dyn SwitcherHost, id: &str, phase: Phase) {
    if phase == Phase::Down {
        host.prefetch_session(id);
    }
}

pub(crate) fn unhide_and_close(host: &mut dyn SwitcherHost, server: &ServerKey, id: &str, tx: &mut Tx) {
    tx.instant();
    host.unhide_session(server, id);
    close(host, true);
}

// --- Shared pure display/path helpers ---

pub(crate) fn ui_direction() -> Direction {
    match opencode_i18n::direction() {
        opencode_i18n::Direction::Ltr => Direction::Ltr,
        opencode_i18n::Direction::Rtl => Direction::Rtl,
    }
}

pub(crate) fn title_of(s: &SessionInfo) -> String {
    if s.title.is_empty() { opencode_i18n::t!("rune.session.untitled").into() } else { s.title.clone() }
}

pub(crate) fn project_or_dir(project: Option<&Project>, dir: &str) -> String {
    project.map(|p| p.name.clone()).unwrap_or_else(|| opencode_kit::path::basename(dir).to_owned())
}

pub(crate) fn clean_dir(raw: &str) -> String {
    opencode_kit::path::trim(raw.trim()).to_owned()
}

pub(crate) fn folder_name(dir: &str) -> String {
    Some(opencode_kit::path::basename(dir)).filter(|n| !n.is_empty()).unwrap_or(dir).to_owned()
}

pub(crate) fn is_automation(title: &str) -> bool {
    title.starts_with("flow:")
}

pub(crate) fn search_fold() -> rune_kit::search::Fold {
    rune_kit::search::Fold::for_language(opencode_i18n::locale().tag())
}
