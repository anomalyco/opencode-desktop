//! Desktop host adapter for `opencode-feature-subagents` (`features/subagents`).

pub use opencode_feature_subagents::*;

use std::collections::HashSet;

use opencode_kernel::{CommandEntry, EscapeLayer, Feature, KernelBuilder, Phase};
use opencode_kit::client::{Prompt, Write};
use opencode_kit::model::{SessionInfo, Transcript};
use rune::Tx;
use rune_knobs::Knobs;

use crate::jobs::Apply;
use crate::keymap::Bind;
use crate::{App, Source};

impl SubagentsHost for App {
    fn shells(&self) -> &Shells {
        &self.shells
    }

    fn shells_mut(&mut self) -> &mut Shells {
        &mut self.shells
    }

    fn session_info(&self, id: &str) -> Option<&SessionInfo> {
        self.session(id)
    }

    fn sessions(&self) -> &[SessionInfo] {
        &self.sessions
    }

    fn transcript(&self, id: &str) -> Option<&Transcript> {
        rune::Tracked::untracked(&self.transcripts).get(id)
    }

    fn transcripts_version(&self) -> Option<u64> {
        Some(rune::Tracked::version(&self.transcripts))
    }

    fn running_sessions(&self) -> &HashSet<String> {
        &self.running
    }

    fn run_started(&self, child: &str, call: Option<i64>) -> i64 {
        App::run_started(self, child, call)
    }

    fn background_activity_enabled(&self) -> bool {
        self.settings.background_activity != crate::settings::BackgroundActivity::Off
    }

    fn composer_is_empty(&self) -> bool {
        self.composer.text.trim().is_empty() && !self.composer_ext.attach.any() && !self.composer_ext.quote.any()
    }

    fn shows_esc_keycap(&self) -> bool {
        self.layout.narrow.sm()
    }

    fn is_shell_console_open(&self, call: &session_view::tools::Call<'_>) -> bool {
        session_view::tools::console_open(self, call)
    }

    fn knobs(&self) -> &Knobs {
        &self.diag.knobs
    }

    fn prefetch_session(&self, id: &str) {
        App::prefetch_session(self, id);
    }

    fn open_subagent_session(&mut self, parent: &str, child: &str, title: &str, agent: &str, tx: &mut Tx) {
        if self.session(child).is_none() {
            let from = self.session(parent).cloned().unwrap_or_default();
            self.sessions.push(SessionInfo {
                id: child.to_owned(),
                parent: Some(parent.to_owned()),
                project: from.project,
                directory: from.directory,
                title: title.to_owned(),
                agent: agent.to_owned(),
                ..Default::default()
            });
            self.fetch_info(child);
        }
        self.open(child, tx);
    }

    fn navigate_to_session(&mut self, id: &str, tx: &mut Tx) {
        self.open(id, tx);
    }

    fn on_activity_toggled(&mut self, _open: bool) {
        *self.timeline_fx.todo_open = false;
    }

    fn spawn_shell_request(&mut self, job: SubagentJob) {
        match job {
            SubagentJob::Server { request, sync_if_headless } => {
                if sync_if_headless && crate::headless() {
                    if let Source::Live { server, .. } = &*self.source {
                        let srv = server.clone();
                        let reply = execute_request(&srv, request);
                        apply_reply(self, reply);
                    }
                    return;
                }
                self.job(move |srv| {
                    let reply = execute_request(srv, request);
                    Box::new(move |s: &mut App, _tx: &mut Tx| apply_reply(s, reply)) as Apply
                });
            }
        }
    }
}

impl App {
    /// Whether `id` is `ancestor` or a session below it.
    pub fn descends(&self, id: &str, ancestor: &str) -> bool {
        descends(self, id, ancestor)
    }

    /// Whether `id` or anything below it is running.
    pub fn subtree_running(&self, id: &str) -> bool {
        subtree_running(self, id)
    }

    /// The sessions running below `id`, oldest first.
    pub fn running_subagents(&self, id: &str) -> Vec<Running> {
        running_subagents(self, id)
    }

    /// Opens a subagent's session from its parent's timeline.
    pub fn open_child(&mut self, parent: &str, child: &str, title: &str, agent: &str, tx: &mut Tx) {
        open_child(self, parent, child, title, agent, tx);
    }

    /// The open session is a subagent's, with the session that started it known.
    pub fn has_parent(&self) -> bool {
        has_parent(self)
    }

    /// Navigates from a subagent's session back to the session that started it.
    pub fn open_parent(&mut self, tx: &mut Tx) -> bool {
        open_parent(self, tx)
    }

    /// The open page's session when it's a subagent's.
    pub fn subagent_page(&self) -> Option<&SessionInfo> {
        reply::subagent_page(self)
    }

    /// The write that sends `prompt` to `session`.
    pub fn send_write(&self, session: &str, prompt: Prompt) -> Write {
        reply::send_write(self, session, prompt)
    }
}

/// Kernel feature registration for subagents activity and background shells polling.
pub struct SubagentsFeature;

impl Feature<App> for SubagentsFeature {
    fn id(&self) -> &'static str {
        "feature.subagents"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        // Neither has a `/name` in Desktop (`use-session-commands.tsx:476-482`, `session-identity-header.tsx`).
        builder.add_command(
            CommandEntry::<App>::from_key("background", opencode_i18n::key!("command.session.background"), |s, tx| {
                s.move_to_background(tx);
            })
            .bind_id(Bind::Background.id())
            .order(190)
            .when(|s| s.active_session_id().is_some_and(|id| !crate::background::blocking(s, id).is_empty())),
        );
        builder.register_escape_layer(EscapeLayer::new(
            "escape.child_session",
            10,
            |s| reply::composer_empty(s) && s.has_parent(),
            |s, _, tx| {
                s.open_parent(tx);
            },
        ));
        builder.register_escape_layer(EscapeLayer::new(
            "escape.activity",
            60,
            |s| s.shells.activity.is_open(),
            |s, _, tx| {
                tx.instant();
                shells::set_open(s, false);
            },
        ));
        builder.add_system(Phase::Derive, "subagents.shells", |s, _, tx| shells::poll(s, tx));
    }
}

mod stories {
    //! A finished subagent's page: its timeline, and the composer that answers it.

    use rune::Tx;

    use crate::fixture_ids::CHILD;
    use crate::stories::{Story, Trigger};
    use crate::{App, Route};

    /// The fixture's finished reviewer, opened from its parent.
    pub fn open_child(s: &mut App) {
        crate::scenes::open_scene(s);
        s.finish_stream();
        *s.route = Route::here(CHILD.into());
    }

    const KEYS: &[Trigger] = &[Trigger { key: 'w', label: "It works on a reply, or finishes", run: |s, tx| work(s, tx) }];

    fn work(s: &mut App, tx: &mut Tx) {
        tx.instant();
        if !s.running.remove(CHILD) {
            s.running.insert(CHILD.into());
        }
    }

    /// Side tabs with the open session's subagent running and two more tabs working, so the
    /// sidebar's spinners, the subagent capsule's and the card's sit on one screen.
    fn busy_glyphs(s: &mut App) {
        crate::scenes::open_scene(s);
        s.finish_stream();
        crate::scenes::side_tabs(s);
        s.running.extend([CHILD, crate::fixture_ids::THIRD, crate::fixture_ids::SIXTH].map(String::from));
    }

    rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("subagent", open_child), ("busy-glyphs", busy_glyphs)]);

    rune_register::register!(crate::stories::STORY_LIST: Story = [
        Story { path: "Composer/Subagent reply", scene: |s| {
            open_child(s);
            crate::stories::stage(s, crate::stories::composer_view);
        }, knobs: &[], keys: KEYS, variants: &[] },
    ]);
}
