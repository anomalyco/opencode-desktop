//! Desktop host adapter for `opencode-feature-timeline-extras::todos`
//! (`features/timeline-extras`) and kernel registration for `TimelineExtrasFeature`.

pub use opencode_feature_timeline_extras::todos::*;

use opencode_kernel::{CommandEntry, CommandTitle, EscapeLayer, Feature, KernelBuilder, Slot};
use opencode_kit::model::Transcript;
use rune::{El, Tx};
use rune_knobs::Knobs;
use session_view::palette::Pal;

use crate::keymap::Bind;
use crate::{App, Route};

pub(crate) static TODO_OPEN: Slot<bool> = Slot::new();
static TODO_TICKS: Slot<Ticks> = Slot::new();

impl TodosHost for App {
    fn is_session_running(&self, id: &str) -> bool {
        self.running.contains(id)
    }

    fn transcript(&self, id: &str) -> Option<&Transcript> {
        rune::Tracked::untracked(&self.transcripts).get(id)
    }

    fn transcript_version(&self, id: &str) -> u64 {
        self.family_transcript_version(id).1
    }

    fn session_revert(&self, id: &str) -> Option<&str> {
        self.session(id).and_then(|i| i.revert.as_deref())
    }

    fn knobs(&self) -> &Knobs {
        &self.diag.knobs
    }

    fn todo_open(&self) -> bool {
        *self.kernel.slots.get(&TODO_OPEN)
    }

    fn set_todo_open(&mut self, open: bool) {
        *self.kernel.slots.get_mut(&TODO_OPEN) = open;
    }

    fn todo_ticks(&self) -> &Ticks {
        self.kernel.slots.get(&TODO_TICKS)
    }

    fn todo_ticks_mut(&mut self) -> &mut Ticks {
        self.kernel.slots.get_mut(&TODO_TICKS)
    }

    fn on_todo_toggled(&mut self, _open: bool) {
        crate::subagents::shells::set_open(self, false);
    }

    fn todo_chip(&self, p: &Pal, key: &str, label: String, open: bool) -> El {
        crate::subagents::chip(self, p, key, label, open)
    }
}

impl App {
    pub fn shown_todos(&self, id: &str) -> Vec<Todo> {
        opencode_feature_timeline_extras::todos::shown_todos(self, id)
    }
}

pub fn sync(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    opencode_feature_timeline_extras::todos::sync(s, now, tx)
}

pub fn pill(s: &App, p: &Pal, id: &str) -> Option<El> {
    opencode_feature_timeline_extras::todos::pill::<App>(s, p, id)
}

pub fn panel(s: &App, p: &Pal, id: &str) -> Option<El> {
    opencode_feature_timeline_extras::todos::panel::<App>(s, p, id)
}

rune_register::register!(crate::knobs::GROUP_LIST: rune_knobs::Group = [GROUP]);

/// Kernel feature registration for session timeline extras (`todos` and `agent_rows`).
pub struct TimelineExtrasFeature;

impl Feature<App> for TimelineExtrasFeature {
    fn id(&self) -> &'static str {
        "feature.timeline_extras"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        // `use-session-commands.tsx:711-722`: its title says what it will do.
        builder.add_command(
            CommandEntry::<App>::from_key(
                "auto-accept",
                opencode_i18n::key!("command.permissions.autoaccept.enable"),
                crate::commands::order::AUTO_ACCEPT,
                |s, tx| s.toggle_auto_accept(tx),
            )
            .titled(|s| {
                CommandTitle::Key(match (
                    s.auto_accepting(),
                    matches!(*s.route, Route::Session { .. })
                        && s.active_session_id()
                            .and_then(|id| s.session(id).and_then(|i| s.project_for_session(i)))
                            .is_some(),
                ) {
                    (true, _) => opencode_i18n::key!("command.permissions.autoaccept.disable"),
                    (false, true) => opencode_i18n::key!("command.permissions.autoaccept.project"),
                    (false, false) => opencode_i18n::key!("command.permissions.autoaccept.enable"),
                })
            })
            .bind_id(Bind::AutoAccept.id())
            .when(|s| s.active_session_id().is_some()),
        );
        builder
            .register_slot(&TODO_OPEN)
            .register_slot(&TODO_TICKS)
            .register_slot(&crate::agent_rows::AGENT_ROWS)
            .register_slot(&crate::question::QUESTION)
            .register_slot(&crate::permission::PERMISSION_SENT);
        builder.register_escape_layer(EscapeLayer::new(
            "escape.todo",
            50,
            |s| *s.kernel.slots.get(&TODO_OPEN),
            |s, _, tx| {
                tx.instant();
                *s.kernel.slots.get_mut(&TODO_OPEN) = false;
            },
        ));
        builder.schedule(crate::tick::stage::DERIVE_TIMELINE, "timeline_extras.todos", sync);
        builder.schedule(
            crate::tick::stage::DERIVE_TIMELINE,
            "timeline_extras.agent_rows",
            |s, _, tx| crate::agent_rows::poll(s, tx),
        );
    }
}
