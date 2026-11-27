//! Which effort ("variant") a model runs with when nobody chose one. Desktop reference:
//! `providers/models/variant.ts` (`resolveModelVariant`, `getConfiguredAgentVariant`),
//! `providers/models/models.tsx` (`getVariant`), and the two places that ask them: a new
//! session's draft (`composer/selection.ts` `variant`) and an open session
//! (`providers/models/selection.tsx` `variant`).
//!
//! The two ask differently since opencode#66107, which changed the draft's alone. In a draft a
//! model's speeds take the effort its agent or the config names for the model, and an effort
//! saved under another of its speeds is passed over when one is named. An open session asks by
//! the model itself and reads every speed's saved effort.

use opencode_kit::model::ModelRef;

use super::Pickers;

/// What a selection says about its model's effort (`variant.ts` `VariantSelection`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Effort {
    /// One was chosen, or the server names it: the model's `variant`, "" for Default.
    #[default]
    Chosen,
    /// None was chosen: the model runs with the effort saved for it, else the one its agent
    /// or the config names. The selection's `variant` is what that came to when it was kept;
    /// [`super::PickerHost::selection`] reads it again.
    Inherited,
}

impl Pickers {
    /// The effort the agent `agent` names for the model `key`, else the config's (`variant.ts`
    /// `getConfiguredAgentVariant`, asked of the agent and then of the config): only where
    /// that one names the model too. In a `draft` the model's speeds take what is named for
    /// the model (opencode#66107).
    fn configured_effort(&self, agent: &str, key: &str, base: &str, draft: bool) -> Option<&str> {
        let names = |m: &&ModelRef| !m.variant.is_empty() && (m.key() == key || (draft && m.key() == base));
        [self.agent_models.get(agent), self.configured.as_ref()].into_iter().flatten().find(names).map(|m| m.variant.as_str())
    }

    /// The effort the model `key` runs with under `agent` when none was chosen, "" for its
    /// default (`variant.ts` `resolveModelVariant` for an inherited selection): the saved one
    /// before the configured one, and only an effort the model has.
    pub(crate) fn inherited_effort(&self, agent: &str, key: &str, draft: bool) -> String {
        let Some(m) = self.models.iter().find(|m| m.key == key).filter(|m| !m.variants.is_empty()) else { return String::new() };
        let base = m.speed.as_ref().map_or_else(|| key.to_owned(), |s| format!("{}:{}", m.provider, s.base));
        let configured = self.configured_effort(agent, key, &base, draft);
        // A draft passes over a sibling speed's saved effort when one is configured
        // (`composer/selection.ts` `siblingFallback`).
        let saved = self.saved_effort(key, &base, !draft || configured.is_none());
        saved.or(configured).filter(|v| *v != "default" && m.variants.iter().any(|x| x.as_str() == *v)).unwrap_or_default().to_owned()
    }

    /// The effort saved for the model `key`, whose standard model is `base` (`models.tsx`
    /// `getVariant`): a model's speeds share its effort, saved under the standard model.
    /// Preferences saved before they did may name one of its speeds instead (opencode#65982):
    /// the model itself is read after the standard one, and with `siblings` its other speeds
    /// after that, in the catalog's order.
    fn saved_effort(&self, key: &str, base: &str, siblings: bool) -> Option<&str> {
        let saved = self.prefs.variant(base).or_else(|| self.prefs.variant(key));
        if saved.is_some() || !siblings {
            return saved;
        }
        self.models.iter()
            .filter(|m| m.speed.as_ref().is_some_and(|s| format!("{}:{}", m.provider, s.base) == base))
            .find_map(|m| self.prefs.variant(&m.key))
    }
}
