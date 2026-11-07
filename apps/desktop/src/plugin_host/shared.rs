//! What plugins share through the host: contracts and services (`plugins-design.md` §2.5 B),
//! and the stores the host keeps for each plugin.
//!
//! A contract moves through `Pending → Active(generation) → Inactive(disabled | failed)`.
//! Publishing again (a hot reload's new setup) replaces the value and bumps the generation in
//! one step on the app's thread, so a consumer never sees a half-swapped provider; every state
//! change wakes the consumers' polls.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use opencode_plugin::abi::{DropFn, LIVE_ACTIVE, LIVE_DISABLED, LIVE_FAILED, LIVE_MISMATCH, LIVE_PENDING, LiveAbi};
use serde_json::Value;

use super::loader::LoadedLibrary;
use super::vtable::slot_bit;

#[derive(Default)]
pub struct Shared {
    pub contracts: Contracts,
    pub stores: Stores,
}

/// A published value, freed by its provider's own destructor while its library is mapped.
struct Provided {
    value: usize,
    drop_value: Option<DropFn>,
    owner: usize,
    type_hash: u64,
    /// The provider generation that published it.
    generation: u32,
    _lib: Arc<LoadedLibrary>,
}

impl Drop for Provided {
    fn drop(&mut self) {
        if let Some(drop_value) = self.drop_value.take() {
            // SAFETY: the provider's destructor for exactly this value, called once.
            unsafe { drop_value(self.value) };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Live {
    Pending,
    Active,
    Disabled,
    Failed,
}

struct Contract {
    id: String,
    state: Live,
    generation: u32,
    value: Option<Provided>,
    /// The value a swap in progress replaced: freed once the swap succeeds, put back if it fails.
    previous: Option<Provided>,
    /// Slots using it, woken on every change.
    consumers: u64,
}

#[derive(Default)]
pub struct Contracts {
    list: Vec<Contract>,
    /// A consumer's handle: the contract and the type hash it expects.
    handles: Vec<(usize, u64)>,
}

impl Contracts {
    fn index(&mut self, id: &str) -> usize {
        if let Some(i) = self.list.iter().position(|c| c.id == id) {
            return i;
        }
        self.list.push(Contract { id: id.to_owned(), state: Live::Pending, generation: 0, value: None, previous: None, consumers: 0 });
        self.list.len() - 1
    }

    /// Plugin `owner_id` (slot `owner`, instance generation `generation`) publishes `id`.
    /// Returns the consumers to wake, or why it was refused.
    #[allow(clippy::too_many_arguments)]
    pub fn provide(&mut self, owner: usize, owner_id: &str, generation: u32, id: &str, type_hash: u64, value: usize, drop_value: Option<DropFn>, lib: Arc<LoadedLibrary>) -> Result<u64, String> {
        if id != owner_id && !id.strip_prefix(owner_id).is_some_and(|rest| rest.starts_with('.')) {
            return Err(format!("plugin {owner_id} can't provide {id}: only {owner_id} and {owner_id}.<name> are its own"));
        }
        let i = self.index(id);
        let c = &mut self.list[i];
        if c.value.as_ref().is_some_and(|v| v.owner != owner) {
            return Err(format!("{id} is already provided"));
        }
        let replaced = c.value.replace(Provided { value, drop_value, owner, type_hash, generation, _lib: lib });
        if c.previous.is_none() && replaced.as_ref().is_some_and(|r| r.generation < generation) {
            c.previous = replaced;
        }
        c.state = Live::Active;
        c.generation += 1;
        Ok(c.consumers)
    }

    /// Slot `slot`'s setup of generation `generation` finished. On success the values it
    /// replaced are freed, and what the old generation provided that the new one didn't goes
    /// inactive; on failure the replaced values come back. Returns the consumers to wake.
    pub fn settle_swap(&mut self, slot: usize, generation: u32, ok: bool) -> u64 {
        let mut wake = 0;
        for c in &mut self.list {
            let owned = c.value.as_ref().is_some_and(|v| v.owner == slot) || c.previous.as_ref().is_some_and(|v| v.owner == slot);
            if !owned {
                continue;
            }
            let fresh = c.value.as_ref().is_some_and(|v| v.generation == generation);
            if ok {
                c.previous = None;
                if !fresh {
                    c.value = None;
                    c.state = Live::Disabled;
                    wake |= c.consumers;
                }
            } else if fresh {
                c.value = c.previous.take();
                c.state = if c.value.is_some() { Live::Active } else { Live::Pending };
                c.generation += 1;
                wake |= c.consumers;
            }
        }
        wake
    }

    /// Slot `slot` uses `id`, expecting `type_hash`.
    pub fn consume(&mut self, slot: usize, id: &str, type_hash: u64) -> u32 {
        let i = self.index(id);
        self.list[i].consumers |= slot_bit(slot);
        self.handles.push((i, type_hash));
        self.handles.len() as u32 - 1
    }

    pub fn live(&self, handle: u32) -> LiveAbi {
        let Some(&(i, hash)) = self.handles.get(handle as usize) else { return LiveAbi { state: LIVE_DISABLED, ..LiveAbi::default() } };
        let c = &self.list[i];
        let state = match (c.state, &c.value) {
            (Live::Active, Some(v)) if v.type_hash != hash => LIVE_MISMATCH,
            (Live::Active, Some(_)) => LIVE_ACTIVE,
            (Live::Active | Live::Pending, _) => LIVE_PENDING,
            (Live::Disabled, _) => LIVE_DISABLED,
            (Live::Failed, _) => LIVE_FAILED,
        };
        let value = if state == LIVE_ACTIVE { c.value.as_ref().map_or(0, |v| v.value) } else { 0 };
        LiveAbi { state, generation: c.generation, value }
    }

    /// Slot `slot` stopped (`Disabled`) or failed: its contracts go inactive and their values
    /// are freed. Returns the consumers to wake.
    pub fn withdraw(&mut self, slot: usize, state: Live) -> u64 {
        let owned = |c: &Contract| c.value.as_ref().is_some_and(|v| v.owner == slot) || c.previous.as_ref().is_some_and(|v| v.owner == slot);
        self.list.iter_mut().filter(|c| owned(c)).fold(0, |wake, c| {
            c.value = None;
            c.previous = None;
            c.state = state;
            wake | c.consumers
        })
    }

    /// Contracts still pending whose provider won't come: `state_of` says what plugin `id` is
    /// doing (`None` when it isn't installed). Returns the consumers to wake.
    pub fn settle(&mut self, state_of: impl Fn(&str) -> Option<Live>) -> u64 {
        self.list.iter_mut().filter(|c| c.state == Live::Pending).fold(0, |wake, c| {
            let owner = c.id.split('.').next().unwrap_or_default();
            match state_of(owner) {
                Some(Live::Pending | Live::Active) => wake,
                Some(state) => {
                    c.state = state;
                    wake | c.consumers
                }
                None => {
                    c.state = Live::Disabled;
                    wake | c.consumers
                }
            }
        })
    }

    /// A provider coming back on: its inactive contracts wait for it again.
    pub fn reopen(&mut self, owner_id: &str) {
        for c in self.list.iter_mut().filter(|c| c.id.split('.').next() == Some(owner_id) && c.value.is_none()) {
            c.state = Live::Pending;
        }
    }

    /// Drops slot `slot` from every contract's consumers (its instance is gone).
    pub fn forget_consumer(&mut self, slot: usize) {
        for c in &mut self.list {
            c.consumers &= !slot_bit(slot);
        }
    }

    /// `(id, state, generation, provider slot)` of each contract, for Settings.
    pub fn summary(&self) -> impl Iterator<Item = (&str, Live, u32, Option<usize>)> {
        self.list.iter().map(|c| (c.id.as_str(), c.state, c.generation, c.value.as_ref().map(|v| v.owner)))
    }
}

/// Each plugin's stores: text values by name (`name` global, `name@<session>` per session),
/// in `<data>/plugins-state/<id>.json`, loaded before the plugin sets up.
#[derive(Default)]
pub struct Stores {
    plugins: BTreeMap<String, BTreeMap<String, String>>,
    pub persist: bool,
}

fn file(plugin: &str) -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("plugins-state").join(format!("{plugin}.json")))
}

pub fn key(name: &str, session: &str) -> String {
    if session.is_empty() { name.to_owned() } else { format!("{name}@{session}") }
}

impl Stores {
    /// Loads plugin `plugin`'s stores once (`PreparingStores`).
    pub fn load(&mut self, plugin: &str) {
        if self.plugins.contains_key(plugin) {
            return;
        }
        let read = || -> Option<BTreeMap<String, String>> {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(file(plugin)?).ok()?).ok()?;
            Some(v.as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        };
        let values = if self.persist { read().unwrap_or_default() } else { BTreeMap::new() };
        self.plugins.insert(plugin.to_owned(), values);
    }

    pub fn get(&self, plugin: &str, name: &str, session: &str) -> Option<&str> {
        self.plugins.get(plugin)?.get(&key(name, session)).map(String::as_str)
    }

    pub fn set(&mut self, plugin: &str, name: &str, session: &str, value: &str) {
        let values = self.plugins.entry(plugin.to_owned()).or_default();
        if values.get(&key(name, session)).map(String::as_str) == Some(value) {
            return;
        }
        values.insert(key(name, session), value.to_owned());
        if !self.persist {
            return;
        }
        let Some(path) = file(plugin) else { return };
        let json = serde_json::to_string(values).unwrap_or_default();
        crate::disk::later(super::icons::intern(&format!("plugin-store-{plugin}")), move || {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).ok();
            }
            crate::disk::replace(&path, json.as_bytes());
        });
    }
}
