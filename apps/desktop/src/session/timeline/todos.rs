//! Desktop host adapter for `opencode-feature-timeline-extras::todos`
//! (`features/timeline-extras`) and kernel registration for `TimelineExtrasFeature`.

pub use opencode_feature_timeline_extras::todos::*;

use opencode_kernel::{Feature, KernelBuilder, Phase};
use opencode_kit::model::Transcript;
use rune::{El, Tx};
use rune_knobs::Knobs;
use session_view::palette::Pal;

use crate::{App, Route};

impl TodosHost for App {
    fn active_session_id(&self) -> Option<&str> {
        match &*self.route {
            Route::Session { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }

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
        &self.knobs
    }

    fn reduced_motion(&self) -> bool {
        *self.reduced_motion
    }

    fn todo_open(&self) -> bool {
        *self.todo_open
    }

    fn set_todo_open(&mut self, open: bool) {
        *self.todo_open = open;
    }

    fn todo_ticks(&self) -> &Ticks {
        &self.todo_ticks
    }

    fn todo_ticks_mut(&mut self) -> &mut Ticks {
        &mut self.todo_ticks
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
        builder.add_system(Phase::Derive, "timeline_extras.todos", |s, now, tx| sync(s, now, tx));
        builder.add_system(Phase::Derive, "timeline_extras.agent_rows", |s, _, tx| crate::agent_rows::poll(s, tx));
    }
}
