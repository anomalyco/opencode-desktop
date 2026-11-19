//! Desktop host adapter for `opencode-feature-timeline-extras::agent_rows`
//! (`features/timeline-extras`).

pub use opencode_feature_timeline_extras::agent_rows::*;

use std::collections::HashSet;

use opencode_kit::model::{SessionInfo, ToolStatus, Transcript};
use rune::Tx;
use serde_json::Value;

use crate::App;

impl AgentRowsHost for App {
    fn agent_rows(&self) -> &Live {
        &self.timeline_fx.agent_rows
    }

    fn agent_rows_mut(&mut self) -> &mut Live {
        &mut self.timeline_fx.agent_rows
    }

    fn session_info(&self, id: &str) -> Option<&SessionInfo> {
        App::session(self, id)
    }

    fn transcript(&self, id: &str) -> Option<&Transcript> {
        self.transcripts.get(id)
    }

    fn running_sessions(&self) -> &HashSet<String> {
        &self.running
    }
}

impl App {
    pub fn note_agent(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        opencode_feature_timeline_extras::agent_rows::note_agent(self, ty, data, sid)
    }

    pub(crate) fn forget_agent_steps(&mut self, child: &str) {
        opencode_feature_timeline_extras::agent_rows::forget_agent_steps(self, child);
    }

    pub fn agent_run(&self, parent: &str, call: &str, child: Option<&str>, status: &ToolStatus) -> Run {
        opencode_feature_timeline_extras::agent_rows::agent_run(self, parent, call, child, status)
    }

    pub fn run_started(&self, child: &str, call: Option<i64>) -> i64 {
        opencode_feature_timeline_extras::agent_rows::run_started(self, child, call)
    }

    pub fn run_clock(&self) -> i64 {
        opencode_feature_timeline_extras::agent_rows::run_clock(self)
    }
}

pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    opencode_feature_timeline_extras::agent_rows::poll(s, tx)
}

pub(crate) mod steered_stories;
mod event_marks_stories;
