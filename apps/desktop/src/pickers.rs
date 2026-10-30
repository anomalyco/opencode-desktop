//! The composer's agent, model and effort. Desktop reference: ModelSelectorPopoverView,
//! ComposerEditorSelect and `providers/models/selection.tsx`. A session runs with what the
//! server last said (`session.agent`, `session.model`); a pick is remembered per session and
//! rides on its next prompt, which switches the session first (`Server::send`). Once the server
//! says it runs with the pick, the memory goes, as Desktop's `reconcile` does.
use std::collections::{HashMap, HashSet};

use rune::{Key, KeyEvent, TextEdit, Tx};
use serde_json::Value;

use crate::model_prefs::ModelPrefs;
use crate::popover::{Highlight, Nav, cycle, nav};
use crate::{App, Route};
use opencode_kit::model::{ModelRef, capitalize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Agent, Model, Effort, Project }

pub const ADD_PROJECT_KEY: &str = "+add";

impl Kind {
    pub fn key(self) -> &'static str {
        match self { Self::Agent => "agent", Self::Model => "model", Self::Effort => "effort", Self::Project => "project" }
    }
}

#[derive(Clone, Debug)]
pub struct Model {
    pub key: String,
    pub id: String,
    pub provider: String,
    pub group: String,
    pub name: String,
    pub variants: Vec<String>,
    /// Left out of the list until a search finds it (Desktop's `visible`).
    pub hidden: bool,
    /// OpenCode's free models (Desktop's `isFreeModel`).
    pub free: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selection { pub model: ModelRef, pub agent: String }

#[derive(Clone, Default)]
pub struct Item {
    pub key: String,
    pub label: String,
    pub group: String,
    /// Faint after the label: a model's provider in Favorites and Recent.
    pub note: String,
    pub free: bool,
}

impl Item {
    fn new(key: impl Into<String>, label: impl Into<String>) -> Item {
        Item { key: key.into(), label: label.into(), ..Item::default() }
    }
}

pub const FAVORITES: &str = "Favorites";
pub const RECENT: &str = "Recent";

#[derive(Default)]
pub struct Pickers {
    pub models: Vec<Model>,
    /// Context windows in tokens by `provider:id`, from the fixture or the server's `/api/model`.
    pub limits: HashMap<String, u64>,
    pub agents: Vec<String>,
    /// Each agent's configured model (`/api/agent`'s `model`), which it switches to when picked.
    pub agent_models: HashMap<String, ModelRef>,
    pub open: Option<Kind>,
    pub search: TextEdit,
    pub toolbar_focus: Option<Kind>,
    active: Highlight,
    /// Picks by session id ("draft" for a new session), until the server runs with them.
    selections: HashMap<String, Selection>,
    /// The model each agent last had in a session, by session then agent, so switching back to
    /// an agent brings its model back (Desktop's `choices`).
    choices: HashMap<String, HashMap<String, ModelRef>>,
    /// The effort last picked for each model, which picking that model again brings back
    /// (Desktop's per-model `variant` preference).
    efforts: HashMap<String, String>,
    /// Efforts chosen with ← → on a model's row in the open list, which picking it applies
    /// (Desktop's staged variant). Opening the list clears them.
    staged: HashMap<String, String>,
    pub prefs: ModelPrefs,
}

impl Pickers {
    pub fn load(&mut self, fixture: &Value) {
        self.models = fixture["models"].as_array().into_iter().flatten().map(|v| {
            let s = |key: &str| v[key].as_str().unwrap_or("").to_owned();
            Model { key: format!("{}:{}", s("provider"), s("id")), id: s("id"), provider: s("provider"),
                group: s("group"), name: s("name"), variants: v["variants"].as_array().into_iter().flatten()
                    .filter_map(Value::as_str).map(str::to_owned).collect(), hidden: false, free: false }
        }).collect();
        self.load_limits(&fixture["models"]);
        self.agents = fixture["agents"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    }

    /// The server's `/api/model` and `/api/agent` lists: variants are `{ id }` objects there.
    /// Deprecated models are left out, as Desktop leaves them out of its catalog.
    pub fn load_live(&mut self, models: &Value, agents: &Value) {
        let now = crate::wall_ms();
        let s = |v: &Value, key: &str| v[key].as_str().unwrap_or("").to_owned();
        let listed: Vec<&Value> = models.as_array().into_iter().flatten()
            .filter(|v| v["enabled"].as_bool().unwrap_or(true) && v["status"].as_str() != Some("deprecated")).collect();
        let shown = shown_models(&listed, now);
        self.models = listed.iter().map(|v| {
            let (id, provider) = (s(v, "id"), s(v, "providerID"));
            let key = format!("{provider}:{id}");
            Model { hidden: !shown.contains(&key), free: is_free(v), key, id, group: provider.clone(), provider, name: s(v, "name").replace("(latest)", "").trim().to_owned(),
                variants: v["variants"].as_array().into_iter().flatten().filter_map(|x| x["id"].as_str().or(x.as_str())).map(str::to_owned).collect() }
        }).collect();
        self.load_limits(models);
        let agents: Vec<&Value> = agents.as_array().into_iter().flatten()
            .filter(|a| a["mode"].as_str() != Some("subagent") && !a["hidden"].as_bool().unwrap_or(false)).collect();
        self.agents = agents.iter().filter_map(|a| a["id"].as_str()).map(str::to_owned).collect();
        self.agent_models = agents.iter().filter_map(|a| {
            let m = &a["model"];
            let model = ModelRef { id: s(m, "id"), provider: s(m, "providerID"), variant: s(m, "variant") };
            (!model.id.is_empty()).then(|| (s(a, "id"), model))
        }).collect();
    }

    /// Reads `limit.context` from a list of models, keyed by `provider` or `providerID`.
    pub fn load_limits(&mut self, models: &Value) {
        for m in models.as_array().into_iter().flatten() {
            let provider = m["providerID"].as_str().or(m["provider"].as_str()).unwrap_or("");
            if let (Some(id), Some(n)) = (m["id"].as_str(), m["limit"]["context"].as_u64()) {
                self.limits.insert(format!("{provider}:{id}"), n);
            }
        }
    }

    pub fn activate(&mut self, key: String) {
        self.active.set(key, self.search.text.clone());
    }

    pub fn model_name(&self, m: &ModelRef) -> String {
        self.models.iter().find(|x| x.id == m.id && x.provider == m.provider).map_or_else(|| m.display(), |x| x.name.clone())
    }
}

/// OpenCode's models that cost nothing (Desktop's `isFreeModel`): the untiered cost's input is 0.
fn is_free(m: &Value) -> bool {
    m["providerID"] == "opencode" && m["cost"].as_array().and_then(|c| c.iter().find(|c| c.get("tier").is_none()).or(c.first())).is_none_or(|c| c["input"].as_f64() == Some(0.0))
}

/// Desktop's default model list (`models.tsx` `visible`): each provider's newest model in a
/// family released in the last six months, OpenCode's free models, and models with no release
/// date. A search finds the rest.
fn shown_models(models: &[&Value], now_ms: i64) -> HashSet<String> {
    const SIX_MONTHS: f64 = 6.0 * 30.436875 * 24.0 * 3600.0 * 1000.0;
    let key = |m: &Value| format!("{}:{}", m["providerID"].as_str().unwrap_or(""), m["id"].as_str().unwrap_or(""));
    let free = |m: &Value| is_free(m);
    let mut out = HashSet::new();
    let mut newest: HashMap<String, (i64, String)> = HashMap::new();
    for m in models {
        let Some(released) = m["time"]["released"].as_f64() else { out.insert(key(m)); continue };
        if free(m) { out.insert(key(m)); }
        if (now_ms as f64 - released).abs() >= SIX_MONTHS { continue; }
        let family = format!("{}/{}/{}", m["providerID"].as_str().unwrap_or(""), m["family"].as_str().unwrap_or(""), free(m));
        // A tie keeps the first listed.
        if newest.get(&family).is_none_or(|(t, _)| released as i64 > *t) {
            newest.insert(family, (released as i64, key(m)));
        }
    }
    out.extend(newest.into_values().map(|(_, k)| k));
    out
}

impl App {
    fn selection_key(&self) -> &str {
        match &self.route { Route::Session(id) => id, _ => "draft" }
    }

    /// What the session runs with now: the agent and model the server last said, or for a new
    /// session the first agent with its configured model.
    fn durable(&self) -> Selection {
        let info = match &self.route { Route::Session(id) => self.session(id), _ => None };
        let agent = info.map(|i| i.agent.clone()).filter(|a| !a.is_empty())
            .unwrap_or_else(|| if self.pickers.agents.iter().any(|a| a == "build") || self.pickers.agents.is_empty() { "build".into() } else { self.pickers.agents[0].clone() });
        let model = info.map(|i| i.model.clone()).filter(|m| !m.id.is_empty())
            .or_else(|| self.pickers.agent_models.get(&agent).cloned())
            .or_else(|| self.sessions.first().map(|s| s.model.clone()))
            .unwrap_or_default();
        Selection { model, agent }
    }

    pub fn selection(&self) -> Selection {
        self.pickers.selections.get(self.selection_key()).cloned().unwrap_or_else(|| self.durable())
    }

    /// What the next prompt switches the session to: a pick that differs from what it runs
    /// with. An agent switch can move the model, so the model goes with it, as Desktop sends it.
    /// A new session takes what the toolbar shows, so it starts as it says.
    pub fn picked_choice(&self) -> (Option<String>, Option<ModelRef>) {
        let draft = !matches!(self.route, Route::Session(_));
        let picked = match self.pickers.selections.get(self.selection_key()) {
            Some(p) => p.clone(),
            None if draft => self.durable(),
            None => return (None, None),
        };
        let now = self.durable();
        let agent = (draft || picked.agent != now.agent).then(|| picked.agent.clone());
        let model = (agent.is_some() || picked.model != now.model).then(|| picked.model.clone()).filter(|m| !m.id.is_empty());
        (agent, model)
    }

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

    pub fn picker_items(&self) -> Vec<Item> {
        let selection = self.selection();
        match self.pickers.open {
            Some(Kind::Model) => self.model_items(&selection),
            Some(Kind::Agent) => self.pickers.agents.iter().map(|a| Item::new(a.clone(), capitalize(a))).collect(),
            Some(Kind::Effort) => {
                let variants = self.pickers.models.iter().find(|m| m.id == selection.model.id && m.provider == selection.model.provider);
                std::iter::once(Item::new("default", "Default"))
                    .chain(variants.into_iter().flat_map(|m| &m.variants).map(|v| Item::new(v.clone(), capitalize(v)))).collect()
            }
            Some(Kind::Project) => self
                .projects
                .iter()
                .map(|p| Item::new(p.id.clone(), p.name.clone()))
                .chain(std::iter::once(Item::new(ADD_PROJECT_KEY, "Add project")))
                .collect(),
            None => vec![],
        }
    }

    /// The model list, as Desktop's: with no search, Favorites then Recent lead it (with their
    /// provider after the name) and their models leave their provider's group.
    fn model_items(&self, selection: &Selection) -> Vec<Item> {
        let q = self.pickers.search.text.to_lowercase();
        let current = format!("{}:{}", selection.model.provider, selection.model.id);
        let item = |m: &Model, group: &str, note: bool| Item {
            key: m.key.clone(), label: m.name.clone(), group: group.to_owned(),
            note: if note { m.group.clone() } else { String::new() }, free: m.free,
        };
        if !q.is_empty() {
            return self.pickers.models.iter()
                .filter(|m| format!("{} {} {}", m.name, m.group, m.id).to_lowercase().contains(&q))
                .map(|m| item(m, &m.group, false)).collect();
        }
        let prefs = &self.pickers.prefs;
        let find = |k: &String| self.pickers.models.iter().find(|m| &m.key == k);
        let favorites: Vec<&Model> = prefs.favorites.iter().filter_map(find).collect();
        let recent: Vec<&Model> = prefs.recent.iter().filter(|k| !prefs.is_favorite(k)).filter_map(find).collect();
        let lead = |m: &Model| favorites.iter().chain(&recent).any(|x| x.key == m.key);
        favorites.iter().map(|m| item(m, FAVORITES, true))
            .chain(recent.iter().map(|m| item(m, RECENT, true)))
            .chain(self.pickers.models.iter().filter(|m| (!m.hidden || m.key == current) && !lead(m)).map(|m| item(m, &m.group, false)))
            .collect()
    }

    /// Stars or unstars a model (⌘F on the highlighted row, or its star).
    /// The effort the model `key` would run with if picked now: "" for its default, or none
    /// when it has no efforts to choose.
    pub fn model_effort(&self, key: &str) -> Option<String> {
        let m = self.pickers.models.iter().find(|m| m.key == key).filter(|m| !m.variants.is_empty())?;
        let s = self.selection();
        let current = s.model.id == m.id && s.model.provider == m.provider;
        Some(self.pickers.staged.get(key).cloned()
            .or_else(|| current.then(|| s.model.variant.clone()))
            .or_else(|| self.pickers.efforts.get(key).filter(|v| m.variants.contains(v)).cloned())
            .unwrap_or_default())
    }

    /// Moves the model `key`'s staged effort `step` along Default and its efforts; false when it
    /// has none.
    pub fn step_effort(&mut self, key: &str, step: i64) -> bool {
        let Some(now) = self.model_effort(key) else { return false };
        let Some(m) = self.pickers.models.iter().find(|m| m.key == key) else { return false };
        let options: Vec<String> = std::iter::once(String::new()).chain(m.variants.iter().cloned()).collect();
        let at = options.iter().position(|v| *v == now);
        let next = options[cycle(options.len(), at, step)].clone();
        self.pickers.staged.insert(key.to_owned(), next);
        true
    }

    pub fn toggle_favorite(&mut self, key: &str) {
        self.pickers.prefs.toggle_favorite(key);
        // The row moves between groups; the highlight goes with it, and typing still searches.
        self.pickers.activate(key.to_owned());
        self.pickers.search.focused = self.pickers.open == Some(Kind::Model);
    }

    /// The project a new session would start in: the one whose folder holds the draft's.
    pub fn draft_project(&self) -> Option<&opencode_kit::model::Project> {
        self.project_at(&self.draft_dir)
    }

    /// Whether the selected model has an effort to choose: Desktop shows the picker (with
    /// "Default" for no choice) for any model that has variants.
    pub fn has_effort(&self) -> bool {
        let m = self.selection().model;
        self.pickers.models.iter().find(|x| x.id == m.id && x.provider == m.provider).is_some_and(|x| !x.variants.is_empty())
    }

    pub fn picker_current(&self, kind: Kind) -> String {
        let s = self.selection();
        match kind {
            Kind::Model => format!("{}:{}", s.model.provider, s.model.id),
            Kind::Agent => s.agent,
            Kind::Effort => if s.model.variant.is_empty() { "default".into() } else { s.model.variant },
            Kind::Project => self.draft_project().map_or_else(String::new, |p| p.id.clone()),
        }
    }

    pub fn picker_active(&self) -> Option<String> {
        let items = self.picker_items();
        self.pickers.active.resolve(&self.pickers.search.text, items.iter().map(|i| i.key.as_str()))
    }

    pub fn open_picker(&mut self, kind: Kind) {
        if self.pickers.open == Some(kind) { self.close_picker(true); return; }
        self.pickers.open = Some(kind);
        self.pickers.search = TextEdit::new("");
        self.pickers.search.focused = kind == Kind::Model;
        self.pickers.staged.clear();
        let current = self.picker_current(kind);
        self.pickers.active.set(current, String::new());
        self.composer.focused = false;
        self.pickers.toolbar_focus = None;
    }

    pub fn close_picker(&mut self, restore: bool) {
        self.pickers.open = None;
        self.pickers.search.focused = false;
        self.pickers.search.preedit = None;
        self.pickers.search.break_history_group();
        self.composer.focused = restore;
    }

    pub fn choose_picker(&mut self, key: &str) {
        self.choose_picker_tx(key, &mut Tx::default());
    }

    pub fn choose_picker_tx(&mut self, key: &str, tx: &mut Tx) {
        // Never accept a stale, filtered-out item or a non-option (including empty results).
        if !self.picker_items().iter().any(|i| i.key == key) { return; }
        if self.pickers.open == Some(Kind::Project) {
            if key == ADD_PROJECT_KEY {
                self.open_project_dialog(tx);
                return;
            }
            if let Some(p) = self.projects.iter().find(|p| p.id == key).cloned() {
                self.draft_dir = p.directory;
                self.touch_project(&p.id);
            }
            self.close_picker(true);
            return;
        }
        let mut selected = self.selection();
        let session = self.selection_key().to_owned();
        match self.pickers.open {
            Some(Kind::Model) => {
                let Some(m) = self.pickers.models.iter().find(|m| m.key == key) else { return };
                self.pickers.prefs.picked(key);
                let staged = self.pickers.staged.get(key).cloned();
                if m.id != selected.model.id || m.provider != selected.model.provider {
                    let variant = staged.clone().or_else(|| self.pickers.efforts.get(key).filter(|v| m.variants.contains(v)).cloned()).unwrap_or_default();
                    selected.model = ModelRef { id: m.id.clone(), provider: m.provider.clone(), variant };
                } else if let Some(v) = staged.clone() {
                    selected.model.variant = v;
                }
                if let Some(v) = staged { self.pickers.efforts.insert(key.to_owned(), v); }
            }
            Some(Kind::Agent) => {
                if selected.agent != key {
                    let remembered = self.pickers.choices.get(&session).and_then(|c| c.get(key));
                    if let Some(m) = remembered.or_else(|| self.pickers.agent_models.get(key)) { selected.model = m.clone(); }
                    selected.agent = key.into();
                }
            }
            Some(Kind::Effort) => {
                selected.model.variant = if key == "default" { String::new() } else { key.into() };
                self.pickers.efforts.insert(format!("{}:{}", selected.model.provider, selected.model.id), selected.model.variant.clone());
            }
            Some(Kind::Project) | None => return,
        }
        self.pickers.choices.entry(session.clone()).or_default().insert(selected.agent.clone(), selected.model.clone());
        let live = matches!(self.route, Route::Session(_));
        if live && selected == self.durable() { self.pickers.selections.remove(&session); } else { self.pickers.selections.insert(session, selected); }
        self.close_picker(true);
    }

    /// A picker's keys. Opening it and moving in it land in the keystroke. (Not closing: an
    /// instant unmount of the focused search field drops the focus handed back to the composer.)
    pub fn picker_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let used = self.picker_keys(ev, tx);
        if used && self.pickers.open.is_some() { tx.instant(); }
        used
    }

    fn picker_keys(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.pickers.open.is_some() {
            match nav(ev) {
                Some(Nav::Close) => self.close_picker(true),
                Some(Nav::Leave) => {
                    let kind = self.pickers.open.filter(|k| *k != Kind::Project);
                    self.close_picker(kind.is_none());
                    self.pickers.toolbar_focus = kind;
                }
                Some(Nav::Step(step)) => {
                    if self.pickers.search.preedit.is_some() { return false; }
                    let items = self.picker_items();
                    if !items.is_empty() {
                        let active = self.picker_active();
                        let at = items.iter().position(|i| Some(&i.key) == active.as_ref());
                        self.pickers.activate(items[cycle(items.len(), at, step)].key.clone());
                    }
                }
                Some(Nav::Choose) => {
                    if self.pickers.search.preedit.is_none() && let Some(key) = self.picker_active() { self.choose_picker_tx(&key, tx); }
                }
                None if ev.is_command('f') && self.pickers.open == Some(Kind::Model) && !ev.mods.shift => {
                    if let Some(key) = self.picker_active() { self.toggle_favorite(&key); }
                }
                // As in Desktop's list: with no query, ← → change the lit model's effort.
                None if matches!(ev.key, Key::Left | Key::Right) && ev.mods == Default::default()
                    && self.pickers.open == Some(Kind::Model) && self.pickers.search.text.is_empty() => {
                    let Some(key) = self.picker_active() else { return false };
                    if !self.step_effort(&key, if ev.key == Key::Right { 1 } else { -1 }) { return false; }
                }
                None => return self.pickers.open != Some(Kind::Model),
            }
            return true;
        }
        if self.composer.focused { self.pickers.toolbar_focus = None; }
        if ev.key == Key::Tab && (self.composer.focused || self.pickers.toolbar_focus.is_some()) {
            let order: Vec<Kind> = [Kind::Agent, Kind::Model, Kind::Effort].into_iter()
                .filter(|k| match k { Kind::Agent => self.settings.show_agent, Kind::Effort => self.has_effort(), _ => true }).collect();
            // The composer is the last stop after the pickers.
            let n = order.len();
            let next = self.pickers.toolbar_focus.and_then(|k| order.iter().position(|i| *i == k))
                .map_or(if ev.mods.shift { n - 1 } else { 0 }, |i| if ev.mods.shift { (i + n) % (n + 1) } else { i + 1 });
            self.pickers.toolbar_focus = order.get(next).copied();
            self.composer.focused = next == n;
            return true;
        }
        if matches!(ev.key, Key::Enter | Key::Down | Key::Up)
            && let Some(kind) = self.pickers.toolbar_focus { self.open_picker(kind); return true; }
        if ev.key == Key::Command('\'') { self.open_picker(Kind::Model); return true; }
        let cycle = match ev.key {
            Key::Command('.') => Some((Kind::Agent, ev.mods.shift)),
            Key::Command('d') if ev.mods.shift && self.has_effort() => Some((Kind::Effort, false)),
            _ => None,
        };
        if let Some((kind, backwards)) = cycle {
            self.open_picker(kind);
            self.picker_keys(&KeyEvent { key: if backwards { Key::Up } else { Key::Down }, mods: Default::default() }, tx);
            if let Some(key) = self.picker_active() { self.choose_picker_tx(&key, tx); }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let menu = find(&rt, "picker-model").unwrap();
            assert!((menu.opacity - 1.0).abs() < 1e-3, "opens without a fade (reduced={reduced})");
            for part in ["picker-search", "picker-options", "option-google:gemini-4-argon"] {
                let e = find(&rt, part).unwrap();
                assert!((e.opacity - 1.0).abs() < 1e-3, "{part} is there at once: {}", e.opacity);
            }
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
#[path = "pickers_tests.rs"]
mod live_tests;
