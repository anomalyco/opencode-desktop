//! What a host tells the pickers, and what they do with it: the selection, the lists, picking,
//! and the keys.

use opencode_kit::model::{ModelRef, capitalize};
use rune::{El, Key, KeyEvent, TextEdit, Tx};

use super::{ADD_PROJECT_KEY, FAVORITES, Item, Kind, Model, ModelAction, Pickers, RECENT, Selection, grouped, hover, search};
use super::hover::ModelHover;
use crate::composer::ComposerHost;
use crate::palette::Pal;
use crate::popover::{self as pop, Nav, cycle, nav};

/// Host hooks for the agent, model and effort pickers. The provided methods are the pickers'
/// behaviour; a host overrides only the hooks above them.
pub trait PickerHost: ComposerHost {
    fn pickers(&self) -> &Pickers;
    fn pickers_mut(&mut self) -> &mut Pickers;

    /// Where picks are kept: the open session's id, or "draft" for a new session.
    fn picker_scope(&self) -> String;
    /// A new session, which starts with what the toolbar shows.
    fn picker_draft(&self) -> bool {
        false
    }
    /// What the server last said the open session runs with (either part may be empty).
    fn picker_running(&self) -> Option<Selection>;
    /// The model when neither the session nor its agent names one.
    fn picker_fallback_model(&self) -> Option<ModelRef> {
        None
    }
    /// On a subagent's page: its session id and what it runs with. A reply to it always names
    /// its agent and model, so it carries on as itself.
    fn picker_subagent(&self) -> Option<(String, Selection)> {
        None
    }
    /// Whether the toolbar shows the agent's picker.
    fn picker_show_agent(&self) -> bool {
        true
    }
    /// A key the model search field doesn't use, for the host's own shortcuts.
    fn picker_search_key(&mut self, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
        false
    }

    /// The buttons beside the model search, in order: Manage models opens here; a host that can
    /// connect a provider puts Connect first.
    fn picker_model_actions(&self) -> Vec<ModelAction> {
        vec![ModelAction::Manage]
    }
    /// Connect was pressed; the list has closed.
    fn picker_model_action(&mut self, _action: ModelAction, _tx: &mut Tx) {}

    /// A provider's mark beside its name in the narrow window's model drawer.
    fn picker_provider_mark(&self, _p: &Pal, _provider: &str) -> Option<El> {
        None
    }

    /// The project picker's rows (a host without one lists none).
    fn picker_projects(&self) -> Vec<Item> {
        Vec::new()
    }
    fn picker_project_current(&self) -> String {
        String::new()
    }
    fn picker_choose_project(&mut self, _key: &str, _tx: &mut Tx) {}
    fn picker_project_lead(&self, _p: &Pal, _key: &str, _label: &str) -> Option<El> {
        None
    }

    /// What the session runs with now: the agent and model the server last said, or for a new
    /// session the first agent with its configured model.
    fn durable(&self) -> Selection {
        let pk = self.pickers();
        let info = self.picker_running();
        let agent = info.as_ref().map(|i| i.agent.clone()).filter(|a| !a.is_empty())
            .unwrap_or_else(|| if pk.agents.iter().any(|a| a == "build") || pk.agents.is_empty() { "build".into() } else { pk.agents[0].clone() });
        let model = info.map(|i| i.model).filter(|m| !m.id.is_empty())
            .or_else(|| pk.agent_models.get(&agent).cloned())
            .or_else(|| self.picker_fallback_model())
            .unwrap_or_default();
        Selection { model, agent }
    }

    fn selection(&self) -> Selection {
        self.pickers().selections.get(&self.picker_scope()).cloned().unwrap_or_else(|| self.durable())
    }

    /// What the next prompt switches the session to: a pick that differs from what it runs
    /// with. An agent switch can move the model, so the model goes with it, as Desktop sends it.
    /// A new session takes what the toolbar shows, so it starts as it says.
    fn picked_choice(&self) -> (Option<String>, Option<ModelRef>) {
        if let Some((id, info)) = self.picker_subagent() {
            let picked = self.pickers().selections.get(&id);
            let agent = picked.map(|p| p.agent.clone()).unwrap_or(info.agent);
            let model = picked.map(|p| p.model.clone()).unwrap_or(info.model);
            return (Some(agent).filter(|a| !a.is_empty()), Some(model).filter(|m| !m.id.is_empty()));
        }
        let draft = self.picker_draft();
        let picked = match self.pickers().selections.get(&self.picker_scope()) {
            Some(p) => p.clone(),
            None if draft => self.durable(),
            None => return (None, None),
        };
        let now = self.durable();
        let agent = (draft || picked.agent != now.agent).then(|| picked.agent.clone());
        let model = (agent.is_some() || picked.model != now.model).then(|| picked.model.clone()).filter(|m| !m.id.is_empty());
        (agent, model)
    }

    fn picker_items(&self) -> Vec<Item> {
        let selection = self.selection();
        let pk = self.pickers();
        match pk.open {
            Some(Kind::Model) => model_items(pk),
            Some(Kind::Agent) => pk.agents.iter().map(|a| Item::new(a.clone(), capitalize(a))).collect(),
            Some(Kind::Effort) => {
                let variants = pk.models.iter().find(|m| m.id == selection.model.id && m.provider == selection.model.provider);
                std::iter::once(Item::new("default", opencode_i18n::t!("common.default")))
                    .chain(variants.into_iter().flat_map(|m| &m.variants).map(|v| Item::new(v.clone(), capitalize(v)))).collect()
            }
            Some(Kind::Project) => self.picker_projects(),
            None => vec![],
        }
    }

    /// The effort the model `key` would run with if picked now: "" for its default, or none
    /// when it has no efforts to choose.
    fn model_effort(&self, key: &str) -> Option<String> {
        let pk = self.pickers();
        let m = pk.models.iter().find(|m| m.key == key).filter(|m| !m.variants.is_empty())?;
        let s = self.selection();
        let current = s.model.id == m.id && s.model.provider == m.provider;
        Some(pk.staged.get(key).cloned()
            .or_else(|| current.then(|| s.model.variant.clone()))
            .or_else(|| pk.efforts.get(key).filter(|v| m.variants.contains(v)).cloned())
            // A session can say "default" for no choice, as the effort trigger reads it.
            .filter(|v| v != "default")
            .unwrap_or_default())
    }

    /// Moves the model `key`'s staged effort `step` along Default and its efforts; false when it
    /// has none.
    fn step_effort(&mut self, key: &str, step: i64) -> bool {
        let Some(now) = self.model_effort(key) else { return false };
        let Some(m) = self.pickers().models.iter().find(|m| m.key == key) else { return false };
        let options: Vec<String> = std::iter::once(String::new()).chain(m.variants.iter().cloned()).collect();
        let at = options.iter().position(|v| *v == now);
        let next = options[cycle(options.len(), at, step)].clone();
        self.pickers_mut().stage_effort(key, &next);
        true
    }

    /// Stars or unstars a model (⌘F on the highlighted row, or Favorite in its flyout).
    fn toggle_favorite(&mut self, key: &str) {
        let pk = self.pickers_mut();
        pk.prefs.toggle_favorite(key);
        // The row moves between groups; the highlight goes with it, and typing still searches.
        pk.activate(key.to_owned());
        pk.search.focused = pk.open == Some(Kind::Model);
    }

    /// Whether the selected model has an effort to choose: Desktop shows the picker (with
    /// "Default" for no choice) for any model that has variants.
    fn has_effort(&self) -> bool {
        let m = self.selection().model;
        self.pickers().models.iter().find(|x| x.id == m.id && x.provider == m.provider).is_some_and(|x| !x.variants.is_empty())
    }

    fn picker_current(&self, kind: Kind) -> String {
        let s = self.selection();
        match kind {
            Kind::Model => format!("{}:{}", s.model.provider, s.model.id),
            Kind::Agent => s.agent,
            Kind::Effort => if s.model.variant.is_empty() { "default".into() } else { s.model.variant },
            Kind::Project => self.picker_project_current(),
        }
    }

    fn picker_active(&self) -> Option<String> {
        let items = self.picker_items();
        let pk = self.pickers();
        pk.active.resolve(&pk.search.text, items.iter().map(|i| i.key.as_str()))
    }

    fn open_picker(&mut self, kind: Kind) {
        if self.pickers().open == Some(kind) { self.close_picker(true); return; }
        let pk = self.pickers_mut();
        pk.open = Some(kind);
        pk.search = TextEdit::new("");
        pk.search.focused = kind == Kind::Model;
        pk.staged.clear();
        pk.hover = hover::Hover::fresh();
        // The model list starts on its first row, as Desktop's `SearchList` does
        // (`typeahead/controller.ts:22`); a menu starts on what's picked.
        if kind == Kind::Model {
            pk.active.clear();
        } else {
            let current = self.picker_current(kind);
            self.pickers_mut().active.set(current, String::new());
        }
        self.composer_edit_mut().focused = false;
    }

    fn close_picker(&mut self, restore: bool) {
        let pk = self.pickers_mut();
        pk.open = None;
        pk.hover = hover::Hover::fresh();
        pk.search.focused = false;
        pk.search.cancel_composition();
        pk.search.break_history_group();
        self.composer_edit_mut().focused = restore;
    }

    fn choose_picker(&mut self, key: &str) {
        self.choose_picker_tx(key, &mut Tx::default());
    }

    fn choose_picker_tx(&mut self, key: &str, tx: &mut Tx) {
        // Never accept a stale, filtered-out item or a non-option (including empty results).
        if !self.picker_items().iter().any(|i| i.key == key) { return; }
        if self.pickers().open == Some(Kind::Project) {
            if key != ADD_PROJECT_KEY { self.close_picker(true); }
            self.picker_choose_project(key, tx);
            return;
        }
        let mut selected = self.selection();
        let session = self.picker_scope();
        let pk = self.pickers_mut();
        match pk.open {
            Some(Kind::Model) => {
                let Some(m) = pk.models.iter().find(|m| m.key == key).cloned() else { return };
                pk.prefs.picked(key);
                let staged = pk.staged.get(key).cloned();
                if m.id != selected.model.id || m.provider != selected.model.provider {
                    let variant = staged.clone().or_else(|| pk.efforts.get(key).filter(|v| m.variants.contains(v)).cloned()).unwrap_or_default();
                    selected.model = ModelRef { id: m.id.clone(), provider: m.provider.clone(), variant };
                } else if let Some(v) = staged.clone() {
                    selected.model.variant = v;
                }
                if let Some(v) = staged { pk.efforts.insert(key.to_owned(), v); }
            }
            Some(Kind::Agent) => {
                if selected.agent != key {
                    let remembered = pk.choices.get(&session).and_then(|c| c.get(key));
                    if let Some(m) = remembered.or_else(|| pk.agent_models.get(key)) { selected.model = m.clone(); }
                    selected.agent = key.into();
                }
            }
            Some(Kind::Effort) => {
                selected.model.variant = if key == "default" { String::new() } else { key.into() };
                pk.efforts.insert(format!("{}:{}", selected.model.provider, selected.model.id), selected.model.variant.clone());
            }
            Some(Kind::Project) | None => return,
        }
        pk.choices.entry(session.clone()).or_default().insert(selected.agent.clone(), selected.model.clone());
        let live = !self.picker_draft();
        let durable = self.durable();
        let pk = self.pickers_mut();
        if live && selected == durable { pk.selections.remove(&session); } else { pk.selections.insert(session, selected); }
        tx.color_motion(rune::Spring::NONE);
        self.close_picker(true);
    }

    /// A picker's keys. Opening it (unless it has an entrance to play) and moving in it land in
    /// the keystroke. (Not closing: an instant unmount of the focused search field drops the focus
    /// handed back to the composer.)
    fn picker_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let was = self.pickers().open;
        let used = picker_keys(self, ev, tx);
        if used && self.pickers().open.is_some() {
            let open = self.pickers().open;
            if open == was || open == Some(Kind::Model) { tx.instant(); } else { pop::motion::opening(self.knobs(), tx); }
        } else if used && was.is_some() {
            // The trigger lets go of its open look at once, as Desktop's button-v2 has no transition.
            tx.color_motion(rune::Spring::NONE);
        }
        used
    }

    /// ⌘. and ⇧⌘D: the next (or previous) agent or variant, picked without showing the list.
    fn cycle_picker(&mut self, kind: Kind, backwards: bool, tx: &mut Tx) {
        self.open_picker(kind);
        picker_keys(self, &KeyEvent { key: if backwards { Key::Up } else { Key::Down }, mods: Default::default() }, tx);
        if let Some(key) = self.picker_active() { self.choose_picker_tx(&key, tx); }
    }
}

/// The model list, as Desktop's (`select-dialog.tsx:561-585`, `:654-665`): with no search,
/// Favorites then Recent lead it, each only when it has a model (with their provider after the
/// name), and their models leave their provider's group, so none is listed twice. Recent is
/// the models picked lately, nothing more.
///
/// A search lists what it finds, best first and ties by name as Desktop lists them, each
/// provider's models together: [`search::score`] finds a typo as well as a word (t1389).
///
/// With no search, each provider's shown models go by name and the providers in Desktop's
/// order (`select-dialog.tsx:85-98`, `:496`): its popular ones first, then the rest by name.
fn model_items(pk: &Pickers) -> Vec<Item> {
    // Rows are grouped by the provider's id; the header says its name.
    let item = |m: &Model, group: &str, heading: &str, note: bool| Item {
        key: m.key.clone(), label: m.name.clone(), group: group.to_owned(), heading: heading.to_owned(),
        note: if note { m.group.clone() } else { String::new() }, free: m.free, latest: m.latest,
    };
    if !pk.search.text.trim().is_empty() {
        let mut hits: Vec<(i64, Item)> = pk.models.iter()
            .filter(|m| pk.visible(m))
            .filter_map(|m| {
                let fields = [m.name.as_str(), m.group.as_str(), m.provider.as_str(), m.id.as_str()];
                search::score(&pk.search.text, &fields).map(|score| (score, item(m, &m.provider, &m.group, false)))
            })
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.to_lowercase().cmp(&b.1.label.to_lowercase())));
        return grouped(hits.into_iter().map(|(_, item)| item).collect());
    }
    let prefs = &pk.prefs;
    let find = |k: &String| pk.models.iter().find(|m| &m.key == k);
    let favorites: Vec<&Model> = prefs.favorites.iter().filter_map(find).collect();
    let recent: Vec<&Model> = prefs.recent.iter().filter(|k| !prefs.is_favorite(k)).filter_map(find).collect();
    let lead = |m: &Model| favorites.iter().chain(&recent).any(|x| x.key == m.key);
    let mut rest: Vec<&Model> = pk.models.iter().filter(|m| pk.visible(m) && !lead(m)).collect();
    rest.sort_by(|a, b| provider_rank(a).cmp(&provider_rank(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    grouped(favorites.iter().map(|m| item(m, FAVORITES, opencode_i18n::t!("dialog.model.favorites"), true))
        .chain(recent.iter().map(|m| item(m, RECENT, opencode_i18n::t!("dialog.model.recent"), true)))
        .chain(rest.into_iter().map(|m| item(m, &m.provider, &m.group, false)))
        .collect())
}

/// Desktop's `popularProviders` (`providers/catalog/order.ts`), which lead the list in this order.
const POPULAR: [&str; 8] = ["opencode-go", "opencode", "anthropic", "github-copilot", "openai", "google", "openrouter", "vercel"];

/// Where a model's provider goes: a popular one by its place, the rest after them by name.
pub(super) fn provider_rank(m: &Model) -> (usize, String) {
    match POPULAR.iter().position(|p| *p == m.provider) {
        Some(at) => (at, String::new()),
        None => (POPULAR.len(), m.group.to_lowercase()),
    }
}

fn picker_keys<S: PickerHost>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if let Some(open) = s.pickers().open {
        match nav(ev) {
            Some(Nav::Close) => s.close_picker(true),
            // In the model dialog Tab moves on to Connect and Manage, as Desktop's does.
            Some(Nav::Leave) if open == Kind::Model => return false,
            Some(Nav::Leave) => {
                // Tab leaves the list on its trigger, as a Kobalte select returns focus there.
                let kind = Some(open).filter(|k| *k != Kind::Project);
                s.close_picker(kind.is_none());
                if let Some(kind) = kind { tx.focus(kind.key(), true); }
            }
            Some(Nav::Step(step)) => {
                if s.pickers().search.is_composing() { return false; }
                let items = s.picker_items();
                if !items.is_empty() {
                    let active = s.picker_active();
                    let at = items.iter().position(|i| Some(&i.key) == active.as_ref());
                    let key = items[cycle(items.len(), at, step)].key.clone();
                    s.model_list_keyed();
                    s.pickers_mut().hover.navigated = true;
                    tx.scroll_into_view_in("picker-scroll", format!("option-{key}"));
                    s.pickers_mut().activate(key);
                }
            }
            Some(Nav::Choose) => {
                if s.pickers().search.preedit().is_none() && let Some(key) = s.picker_active() { s.choose_picker_tx(&key, tx); }
            }
            None if crate::keymap::Bind::Favorite.matches(ev) && open == Kind::Model => {
                if let Some(key) = s.picker_active() { s.toggle_favorite(&key); }
                s.model_list_keyed();
            }
            // As in Desktop's list: ← → change the lit model's effort, or while searching edit
            // the query until the arrows have moved through the results (`select-dialog.tsx:897-908`).
            None if matches!(ev.key, Key::Left | Key::Right) && ev.mods == Default::default()
                && open == Kind::Model && (s.pickers().search.text.is_empty() || s.pickers().hover.navigated) => {
                let Some(key) = s.picker_active() else { return false };
                // Right to left, ← is forward (`select-dialog.tsx:904`).
                let step = match (ev.key == Key::Right, opencode_i18n::direction()) {
                    (true, opencode_i18n::Direction::Ltr) | (false, opencode_i18n::Direction::Rtl) => 1,
                    (false, opencode_i18n::Direction::Ltr) | (true, opencode_i18n::Direction::Rtl) => -1,
                };
                if !s.step_effort(&key, step) { return false; }
                s.model_list_keyed();
            }
            None => return open != Kind::Model,
        }
        return true;
    }
    // A focused trigger opens its list on the arrows too, as a Kobalte select's does; Tab moves
    // through them as Rune's document order has it, and Return and Space press them.
    let toolbar_focus = [Kind::Agent, Kind::Model, Kind::Effort].into_iter().find(|k| tx.focused() == Some(k.key()));
    if matches!(ev.key, Key::Down | Key::Up) && ev.mods == Default::default()
        && let Some(kind) = toolbar_focus { s.open_picker(kind); return true; }
    use crate::keymap::Bind;
    if Bind::Model.matches_tx(ev, tx) { s.open_picker(Kind::Model); return true; }
    let cycle = match () {
        _ if Bind::Agent.matches_tx(ev, tx) => Some((Kind::Agent, false)),
        _ if Bind::AgentBack.matches_tx(ev, tx) => Some((Kind::Agent, true)),
        _ if Bind::Variant.matches_tx(ev, tx) && s.has_effort() => Some((Kind::Effort, false)),
        _ => None,
    };
    if let Some((kind, backwards)) = cycle {
        s.cycle_picker(kind, backwards, tx);
        return true;
    }
    false
}
