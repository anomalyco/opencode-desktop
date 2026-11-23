//! Desktop host adapter for `opencode-feature-workspace-tools::new_worktree`
//! (`features/workspace-tools`) and kernel registration for `WorkspaceToolsFeature`.

pub use opencode_feature_workspace_tools::new_worktree::*;

use opencode_kernel::{CommandEntry, Feature, KernelBuilder, Slot};
use opencode_kit::client::{Prompt, Request, Write};
use opencode_kit::model::Project;
use rune::{El, KeyEvent, Tx};
use session_view::palette::Pal;

use crate::keymap::Bind;
use crate::{App, Route, Source};

pub(crate) static NEW_WORKTREE: Slot<NewWorktree> = Slot::new();

impl NewWorktreeHost for App {
    fn new_worktree(&self) -> &NewWorktree {
        self.kernel.slots.get(&NEW_WORKTREE)
    }

    fn new_worktree_mut(&mut self) -> &mut NewWorktree {
        self.kernel.slots.get_mut(&NEW_WORKTREE)
    }

    fn draft_id(&self) -> Option<&str> {
        match &*self.route {
            Route::Draft(id) => Some(id.as_str()),
            _ => None,
        }
    }

    fn draft_dir(&self) -> &str {
        App::draft_dir(self)
    }

    fn set_draft_dir(&mut self, directory: String) {
        App::set_draft_dir(self, directory);
    }

    fn draft_project(&self) -> Option<&Project> {
        App::draft_project(self)
    }

    fn projects(&self) -> &[Project] {
        &self.projects
    }

    fn project(&self, id: &str) -> Option<&Project> {
        App::project(self, id)
    }

    fn project_at(&self, dir: &str) -> Option<&Project> {
        App::project_at(self, dir)
    }

    fn touch_project(&mut self, id: &str) {
        App::touch_project(self, id);
    }

    fn checkout_branch(&self, dir: &str) -> Option<&str> {
        self.sidebar_inbox.branches.get(crate::servers::runtime::active(self), dir)
    }

    fn settings_open(&self) -> bool {
        App::settings_open(self)
    }

    fn project_switcher_open(&self) -> bool {
        self.kernel.slots.get(&crate::project_switcher::PROJECT_SWITCHER).is_open()
    }

    fn is_stage(&self) -> bool {
        self.fixture_run.stage.is_some()
    }

    fn set_composer_focused(&mut self, focused: bool) {
        self.composer.focused = focused;
    }

    fn open_git_location(&mut self, base: bool, tx: &mut Tx) {
        App::open_git_location(self, base, tx);
    }

    fn mark_worktree_onboarding_used(&mut self) {
        crate::view::draft_location::mark_used(self);
    }

    fn invalidate_session_search(&mut self) {
        self.session_search.invalidate();
    }

    fn draft_worktree(&self) -> DraftWorktree {
        use crate::settings::{StartIn, Worktree};
        match (self.settings.start_in, self.settings.worktree) {
            (StartIn::Directory, _) | (StartIn::Project, Worktree::Default) => DraftWorktree::Default,
            (StartIn::Project, Worktree::New) => DraftWorktree::New,
            (StartIn::Project, Worktree::LastUsed) => DraftWorktree::LastUsed,
        }
    }

    fn last_used_destination(&self, project_id: &str) -> Option<&str> {
        self.settings.last_used.get(project_id).map(String::as_str)
    }

    fn set_last_used_destination(&mut self, project_id: String, place: &str) {
        let place = place.to_owned();
        self.change_settings(|s| {
            s.last_used.insert(project_id, place);
        });
    }

    fn send_request(&self, req: Request) {
        if let Source::Live { requests: Some(r), .. } = &*self.source {
            r.send(req).ok();
        }
    }
}

impl App {
    pub fn draft_creates(&self) -> bool {
        opencode_feature_workspace_tools::new_worktree::draft_creates(self)
    }

    pub(crate) fn draft_base(&self) -> Option<&str> {
        opencode_feature_workspace_tools::new_worktree::draft_base(self)
    }

    pub(crate) fn set_draft_base(&mut self, base: Option<String>) {
        opencode_feature_workspace_tools::new_worktree::set_draft_base(self, base);
    }

    pub fn choose_new_worktree(&mut self, project: &str, directory: String) {
        opencode_feature_workspace_tools::new_worktree::choose_new_worktree(self, project, directory);
    }

    pub(crate) fn draft_location_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        opencode_feature_workspace_tools::new_worktree::draft_location_key(self, ev, tx)
    }

    pub(crate) fn note_existing_worktree(&mut self, project: &str, dir: String) {
        opencode_feature_workspace_tools::new_worktree::note_existing_worktree(self, project, dir);
    }

    pub(crate) fn note_destination(&mut self, dir: &str, place: &str) {
        opencode_feature_workspace_tools::new_worktree::note_destination(self, dir, place);
    }

    pub(crate) fn apply_default_destination(&mut self) {
        opencode_feature_workspace_tools::new_worktree::apply_default_destination(self);
    }

    pub(crate) fn forget_new_worktree(&mut self) {
        opencode_feature_workspace_tools::new_worktree::forget_new_worktree(self);
    }

    pub(crate) fn start_write(&self, session: String, prompt: Prompt) -> Write {
        opencode_feature_workspace_tools::new_worktree::start_write(self, session, prompt)
    }

    pub fn project_worktrees(&self, project: &str, root: &str) -> Vec<String> {
        opencode_feature_workspace_tools::new_worktree::project_worktrees(self, project, root)
    }

    pub(crate) fn has_worktree_at(&self, project: &Project, dir: &str) -> bool {
        opencode_feature_workspace_tools::new_worktree::has_worktree_at(self, project, dir)
    }

    pub fn list_worktrees(&self, project: &str) {
        opencode_feature_workspace_tools::new_worktree::list_worktrees(self, project);
    }

    pub(crate) fn worktrees_listed(&mut self, project: String, directories: Vec<String>, managed: Vec<String>) {
        opencode_feature_workspace_tools::new_worktree::worktrees_listed(self, project, directories, managed);
    }

    /// The worktrees OpenCode made for `project` (Settings > Worktrees).
    pub fn managed_worktrees(&self, project: &str) -> Vec<String> {
        opencode_feature_workspace_tools::new_worktree::managed_worktrees(self, project)
    }

    pub(crate) fn worktree_updated(&self, project: &str) {
        opencode_feature_workspace_tools::new_worktree::worktree_updated(self, project);
    }

    pub(crate) fn branches_listed(&mut self, directory: &str, search: &str, branches: Option<Vec<String>>) -> bool {
        opencode_feature_workspace_tools::new_worktree::branches_listed(self, directory, search, branches)
    }

    pub fn open_branch_picker(&mut self, tx: &mut Tx) {
        opencode_feature_workspace_tools::new_worktree::open_branch_picker(self, tx);
    }

    pub fn close_branch_picker(&mut self, tx: &mut Tx) {
        opencode_feature_workspace_tools::new_worktree::close_branch_picker(self, tx);
    }

    pub(crate) fn poll_branch_picker(&mut self, now: f64, tx: &mut Tx) -> bool {
        opencode_feature_workspace_tools::new_worktree::poll_branch_picker(self, now, tx)
    }
}

pub fn trigger(s: &App, p: &Pal, current: &str) -> El {
    opencode_feature_workspace_tools::new_worktree::trigger::<App>(s, p, current)
}

pub fn picker(s: &App, p: &Pal) -> Option<[El; 2]> {
    opencode_feature_workspace_tools::new_worktree::picker::<App>(s, p)
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("worktree-draft", |s| {
        let Some(project) = opencode_kit::model::parse_project(&serde_json::json!({
            "id":"rune", "name":"Rune", "canonical":"/workspace/rune", "vcs":"git"
        })) else { return };
        s.projects.push(project.clone());
        s.open_draft_tab();
        s.choose_new_worktree(&project.id, project.directory);
        s.set_draft_base(Some("release/desktop".into()));
    }),
]);

fn git_draft(s: &App) -> bool {
    matches!(*s.route, Route::Draft(_)) && s.draft_project().is_some_and(|p| opencode_kit::model::project_git(&p.id))
}

/// Kernel feature registration for new-worktree branch picker, client MCP, and Home session search.
pub struct WorkspaceToolsFeature;

impl Feature<App> for WorkspaceToolsFeature {
    fn id(&self) -> &'static str {
        "feature.workspace_tools"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder
            .register_slot(&NEW_WORKTREE)
            .register_slot(&crate::start_location::SERVER_DIRECTORIES)
            .register_slot(&crate::fulltext::FULLTEXT)
            .register_slot(&crate::home_search::HOME_SEARCH)
            .register_slot(&crate::rune_mcp::RUNE_MCP);
        builder.add_command(
            CommandEntry::<App>::from_key(
                "session.location.select",
                opencode_i18n::key!("session.location.worktree"),
                crate::commands::order::SESSION_LOCATION_SELECT,
                |s, tx| s.open_git_location(false, tx),
            )
            .bind_id(Bind::LocationSelect.id())
            .when(git_draft),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "session.branch.choose",
                opencode_i18n::key!("command.session.branch.choose"),
                crate::commands::order::SESSION_BRANCH_CHOOSE,
                |s, tx| s.open_git_location(true, tx),
            )
            .bind_id(Bind::BranchChoose.id())
            .when(git_draft),
        );
        builder.register_overlay(
            opencode_kernel::OverlayRegistration::<App>::new("overlay.worktree_picker")
                .with_render(crate::overlays::layer::SURFACE_POPOVER, |s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    picker(s, p).into_iter().flatten().collect()
                })
                .above(crate::view::home::browse::OVERLAY_ID),
        );
        builder.add_setup("workspace_tools.home_search_waker", |s, waker| s.kernel.slots.get_mut(&crate::home_search::HOME_SEARCH).set_waker(waker));
        builder.add_setup("workspace_tools.rune_mcp", |s, waker| {
            s.setup_rune_mcp(waker);
            rune::marks::mark("mcp.setup");
        });
        builder.schedule(
            crate::tick::stage::DERIVE_SEARCH,
            "workspace_tools.home_search",
            |s, _, tx| s.poll_home_search(tx),
        );
        builder.schedule(
            crate::tick::stage::DERIVE_COMPOSER,
            "workspace_tools.branch_picker",
            |s, now, tx| s.poll_branch_picker(now, tx),
        );
        // Known: reads `mcp_tools_generation` updated by `plugin_host.poll` (`contrib::sync`).
        builder
            .schedule(
                crate::tick::stage::REACT_PANELS,
                "workspace_tools.rune_mcp",
                |s, _, tx| crate::rune_mcp::poll(s, tx),
            )
            .after("plugin_host.poll");
    }
}
