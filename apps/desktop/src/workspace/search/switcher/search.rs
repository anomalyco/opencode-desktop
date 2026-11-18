//! One remote source of the palette, as Desktop's search machines run them
//! (`shell/commands/search-machine.ts`): the server's sessions and the folder's files each settle
//! on their own, so a slow or failing one never holds back or hides the other. A typed session
//! search first waits for a pause (`SessionSourceState`'s `debouncing`, 150 ms); every request
//! names its attempt, an answer for another attempt changes nothing, and one that takes longer
//! than 10 s fails (`PALETTE_SEARCH_TIMEOUT_MS`), so the palette never says Loading forever.

use rune::machine::{Attempt, Effects, Seq};

/// `PALETTE_SESSION_DEBOUNCE_MS`.
pub(crate) const DEBOUNCE: f64 = 0.100;
/// `PALETTE_SEARCH_TIMEOUT_MS`.
pub(crate) const TIMEOUT: f64 = 10.0;

/// A source's last answer and the query it answered (`PaletteSearchSettled`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settled<T> {
    pub query: String,
    pub items: Vec<T>,
}

impl<T> Default for Settled<T> {
    fn default() -> Self {
        Settled { query: String::new(), items: Vec::new() }
    }
}

/// `PaletteSourceState`, with the session source's debounce (`SessionSourceState`) folded in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source<T> {
    /// Opened, no query seen yet (`uninitialized`): the first one is always new.
    Unasked,
    /// Nothing to ask for this query: it's answered, or it's one the source never asks about.
    Ready { query: String, settled: Settled<T> },
    /// A typed query waiting for the pause before it's sent.
    Debouncing { attempt: Attempt, query: String, due: f64, settled: Settled<T> },
    /// Sent; `due` is when it counts as failed.
    Loading { attempt: Attempt, query: String, due: f64, settled: Settled<T> },
    /// The current query's request failed or timed out.
    Failed { query: String, settled: Settled<T> },
}

impl<T> Default for Source<T> {
    fn default() -> Self {
        Source::Unasked
    }
}

pub(crate) enum Event<T> {
    /// The query is now `query`: `ask` is whether the source asks the server about it, `wait`
    /// whether a pause comes first.
    Query { query: String, ask: bool, wait: bool, now: f64 },
    /// `poll` at `now`: a debounce or timeout that came due.
    Tick { now: f64 },
    Answered { attempt: Attempt, items: Vec<T> },
    Failed { attempt: Attempt },
}

#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    WakeAt(f64),
    /// Ask the server for `query`; its answer carries `attempt`.
    Fetch { attempt: Attempt, query: String },
    /// Drop the request in flight.
    Cancel,
}

impl<T> Source<T> {
    /// The query the source is on.
    pub fn query(&self) -> Option<&str> {
        match self {
            Source::Unasked => None,
            Source::Ready { query, .. } | Source::Debouncing { query, .. } | Source::Loading { query, .. } | Source::Failed { query, .. } => Some(query),
        }
    }

    /// The last answer, for whichever query it was.
    pub fn settled(&self) -> Option<&Settled<T>> {
        match self {
            Source::Unasked => None,
            Source::Ready { settled, .. } | Source::Debouncing { settled, .. } | Source::Loading { settled, .. } | Source::Failed { settled, .. } => Some(settled),
        }
    }

    /// Which state it's in, and for which attempt: what a poll compares to see it move.
    pub fn status(&self) -> (std::mem::Discriminant<Self>, Option<Attempt>) {
        let attempt = match self {
            Source::Debouncing { attempt, .. } | Source::Loading { attempt, .. } => Some(*attempt),
            _ => None,
        };
        (std::mem::discriminant(self), attempt)
    }

    /// The attempt in flight.
    pub fn loading_attempt(&self) -> Option<Attempt> {
        match self {
            Source::Loading { attempt, .. } => Some(*attempt),
            _ => None,
        }
    }

    /// Still answering the current query (`selectPaletteSearchLoading`).
    pub fn loading(&self) -> bool {
        matches!(self, Source::Debouncing { .. } | Source::Loading { .. })
    }

    /// The current query failed (`selectPaletteSearchFailed`).
    pub fn failed(&self) -> bool {
        matches!(self, Source::Failed { .. })
    }

    fn take_settled(&mut self) -> Settled<T> {
        match self {
            Source::Unasked => Settled::default(),
            Source::Ready { settled, .. } | Source::Debouncing { settled, .. } | Source::Loading { settled, .. } | Source::Failed { settled, .. } => std::mem::take(settled),
        }
    }
}

pub(crate) fn step<T>(seq: &mut Seq, source: &mut Source<T>, event: Event<T>) -> Effects<Effect> {
    let busy = source.loading();
    match event {
        Event::Query { query, .. } if source.query() == Some(query.as_str()) => Effects::none(),
        Event::Query { query, ask, wait, now } => {
            let settled = source.take_settled();
            let cancel = busy.then_some(Effect::Cancel);
            if !ask {
                *source = Source::Ready { query, settled };
                return cancel.map_or_else(Effects::none, Effects::one);
            }
            let attempt = seq.next();
            let (next, effect) = if wait {
                let due = now + DEBOUNCE;
                (Source::Debouncing { attempt, query, due, settled }, Effect::WakeAt(due))
            } else {
                let due = now + TIMEOUT;
                let fetch = Effect::Fetch { attempt, query: query.clone() };
                *source = Source::Loading { attempt, query, due, settled };
                return match cancel {
                    Some(c) => Effects::Many(vec![c, fetch, Effect::WakeAt(due)]),
                    None => Effects::two(fetch, Effect::WakeAt(due)),
                };
            };
            *source = next;
            match cancel {
                Some(c) => Effects::two(c, effect),
                None => Effects::one(effect),
            }
        }
        Event::Tick { now } => match source {
            Source::Debouncing { attempt, query, due, settled } if now + 1e-9 >= *due => {
                let (attempt, query, settled) = (*attempt, std::mem::take(query), std::mem::take(settled));
                let due = now + TIMEOUT;
                *source = Source::Loading { attempt, query: query.clone(), due, settled };
                Effects::two(Effect::Fetch { attempt, query }, Effect::WakeAt(due))
            }
            Source::Loading { query, due, .. } if now + 1e-9 >= *due => {
                let query = std::mem::take(query);
                *source = Source::Failed { settled: Settled { query: query.clone(), items: Vec::new() }, query };
                Effects::one(Effect::Cancel)
            }
            _ => Effects::none(),
        },
        Event::Answered { attempt, items } => match source {
            Source::Loading { attempt: a, query, .. } if *a == attempt => {
                let query = std::mem::take(query);
                *source = Source::Ready { settled: Settled { query: query.clone(), items }, query };
                Effects::none()
            }
            _ => Effects::none(),
        },
        Event::Failed { attempt } => match source {
            Source::Loading { attempt: a, query, .. } if *a == attempt => {
                let query = std::mem::take(query);
                *source = Source::Failed { settled: Settled { query: query.clone(), items: Vec::new() }, query };
                Effects::none()
            }
            _ => Effects::none(),
        },
    }
}
