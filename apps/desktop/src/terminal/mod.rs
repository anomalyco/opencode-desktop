//! Native Ghostty terminal pane (a side panel, `⌃\``), workspace tabs, persistence,
//! theme sync, and knobs (`apps/desktop/src/terminal/`).

mod draw;
pub mod grid;
pub mod input;
pub mod narrow;
pub mod persist;
mod rename;
mod side_ops;
pub mod story;
pub mod mount;
pub mod streams;

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use opencode_kit::persistent_pty::{PersistentPtyInfo, TerminalCreate};
use rune::{Tx, Waker};
use rune_knobs::{Group, Knob};
use terminal::{
    RestoreState, SessionStatus, StreamHandle, TerminalConfig, TerminalSession, TerminalTheme,
    VtEvent,
};

use crate::palette::{Pal, pal};
pub use grid::set_custom_font;
use crate::{App, Route, Source};
use persist::StoredWorkspace;

pub const MAX_TERMINAL_WORKSPACES: usize = 20;

pub const TERM_FONT: Knob = Knob::choice(
    "Terminal font",
    &["Commit Mono", "IBM Plex Mono", "SF Mono", "JetBrains Mono"],
    3,
)
.about("Monospace font family in the terminal")
.hints(&[
    "Kit's Ghostty Commit Mono face",
    "OpenCode bundled IBM Plex Mono",
    "macOS SF Mono system face",
    "Desktop's JetBrainsMono Nerd Font Mono",
]);

pub const TERM_SIZE: Knob = Knob::new("Font size", 11.0, 18.0, 14.0)
    .step(0.5)
    .about("Terminal font size in points");

pub const TERMINAL_KNOBS: &[Knob] = &[TERM_FONT, TERM_SIZE];

rune_register::register!(crate::knobs::GROUP_LIST: Group = [
    Group::new("Terminal", "terminal", TERMINAL_KNOBS),
]);

pub(crate) use rename::stamp as rename_stamp;
pub(crate) use side_ops::{ime_terminal, key_terminal, poll_panel, surface, tab_label};

impl App {
    /// Makes terminal `id` session `sid`'s active one; with `focus` it takes the keyboard (a tab
    /// the user selected, not one a restore did).
    pub(crate) fn select_session_terminal(&mut self, sid: &str, id: &str, focus: bool) {
        side_ops::on_focus(self, sid, id, !focus);
    }
}

pub struct TerminalTab {
    pub id: String,
    pub title: String,
    pub title_number: usize,
    pub custom_title: bool,
    pub session: TerminalSession,
    pub stream: Option<StreamHandle>,
    pub fixture_shell: bool,
    pub scroll_px_accum: f32,
    pub drag: mount::TextDrag,
    /// Whether it has an attachment (`surface-machine.ts`).
    pub mount: mount::Mount,
    pub(crate) fit: Cell<mount::Fit>,
}

impl TerminalTab {
    pub fn display_title(&self) -> String {
        if !self.title.trim().is_empty() {
            self.title.clone()
        } else {
            opencode_i18n::t!("plugin.terminal.title.numbered", number = self.title_number.max(1))
        }
    }

    /// Asks the session for the pane's grid when it changed (the worker debounces it).
    pub fn ensure_fit(&self, cols: u16, rows: u16, cw_px: u32, ch_px: u32) {
        let next = mount::Fit::Fitted { cols, rows, cell_px: (cw_px, ch_px) };
        if self.fit.get() != next {
            self.fit.set(next);
            self.session.resize(cols, rows, cw_px, ch_px);
        }
    }
}

/// One session's terminals (Desktop keeps them per session, on the server).
#[derive(Default)]
pub struct TerminalWorkspace {
    /// Where the session's shells start.
    pub directory: String,
    pub active: Option<String>,
    pub tabs: Vec<TerminalTab>,
    /// Why the server's terminals couldn't be listed or started; shown instead of a shell.
    pub failure: Option<String>,
}

impl TerminalWorkspace {
    pub fn next_title_number(&self) -> usize {
        let used: BTreeSet<usize> = self
            .tabs
            .iter()
            .filter_map(|t| {
                if t.title_number > 0 {
                    Some(t.title_number)
                } else {
                    persist::number_from_title(&t.title)
                }
            })
            .collect();
        (1..=used.len() + 1).find(|n| !used.contains(n)).unwrap_or(1)
    }
}

pub struct TerminalPane {
    pub workspaces: HashMap<String, TerminalWorkspace>,
    pub stored: HashMap<String, StoredWorkspace>,
    /// Whether the saved workspaces were taken in.
    pub saved: persist::Saved,
    pub lru_order: Vec<String>,
    pub blink_epoch: f64,
    pub bell_until: f64,
    pub last_blink_lit: bool,
    /// The appearance and theme (`Pal::theme`) the open terminals were last painted in.
    pub last_dark: Option<(bool, usize, u64)>,
    /// The appearance the window last drew in, resolved from the System setting; state
    /// updates have no `Cx` to ask.
    pub painted_dark: Cell<Option<bool>>,
    pub dirty_flag: Arc<AtomicBool>,
    pub story_focused: bool,
    pub story_built_variant: Option<usize>,
    /// The tab whose title is a field (`TerminalTabLabel`'s inline rename).
    pub renaming: rename::Renames,
    /// The server's connections so far: each reconnect starts the next (`machine.ts` `connections`).
    pub connections: u32,
}

impl Default for TerminalPane {
    fn default() -> Self {
        Self {
            workspaces: HashMap::new(),
            stored: HashMap::new(),
            saved: persist::Saved::default(),
            lru_order: Vec::new(),
            blink_epoch: 0.0,
            bell_until: 0.0,
            last_blink_lit: true,
            last_dark: None,
            painted_dark: Cell::new(None),
            dirty_flag: Arc::new(AtomicBool::new(false)),
            story_focused: true,
            story_built_variant: None,
            renaming: rename::Renames::default(),
            connections: 0,
        }
    }
}

impl TerminalPane {
    pub fn cursor_lit(&self, clock: f64) -> bool {
        let dt = (clock - self.blink_epoch).max(0.0);
        (dt % 1.0) < 0.5
    }

    pub fn reset_blink(&mut self, clock: f64) {
        self.blink_epoch = clock;
        self.last_blink_lit = true;
    }

    pub(crate) fn touch_lru(&mut self, key: &str) {
        self.lru_order.retain(|k| k != key);
        self.lru_order.push(key.to_owned());
        while self.lru_order.len() > MAX_TERMINAL_WORKSPACES {
            let oldest = self.lru_order.remove(0);
            self.workspaces.remove(&oldest);
            self.stored.remove(&oldest);
        }
    }
}

pub fn theme_from_pal(p: &Pal) -> TerminalTheme {
    let base = if p.dark {
        TerminalTheme::opencode_dark()
    } else {
        TerminalTheme::opencode_light()
    };
    let fg = terminal::Rgb::from_rune(p.term_fg);
    let bg = terminal::Rgb::from_rune(p.bg_base);
    TerminalTheme {
        dark: p.dark,
        fg,
        bg,
        cursor: fg,
        selection_alpha: if p.dark { 0.25 } else { 0.20 },
        ansi_base: base.ansi_base,
    }
}

/// A session's terminals, keyed by server and session (`TerminalModel.load(session)`).
pub fn workspace_key(server_base: &str, session_id: &str) -> String {
    format!("{server_base}\0{session_id}")
}

/// The last path segment of a Windows or POSIX directory.
pub(crate) fn dir_name(directory: &str) -> &str {
    Some(opencode_kit::path::basename(directory)).filter(|d| !d.is_empty()).unwrap_or("workspace")
}

impl App {
    pub(crate) fn server_id_str(&self) -> &str {
        match &*self.source {
            Source::Live { server, .. } => server.base.as_str(),
            Source::Fixture => "fixture",
        }
    }

    /// The server whose session terminals this window shows, and whether it may only watch
    /// them: `--read-only` and headless runs never start a shell or type into one.
    pub(crate) fn live_terminals(&self) -> Option<(opencode_kit::client::Server, bool)> {
        if self.stage.is_some() {
            return None;
        }
        match &*self.source {
            Source::Live { server, allow_send, .. } => Some((server.clone(), !allow_send)),
            Source::Fixture => None,
        }
    }

    /// The appearance the terminals paint in: the one the window last drew, else the setting.
    pub(crate) fn terminal_dark(&self) -> bool {
        self.terminal
            .painted_dark
            .get()
            .unwrap_or(self.settings.scheme == crate::settings::Scheme::Dark)
    }

    /// The session a terminal workspace belongs to, and the directory its shells start in.
    fn terminal_session_dir(&self, ws_key: &str) -> (String, String) {
        let sid = ws_key.split_once('\0').map_or("", |(_, s)| s).to_owned();
        let dir = self.session(&sid).map_or_else(|| self.draft_dir().to_owned(), |s| s.directory.clone());
        (sid, dir)
    }

    pub fn terminal_waker(&self) -> Waker {
        let dirty = self.terminal.dirty_flag.clone();
        let app_waker = (*self.waker).clone();
        Arc::new(move || {
            dirty.store(true, Ordering::Release);
            if let Some(w) = &app_waker {
                w();
            }
        })
    }

    pub fn terminal_workspace_key_for_session(&self, session_id: &str) -> String {
        workspace_key(self.server_id_str(), session_id)
    }

    pub fn active_terminal_workspace_key(&self) -> Option<String> {
        if self.stage.is_some() {
            return Some("story\0/workspace/seer".into());
        }
        let Route::Session { id: sid, .. } = &*self.route else {
            return None;
        };
        Some(self.terminal_workspace_key_for_session(sid))
    }

    /// Loads a session's terminals and, when it has none, starts one, as opening Desktop's
    /// terminal panel does. A read-only window only shows the ones the server already has.
    pub fn ensure_workspace_ready(&mut self, ws_key: &str) {
        self.ensure_terminal_workspace_hydrated(ws_key);
        let empty = self
            .terminal
            .workspaces
            .get(ws_key)
            .is_none_or(|ws| ws.tabs.is_empty() && ws.failure.is_none());
        let watch_only = self.live_terminals().is_some_and(|(_, observer)| observer);
        if empty && !watch_only {
            self.new_terminal_in(ws_key);
        }
    }

    /// Opens session `sid`'s terminal tab (a new terminal when `new_tab` or it has none), leaving
    /// the keyboard where it was.
    pub fn open_terminal_for_session(&mut self, sid: &str, new_tab: bool) -> Option<String> {
        self.keeping_keyboard(|s| {
            let ws_key = s.terminal_workspace_key_for_session(sid);
            if new_tab {
                s.ensure_terminal_workspace_hydrated(&ws_key);
                s.new_terminal_in(&ws_key);
            } else {
                s.ensure_workspace_ready(&ws_key);
            }
            let id = s.terminal.workspaces.get(&ws_key).and_then(|ws| ws.active.clone())?;
            s.show_side_tab(sid, &crate::panels::panel_key(side_ops::PLUGIN, &id), None);
            Some(id)
        })
    }

    /// The keyboard goes to the routed session's active terminal, in the pane showing its tab.
    pub fn focus_terminal_surface(&mut self) {
        self.composer.focused = false;
        self.side.focused = true;
        if let Some(sid) = self.active_session_id().map(str::to_owned)
            && let Some(id) = self.terminal.workspaces.get(&self.terminal_workspace_key_for_session(&sid)).and_then(|ws| ws.active.clone())
        {
            let key = crate::panels::panel_key(side_ops::PLUGIN, &id);
            self.side.active_pane_focus = crate::panels::tabs::session_tab_pane(&self.session_side_mut(&sid).tab_state.tabs, &key);
        }
        self.terminal.story_focused = true;
        self.terminal.reset_blink(*self.clock);
    }

    pub fn terminal_tab_mut(&mut self, ws_key: &str, tab_id: &str) -> Option<&mut TerminalTab> {
        self.terminal
            .workspaces
            .get_mut(ws_key)?
            .tabs
            .iter_mut()
            .find(|t| t.id == tab_id)
    }

    pub fn select_terminal_tab(&mut self, ws_key: &str, tab_id: &str) {
        let mut changed = false;
        if let Some(ws) = self.terminal.workspaces.get_mut(ws_key)
            && ws.tabs.iter().any(|t| t.id == tab_id)
            && ws.active.as_deref() != Some(tab_id)
        {
            ws.active = Some(tab_id.to_owned());
            changed = true;
        }
        self.focus_terminal_surface();
        if changed {
            self.queue_terminal_save();
        }
    }

    pub fn select_adjacent_terminal_tab(&mut self, ws_key: &str, offset: i32, tx: &mut Tx) -> bool {
        let next = self.terminal.workspaces.get(ws_key).and_then(|ws| {
            if ws.tabs.len() <= 1 {
                return None;
            }
            let keys: Vec<&str> = ws.tabs.iter().map(|t| t.id.as_str()).collect();
            crate::shortcuts::adjacent(&keys, ws.active.as_deref(), offset, |_| true)
        });
        if let Some(id) = next {
            tx.instant();
            self.select_terminal_tab(ws_key, &id);
            return true;
        }
        false
    }

    /// Removes terminal `tab_id` and ends its shell; its tab has left, or leaves, the strip.
    pub fn close_terminal_tab(&mut self, ws_key: &str, tab_id: &str, tx: &mut Tx) {
        if let Some(ws) = self.terminal.workspaces.get_mut(ws_key) {
            if let Some(idx) = ws.tabs.iter().position(|t| t.id == tab_id) {
                ws.tabs.remove(idx);
                if ws.active.as_deref() == Some(tab_id) {
                    ws.active = if idx > 0 {
                        ws.tabs.get(idx - 1).map(|t| t.id.clone())
                    } else {
                        ws.tabs.first().map(|t| t.id.clone())
                    };
                }
            }
        }

        // Closing a tab ends its shell on the server, as Desktop's close does.
        if let Some((srv, false)) = self.live_terminals()
            && tab_id.starts_with("pty_")
        {
            let id = tab_id.to_owned();
            std::thread::spawn(move || {
                let _ = srv.terminal_remove(&id);
            });
        }

        tx.instant();
        self.queue_terminal_save();
    }

    /// Starts a terminal: on a live server, a session shell (`persistentPty.create`) named
    /// after the lowest free "Terminal N"; in a fixture scene, a recorded prompt.
    pub fn new_terminal_in(&mut self, ws_key: &str) -> Option<String> {
        let theme = theme_from_pal(pal(self.terminal_dark()));
        let (session_id, directory) = self.terminal_session_dir(ws_key);
        self.terminal.touch_lru(ws_key);
        let num = self
            .terminal
            .workspaces
            .entry(ws_key.to_owned())
            .or_insert_with(|| TerminalWorkspace { directory: directory.clone(), ..TerminalWorkspace::default() })
            .next_title_number();
        let title = format!("Terminal {num}");
        let (init_cols, init_rows) = self.estimated_terminal_grid();

        if let Some((srv, observer)) = self.live_terminals() {
            if observer {
                return None;
            }
            let created = srv.terminal_create(
                &session_id,
                &TerminalCreate::new(title).cwd(Some(directory)).size(Some((init_cols, init_rows))),
            );
            let tab = created.and_then(|info| self.persistent_tab(srv, info));
            let ws = self.terminal.workspaces.get_mut(ws_key)?;
            return match tab {
                Ok(tab) => {
                    let id = tab.id.clone();
                    ws.failure = None;
                    ws.tabs.push(tab);
                    ws.active = Some(id.clone());
                    self.focus_terminal_surface();
                    self.queue_terminal_save();
                    Some(id)
                }
                Err(_) if !ws.tabs.is_empty() => None,
                Err(e) => {
                    ws.failure = Some(e);
                    None
                }
            };
        }

        let short_dir = dir_name(&directory);
        let prompt = format!(
            "\x1b[1;34m~/{short_dir}\x1b[0m \x1b[38;5;245mgit:(\x1b[1;36mmain\x1b[0;38;5;245m)\x1b[0m \x1b[1;32m\u{276f}\x1b[0m "
        );
        let banner = if num == 1 {
            format!(
                "{prompt}cargo test --quiet\r\n\x1b[1;32m   Finished\x1b[0m `test` profile in 0.42s\r\n\
                 \x1b[1;32mtest result: ok.\x1b[0m 14 passed; 0 failed\r\n\r\n{prompt}"
            )
        } else {
            prompt
        };
        let id = self.insert_stream_terminal(
            ws_key,
            &format!("term_{num}"),
            &title,
            num,
            TerminalConfig { cols: init_cols, rows: init_rows, theme, ..TerminalConfig::default() },
            Some(RestoreState {
                buffer: banner.into_bytes(),
                cols: Some(init_cols),
                rows: Some(init_rows),
                scroll_y: None,
                cursor: None,
            }),
            b"",
        )?;
        if let Some(tab) = self.terminal_tab_mut(ws_key, &id) {
            tab.fixture_shell = true;
        }
        self.focus_terminal_surface();
        self.queue_terminal_save();
        Some(id)
    }

    /// `Terminal:Create`: starts a terminal of session `sid` on the server the window works
    /// against, as [`App::new_terminal_in`] does (on this thread), running `command` with `args`
    /// in `cwd` instead of the user's shell when given; named `title`, else the lowest free
    /// "Terminal N". [`App::adopt_terminal`] then adds it to the session's terminals.
    pub(crate) fn start_terminal(&self, sid: &str, title: Option<String>, cwd: Option<String>, command: Option<String>, args: Vec<String>) -> Result<PersistentPtyInfo, String> {
        let Some((srv, false)) = self.live_terminals() else { return Err("This window can't start terminals".into()) };
        let wk = self.terminal_workspace_key_for_session(sid);
        let title = title.filter(|t| !t.is_empty()).unwrap_or_else(|| format!("Terminal {}", self.terminal.workspaces.get(&wk).map_or(1, TerminalWorkspace::next_title_number)));
        let cwd = cwd.filter(|c| !c.is_empty()).or_else(|| self.session(sid).map(|s| s.directory.clone()));
        srv.terminal_create(sid, &TerminalCreate::new(title).cwd(cwd).size(Some(self.estimated_terminal_grid())).command(command, args))
    }

    /// A terminal [`App::start_terminal`] started joins session `sid`'s terminals and becomes
    /// its active one, unless the server's event already brought it.
    pub(crate) fn adopt_terminal(&mut self, sid: &str, info: PersistentPtyInfo) {
        let wk = self.terminal_workspace_key_for_session(sid);
        self.ensure_terminal_workspace_hydrated(&wk);
        let Some((srv, _)) = self.live_terminals() else { return };
        let directory = self.session(sid).map(|s| s.directory.clone()).unwrap_or_default();
        let ws = self.terminal.workspaces.entry(wk.clone()).or_insert_with(|| TerminalWorkspace { directory, ..TerminalWorkspace::default() });
        let id = info.id.clone();
        if !ws.tabs.iter().any(|t| t.id == id) {
            let Ok(tab) = self.persistent_tab(srv, info) else { return };
            let Some(ws) = self.terminal.workspaces.get_mut(&wk) else { return };
            ws.failure = None;
            ws.tabs.push(tab);
        }
        self.select_terminal_tab(&wk, &id);
        self.queue_terminal_save();
    }

    /// The grid a new terminal starts at, before the pane first measures itself.
    fn estimated_terminal_grid(&self) -> (u16, u16) {
        let (font_choice, style) = grid::terminal_font_style(&self.knobs);
        let (cell_w, cell_h) = grid::cell_metrics(font_choice, style);
        let est_w = (self.side_width(self.side.laid_w.get()) - grid::PAD_X * 2.0 - 8.0).max(cell_w * 10.0);
        let est_bar = grid::BAR_H;
        let est_pane_h = if self.dock_placement() == crate::panels::DockPlacement::Bottom { self.bottom_height() } else { 744.0 / *self.zoom };
        let est_h = (est_pane_h - est_bar - grid::PAD_Y * 2.0).max(cell_h * 2.0);
        (((est_w / cell_w).floor() as u16).clamp(10, 500), ((est_h / cell_h).floor() as u16).clamp(2, 240))
    }

    /// A tab attached to one of the server's session terminals.
    pub(crate) fn persistent_tab(
        &self,
        srv: opencode_kit::client::Server,
        info: PersistentPtyInfo,
    ) -> Result<TerminalTab, String> {
        let observer = self.live_terminals().is_none_or(|(_, observer)| observer);
        let config = TerminalConfig {
            cols: info.size.0,
            rows: info.size.1,
            theme: theme_from_pal(pal(self.terminal_dark())),
            ..TerminalConfig::default()
        };
        let session = TerminalSession::spawn_persistent_pty(srv, info.id.clone(), config, observer, self.terminal_waker())?;
        Ok(TerminalTab {
            title_number: persist::number_from_title(&info.title).unwrap_or(0),
            custom_title: info.user_title.is_some(),
            id: info.id,
            title: info.title,
            session,
            stream: None,
            fixture_shell: false,
            scroll_px_accum: 0.0,
            drag: mount::TextDrag::Idle,
            mount: mount::Mount::Live { connection: self.terminal.connections },
            fit: Cell::new(mount::Fit::Unmeasured),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_stream_terminal(
        &mut self,
        ws_key: &str,
        id: &str,
        title: &str,
        title_number: usize,
        config: TerminalConfig,
        restore: Option<RestoreState>,
        initial_bytes: &[u8],
    ) -> Option<String> {
        let (_, directory) = self.terminal_session_dir(ws_key);
        let connection = self.terminal.connections;
        self.terminal.touch_lru(ws_key);
        let (session, handle) =
            TerminalSession::spawn_stream(config, restore, self.terminal_waker()).ok()?;
        if !initial_bytes.is_empty() {
            handle.feed(initial_bytes);
        }
        let ws = self
            .terminal
            .workspaces
            .entry(ws_key.to_owned())
            .or_insert_with(|| TerminalWorkspace { directory, ..TerminalWorkspace::default() });
        ws.tabs.push(TerminalTab {
            id: id.to_owned(),
            title: title.to_owned(),
            title_number,
            custom_title: false,
            session,
            stream: Some(handle),
            fixture_shell: false,
            scroll_px_accum: 0.0,
            drag: mount::TextDrag::Idle,
            mount: mount::Mount::Live { connection },
            fit: Cell::new(mount::Fit::Unmeasured),
        });
        ws.active = Some(id.to_owned());
        Some(id.to_owned())
    }

    /// Steps each session terminal's [`mount::Mount`]: a failed one leaves once it doesn't show,
    /// and showing it again, or the server reconnecting, attaches it anew. Quiet (and free of
    /// allocations) while every terminal is live.
    fn sync_terminal_mounts(&mut self) -> bool {
        let Some((srv, observer)) = self.live_terminals() else { return false };
        let failed = |t: &TerminalTab| matches!(t.session.status(), SessionStatus::Failed(_));
        let quiet = self.terminal.workspaces.values().flat_map(|ws| &ws.tabs).all(|t| matches!(t.mount, mount::Mount::Live { .. }) && !failed(t));
        if quiet {
            return false;
        }
        let routed = self.active_terminal_workspace_key();
        let connection = self.terminal.connections;
        let mut attach = Vec::new();
        let mut changed = false;
        let ids: Vec<(String, String)> = self.terminal.workspaces.iter().flat_map(|(wk, ws)| ws.tabs.iter().map(|t| (wk.clone(), t.id.clone()))).collect();
        for (wk, id) in ids {
            let shown = routed.as_deref() == Some(wk.as_str()) && self.side_tab_visible(&crate::panels::panel_key(side_ops::PLUGIN, &id));
            let Some(tab) = self.terminal_tab_mut(&wk, &id) else { continue };
            let event = if matches!(tab.mount, mount::Mount::Live { .. }) && failed(tab) {
                mount::MountEvent::Failed { shown, connection }
            } else {
                mount::MountEvent::Sync { shown, connection }
            };
            let before = tab.mount;
            for effect in tab.mount.step(event) {
                match effect {
                    mount::MountEffect::Mount => attach.push((wk.clone(), id.clone())),
                }
            }
            changed |= tab.mount != before;
        }
        for (wk, id) in attach {
            self.attach_terminal(srv.clone(), &wk, &id, observer);
        }
        changed
    }

    /// A new attachment for terminal `id`, at the size its screen had.
    fn attach_terminal(&mut self, srv: opencode_kit::client::Server, wk: &str, id: &str, observer: bool) {
        let theme = theme_from_pal(pal(self.terminal_dark()));
        let waker = self.terminal_waker();
        let Some(tab) = self.terminal_tab_mut(wk, id) else { return };
        let snap = tab.session.snapshot();
        let config = TerminalConfig { cols: snap.cols.max(1), rows: snap.rows.max(1), theme, ..TerminalConfig::default() };
        if let Ok(session) = TerminalSession::spawn_persistent_pty(srv, id.to_owned(), config, observer, waker) {
            tab.session = session;
            tab.fit.set(mount::Fit::Unmeasured);
            tab.drag = mount::TextDrag::Idle;
        }
    }

    /// Drains worker updates, visual bell flashes, exited/recovered PTYs, and phase-locked cursor blink edges.
    /// Zero heap allocations and `false` return on idle frames so `idle_poll` stays silent.
    pub fn poll_terminal(&mut self, tx: &mut Tx) -> bool {
        if self.stage.is_some()
            && self.terminal.story_built_variant.is_some_and(|v| v != *self.variant)
        {
            story::init_story(self);
            return true;
        }

        // A session whose strip holds terminal tabs loads its terminals, so the tabs can show.
        let stored_tabs = self.stage.is_none()
            && self
                .active_session_id()
                .and_then(|id| self.side.sessions.get(id))
                .is_some_and(|st| st.tab_state.tabs.all.iter().any(|k| k.starts_with("terminal:")));
        if stored_tabs
            && let Some(wk) = self.active_terminal_workspace_key()
            && !self.terminal.workspaces.contains_key(&wk)
        {
            self.ensure_terminal_workspace_hydrated(&wk);
            return true;
        }

        if !self.terminal.workspaces.is_empty() {
            let dark = self.terminal_dark();
            let painted = Some((dark, crate::themes::active(), crate::themes::generation()));
            if self.terminal.last_dark != painted {
                self.terminal.last_dark = painted;
                let theme = theme_from_pal(pal(dark));
                for ws in self.terminal.workspaces.values() {
                    for tab in &ws.tabs {
                        tab.session.set_theme(theme.clone());
                    }
                }
            }
        }

        let mut changed = self.terminal.dirty_flag.swap(false, Ordering::AcqRel);

        if changed {
            let now = *self.clock;
            let mut bell = false;
            let mut title_changed = false;
            let mut to_remove: Vec<String> = Vec::new();

            for ws in self.terminal.workspaces.values_mut() {
                for tab in &mut ws.tabs {
                    for ev in tab.session.drain_events() {
                        match ev {
                            VtEvent::Bell => bell = true,
                            VtEvent::TitleChanged(new_title)
                                if !tab.custom_title && !new_title.is_empty() =>
                            {
                                if tab.title != new_title {
                                    tab.title = new_title;
                                    title_changed = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    if matches!(tab.session.status(), SessionStatus::Exited { .. }) && self.stage.is_none() {
                        to_remove.push(tab.id.clone());
                    }
                }
            }

            for tid in to_remove {
                let _ = self.remove_exited_pty(&tid);
            }
            if title_changed {
                self.queue_terminal_save();
            }
            if bell {
                self.terminal.bell_until = now + 0.14;
                tx.wake_after(Duration::from_millis(145));
            }
        }

        changed |= self.sync_terminal_mounts();

        if self.terminal.bell_until > 0.0 && *self.clock >= self.terminal.bell_until {
            self.terminal.bell_until = 0.0;
            changed = true;
        }

        let visible_and_focused = if self.terminal.workspaces.is_empty() {
            false
        } else if self.stage.is_some() {
            self.terminal.story_focused
        } else {
            self.terminal_focus().is_some()
        };

        if visible_and_focused {
            let blinks = self
                .active_terminal_workspace_key()
                .and_then(|k| {
                    let ws = self.terminal.workspaces.get(&k)?;
                    let tab = ws
                        .active
                        .as_deref()
                        .and_then(|id| ws.tabs.iter().find(|t| t.id == id))
                        .or_else(|| ws.tabs.first())?;
                    let c = tab.session.snapshot().cursor;
                    Some(c.visible && c.in_viewport && c.blinking)
                })
                .unwrap_or(false);

            if blinks {
                let lit = self.terminal.cursor_lit(*self.clock);
                if lit != self.terminal.last_blink_lit {
                    self.terminal.last_blink_lit = lit;
                    changed = true;
                }
                let dt = (*self.clock - self.terminal.blink_epoch).max(0.0);
                let rem = (0.5 - (dt % 0.5)).clamp(0.01, 0.5);
                tx.wake_after(Duration::from_secs_f64(rem));
            }
        }

        changed
    }
}
