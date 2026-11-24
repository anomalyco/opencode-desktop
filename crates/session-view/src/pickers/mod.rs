//! The composer's agent, model and effort. Desktop reference: `DialogSelectModel`
//! (`providers/models/select-dialog.tsx`, [`dialog`]), ComposerEditorSelect and
//! `providers/models/selection.tsx`, with its model search in
//! `providers/models/search.ts` ([`search`]). A session runs with what the
//! server last said (`session.agent`, `session.model`); a pick is remembered per session and
//! rides on its next prompt, which switches the session first (`Server::send`). Once the server
//! says it runs with the pick, the memory goes, as Desktop's `reconcile` does.
//!
//! The state is [`Pickers`]; a host implements [`PickerHost`] for the session it shows, mounts
//! [`view::layer`] over its window and calls [`hover::poll`] each frame.

pub mod dialog;
pub mod drawer;
pub mod flyout;
mod host;
pub mod hover;
pub mod manage;
pub use opencode_ui::provider_mark as mark;
pub mod menu;
pub mod prefs;
pub mod speed;
pub use opencode_search::model_search as search;
pub mod view;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub use opencode_kit::model::{AttachmentInput, Model};
use opencode_kit::model::ModelRef;
use rune::TextEdit;
use serde_json::Value;

use crate::popover::Highlight;

pub use flyout::EffortFlyout;
pub use host::PickerHost;
pub use hover::ModelHover;
pub use prefs::ModelPrefs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Agent, Model, Effort, Speed, Project }

pub const ADD_PROJECT_KEY: &str = "+add";

/// The model list's buttons beside its search (`select-dialog.tsx:1010-1034`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelAction {
    /// `+`: Connect an integration.
    Connect,
    /// The sliders: Manage models.
    Manage,
}

impl ModelAction {
    pub fn label(self) -> &'static str {
        match self { Self::Connect => opencode_i18n::t!("command.provider.connect"), Self::Manage => opencode_i18n::t!("dialog.model.manage") }
    }
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self { Self::Agent => "agent", Self::Model => "model", Self::Effort => "effort", Self::Speed => "speed", Self::Project => "project" }
    }

    /// Its trigger's and menu's name: Choose model, Choose agent, Choose model variant, Choose model speed.
    pub fn choose_label(self) -> &'static str {
        match self {
            Self::Agent => opencode_i18n::t!("ui.promptInput.chooseAgent"),
            Self::Model => opencode_i18n::t!("ui.promptInput.chooseModel"),
            Self::Effort => opencode_i18n::t!("ui.promptInput.chooseVariant"),
            Self::Speed => opencode_i18n::t!("ui.promptInput.chooseSpeed"),
            Self::Project => opencode_i18n::t!("session.new.project.trigger.tooltip"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selection { pub model: ModelRef, pub agent: String }

/// Picks by session ("draft" for a new one), each installed as its own edit: picking again
/// starts a new one even when it revisits the same model and effort, so a send can release the
/// edit it carried and leave a later pick, or the same pick again, alone.
#[derive(Default)]
pub struct Selections(HashMap<String, Arc<Selection>>);

impl Selections {
    pub fn insert(&mut self, scope: String, pick: Selection) {
        self.0.insert(scope, Arc::new(pick));
    }

    pub fn get(&self, scope: &str) -> Option<&Selection> {
        self.0.get(scope).map(Arc::as_ref)
    }

    pub fn remove(&mut self, scope: &str) {
        self.0.remove(scope);
    }

    /// The edit a prompt carries, for its admission to release: only when the prompt names the
    /// agent or model this session's pick holds.
    pub fn submitted(&self, scope: &str, agent: Option<&str>, model: Option<&ModelRef>) -> Option<Arc<Selection>> {
        let pick = self.0.get(scope)?;
        ((agent.is_some() || model.is_some())
            && agent.is_none_or(|a| a == pick.agent)
            && model.is_none_or(|m| *m == pick.model))
            .then(|| Arc::clone(pick))
    }

    /// The edit `edit` is still the one held: the server runs with it, so it goes and the
    /// session follows changes made elsewhere. A later pick stays, even one that chose the same
    /// model again.
    pub fn admitted(&mut self, scope: &str, edit: &Arc<Selection>) -> bool {
        if !self.0.get(scope).is_some_and(|held| Arc::ptr_eq(held, edit)) {
            return false;
        }
        self.0.remove(scope);
        true
    }
}

#[derive(Clone, Default)]
pub struct Item {
    pub key: String,
    pub label: String,
    /// The group's identity, which keeps its rows under one header: a provider's id, or
    /// Favorites and Recent.
    pub group: String,
    /// What the header says where that differs from the identity: a provider's name.
    pub heading: String,
    /// Faint after the label: a model's provider in Favorites and Recent.
    pub note: String,
    pub free: bool,
    pub latest: bool,
}

impl Item {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Item {
        Item { key: key.into(), label: label.into(), ..Item::default() }
    }
}

pub const FAVORITES: &str = "Favorites";
pub const RECENT: &str = "Recent";

#[derive(Default)]
pub struct Pickers {
    pub models: Vec<Model>,
    /// Provider names by id from the server's `/api/provider` (`console-openai-test` is
    /// "Anomaly / RITB"): what a model's group header says, and what a search matches.
    pub providers: HashMap<String, String>,
    /// `/api/provider`'s ids in its order: the connected providers (`global-sync/utils.ts`
    /// `normalizeProviderList`).
    pub provider_order: Vec<String>,
    /// The `model` the config names (`/api/config`, `configured.ts`).
    pub configured: Option<ModelRef>,
    /// Context windows in tokens by `provider:id`, from the fixture or the server's `/api/model`.
    pub limits: HashMap<String, u64>,
    /// The `provider:id` of models whose tokens cost money (any `cost` tier above 0).
    pub paid: HashSet<String>,
    pub agents: Vec<String>,
    /// Each agent's configured model (`/api/agent`'s `model`), which it switches to when picked.
    pub agent_models: HashMap<String, ModelRef>,
    /// Explicit state machine for the open overlay (`Closed | Model | Menu`).
    pub menu: menu::State,
    pub search: TextEdit,
    pub(crate) active: Highlight,
    /// Picks by session id ("draft" for a new session), until the server runs with them.
    pub selections: Selections,
    /// The model each agent last had in a session, by session then agent, so switching back to
    /// an agent brings its model back (Desktop's `choices`).
    pub(crate) choices: HashMap<String, HashMap<String, ModelRef>>,

    /// Efforts chosen with ← → on a model's row in the open list, which picking it applies
    /// (Desktop's staged variant). Opening the list clears them.
    pub(crate) staged: HashMap<String, String>,
    /// The lit model's efforts beside its row: when they open on hover, and which option the
    /// pointer lights in them.
    pub hover: hover::Hover,
    pub prefs: ModelPrefs,
    /// Manage models, while it's open.
    pub manage: Option<manage::Manage>,
    manage_scratch: TextEdit,
    /// Whether the pointer is currently over the composer box (used to reveal the default effort
    /// chip without lighting on prompt keyboard focus, `editor.css:87`).
    pub composer_hovered: bool,
}

/// `items` with each group's rows together, groups in the order they first appear: the server
/// needn't list a provider's models side by side, and a group gets one heading.
fn grouped(mut items: Vec<Item>) -> Vec<Item> {
    let mut order: Vec<String> = Vec::new();
    for i in &items {
        if !order.contains(&i.group) { order.push(i.group.clone()); }
    }
    items.sort_by_key(|i| order.iter().position(|g| *g == i.group));
    items
}

fn wall_ms() -> i64 {
    // session-view does not depend on opencode-platform; rune::time::SystemTime is wasm32-safe for model release age checks.
    #[allow(clippy::disallowed_methods)]
    rune::time::SystemTime::now().duration_since(rune::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl Pickers {
    /// A session need not name its agent, so a pick and the server's confirmation compare
    /// through the same default.
    pub fn effective_agent<'a>(&'a self, agent: &'a str) -> &'a str {
        if !agent.is_empty() {
            return agent;
        }
        if self.agents.iter().any(|a| a == "build") {
            return "build";
        }
        self.agents.first().map_or("build", String::as_str)
    }

    pub(crate) fn manage_search_mut(&mut self) -> &mut TextEdit {
        match &mut self.manage {
            Some(m) => &mut m.search,
            None => &mut self.manage_scratch,
        }
    }

    /// Which picker overlay is currently open (`None` when `State::Closed`).
    pub fn open(&self) -> Option<Kind> {
        self.menu.kind()
    }

    pub fn load(&mut self, fixture: &Value) {
        self.models = fixture["models"].as_array().into_iter().flatten().map(|v| {
            let s = |key: &str| v[key].as_str().unwrap_or("").to_owned();
            Model { key: format!("{}:{}", s("provider"), s("id")), id: s("id"), provider: s("provider"),
                group: s("group"), name: s("name"), variants: v["variants"].as_array().into_iter().flatten()
                    .filter_map(Value::as_str).map(str::to_owned).collect(), hidden: false, free: false, latest: false,
                inputs: inputs(v), tools: v["capabilities"]["tools"].as_bool().unwrap_or(false), speed: speed::parse(&v["speed"]) }
        }).collect();
        speed::settle(&mut self.models);
        self.load_limits(&fixture["models"]);
        // A fixture may name its providers too, as the server's `/api/provider` does.
        self.load_providers(&fixture["providers"]);
        self.agents = fixture["agents"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    }

    /// The server's `/api/provider`: the name a model's group header says, where `/api/model`
    /// only names the provider by id. A provider the list leaves out, or names as its id, keeps
    /// its id; the groups stay keyed by the id. A list that names nothing leaves
    /// the headers as they are, and a fixture may give one too.
    pub fn load_providers(&mut self, providers: &Value) {
        self.provider_order = providers.as_array().into_iter().flatten().filter_map(|p| p["id"].as_str()).map(str::to_owned).collect();
        self.providers = providers.as_array().into_iter().flatten().filter_map(|p| {
            let (id, name) = (p["id"].as_str()?, p["name"].as_str()?);
            (!name.is_empty() && name != id).then(|| (id.to_owned(), name.to_owned()))
        }).collect();
        if self.providers.is_empty() {
            return;
        }
        // The provider list may land after the models do.
        for m in &mut self.models {
            m.group = self.providers.get(&m.provider).cloned().unwrap_or_else(|| m.provider.clone());
        }
    }

    /// The server's `/api/model` and `/api/agent` lists: variants are `{ id }` objects there.
    /// Deprecated models are left out, as Desktop leaves them out of its catalog.
    pub fn load_live(&mut self, models: &Value, agents: &Value) {
        self.load_models(models);
        self.load_agents(agents);
    }

    pub fn load_models(&mut self, models: &Value) {
        let now = wall_ms();
        let s = |v: &Value, key: &str| v[key].as_str().unwrap_or("").to_owned();
        let listed: Vec<&Value> = models.as_array().into_iter().flatten()
            .filter(|v| v["enabled"].as_bool().unwrap_or(true) && v["status"].as_str() != Some("deprecated")).collect();
        let shown = shown_models(&listed, now);
        let names = self.providers.clone();
        self.models = listed.iter().map(|v| {
            let (id, provider) = (s(v, "id"), s(v, "providerID"));
            let key = format!("{provider}:{id}");
            let name = s(v, "name");
            Model { speed: speed::parse(&v["experimental"]["speed"]), hidden: !shown.contains(&key), free: is_free(v), latest: name.contains("(latest)"), inputs: inputs(v), tools: v["capabilities"]["tools"].as_bool().unwrap_or(false),
                key, id, group: names.get(&provider).cloned().unwrap_or_else(|| provider.clone()), provider, name: name.replace("(latest)", "").trim().to_owned(),
                variants: v["variants"].as_array().into_iter().flatten().filter_map(|x| x["id"].as_str().or(x.as_str())).map(str::to_owned).collect() }
        }).collect();
        speed::settle(&mut self.models);
        self.load_limits(models);
    }

    pub fn load_agents(&mut self, agents: &Value) {
        let s = |v: &Value, key: &str| v[key].as_str().unwrap_or("").to_owned();
        let agents: Vec<&Value> = agents.as_array().into_iter().flatten()
            .filter(|a| a["mode"].as_str() != Some("subagent") && !a["hidden"].as_bool().unwrap_or(false)).collect();
        self.agents = agents.iter().filter_map(|a| a["id"].as_str()).map(str::to_owned).collect();
        self.agent_models = agents.iter().filter_map(|a| {
            let m = &a["model"];
            let model = ModelRef { id: s(m, "id"), provider: s(m, "providerID"), variant: s(m, "variant") };
            (!model.id.is_empty()).then(|| (s(a, "id"), model))
        }).collect();
    }

    /// The server's `/api/config` documents: the last that names a `model` decides, as
    /// "provider/model" or `{ providerID, model, variant }` (`configured.ts`).
    pub fn load_config(&mut self, config: &Value) {
        let list = config.as_array().or_else(|| config["data"].as_array());
        let last = list.into_iter().flatten().rfind(|e| e["type"] == "document" && !e["info"]["model"].is_null());
        self.configured = last.map(|e| &e["info"]["model"]).and_then(|m| match m.as_str() {
            Some(s) => s.split_once('/').map(|(provider, id)| ModelRef { id: id.to_owned(), provider: provider.to_owned(), variant: String::new() }),
            None => Some(ModelRef {
                id: m["model"].as_str().unwrap_or("").to_owned(),
                provider: m["providerID"].as_str().unwrap_or("").to_owned(),
                variant: m["variant"].as_str().unwrap_or("").to_owned(),
            }),
        });
    }

    /// Whether any connected provider beyond keyless free OpenCode Zen models is available
    /// (`providers/catalog/providers.ts:96-109`).
    pub fn has_paid_provider(&self) -> bool {
        let connected = |id: &str| self.provider_order.is_empty() || self.provider_order.iter().any(|p| p == id);
        self.provider_order.iter().any(|id| id != "opencode")
            || self.models.iter().any(|m| connected(&m.provider) && (m.provider != "opencode" || !m.free))
    }

    /// Desktop's model when neither the session nor its agent names one (`selection.tsx:366-404`):
    /// the configured model, else the most recent pick, else the first model of the first
    /// connected provider, each only while a connected provider lists it.
    pub fn fallback_model(&self) -> Option<ModelRef> {
        let connected = |provider: &str| self.provider_order.is_empty() || self.provider_order.iter().any(|p| p == provider);
        let find = |provider: &str, id: &str| self.models.iter().find(|m| m.provider == provider && m.id == id && connected(provider));
        let pick = |m: &Model| ModelRef { id: m.id.clone(), provider: m.provider.clone(), variant: String::new() };
        let configured = self.configured.clone().filter(|c| find(&c.provider, &c.id).is_some());
        configured
            .or_else(|| self.prefs.recent.iter().find_map(|k| k.split_once(':').and_then(|(p, id)| find(p, id))).map(pick))
            .or_else(|| match self.provider_order.is_empty() {
                true => self.models.first().map(pick),
                false => self.provider_order.iter().find_map(|p| self.models.iter().find(|m| m.provider == *p)).map(pick),
            })
    }

    /// Reads `limit.context` from a list of models, keyed by `provider` or `providerID`.
    pub fn load_limits(&mut self, models: &Value) {
        for m in models.as_array().into_iter().flatten() {
            let provider = m["providerID"].as_str().or(m["provider"].as_str()).unwrap_or("");
            if let (Some(id), Some(n)) = (m["id"].as_str(), m["limit"]["context"].as_u64()) {
                self.limits.insert(format!("{provider}:{id}"), n);
            }
            // Desktop's `showCost`: any tier's input, output or cache costs something.
            let costs = m["cost"].as_array().cloned().unwrap_or_else(|| vec![m["cost"].clone()]);
            let paid = costs.iter().any(|c| [&c["input"], &c["output"], &c["cache"]["read"], &c["cache"]["write"]].iter().any(|v| v.as_f64().is_some_and(|x| x > 0.0)));
            if let (Some(id), true) = (m["id"].as_str(), paid) {
                self.paid.insert(format!("{provider}:{id}"));
            }
        }
    }

    /// Lights the row `key` (Desktop's `list.setActive`).
    pub fn activate(&mut self, key: String) {
        self.active.set(key);
    }

    /// The pointer moved onto row `key` (`list-machine.ts` `pointer.move`).
    pub fn point(&mut self, key: String) {
        self.active.step(crate::popover::HighlightEvent::Pointer(key));
    }

    /// The search's text changed: a new query lights its first row (`list-machine.ts`
    /// `query.change`, whose answer opens on `keys[0]`).
    pub fn query_changed(&mut self) {
        self.active.step(crate::popover::HighlightEvent::QueryChanged);
    }

    /// Picking the model `key` now runs it with `effort` ("" for its default).
    pub fn stage_effort(&mut self, key: &str, effort: &str) {
        self.staged.insert(key.to_owned(), effort.to_owned());
    }

    /// Whether the model is listed: as Manage models set it, else as Desktop lists it by default
    /// (`models.tsx:128-140`).
    pub fn visible(&self, m: &Model) -> bool {
        self.prefs.visibility(&m.key).unwrap_or(!m.hidden)
    }

    /// Whether the model reads images and PDFs itself (`capabilities.input`), as Desktop's composer
    /// asks the selected model (`destination.ts:25`, `session/composer/controller.ts:79`); neither
    /// for a model the server doesn't list.
    pub fn attachment_input(&self, m: &ModelRef) -> AttachmentInput {
        let has = |kind: &str| self.models.iter().any(|x| x.id == m.id && x.provider == m.provider && x.inputs.iter().any(|i| i == kind));
        AttachmentInput { image: has("image"), pdf: has("pdf") }
    }

    pub fn model_name(&self, m: &ModelRef) -> String {
        self.models.iter().find(|x| x.id == m.id && x.provider == m.provider).map_or_else(|| m.display(), |x| x.name.clone())
    }

    /// The provider before the model's name in the composer (`composerProviderName`): its
    /// name, or its id when the server doesn't name it; none for OpenCode's own models.
    pub fn provider_label(&self, m: &ModelRef) -> Option<String> {
        if m.provider.is_empty() || m.provider == "opencode" {
            return None;
        }
        Some(self.providers.get(&m.provider).cloned().unwrap_or_else(|| m.provider.clone()))
    }
}

/// The server's `capabilities.input` (a list, or Desktop's `{ text: true, … }`) in Desktop's
/// order (`details.tsx:13`).
fn inputs(m: &Value) -> Vec<String> {
    let input = &m["capabilities"]["input"];
    let has = |kind: &str| input.as_array().map_or_else(|| input[kind].as_bool().unwrap_or(false), |list| list.iter().any(|v| v == kind));
    ["text", "image", "pdf", "audio", "video"].into_iter().filter(|k| has(k)).map(str::to_owned).collect()
}

/// OpenCode's models that cost nothing (Desktop's `isFreeModel`): the untiered cost's input is 0.
fn is_free(m: &Value) -> bool {
    m["providerID"] == "opencode" && m["cost"].as_array().and_then(|c| c.iter().find(|c| c.get("tier").is_none()).or(c.first())).is_none_or(|c| c["input"].as_f64() == Some(0.0))
}

/// Desktop's default model list (`models.tsx:128-140` `visible`): each provider's newest model
/// in a family released in the last six months, OpenCode's free models, and models with no
/// release date. A search finds the rest. `time.released` 0 is no date
/// (`global-sync/utils.ts:122`), so a configured model the server knows no date for stays listed.
fn shown_models(models: &[&Value], now_ms: i64) -> HashSet<String> {
    const SIX_MONTHS: f64 = 6.0 * 30.436875 * 24.0 * 3600.0 * 1000.0;
    let key = |m: &Value| format!("{}:{}", m["providerID"].as_str().unwrap_or(""), m["id"].as_str().unwrap_or(""));
    let free = |m: &Value| is_free(m);
    let mut out = HashSet::new();
    let mut newest: HashMap<String, (i64, String)> = HashMap::new();
    for m in models {
        let Some(released) = m["time"]["released"].as_f64().filter(|r| *r != 0.0) else { out.insert(key(m)); continue };
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
