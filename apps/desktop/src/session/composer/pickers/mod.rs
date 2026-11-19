//! The composer's agent, model and effort live in `session_view::pickers`, shared with Seer.
//! This module is the app's side: where picks are kept, what the open session runs with, the
//! project picker, and the server's word on a switch.
use rune::{El, KeyEvent, Tx};
use serde_json::Value;

use crate::palette::Pal;
use crate::{App, Route};
use opencode_kit::model::ModelRef;

pub use session_view::pickers::*;

impl PickerHost for App {
    fn pickers(&self) -> &Pickers {
        &self.composer_ext.pickers
    }
    fn pickers_mut(&mut self) -> &mut Pickers {
        &mut self.composer_ext.pickers
    }
    fn picker_provider_mark(&self, p: &crate::palette::Pal, provider: &str) -> Option<rune::El> {
        Some(crate::widgets::provider_mark(provider, p.icon))
    }
    fn picker_scope(&self) -> String {
        match &*self.route { Route::Session { id, .. } => id.clone(), _ => "draft".into() }
    }
    fn picker_draft(&self) -> bool {
        !matches!(*self.route, Route::Session { .. })
    }
    fn picker_running(&self) -> Option<Selection> {
        let Route::Session { id, .. } = &*self.route else { return None };
        self.session(id).map(|i| Selection { model: i.model.clone(), agent: i.agent.clone() })
    }
    /// Desktop's order, not another session's model: a server with no sessions still names one.
    fn picker_fallback_model(&self) -> Option<ModelRef> {
        self.composer_ext.pickers.fallback_model()
    }
    fn picker_subagent(&self) -> Option<(String, Selection)> {
        self.subagent_page().map(|i| (i.id.clone(), Selection { model: i.model.clone(), agent: i.agent.clone() }))
    }
    fn picker_show_agent(&self) -> bool {
        self.settings.show_agent || self.subagent_page().is_some()
    }
    fn picker_search_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.shortcut(ev, tx)
    }
    fn picker_model_opening(&mut self) {
        self.refresh_picker_usage();
        // A valid list can still be incomplete while a provider starts. Opening the picker is
        // a chance to ask again, without polling a genuinely empty server at rest.
        let selected = self.selection().model;
        if self.live() && (self.composer_ext.pickers.models.is_empty() || (!selected.id.is_empty()
            && !self.composer_ext.pickers.models.iter().any(|m| m.id == selected.id && m.provider == selected.provider))) {
            crate::connect::picker_reads::refresh(self);
        }
    }
    /// `+` connects an integration (`select-dialog.tsx:1010-1034`), then the sliders.
    fn model_budget(&self, key: &str) -> Option<(String, bool)> { self.picker_budget_label(key) }

    fn picker_model_actions(&self) -> Vec<ModelAction> {
        vec![ModelAction::Connect, ModelAction::Manage]
    }
    fn picker_model_action(&mut self, action: ModelAction, tx: &mut Tx) {
        if action == ModelAction::Connect {
            self.open_connect(None, false, tx);
        }
    }
    fn picker_projects(&self) -> Vec<Item> {
        self.projects.iter().map(|p| Item::new(p.id.clone(), p.name.clone()))
            .chain(std::iter::once(Item::new(ADD_PROJECT_KEY, opencode_i18n::t!("session.new.project.add"))))
            .collect()
    }
    fn picker_project_current(&self) -> String {
        self.draft_project().map_or_else(String::new, |p| p.id.clone())
    }
    fn picker_choose_project(&mut self, key: &str, tx: &mut Tx) {
        if key == ADD_PROJECT_KEY {
            self.open_project_dialog(tx);
            return;
        }
        if let Some(p) = self.projects.iter().find(|p| p.id == key).cloned() {
            self.set_draft_dir(p.directory);
            self.touch_project(&p.id);
        }
    }
    fn picker_project_lead(&self, p: &Pal, key: &str, label: &str) -> Option<El> {
        Some(crate::icons::badge(self, p, self.project(key), label))
    }
}

impl App {
    /// `session.agent.selected` and `session.model.selected`: what the session runs with now,
    /// in the server's copy (`App::apply_shared`).
    pub fn apply_selected(&mut self, ty: &str, data: &Value, sid: &str) -> Option<Box<Selection>> {
        if !matches!(ty, "session.agent.selected" | "session.model.selected") { return None; }
        let info = self.sessions.iter_mut().find(|s| s.id == sid)?;
        if ty == "session.agent.selected" {
            info.agent = data["agent"].as_str().unwrap_or("").to_owned();
        } else {
            let m = &data["model"];
            let s = |v: &Value| v.as_str().unwrap_or("").to_owned();
            info.model = ModelRef { id: s(&m["id"]), provider: s(&m["providerID"]), variant: s(&m["variant"]) };
        }
        Some(Box::new(Selection { model: info.model.clone(), agent: info.agent.clone() }))
    }

    /// After `apply_selected`, in each window: a pick it now runs with is forgotten, so the
    /// session follows changes made elsewhere.
    pub fn selected_landed(&mut self, sid: &str, confirmed: &Selection) {
        if self.composer_ext.pickers.selections.get(sid) == Some(confirmed) { self.composer_ext.pickers.selections.remove(sid); }
    }

    /// The fixture's server: a prompt that switches the session says so in its stream, before
    /// its reply, as a live server does.
    pub fn serve_choice(&mut self, session: &str, prompt: &opencode_kit::client::Prompt, at: f64) {
        if let Some(agent) = &prompt.agent {
            self.schedule(at, "session.agent.selected", serde_json::json!({ "sessionID": session, "agent": agent }));
        }
        if let Some(m) = &prompt.model {
            let model = serde_json::json!({ "id": m.id, "providerID": m.provider, "variant": m.variant });
            self.schedule(at, "session.model.selected", serde_json::json!({ "sessionID": session, "model": model }));
        }
    }

    /// The project a new session would start in: the one whose folder holds the draft's.
    pub fn draft_project(&self) -> Option<&opencode_kit::model::Project> {
        self.project_at(self.draft_dir())
    }
}

mod stories;
