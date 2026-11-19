//! Desktop host adapter for `opencode-feature-terminal-surface` (`features/terminal-surface`).

pub use opencode_feature_terminal_surface::*;

pub mod story;

use std::path::PathBuf;

use opencode_kernel::{Feature, KernelBuilder, OverlayRegistration, Slot};
use opencode_kit::client::Server;
use opencode_kit::persistent_pty::PersistentPtyInfo;
use rune::{Drag, KeyEvent, PointerPress, Tx, Waker};
use rune_knobs::Knobs;
use serde_json::Value;
use terminal::{RestoreState, TerminalConfig, TerminalTheme};

use crate::jobs::Apply;
use crate::panels::{DockPlacement, SessionPane};
use crate::{App, Source};

rune_register::register!(crate::knobs::GROUP_LIST: rune_knobs::Group = [TERMINAL_GROUP]);

fn to_session_pane(pane: PaneTarget) -> SessionPane {
    match pane {
        PaneTarget::Side => SessionPane::Side,
        PaneTarget::Split => SessionPane::Split,
        PaneTarget::Bottom => SessionPane::Bottom,
    }
}

fn from_session_pane(pane: SessionPane) -> PaneTarget {
    match pane {
        SessionPane::Side => PaneTarget::Side,
        SessionPane::Split => PaneTarget::Split,
        SessionPane::Bottom => PaneTarget::Bottom,
    }
}

fn write_terminal_storage_batch(
    namespace: &'static str,
    prefix: String,
    rows: Vec<(String, Option<String>)>,
) {
    crate::storage::with_storage(move |s| {
        // Evicted workspaces (past the 20 kept) leave with the save that dropped them.
        for (key, _) in s.state().items(namespace).unwrap_or_default() {
            if key.starts_with(&prefix) && !rows.iter().any(|(k, _)| *k == key) {
                s.state().delete(namespace, &key);
            }
        }
        for (key, value) in rows {
            match value {
                Some(v) => s.state().set(namespace, &key, v),
                None => s.state().delete(namespace, &key),
            };
        }
    });
}

pub(crate) static TERMINAL: Slot<TerminalPane> = Slot::new();

impl TerminalSurfaceHost for App {
    fn terminal(&self) -> &TerminalPane {
        self.kernel.slots.get(&TERMINAL)
    }

    fn terminal_mut(&mut self) -> &mut TerminalPane {
        self.kernel.slots.get_mut(&TERMINAL)
    }

    fn session_directory(&self, sid: &str) -> Option<&str> {
        self.session(sid).map(|s| s.directory.as_str())
    }

    fn draft_directory(&self) -> &str {
        self.draft_dir()
    }

    fn is_story_stage(&self) -> bool {
        self.fixture_run.stage.is_some()
    }

    fn poll_story_variant(&mut self) -> bool {
        if self.fixture_run.stage.is_some()
            && self.kernel.slots.get(&TERMINAL).story.needs_rebuild(*self.fixture_run.variant)
        {
            story::init_story(self);
            return true;
        }
        false
    }

    fn live_server(&self) -> Option<&Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server),
            Source::Fixture => None,
        }
    }

    fn allows_terminal_input(&self) -> bool {
        match &*self.source {
            Source::Live { allow_send, .. } => *allow_send,
            Source::Fixture => true,
        }
    }

    fn is_window_blurred(&self) -> bool {
        self.plugins.window_blurred.get()
    }

    fn is_composer_focused(&self) -> bool {
        self.composer.focused
    }

    fn narrow_width(&self) -> f32 {
        self.layout.narrow.width()
    }

    fn knobs(&self) -> &Knobs {
        &self.diag.knobs
    }

    fn terminal_font(&self) -> (usize, &str) {
        (self.settings.terminal_font, &self.settings.terminal_font_name)
    }

    fn terminal_dark_default(&self) -> bool {
        self.settings.scheme == crate::settings::Scheme::Dark
    }

    fn terminal_theme(&self, dark: bool) -> TerminalTheme {
        theme_from_pal(crate::palette::pal(dark))
    }

    fn terminal_option_meta(&self) -> bool {
        self.settings.terminal_option_meta
    }

    fn terminal_marks(&self) -> &[String] {
        &self.settings.terminal_marks
    }

    fn shell_integration_dir(&self) -> Option<PathBuf> {
        crate::dirs::data().map(|d| d.join("shell-integration"))
    }

    fn fetch_terminal_fonts_if_wasm(&self) {
        #[cfg(target_arch = "wasm32")]
        for url in crate::TERMINAL_FONTS {
            rune::web::font_url_now(url);
        }
    }

    fn read_json(&self, ns: &str, key: &str) -> Option<Value> {
        crate::storage::json(ns, key)
    }

    fn write_json(&self, ns: &str, key: &str, value: &Value) {
        crate::storage::set_json(ns, key, value);
    }

    fn load_storage_namespace(&self, ns: &str) {
        let ns = ns.to_owned();
        crate::storage::with_storage(move |s| s.state().load(&[ns.as_str()], None));
    }

    fn storage_items(&self, ns: &str) -> Option<Vec<(String, String)>> {
        crate::storage::items(ns)
            .map(|rows| rows.into_iter().map(|(k, v)| (k, (*v).to_owned())).collect())
    }

    fn terminal_storage_writer(&self) -> Option<persist::StorageBatchWriter> {
        Some(write_terminal_storage_batch)
    }

    fn open_link(&mut self, sid: &str, href: &str, tx: &mut Tx) {
        App::open_link(self, sid, None, href, tx);
    }

    fn open_context_menu(
        &mut self,
        spec: ContextMenuSpec,
        press: PointerPress,
        tx: &mut Tx,
        on_pick: ContextMenuPick,
    ) {
        let mut menu = crate::popup::Menu::new(spec.label);
        for entry in spec.entries {
            menu = match entry {
                ContextMenuEntry::Item { id, label, enabled } => {
                    menu.item(crate::popup::Item::new(id, label).enabled(enabled))
                }
                ContextMenuEntry::Separator => menu.separator(),
            };
        }
        crate::popup::open(self, menu, press, tx, move |app: &mut App, picked: Option<&str>, tx: &mut Tx| {
            on_pick(app, picked, tx);
        });
    }

    fn read_clipboard_for_paste(&mut self, ws_key: String, tab_id: String, tx: &mut Tx) {
        input::read_clipboard_into_terminal::<App>(ws_key, tab_id, tx);
    }

    fn can_quote_terminal_selection(&self, selected: &str) -> bool {
        crate::quote::can_quote(self, selected)
    }

    fn quote_terminal_selection(&mut self, selected: &str, tx: &mut Tx) {
        crate::quote::quote_selected(self, selected, tx);
    }

    fn speech_terminal_key(&mut self, ws_key: &str, tab_id: &str) {
        crate::speech::terminal_key(self, ws_key, tab_id);
    }

    fn plugin_takes_chord(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        crate::plugin_host::commands::takes_chord(self, ev, tx)
    }

    fn is_side_focused(&self) -> bool {
        self.kernel.slots.get(&crate::panels::SIDE).focus.is_focused()
    }

    fn focused_pane_shown_tab(&self) -> Option<String> {
        let pane = self.kernel.slots.get(&crate::panels::SIDE).active_pane_focus;
        let visible = if pane == SessionPane::Bottom { self.bottom_open() } else { self.side_open() };
        visible.then(|| self.pane_shown_tab(pane)).flatten()
    }

    fn side_tab_visible(&self, key: &str) -> bool {
        App::side_tab_visible(self, key)
    }

    fn session_has_terminal_tabs(&self, sid: &str) -> bool {
        self.kernel
            .slots
            .get(&crate::panels::SIDE)
            .sessions
            .get(sid)
            .is_some_and(|st| st.tab_state.tabs.all.iter().any(|k| k.starts_with("terminal:")))
    }

    fn terminal_pane_estimate(&self) -> (f32, f32) {
        let w = self.side_width(self.kernel.slots.get(&crate::panels::SIDE).laid_w.get());
        let h = if self.dock_placement() == DockPlacement::Bottom {
            self.bottom_height()
        } else {
            744.0 / *self.zoom
        };
        (w, h)
    }

    fn focus_side_pane(&mut self, tab: Option<(&str, &str)>) {
        self.composer.focused = false;
        self.kernel.slots.get_mut(&crate::panels::SIDE).focus.focus();
        if let Some((sid, key)) = tab {
            let pane = crate::panels::tabs::session_tab_pane(&self.session_side_mut(sid).tab_state.tabs, key);
            self.kernel.slots.get_mut(&crate::panels::SIDE).active_pane_focus = pane;
        }
    }

    fn keeping_keyboard(&mut self, f: &mut dyn FnMut(&mut dyn TerminalSurfaceHost)) {
        App::keeping_keyboard(self, |s| f(s));
    }

    fn show_side_tab(&mut self, sid: &str, key: &str, target: Option<PaneTarget>) {
        App::show_side_tab(self, sid, key, target.map(to_session_pane));
    }

    fn show_pane_tab(&mut self, sid: &str, key: &str, target: Option<PaneTarget>, tx: &mut Tx) {
        let pane = target.map(to_session_pane);
        App::show_pane_tab(self, tx, |s| s.show_side_tab(sid, key, pane));
    }

    fn hide_side_tab(&mut self, key: &str, tx: &mut Tx) {
        App::hide_side_tab(self, key, tx);
    }

    fn close_side_tab(&mut self, key: &str, tx: &mut Tx) {
        App::close_side_tab(self, key, tx);
    }

    fn side_tab_gesture(&mut self, key: &str, title: &str, draggable: bool, shown: bool, drag: &Drag, tx: &mut Tx) {
        App::side_tab_gesture(self, key, title, draggable, shown, drag, tx);
    }

    fn spawn_job(&mut self, job: TerminalJob) {
        self.job(move |srv| {
            let reply = execute_job(srv, job);
            Box::new(move |s: &mut App, tx: &mut Tx| apply_reply(s, reply, tx)) as Apply
        });
    }
}

impl App {
    pub(crate) fn select_session_terminal(&mut self, sid: &str, id: &str, focus: bool) {
        select_session_terminal(self, sid, id, focus);
    }

    pub(crate) fn live_terminals(&self) -> Option<(Server, bool)> {
        live_terminals(self)
    }

    pub(crate) fn terminal_dark(&self) -> bool {
        terminal_dark(self)
    }

    pub fn terminal_waker(&self) -> Waker {
        terminal_waker(self)
    }

    pub fn terminal_workspace_key_for_session(&self, session_id: &str) -> String {
        terminal_workspace_key_for_session(self, session_id)
    }

    pub fn active_terminal_workspace_key(&self) -> Option<String> {
        active_terminal_workspace_key(self)
    }

    pub fn ensure_workspace_ready(&mut self, ws_key: &str) {
        ensure_workspace_ready(self, ws_key);
    }

    pub fn open_terminal_for_session(&mut self, sid: &str, new_tab: bool) -> Option<String> {
        open_terminal_for_session(self, sid, new_tab)
    }

    pub fn focus_terminal_surface(&mut self) {
        focus_terminal_surface(self);
    }

    pub fn terminal_tab_mut(&mut self, ws_key: &str, tab_id: &str) -> Option<&mut TerminalTab> {
        terminal_tab_mut(self, ws_key, tab_id)
    }

    pub fn select_terminal_tab(&mut self, ws_key: &str, tab_id: &str) {
        select_terminal_tab(self, ws_key, tab_id);
    }

    pub fn select_adjacent_terminal_tab(&mut self, ws_key: &str, offset: i32, tx: &mut Tx) -> bool {
        select_adjacent_terminal_tab(self, ws_key, offset, tx)
    }

    pub fn close_terminal_tab(&mut self, ws_key: &str, tab_id: &str, tx: &mut Tx) {
        close_terminal_tab(self, ws_key, tab_id, tx);
    }

    pub fn new_terminal_in(&mut self, ws_key: &str) -> Option<String> {
        new_terminal_in(self, ws_key)
    }

    pub(crate) fn start_terminal(
        &self,
        sid: &str,
        title: Option<String>,
        cwd: Option<String>,
        command: Option<String>,
        args: Vec<String>,
    ) -> Result<PersistentPtyInfo, String> {
        start_terminal(self, sid, title, cwd, command, args)
    }

    pub(crate) fn adopt_terminal(&mut self, sid: &str, info: PersistentPtyInfo) {
        adopt_terminal(self, sid, info);
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
        insert_stream_terminal(self, ws_key, id, title, title_number, config, restore, initial_bytes)
    }

    pub fn poll_terminal(&mut self, tx: &mut Tx) -> bool {
        poll_terminal(self, tx)
    }

    pub(crate) fn terminal_focus(&self) -> Option<(String, String)> {
        terminal_focus(self)
    }

    pub(crate) fn create_terminal_tab(&mut self, pane: Option<SessionPane>, tx: &mut Tx) {
        create_terminal_tab(self, pane.map(from_session_pane), tx);
    }

    pub(crate) fn toggle_terminal(&mut self, tx: &mut Tx) {
        toggle_terminal(self, tx);
    }

    pub fn apply_pty_event(&mut self, ty: &str, data: &Value) -> bool {
        apply_pty_event(self, ty, data)
    }

    pub fn load_persisted_terminals(&mut self, server_base: &str) {
        load_persisted_terminals(self, server_base);
    }

    pub fn queue_terminal_save(&mut self) {
        queue_terminal_save(self);
    }

    pub fn ensure_terminal_workspace_hydrated(&mut self, ws_key: &str) -> bool {
        ensure_terminal_workspace_hydrated(self, ws_key)
    }

    pub(crate) fn paste_into_terminal(&mut self, ws: &str, tab: &str, text: String, tx: &mut Tx) {
        paste_into_terminal(self, ws, tab, text, tx);
    }
}

/// Kernel feature registration for the Ghostty-VT terminal surface.
pub struct TerminalSurfaceFeature;

impl Feature<App> for TerminalSurfaceFeature {
    fn id(&self) -> &'static str {
        "feature.terminal-surface"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&TERMINAL);
        builder.register_overlay(
            OverlayRegistration::<App>::new("dialog.terminal.paste")
                .with_z(220)
                .with_render(|s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    paste::dialog(s, p, cx)
                })
                .with_dismiss(|s, tx| paste::answer_paste(s, false, tx)),
        );
    }
}
