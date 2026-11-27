//! A model's speeds. A provider's service tiers reach the app as models of their own
//! ("gpt-5.5-fast"), each naming the model it is a speed of (`experimental.speed`): the lists show
//! that model once, and the composer offers its speeds beside it. Desktop reference
//! (opencode#65749): `providers/models/models.tsx` (`standard`, `speed`),
//! `providers/models/speed.ts` and `composer/selection.ts`.

use std::collections::HashSet;

use opencode_kit::model::{Model, ModelRef, Speed};
use serde_json::Value;

use super::Pickers;

/// The speed picker's key for a model's own speed (`composer/model.ts` `speed.options`).
pub const STANDARD: &str = "standard";

/// The command that moves to the next speed (`composer/commands.tsx` `model.speed.cycle`).
pub const CYCLE_COMMAND: &str = "model.speed.cycle";

pub(super) fn parse(v: &Value) -> Option<Speed> {
    Some(Speed { base: v["base"].as_str()?.to_owned(), id: v["id"].as_str()?.to_owned(), faster: v["faster"].as_bool().unwrap_or(false) })
}

/// A speed whose model the list leaves out is a model of its own (`models.tsx` `standard`).
pub(super) fn settle(models: &mut [Model]) {
    let keys: HashSet<String> = models.iter().map(|m| m.key.clone()).collect();
    for m in models {
        if m.speed.as_ref().is_some_and(|s| !keys.contains(&format!("{}:{}", m.provider, s.base))) {
            m.speed = None;
        }
    }
}

impl Pickers {
    fn model(&self, key: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.key == key)
    }

    /// The model whose row stands for `key` in a list: itself, or the model it is a speed of.
    pub fn standard_key(&self, key: &str) -> String {
        self.model(key).and_then(|m| m.speed.as_ref().map(|s| format!("{}:{}", m.provider, s.base))).unwrap_or_else(|| key.to_owned())
    }

    /// `m` at its standard speed, with its effort.
    pub fn standard_ref(&self, m: &ModelRef) -> ModelRef {
        match self.model(&m.key()).and_then(|x| x.speed.as_ref()) {
            Some(speed) => ModelRef { id: speed.base.clone(), ..m.clone() },
            None => m.clone(),
        }
    }

    /// The speeds of the model `key` names (at any of them) as `(speed, model key)`, in
    /// Desktop's order: the slower ones, [`STANDARD`], then the faster ones (`speed.ts`
    /// `orderedSpeeds`). None for a model with no other speed.
    pub fn speeds(&self, key: &str) -> Vec<(String, String)> {
        let base = self.standard_key(key);
        let others: Vec<(&Model, &Speed)> = self.models.iter()
            .filter_map(|m| m.speed.as_ref().filter(|s| format!("{}:{}", m.provider, s.base) == base).map(|s| (m, s)))
            .collect();
        if others.is_empty() {
            return Vec::new();
        }
        let part = |faster: bool| others.iter().filter(move |(_, s)| s.faster == faster).map(|(m, s)| (s.id.clone(), m.key.clone()));
        part(false).chain(std::iter::once((STANDARD.to_owned(), base.clone()))).chain(part(true)).collect()
    }

    /// The speed the model runs at: [`STANDARD`] for a model that is no other's speed.
    pub fn speed_of(&self, key: &str) -> String {
        self.model(key).and_then(|m| m.speed.as_ref()).map_or_else(|| STANDARD.to_owned(), |s| s.id.clone())
    }

    /// The model to run when `key` is picked: itself, at the speed last used with it
    /// (`models.tsx` `speed.remembered`).
    pub fn remembered_speed(&self, key: &str) -> String {
        if self.model(key).is_none_or(|m| m.speed.is_some()) {
            return key.to_owned();
        }
        let Some(last) = self.prefs.speed(key) else { return key.to_owned() };
        self.speeds(key).into_iter().find(|(speed, _)| speed == last).map_or_else(|| key.to_owned(), |(_, model)| model)
    }

    /// A session that moved from `previous` to `model` changed speed, not model: the speed it
    /// moved to, none for the standard one (`timeline-next/row.tsx` `model-switched`). Both must
    /// be models the provider lists.
    pub fn speed_switch(&self, model: &ModelRef, previous: &ModelRef) -> Option<Option<String>> {
        if model.provider != previous.provider || model.id == previous.id {
            return None;
        }
        let (now, before) = (self.model(&model.key())?, self.model(&previous.key())?);
        let base = |m: &Model| m.speed.as_ref().map_or(m.id.clone(), |s| s.base.clone());
        (base(now) == base(before)).then(|| now.speed.as_ref().map(|s| s.id.clone()))
    }

    /// The model's name in the composer: its speed shows beside it, so the name is the model's
    /// own (`composer/composer.tsx` `selection.standard`, `model-label.ts`).
    pub fn composer_model_name(&self, m: &ModelRef) -> String {
        self.model_name(&self.standard_ref(m))
    }
}
