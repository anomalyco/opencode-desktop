//! The composer's agent, model and effort live in `session_view::pickers`, shared with other Rune apps.
//! This module is the app's side: where picks are kept, what the open session runs with, the
//! project picker, and the server's word on a switch.
use std::collections::HashMap;
use std::sync::Arc;

use opencode_kernel::Slot;
use opencode_kit::client::Prompt;
use rune::{El, KeyEvent, Tx};
use serde_json::Value;

use crate::palette::Pal;
use crate::send_failed::State;
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
        // Each draft tab has its own picks, as official's drafts do (`composer/persistence.tsx`,
        // `new-session/route.tsx`: a draft tab's own composer state and providers). Official
        // keeps a draft's model and effort but shows the first agent again whenever the draft
        // is shown again; here the draft keeps the agent it was given too, with its model.
        match &*self.route { Route::Session { id, .. } | Route::Draft(id) => id.clone(), _ => DRAFT.into() }
    }
    fn picker_draft(&self) -> bool {
        !matches!(*self.route, Route::Session { .. })
    }
    fn picker_running(&self) -> Option<Selection> {
        let Route::Session { id, .. } = &*self.route else { return None };
        self.session(id).map(|i| Selection { model: i.model.clone(), agent: i.agent.clone(), effort: Effort::Chosen })
    }
    /// Desktop's order, not another session's model: a server with no sessions still names one.
    fn picker_fallback_model(&self) -> Option<ModelRef> {
        self.composer_ext.pickers.fallback_model()
    }
    fn picker_subagent(&self) -> Option<(String, Selection)> {
        self.subagent_page().map(|i| (i.id.clone(), Selection { model: i.model.clone(), agent: i.agent.clone(), effort: Effort::Chosen }))
    }
    fn picker_changed(&mut self, scope: &str) {
        saved::save(self, scope);
    }
    fn picker_show_agent(&self) -> bool {
        self.settings.show_agent || self.subagent_page().is_some()
    }
    fn picker_search_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.shortcut(ev, tx)
    }
    fn picker_model_opening(&mut self, tx: &mut Tx) {
        crate::plugin_host::notify_picker_opening(self, Kind::Model, tx);
        // A valid list can still be incomplete while a provider starts. Opening the picker is
        // a chance to ask again, without polling a genuinely empty server at rest.
        let selected = self.selection().model;
        if self.live() && (self.composer_ext.pickers.models.is_empty() || (!selected.id.is_empty()
            && !self.composer_ext.pickers.models.iter().any(|m| m.id == selected.id && m.provider == selected.provider))) {
            crate::connect::picker_reads::refresh(self);
        }
    }
    /// `+` connects an integration (`select-dialog.tsx:1010-1034`), then the sliders.
    fn model_budget(&self, key: &str) -> Option<(String, bool)> {
        self.plugins.model_badge(key)
    }

    fn picker_model_actions(&self) -> Vec<ModelAction> {
        vec![ModelAction::Connect, ModelAction::Manage]
    }
    fn picker_model_action(&mut self, action: ModelAction, tx: &mut Tx) {
        if action == ModelAction::Connect {
            self.open_connect(None, false, tx);
        }
    }
    fn picker_open_unpaid(&mut self, tx: &mut Tx) -> bool {
        crate::connect::unpaid::open(self, tx);
        true
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

/// The picker edit each prompt went out with, by message id, until the server has it. This
/// window's alone: another window's send carries an edit this one doesn't hold, and an edit is
/// never replayed from disk.
#[derive(Default)]
struct SentPicks(HashMap<String, (String, Arc<Selection>)>);

/// Where a window keeps them: not a pick by session, so not `Pickers`' map.
static SENT_PICKS: Slot<SentPicks> = Slot::new();

impl SentPicks {
    /// Prompt `id` goes to `scope` carrying `pick`, which goes once the server has it.
    fn record(&mut self, id: &str, scope: &str, pick: Option<Arc<Selection>>) {
        match pick {
            Some(pick) => self.0.insert(id.to_owned(), (scope.to_owned(), pick)),
            None => self.0.remove(id),
        };
    }

    /// The edit prompt `id` carried.
    fn release(&mut self, id: &str) -> Option<(String, Arc<Selection>)> {
        self.0.remove(id)
    }

    /// Prompt `id` never reached the outbox: there is nothing to release.
    fn forget(&mut self, id: &str) {
        self.0.remove(id);
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
        Some(Box::new(Selection { model: info.model.clone(), agent: info.agent.clone(), effort: Effort::Chosen }))
    }

    /// After `apply_selected`, in each window: a pick it now runs with is forgotten, so the
    /// session follows changes made elsewhere. A picker records the agent a session leaves
    /// unnamed, so both sides compare through the same default.
    pub fn selected_landed(&mut self, sid: &str, confirmed: &Selection) {
        let pickers = &self.composer_ext.pickers;
        let agent = pickers.effective_agent(&confirmed.agent);
        let landed = pickers.selections.get(sid).is_some_and(|held| held.agent == agent && held.model == confirmed.model);
        if landed {
            self.composer_ext.pickers.selections.remove(sid);
            saved::save(self, sid);
        }
    }

    /// Notes the pick prompt `id` goes out with. A queued prompt is left out: it only records a
    /// choice, without switching the running session, so its pick stays held.
    pub(crate) fn note_pick(&mut self, id: &str, prompt: &Prompt) {
        let scope = self.picker_scope();
        let pick = self.composer_ext.pickers.selections.submitted(&scope, prompt.agent.as_deref(), prompt.model.as_ref());
        self.kernel.slots.get_or_default(&SENT_PICKS).record(id, &scope, pick);
    }

    /// The server has a prompt this window sent with a pick: the session runs with that edit, so
    /// it goes and the session follows changes made elsewhere. The outbox is every window's, so
    /// the release lands in the window holding the pick whichever one heard back first; a failed
    /// prompt keeps its edit for the retry.
    pub(crate) fn settle_picks(&mut self) -> bool {
        let waiting: Vec<String> = match self.kernel.slots.try_untracked(&SENT_PICKS) {
            Some(picks) if !picks.0.is_empty() => picks.0.keys().cloned().collect(),
            _ => return false,
        };
        let mut landed = false;
        for id in waiting {
            match self.unsent.get(&id).map(|sent| sent.state()) {
                Some(State::Accepted) => {
                    let released = self.kernel.slots.get_or_default(&SENT_PICKS).release(&id);
                    if let Some((scope, pick)) = released
                        && self.composer_ext.pickers.selections.admitted(&scope, &pick)
                    {
                        saved::save(self, &scope);
                        landed = true;
                    }
                }
                // A refused send, a story's, or another server's: nothing to release here.
                None => self.kernel.slots.get_or_default(&SENT_PICKS).forget(&id),
                // Still on its way, or failed and waiting on its retry: the edit stays.
                Some(_) => {}
            }
        }
        landed
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

pub(crate) mod saved;
mod stories;
