//! The model each agent last had, so switching back to an agent brings its model back. Desktop
//! reference: an open session's `choices` (`providers/models/selection-machine.ts`
//! `editSelection`) and a new session's draft's `remembered` (`composer/selection.ts`), which
//! the draft hands to the session it starts (`new-session/composer-adapter.ts`
//! `choices: model.remembered()`, `selection.tsx` `promote`).
//!
//! The two remember differently. A session keeps what the agent it leaves showed, with the
//! effort it showed. A draft keeps only a model that was picked for the agent, with no effort
//! of its own: an agent nobody picked a model for comes back to its own model, the config's, a
//! recent one or the first, as they are then.

use opencode_kit::model::ModelRef;
use serde_json::{Map, Value, json};

use super::{Effort, Pickers, Selection};

/// The scope of the picks of a page that is neither a session nor a draft tab
/// ([`super::PickerHost::picker_scope`]). Each draft tab keeps its own picks under its own id,
/// as Desktop's drafts do (`composer/persistence.tsx`: the tab's own composer state;
/// `selection.tsx`: "A draft's selection is this provider's own, so two drafts on one workspace
/// choose independently").
pub const DRAFT: &str = "draft";

/// A model an agent had, and whether its effort was chosen (`selection-machine.ts`
/// `ModelChoice`).
#[derive(Clone, Debug, PartialEq)]
pub struct Choice { pub model: ModelRef, pub effort: Effort }

impl Choice {
    /// Desktop's stored `ModelChoice` (`selection.tsx` `ModelChoiceSchema`).
    fn to_json(&self) -> Value {
        let variant = match (self.effort, self.model.variant.as_str()) {
            (Effort::Inherited, _) => json!({ "type": "inherit" }),
            (Effort::Chosen, "" | "default") => json!({ "type": "default" }),
            (Effort::Chosen, name) => json!({ "type": "named", "value": name }),
        };
        json!({ "providerID": self.model.provider, "modelID": self.model.id, "variant": variant })
    }

    fn from_json(v: &Value) -> Option<Choice> {
        let (variant, effort) = match v["variant"]["type"].as_str() {
            Some("named") => (v["variant"]["value"].as_str().unwrap_or(""), Effort::Chosen),
            Some("default") => ("", Effort::Chosen),
            _ => ("", Effort::Inherited),
        };
        let model = ModelRef { id: v["modelID"].as_str()?.to_owned(), provider: v["providerID"].as_str()?.to_owned(), variant: variant.to_owned() };
        Some(Choice { model, effort })
    }
}

impl Pickers {
    /// What the session `scope` keeps across a relaunch, in Desktop's shape (`selection.tsx`
    /// `writeStored`, `selection-machine.ts` `persistedSelection`): its pick while the server
    /// does not run with it, and the model each agent last had. None when it holds neither.
    pub fn stored(&self, scope: &str) -> Option<Value> {
        let pick = self.selections.get(scope);
        let choices: Map<String, Value> = self.choices.get(scope).into_iter().flatten().map(|(agent, c)| (agent.clone(), c.to_json())).collect();
        if pick.is_none() && choices.is_empty() {
            return None;
        }
        let mut entry = Map::new();
        if let Some(pick) = pick {
            entry.insert("agent".into(), json!(pick.agent));
            entry.insert("choice".into(), Choice { model: pick.model.clone(), effort: pick.effort }.to_json());
        }
        entry.insert("choices".into(), Value::Object(choices));
        Some(Value::Object(entry))
    }

    /// Takes what [`Self::stored`] kept for the session `scope` (`selection.tsx` `readStored`).
    pub fn restore(&mut self, scope: &str, entry: &Value) {
        let choices: std::collections::HashMap<String, Choice> = entry["choices"].as_object().into_iter().flatten()
            .filter_map(|(agent, c)| Some((agent.clone(), Choice::from_json(c)?))).collect();
        if !choices.is_empty() {
            self.choices.insert(scope.to_owned(), choices);
        }
        if let (Some(agent), Some(choice)) = (entry["agent"].as_str(), Choice::from_json(&entry["choice"])) {
            self.selections.insert(scope.to_owned(), Selection { model: choice.model, agent: agent.to_owned(), effort: choice.effort });
        }
    }

    /// The session `scope` is deleted: its picks go (`selection-machine.ts` `session.deleted`).
    pub fn forget_session(&mut self, scope: &str) {
        self.selections.remove(scope);
        self.choices.remove(scope);
    }

    /// A model was picked for the draft's agent: the draft remembers it, with no effort of its
    /// own (`composer/selection.ts` `remembered`: `variant: INHERIT_VARIANT`).
    pub(crate) fn remember_draft_pick(&mut self, draft: &str, pick: &Selection) {
        let choice = Choice { model: ModelRef { variant: String::new(), ..pick.model.clone() }, effort: Effort::Inherited };
        self.choices.entry(draft.to_owned()).or_default().insert(pick.agent.clone(), choice);
    }

    /// A pick made outside the pickers (a model connected or offered in a dialog) for `scope`,
    /// a `draft`'s or a session's.
    pub fn pick(&mut self, scope: String, pick: Selection, draft: bool) {
        if draft { self.remember_draft_pick(&scope, &pick); }
        self.selections.insert(scope, pick);
    }

    /// The draft `draft` became the session `id`: the models picked for its agents go with the
    /// session (`selection.tsx` `promote`; official's test "new-session drafts remember each
    /// agent's model and hand off inactive choices").
    pub fn promote_draft(&mut self, draft: &str, id: &str) {
        if let Some(choices) = self.choices.remove(draft).filter(|c| !c.is_empty()) {
            self.choices.insert(id.to_owned(), choices);
        }
    }

    /// The session `id` could not start and its draft `draft` is back: its agents' models with it.
    pub fn demote_session(&mut self, id: &str, draft: &str) {
        if let Some(choices) = self.choices.remove(id) {
            self.choices.insert(draft.to_owned(), choices);
        }
    }
}
