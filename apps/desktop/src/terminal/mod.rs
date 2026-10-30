//! Native Ghostty terminal pane state, workspace tabs, theme sync, and knobs (`apps/desktop/src/terminal/`).

#[cfg(test)]
mod bench;
mod draw;
pub mod grid;
pub mod input;
pub mod story;
pub mod streams;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use opencode_kit::pty::PtyCreate;
use rune::{Tx, Waker};
use rune_knobs::{Group, Knob};
use terminal::{
    RestoreState, StreamHandle, TerminalConfig, TerminalSession, TerminalTheme, VtEvent,
};

use crate::palette::{Pal, pal};
use crate::{App, Route, Source};

pub const MAX_TERMINAL_WORKSPACES: usize = 20;

pub const TERM_FONT: Knob = Knob::choice(
    "Terminal font",
    &["Commit Mono", "IBM Plex Mono", "SF Mono"],
    0,
)
.about("Monospace font family in the terminal")
.hints(&[
    "Kit's Ghostty Commit Mono face",
    "OpenCode bundled IBM Plex Mono",
    "macOS SF Mono system face",
]);

pub const TERM_SIZE: Knob = Knob::new("Font size", 11.0, 18.0, 13.0)
    .step(0.5)
    .about("Terminal font size in points");

pub const TERMINAL_KNOBS: &[Knob] = &[TERM_FONT, TERM_SIZE];

rune_register::register!(crate::knobs::GROUP_LIST: Group = [
    Group::new("Terminal", "terminal", TERMINAL_KNOBS),
]);

pub struct TerminalTab {
    pub id: String,
    pub title: String,
    pub title_number: usize,
    pub custom_title: bool,
    pub session: TerminalSession,
    pub stream: Option<StreamHandle>,
    pub scroll_px_accum: f32,
    pub drag_anchor: Option<(u16, u16)>,
    last_fit: Cell<(u16, u16, u32, u32)>,
}

impl TerminalTab {
    pub fn display_title(&self) -> String {
        if !self.title.trim().is_empty() {
            self.title.clone()
        } else {
            format!("Terminal {}", self.title_number.max(1))
        }
    }

    pub fn ensure_fit(&self, cols: u16, rows: u16, cw_px: u32, ch_px: u32) {
        let next = (cols, rows, cw_px, ch_px);
        if self.last_fit.get() != next {
            self.last_fit.set(next);
            self.session.resize(cols, rows, cw_px, ch_px);
        }
    }
}

#[derive(Default)]
pub struct TerminalWorkspace {
    pub directory: String,
    pub active: Option<String>,
    pub tabs: Vec<TerminalTab>,
}

impl TerminalWorkspace {
    pub fn next_title_number(&self) -> usize {
        let used: BTreeSet<usize> = self
            .tabs
            .iter()
            .filter_map(|t| (t.title_number > 0).then_some(t.title_number))
            .collect();
        (1..=used.len() + 1).find(|n| !used.contains(n)).unwrap_or(1)
    }
}

pub struct TerminalPane {
    pub workspaces: HashMap<String, TerminalWorkspace>,
    pub lru_order: Vec<String>,
    pub blink_epoch: f64,
    pub bell_until: f64,
    pub last_blink_lit: bool,
    pub last_dark: Option<bool>,
    pub dirty_flag: Arc<AtomicBool>,
    pub story_focused: bool,
    pub story_built_variant: Option<usize>,
}

impl Default for TerminalPane {
    fn default() -> Self {
        Self {
            workspaces: HashMap::new(),
            lru_order: Vec::new(),
            blink_epoch: 0.0,
            bell_until: 0.0,
            last_blink_lit: true,
            last_dark: None,
            dirty_flag: Arc::new(AtomicBool::new(false)),
            story_focused: true,
            story_built_variant: None,
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

    fn touch_lru(&mut self, key: &str) {
        self.lru_order.retain(|k| k != key);
        self.lru_order.push(key.to_owned());
        while self.lru_order.len() > MAX_TERMINAL_WORKSPACES {
            let oldest = self.lru_order.remove(0);
            self.workspaces.remove(&oldest);
        }
    }
}

pub fn theme_from_pal(p: &Pal) -> TerminalTheme {
    let base = if p.dark {
        TerminalTheme::opencode_dark()
    } else {
        TerminalTheme::opencode_light()
    };
    let fg = terminal::Rgb::from_rune(p.text);
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

pub fn workspace_key(server_base: &str, directory: &str) -> String {
    format!("{server_base}\0{directory}")
}

impl App {
    pub fn terminal_waker(&self) -> Waker {
        let dirty = self.terminal.dirty_flag.clone();
        let app_waker = self.waker.clone();
        Arc::new(move || {
            dirty.store(true, Ordering::Release);
            if let Some(w) = &app_waker {
                w();
            }
        })
    }

    pub fn active_terminal_workspace_key(&self) -> Option<String> {
        if self.stage.is_some() {
            return Some("story\0/workspace/seer".into());
        }
        let Route::Session(sid) = &self.route else {
            return None;
        };
        let dir = self
            .session(sid)
            .map(|s| s.directory.as_str())
            .unwrap_or(&self.draft_dir);
        let server_id = match &self.source {
            Source::Live { server, .. } => server.base.as_str(),
            Source::Fixture => "fixture",
        };
        Some(workspace_key(server_id, dir))
    }

    pub fn focus_terminal_surface(&mut self) {
        self.composer.focused = false;
        self.side.focused = false;
        self.side.dock_focused = true;
        self.terminal.story_focused = true;
        self.terminal.reset_blink(self.clock);
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
        if let Some(ws) = self.terminal.workspaces.get_mut(ws_key)
            && ws.tabs.iter().any(|t| t.id == tab_id)
        {
            ws.active = Some(tab_id.to_owned());
        }
        self.focus_terminal_surface();
    }

    pub fn close_terminal_tab(&mut self, ws_key: &str, tab_id: &str, tx: &mut Tx) {
        tx.instant();
        let mut became_empty = false;
        let mut directory = String::new();
        if let Some(ws) = self.terminal.workspaces.get_mut(ws_key) {
            directory.clone_from(&ws.directory);
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
            became_empty = ws.tabs.is_empty();
        }

        if let Source::Live {
            server,
            allow_send: true,
            ..
        } = &self.source
            && tab_id.starts_with("pty_")
        {
            let srv = server.clone();
            let id = tab_id.to_owned();
            std::thread::spawn(move || {
                let _ = srv.pty_remove(&directory, &id);
            });
        }

        if became_empty && self.stage.is_none() && self.dock_open() {
            self.side.emptied_guard = true;
            self.close_dock(tx);
        }
    }

    pub fn new_terminal_in(&mut self, ws_key: &str) -> Option<String> {
        let dark = self.settings.scheme != crate::settings::Scheme::Light;
        let theme = theme_from_pal(pal(dark));
        let directory = ws_key
            .split_once('\0')
            .map_or_else(String::new, |(_, d)| d.to_owned());
        self.terminal.touch_lru(ws_key);
        let num = self
            .terminal
            .workspaces
            .entry(ws_key.to_owned())
            .or_insert_with(|| TerminalWorkspace {
                directory: directory.clone(),
                active: None,
                tabs: Vec::new(),
            })
            .next_title_number();
        let title = format!("Terminal {num}");
        let waker = self.terminal_waker();

        let use_live_pty = matches!(
            &self.source,
            Source::Live {
                allow_send: true,
                ..
            }
        ) && !crate::headless()
            && !cfg!(test)
            && self.stage.is_none();

        if use_live_pty
            && let Source::Live { server, .. } = &self.source
        {
            let srv = server.clone();
            let dir_for_create = directory.clone();
            let create_title = title.clone();
            if let Ok(info) = srv.pty_create(
                &dir_for_create,
                &PtyCreate {
                    title: Some(create_title),
                    cwd: (!dir_for_create.is_empty()).then_some(dir_for_create.clone()),
                    ..PtyCreate::default()
                },
            ) && let Ok(session) = TerminalSession::spawn_server_pty(
                srv,
                directory,
                info.id.clone(),
                TerminalConfig {
                    theme: theme.clone(),
                    ..TerminalConfig::default()
                },
                None,
                waker,
            ) {
                let id = info.id;
                let ws = self.terminal.workspaces.get_mut(ws_key)?;
                ws.tabs.push(TerminalTab {
                    id: id.clone(),
                    title: if info.title.is_empty() { title } else { info.title },
                    title_number: num,
                    custom_title: false,
                    session,
                    stream: None,
                    scroll_px_accum: 0.0,
                    drag_anchor: None,
                    last_fit: Cell::new((80, 24, 8, 18)),
                });
                ws.active = Some(id.clone());
                self.focus_terminal_surface();
                return Some(id);
            }
        }

        self.insert_stream_terminal(
            ws_key,
            &format!("term_{num}"),
            &title,
            num,
            TerminalConfig {
                theme,
                ..TerminalConfig::default()
            },
            None,
            b"\x1b[1;34m~/workspace\x1b[0m \x1b[1;32m\xe2\x9d\xaf\x1b[0m ",
        )
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
        let directory = ws_key
            .split_once('\0')
            .map_or_else(String::new, |(_, d)| d.to_owned());
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
            .or_insert_with(|| TerminalWorkspace {
                directory,
                active: None,
                tabs: Vec::new(),
            });
        ws.tabs.push(TerminalTab {
            id: id.to_owned(),
            title: title.to_owned(),
            title_number,
            custom_title: false,
            session,
            stream: Some(handle),
            scroll_px_accum: 0.0,
            drag_anchor: None,
            last_fit: Cell::new((0, 0, 0, 0)),
        });
        ws.active = Some(id.to_owned());
        Some(id.to_owned())
    }

    /// Drains worker updates, visual bell flashes, and phase-locked cursor blink edges.
    /// Zero heap allocations and `false` return on idle frames so `idle_poll` stays silent.
    pub fn poll_terminal(&mut self, tx: &mut Tx) -> bool {
        if self.stage.is_some()
            && self.terminal.story_built_variant.is_some_and(|v| v != self.variant)
        {
            story::init_story(self);
            return true;
        }
        let mut changed = self.terminal.dirty_flag.swap(false, Ordering::AcqRel);

        if changed {
            let now = self.clock;
            let mut bell = false;
            for ws in self.terminal.workspaces.values_mut() {
                for tab in &mut ws.tabs {
                    for ev in tab.session.drain_events() {
                        match ev {
                            VtEvent::Bell => bell = true,
                            VtEvent::TitleChanged(new_title) if !tab.custom_title && !new_title.is_empty() => {
                                tab.title = new_title;
                            }
                            _ => {}
                        }
                    }
                }
            }
            if bell {
                self.terminal.bell_until = now + 0.14;
                tx.wake_after(Duration::from_millis(145));
            }
        }

        if self.terminal.bell_until > 0.0 && self.clock >= self.terminal.bell_until {
            self.terminal.bell_until = 0.0;
            changed = true;
        }

        let visible_and_focused = if self.terminal.workspaces.is_empty() {
            false
        } else if self.stage.is_some() {
            self.terminal.story_focused
        } else {
            self.side.dock_focused
                && self
                    .active_session_id()
                    .and_then(|id| self.side.sessions.get(id))
                    .is_some_and(|st| st.dock_open)
        };

        if visible_and_focused {
            let blinks = self
                .active_terminal_workspace_key()
                .and_then(|k| {
                    let ws = self.terminal.workspaces.get(&k)?;
                    let tab = ws
                        .active
                        .as_deref()
                        .and_then(|id| ws.tabs.iter().find(|t| t.id == id))?;
                    let c = tab.session.snapshot().cursor;
                    Some(c.visible && c.in_viewport && c.blinking)
                })
                .unwrap_or(false);

            if blinks {
                let lit = self.terminal.cursor_lit(self.clock);
                if lit != self.terminal.last_blink_lit {
                    self.terminal.last_blink_lit = lit;
                    changed = true;
                }
                let dt = (self.clock - self.terminal.blink_epoch).max(0.0);
                let rem = (0.5 - (dt % 0.5)).clamp(0.01, 0.5);
                tx.wake_after(Duration::from_secs_f64(rem));
            }
        }

        changed
    }
}
