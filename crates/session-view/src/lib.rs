//! Shared OpenCode session timeline view and leaf parts (`p255`).
//!
//! Driven by [`opencode_kit`] models and a [`TimelineHost`] implemented by the embedding Rune app,
//! with no OpenCode (Rune)-only shell or side-panel assumptions inside.

pub mod agent_start;
pub mod compaction;
pub mod copied;
pub mod diagram;
pub mod disclosure;
pub mod genui;
pub mod images;
pub mod knobs;
pub mod odometer;
pub mod palette;
pub mod question_row;
pub mod style;
pub mod themes;
pub mod turn_errors;
pub mod turns;
pub mod working;

use std::collections::{HashMap, HashSet};

use opencode_kit::client::Request;
use opencode_kit::model::{ModelRef, ToolStatus, Transcript};
use rune::{El, Tx};
use rune_knobs::Knobs;
use serde_json::Value;

/// Bundled per-window state for a host that embeds a session timeline.
#[derive(Default)]
pub struct ViewState {
    pub expanded: HashSet<String>,
    pub shutting: HashMap<String, f64>,
    pub copied: copied::Copied,
    pub working: working::Working,
    pub compactions: compaction::Compactions,
    pub diagram: diagram::Diagrams,
    pub media: images::MediaStore,
}

/// Host state and callbacks required by interactive timeline parts and overlays.
pub trait TimelineHost: Sized + 'static {
    fn knobs(&self) -> &Knobs;
    fn clock(&self) -> f64;
    fn now_ms(&self) -> i64 {
        0
    }
    fn reduced_motion(&self) -> bool {
        false
    }
    fn zoom(&self) -> f32 {
        1.0
    }

    fn active_session_id(&self) -> Option<&str>;
    fn transcript(&self, session: &str) -> Option<&Transcript>;
    fn session_directory(&self, _session: &str) -> Option<&str> {
        None
    }
    fn is_session_live(&self, session: &str) -> bool {
        self.transcript(session).is_some_and(Transcript::busy)
    }
    fn for_each_tracked_session(&self, mut f: impl FnMut(&str)) {
        if let Some(id) = self.active_session_id() {
            f(id);
        }
    }

    fn expanded(&self) -> &HashSet<String>;
    fn expanded_mut(&mut self) -> &mut HashSet<String>;
    fn shutting_map(&self) -> &HashMap<String, f64>;
    fn shutting_map_mut(&mut self) -> &mut HashMap<String, f64>;

    fn copied_state(&self) -> &copied::Copied;
    fn copied_state_mut(&mut self) -> &mut copied::Copied;

    fn working_state(&self) -> &working::Working;
    fn working_state_mut(&mut self) -> &mut working::Working;

    fn compactions_state(&self) -> &compaction::Compactions;
    fn compactions_state_mut(&mut self) -> &mut compaction::Compactions;

    fn diagrams(&self) -> &diagram::Diagrams;
    fn diagrams_mut(&mut self) -> &mut diagram::Diagrams;

    fn media_store(&self) -> &images::MediaStore;
    fn media_store_mut(&mut self) -> &mut images::MediaStore;

    fn open_link(&mut self, _url: &str, _tx: &mut Tx) {}
    fn reply_link_help(_url: &str) -> Option<String> {
        None
    }
    fn send_request(&self, _req: Request) {}
    fn can_switch_model(&self) -> bool {
        false
    }
    fn switch_model(&mut self, _tx: &mut Tx) {}
    fn model_name(&self, model: &ModelRef) -> String {
        if model.id.is_empty() {
            model.provider.clone()
        } else {
            model.id.clone()
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn decorate_tool_row(
        &self,
        _p: &palette::Pal,
        _id: &str,
        row: El,
        _name: &str,
        _status: &ToolStatus,
        _input: &Value,
        _metadata: &Value,
    ) -> El {
        row
    }
}
