//! The branch picker's search, as Desktop's typeahead machine runs it (`ui/src/forms/typeahead/machine.ts`):
//! a cached or empty query answers at once, a typed one waits for typing to pause (100 ms in
//! the branch picker, `selector.tsx:202`) and then asks the server. While one is pending the
//! rows narrow to what's typed, or stay on screen dimmed (`stale`) when nothing would remain,
//! so speculative rows never flash. Only the current query's failure is `Failed`; a request
//! that doesn't answer in 15 s fails (`TYPEAHEAD_REQUEST_TIMEOUT_MS`).

use std::collections::HashMap;

use rune::machine::Effects;

/// `delay={100}` in `new-session/workspace/selector.tsx`.
pub(crate) const DEBOUNCE: f64 = 0.1;
/// `TYPEAHEAD_REQUEST_TIMEOUT_MS`.
pub(crate) const TIMEOUT: f64 = 15.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Phase {
    Ready,
    Failed,
    /// Waiting for typing to pause before asking for `query`.
    Debouncing { query: String, due: f64 },
    /// Asked for `query`; it fails at `due`.
    Loading { query: String, due: f64 },
}

#[derive(Clone, Debug)]
pub(crate) struct Typeahead {
    pub phase: Phase,
    /// Each query's answer while the picker is open.
    cache: HashMap<String, Vec<String>>,
    pub shown: Vec<String>,
    pub stale: bool,
    /// The field's text, trimmed.
    input: String,
}

impl Default for Typeahead {
    fn default() -> Self {
        Typeahead { phase: Phase::Ready, cache: HashMap::new(), shown: Vec::new(), stale: false, input: String::new() }
    }
}

pub(crate) enum Event<'a> {
    /// The picker opens on the parent's list (`initial`): `None` while it loads.
    Opened { initial: Option<&'a [String]>, failed: bool, now: f64 },
    /// The field's text is now `input` (`input.changed`).
    Input { input: &'a str, now: f64 },
    Tick { now: f64 },
    /// The parent's opening list changed (`initial.synced`): an open picker on the empty query
    /// shows it, unless its own request for it is on the way.
    Synced(&'a [String]),
    /// The server's branches for `query`; `None` when it failed.
    Answered { query: &'a str, items: Option<Vec<String>> },
}

#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    WakeAt(f64),
    Request(String),
}

impl Typeahead {
    pub fn pending(&self) -> bool {
        matches!(self.phase, Phase::Debouncing { .. } | Phase::Loading { .. })
    }

    pub fn failed(&self) -> bool {
        self.phase == Phase::Failed
    }

    pub fn step(&mut self, event: Event<'_>) -> Effects<Effect> {
        match event {
            Event::Opened { initial, failed, now } => {
                *self = Typeahead::default();
                match initial {
                    Some(_) if failed => {
                        self.phase = Phase::Failed;
                        Effects::none()
                    }
                    Some(items) => {
                        self.shown = items.to_vec();
                        self.cache.insert(String::new(), items.to_vec());
                        Effects::none()
                    }
                    None => self.load(String::new(), now),
                }
            }
            Event::Input { input, now } => {
                let next = input.trim();
                if next == self.input {
                    return Effects::none();
                }
                self.input = next.to_owned();
                if let Some(cached) = self.cache.get(next).cloned() {
                    (self.shown, self.stale, self.phase) = (cached, false, Phase::Ready);
                    return Effects::none();
                }
                let lower = next.to_lowercase();
                let narrowed: Vec<String> = self.shown.iter().filter(|b| b.to_lowercase().contains(&lower)).cloned().collect();
                if narrowed.is_empty() {
                    self.stale = !self.shown.is_empty();
                } else {
                    (self.shown, self.stale) = (narrowed, false);
                }
                if next.is_empty() {
                    return self.load(String::new(), now);
                }
                let due = now + DEBOUNCE;
                self.phase = Phase::Debouncing { query: next.to_owned(), due };
                Effects::one(Effect::WakeAt(due))
            }
            Event::Tick { now } => match &self.phase {
                Phase::Debouncing { query, due } if now + 1e-9 >= *due => {
                    let query = query.clone();
                    self.load(query, now)
                }
                Phase::Loading { due, .. } if now + 1e-9 >= *due => {
                    (self.shown, self.stale, self.phase) = (Vec::new(), false, Phase::Failed);
                    Effects::none()
                }
                _ => Effects::none(),
            },
            Event::Synced(items) => {
                if self.input.is_empty() && !matches!(self.phase, Phase::Loading { .. }) {
                    self.cache.insert(String::new(), items.to_vec());
                    (self.shown, self.stale, self.phase) = (items.to_vec(), false, Phase::Ready);
                }
                Effects::none()
            }
            Event::Answered { query, items } => {
                // Every answer fills the cache; only the pending query's settles the list.
                if let Some(items) = &items {
                    self.cache.insert(query.to_owned(), items.clone());
                }
                if !matches!(&self.phase, Phase::Loading { query: q, .. } if q == query) {
                    return Effects::none();
                }
                match items {
                    Some(items) => (self.shown, self.stale, self.phase) = (items, false, Phase::Ready),
                    None => (self.shown, self.stale, self.phase) = (Vec::new(), false, Phase::Failed),
                }
                Effects::none()
            }
        }
    }


    fn load(&mut self, query: String, now: f64) -> Effects<Effect> {
        let due = now + TIMEOUT;
        self.phase = Phase::Loading { query: query.clone(), due };
        Effects::two(Effect::Request(query), Effect::WakeAt(due))
    }
}
