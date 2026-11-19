//! Desktop host adapter for `opencode-feature-narrow` (`features/narrow`).

pub use opencode_feature_narrow::*;

use opencode_kernel::{Feature, KernelBuilder, Phase};
use opencode_plugin_spec::PanelMobileKind;
use rune::{El, PointerPress, Tx};
use session_view::palette::Pal;

use crate::{App, Route};

fn make_narrow_tab(s: &App, t: &crate::tabs::Tab) -> NarrowTab {
    let title = crate::view::tab_strip::face(s, t).title;
    let mark = if t.is_draft() { s.tab_status(&t.id) } else { s.tab_status_of(t) };
    NarrowTab { uid: t.uid(), title, is_draft: t.is_draft(), status: mark }
}

impl NarrowHost for App {
    fn narrow_state(&self) -> &Narrow {
        &self.layout.narrow
    }

    fn narrow_state_mut(&mut self) -> &mut Narrow {
        &mut self.layout.narrow
    }

    fn stage_active(&self) -> bool {
        self.fixture_run.stage.is_some()
    }

    fn layout_has_sidebar(&self) -> bool {
        self.tab_layout().has_sidebar()
    }

    fn settings_open(&self) -> bool {
        self.settings.open
    }

    fn is_home_route(&self) -> bool {
        self.route == Route::Home && !self.settings.open
    }

    fn chrome(&self) -> NarrowChrome {
        let c = crate::view::chrome();
        NarrowChrome { hairline: c.hairline, bottom: c.bottom }
    }

    fn current_tab_uid(&self) -> Option<String> {
        self.current_tab()
    }

    fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    fn narrow_tab(&self, uid: &str) -> Option<NarrowTab> {
        self.tab(uid).map(|t| make_narrow_tab(self, t))
    }

    fn narrow_tabs(&self) -> Vec<NarrowTab> {
        self.tabs.iter().map(|t| make_narrow_tab(self, t)).collect()
    }

    fn tab_avatar(&self, p: &Pal, uid: &str) -> El {
        let Some(t) = self.tab(uid) else { return rune::el() };
        let face = crate::view::tab_strip::face(self, t);
        crate::icons::badge(self, p, face.project, &crate::view::project_or_dir(face.project, face.dir))
    }

    fn narrow_views(&self) -> Vec<NarrowView> {
        crate::plugin_host::panels::narrow_views(self)
            .into_iter()
            .map(|(id, v)| NarrowView {
                id,
                title: v.title,
                order: v.order,
                kind: match v.kind {
                    PanelMobileKind::Tab => Kind::Tab,
                    PanelMobileKind::Drawer => Kind::Drawer,
                    _ => Kind::Menu,
                },
                icon: crate::plugin_host::icons::resolve("", v.icon.as_deref().unwrap_or("window-analytics")),
            })
            .collect()
    }

    fn location_trigger(&self, p: &Pal, session_id: &str) -> Option<El> {
        let info = self.session(session_id)?;
        let project = self.project(&info.project);
        let name = crate::view::project_or_dir(project, &info.directory);
        let worktree = info
            .parent
            .is_none()
            .then(|| crate::view::location::worktree_name(project, &info.directory))
            .flatten();
        let collapse = if info.parent.is_some() { 2 } else { 0 };
        Some(crate::view::location::trigger(self, p, project, &name, worktree.as_deref(), collapse))
    }

    fn render_narrow_panel(&self, p: &Pal, session_id: &str, view: &str, view_title: Option<&str>, width: f32) -> El {
        let Some(reg) = self.resolve_side_region() else { return rune::el() };
        let fallback;
        let plugin_tab = crate::panels::split_panel_key(view).and_then(|(plugin, tab)| {
            let title = view_title?;
            let provider = self.find_panel(plugin)?;
            Some((provider, crate::panels::PanelTabDesc::new(tab, title)))
        });
        let (provider, tab) = match plugin_tab {
            Some((provider, tab)) => {
                fallback = tab;
                (provider, &fallback)
            }
            None => {
                let stored = self.workspace_panes.side.sessions.get(session_id).map(|st| &st.tab_state.tabs);
                let active = stored.and_then(|t| t.active.clone()).filter(|k| k.starts_with(&format!("{view}:")));
                match active
                    .as_deref()
                    .and_then(|k| reg.entry(k))
                    .or_else(|| reg.entries.iter().find(|e| e.plugin == view && !e.tab.hidden))
                {
                    Some(e) => (e.provider, &e.tab),
                    None => {
                        let Some(provider) = self.find_panel(view) else { return rune::el() };
                        fallback = crate::panels::PanelTabDesc::new("", "");
                        (provider, &fallback)
                    }
                }
            }
        };
        let ctx = crate::panels::PanelViewCtx {
            plugin: provider.plugin,
            session_id: &reg.session_id,
            directory: &reg.directory,
            project_id: &reg.project_id,
            pane: crate::panels::SessionPane::Side,
            pane_size: (width, 600.0),
            env_scale: 1.0,
            tab,
            visible: true,
            focused: self.workspace_panes.side.focus.is_focused(),
            mobile: true,
        };
        provider.render(self, p, &ctx)
    }

    fn on_enter_narrow_view(&mut self, session_id: &str, view: &str, tx: &mut Tx) {
        if view == "terminal:main" {
            let wk = self.terminal_workspace_key_for_session(session_id);
            self.ensure_terminal_workspace_hydrated(&wk);
            let empty = self.workspace_panes.terminal.workspaces.get(&wk).is_none_or(|w| w.tabs.is_empty() && w.failure.is_none());
            if empty && self.live_terminals().is_some_and(|(_, observer)| !observer) {
                self.create_terminal_tab(None, tx);
            }
        }
    }

    fn sync_narrow_review_wrap(&mut self) {
        let wrap = self.settings.mobile_diff_wrap;
        if self.workspace_panes.review.sessions.values().any(|sr| sr.narrow.wraps_otherwise(wrap)) {
            for sr in self.workspace_panes.review.sessions.values_mut() {
                sr.narrow.set_wrap(wrap);
            }
        }
    }

    fn open_settings(&mut self) {
        App::open_settings(self);
    }

    fn new_session(&mut self, tx: &mut Tx) {
        App::new_session(self, tx);
    }

    fn toggle_home(&mut self, tx: &mut Tx) {
        App::toggle_home(self, tx);
    }

    fn select_tab(&mut self, uid: &str, tx: &mut Tx) {
        App::select_tab(self, uid, tx);
    }

    fn close_tab(&mut self, uid: &str, tx: &mut Tx) {
        App::close_tab(self, uid, tx);
    }

    fn open_tab_menu(&mut self, uid: &str, press: PointerPress, tx: &mut Tx) {
        let Some(target) = self.tab(uid).map(|t| crate::view::tab_strip::target(self, t)) else {
            return;
        };
        crate::session_menu::open(self, target, press, tx);
    }
}

impl App {
    pub fn narrow(&self) -> bool {
        narrow(self)
    }

    pub fn shows_sidebar(&self) -> bool {
        shows_sidebar(self)
    }

    pub fn narrow_view(&self) -> &str {
        narrow_view(self)
    }

    pub fn select_narrow_view(&mut self, session: &str, view: &str, tx: &mut Tx) {
        select_narrow_view(self, session, view, tx);
    }

    pub fn place_narrow(&mut self, session: &str, key: &str, was: (crate::panels::state::Visibility, bool)) {
        if place_narrow(self, session, key) {
            let st = self.session_side_mut(session);
            (st.visibility, st.bottom_open) = was;
        }
    }

    pub fn tabs_drawer(&mut self, event: TabsEvent, tx: &mut Tx) {
        tabs_drawer(self, event, tx);
    }

    pub fn more_drawer(&mut self, event: MoreEvent, tx: &mut Tx) {
        more_drawer(self, event, tx);
    }
}

/// Kernel feature registration for narrow-window layout and drawers.
pub struct NarrowFeature;

impl Feature<App> for NarrowFeature {
    fn id(&self) -> &'static str {
        "feature.narrow"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_system(Phase::React, "narrow.poll", |s, now, tx| {
            let changed = poll(s, now, tx);
            s.note_tick_change(changed)
        });
    }
}
