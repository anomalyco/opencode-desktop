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
        &self.pickers
    }
    fn pickers_mut(&mut self) -> &mut Pickers {
        &mut self.pickers
    }
    fn picker_scope(&self) -> String {
        match &self.route { Route::Session(id) => id.clone(), _ => "draft".into() }
    }
    fn picker_draft(&self) -> bool {
        !matches!(self.route, Route::Session(_))
    }
    fn picker_running(&self) -> Option<Selection> {
        let Route::Session(id) = &self.route else { return None };
        self.session(id).map(|i| Selection { model: i.model.clone(), agent: i.agent.clone() })
    }
    fn picker_fallback_model(&self) -> Option<ModelRef> {
        self.sessions.first().map(|s| s.model.clone())
    }
    fn picker_subagent(&self) -> Option<(String, Selection)> {
        self.subagent_page().map(|i| (i.id.clone(), Selection { model: i.model.clone(), agent: i.agent.clone() }))
    }
    fn picker_show_agent(&self) -> bool {
        self.settings.show_agent || self.subagent_page().is_some()
    }
    /// A narrow window's folded pickers aren't stops (`view::narrow`).
    fn picker_tab_stops(&self) -> Vec<Kind> {
        let shown = crate::view::toolbar_shown();
        [Kind::Agent, Kind::Model, Kind::Effort].into_iter()
            .filter(|k| match k {
                Kind::Agent => self.settings.show_agent && shown.agent != crate::view::ToolbarAgent::Gone,
                Kind::Effort => self.has_effort() && shown.effort,
                _ => true,
            }).collect()
    }
    fn picker_search_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.shortcut(ev, tx)
    }
    fn picker_projects(&self) -> Vec<Item> {
        self.projects.iter().map(|p| Item::new(p.id.clone(), p.name.clone()))
            .chain(std::iter::once(Item::new(ADD_PROJECT_KEY, "Add project")))
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
            self.draft_dir = p.directory;
            self.touch_project(&p.id);
        }
    }
    fn picker_project_lead(&self, p: &Pal, key: &str, label: &str) -> Option<El> {
        Some(crate::icons::badge(self, p, self.project(key), label))
    }
}

impl App {
    /// `session.agent.selected` and `session.model.selected`: what the session runs with now.
    /// A pick it now runs with is forgotten, so the session follows changes made elsewhere.
    pub fn apply_selected(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        if !matches!(ty, "session.agent.selected" | "session.model.selected") { return false; }
        let Some(info) = self.sessions.iter_mut().find(|s| s.id == sid) else { return false };
        if ty == "session.agent.selected" {
            info.agent = data["agent"].as_str().unwrap_or("").to_owned();
        } else {
            let m = &data["model"];
            let s = |v: &Value| v.as_str().unwrap_or("").to_owned();
            info.model = ModelRef { id: s(&m["id"]), provider: s(&m["providerID"]), variant: s(&m["variant"]) };
        }
        let now = Selection { model: info.model.clone(), agent: info.agent.clone() };
        if self.pickers.selections.get(sid) == Some(&now) { self.pickers.selections.remove(sid); }
        true
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
        self.project_at(&self.draft_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Key, KeyEvent};
    use crate::fixture_runtime as runtime;
    use rune::Runtime;
    fn key(rt: &mut Runtime<App>, key: Key) {
        rt.key(KeyEvent { key, mods: Default::default() });
        rt.frame(1.0 / 60.0);
    }
    fn tap(rt: &mut Runtime<App>, target: &str) {
        let (x, y) = rt.find(target).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn model_search_wrap_empty_ime_and_focus_restore() {
        let mut rt = runtime();
        tap(&mut rt, "prompt");
        key(&mut rt, Key::Text("draft".into()));
        tap(&mut rt, "model");
        assert_eq!(rt.state.picker_active().as_deref(), Some("anthropic:claude-opus-5-5"));
        key(&mut rt, Key::Up);
        assert_eq!(rt.state.picker_active().as_deref(), Some("google:gemini-4-argon"));
        key(&mut rt, Key::Down);
        key(&mut rt, Key::Text("OPENAI".into()));
        assert_eq!(rt.state.picker_active().as_deref(), Some("openai:gpt-6-astra"));
        assert_eq!(rt.state.composer.text, "draft");
        rt.ime(rune::Ime::Preedit("x".into(), None));
        rt.frame(1.0 / 60.0);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.pickers.open, Some(Kind::Model));
        rt.ime(rune::Ime::Preedit(String::new(), None));
        rt.frame(1.0 / 60.0);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.selection().model.id, "gpt-6-astra");
        assert!(rt.state.selection().model.variant.is_empty());
        assert_eq!(rt.state.sends, 0);
        // Exit is still on screen, but keyboard focus already belongs to the composer.
        key(&mut rt, Key::Text("!".into()));
        assert_eq!(rt.state.composer.text, "draft!");
        tap(&mut rt, "model");
        key(&mut rt, Key::Text("no-such-model".into()));
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.pickers.open, Some(Kind::Model));
        key(&mut rt, Key::Escape);
        assert!(rt.state.composer.focused);
    }

    #[test]
    fn agent_effort_tab_pointer_and_session_isolation() {
        let mut rt = runtime();
        rt.state.settings.show_agent = true;
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        tap(&mut rt, "prompt");
        for expected in [Some(Kind::Agent), Some(Kind::Model), Some(Kind::Effort), None] {
            key(&mut rt, Key::Tab);
            assert_eq!(rt.state.pickers.toolbar_focus, expected);
        }
        assert!(rt.state.composer.focused);
        key(&mut rt, Key::Tab);
        key(&mut rt, Key::Enter);
        key(&mut rt, Key::Down);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.selection().agent, "plan");
        tap(&mut rt, "effort");
        key(&mut rt, Key::Down); // high wraps to default
        key(&mut rt, Key::Down); // low
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.selection().model.variant, "low");
        let effort = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("effort")).unwrap();
        assert!(!effort.moving && effort.rect == effort.target, "the new label's width cuts: {:?} → {:?}", effort.rect, effort.target);
        tap(&mut rt, "model");
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let (x, y) = rt.find("option-anthropic:claude-sonnet-5-5").unwrap();
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.selection().model.id, "claude-sonnet-5-5");
        assert!(rt.state.selection().model.variant.is_empty());
        let old_route = rt.state.route.clone();
        rt.state.route = Route::Draft;
        assert_eq!(rt.state.selection().agent, "build");
        rt.state.route = old_route;
        tap(&mut rt, "model");
        rt.set_viewport((960.0, 700.0), 1.0);
        rt.frame(1.0 / 60.0);
        let elements = rt.elements();
        let menu = elements.iter().find(|e| e.key.as_deref() == Some("picker-model")).unwrap();
        let anchor = elements.iter().find(|e| e.key.as_deref() == Some("model")).unwrap();
        assert!((menu.target[1] + menu.target[3] + 6.0 - anchor.target[1]).abs() < 0.1);
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.pickers.open, None);
        assert!(!rt.state.composer.focused);
    }

    #[test]
    fn filter_resizes_and_escape_closes_at_once() {
        for reduced in [false, true] {
            let mut rt = runtime();
            rt.state.reduced_motion = reduced;
            rt.frame(1.0 / 60.0);
            tap(&mut rt, "model");
            rt.frame(1.0 / 60.0);
            let find = |rt: &Runtime<App>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
            // Under Reduce Motion the grow is only its fade.
            let menu = find(&rt, "picker-model").unwrap();
            assert!(menu.opacity < 1.0 && (menu.scale < 1.0) != reduced, "it grows from the trigger (reduced={reduced}): {}", menu.scale);
            key(&mut rt, Key::Escape);
            rt.frame(1.0 / 60.0);
            assert!(find(&rt, "picker-model").is_none(), "Escape removes it in one frame (reduced={reduced})");
        }
    }

    #[test]
    fn filtering_lands_in_the_keystroke() {
        let mut rt = runtime();
        tap(&mut rt, "model");
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let find = |rt: &Runtime<App>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting);
        key(&mut rt, Key::Text("missing".into()));
        assert!(find(&rt, "option-openai:gpt-6-astra").is_none(), "a row that stops matching goes at once");
        let empty = find(&rt, "picker-empty").unwrap();
        assert_eq!(empty.opacity, 1.0);
        let scroll = find(&rt, "picker-scroll").unwrap();
        assert!((scroll.rect[3] - 48.0).abs() < 0.1, "the menu is the empty state's height at once: {}", scroll.rect[3]);
        key(&mut rt, Key::Backspace);
        for _ in 0..6 { key(&mut rt, Key::Backspace); }
        let row = find(&rt, "option-openai:gpt-6-astra").unwrap();
        assert!((row.opacity - 1.0).abs() < 1e-3 && (row.rect[1] - row.target[1]).abs() < 0.1, "a row that matches again lands in place");
    }
}

#[cfg(test)]
mod live_tests;

mod stories;
