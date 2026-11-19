//! Explicit four-phase tick scheduler and window lifecycle hooks (`setup`, `rest`, `visibility`,
//! `close`).

use std::fmt;
use std::sync::Arc;

/// Ordered execution phases within a single frame poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Pull external inputs, worker results, network events, and timers into state.
    Ingest = 0,
    /// Recompute derived views, indices, and projections from ingested state.
    Derive = 1,
    /// Trigger follow-up actions, animations, focus shifts, and UI reactions.
    React = 2,
    /// Flush dirty persistence state, window geometry, and background caches.
    Persist = 3,
}

impl Phase {
    /// All four phases in execution order.
    pub const ALL: [Phase; 4] = [Phase::Ingest, Phase::Derive, Phase::React, Phase::Persist];

    const fn index(self) -> usize {
        self as usize
    }
}

pub type SystemFn<S> = fn(&mut S, f64, &mut rune::Tx) -> bool;
pub type SetupFn<S> = fn(&mut S, rune::Waker);
pub type RestFn<S> = fn(&mut S, &dyn Fn(&str) -> bool);
pub type VisibilityFn<S> = fn(&mut S, bool, &mut rune::Tx);
pub type CloseFn<S> = fn(&mut S);

struct SystemEntry<S> {
    order: u16,
    name: &'static str,
    run: SystemFn<S>,
}

impl<S> Clone for SystemEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for SystemEntry<S> {}

struct SetupEntry<S> {
    name: &'static str,
    run: SetupFn<S>,
}

impl<S> Clone for SetupEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for SetupEntry<S> {}

struct RestEntry<S> {
    name: &'static str,
    run: RestFn<S>,
}

impl<S> Clone for RestEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for RestEntry<S> {}

struct VisibilityEntry<S> {
    name: &'static str,
    run: VisibilityFn<S>,
}

impl<S> Clone for VisibilityEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for VisibilityEntry<S> {}

struct CloseEntry<S> {
    name: &'static str,
    run: CloseFn<S>,
}

impl<S> Clone for CloseEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for CloseEntry<S> {}

struct Schedule<S> {
    phases: [Vec<SystemEntry<S>>; 4],
    setup: Vec<SetupEntry<S>>,
    rest: Vec<RestEntry<S>>,
    visibility: Vec<VisibilityEntry<S>>,
    close: Vec<CloseEntry<S>>,
}

impl<S> Default for Schedule<S> {
    fn default() -> Self {
        Self {
            phases: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            setup: Vec::new(),
            rest: Vec::new(),
            visibility: Vec::new(),
            close: Vec::new(),
        }
    }
}

impl<S> Clone for Schedule<S> {
    fn clone(&self) -> Self {
        Self {
            phases: self.phases.clone(),
            setup: self.setup.clone(),
            rest: self.rest.clone(),
            visibility: self.visibility.clone(),
            close: self.close.clone(),
        }
    }
}

/// Phase-ordered system scheduler and lifecycle hook registry for host state `S`.
///
/// Cloning is O(1) (shared immutable hook lists once built), allowing a host that embeds
/// [`crate::Kernel<S>`] inside `S` to clone the scheduler handle before passing `&mut S` into
/// registered systems without borrow conflicts.
pub struct LifecycleScheduler<S> {
    inner: Option<Arc<Schedule<S>>>,
}

impl<S> Clone for LifecycleScheduler<S> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<S> Default for LifecycleScheduler<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> LifecycleScheduler<S> {
    /// Creates an empty scheduler with no registered systems or hooks.
    pub const fn new() -> Self {
        Self { inner: None }
    }

    fn schedule_mut(&mut self) -> &mut Schedule<S> {
        Arc::make_mut(self.inner.get_or_insert_with(|| Arc::new(Schedule::default())))
    }

    /// Registers a per-tick system in `phase` at the end of the phase (`u16::MAX`).
    /// Systems with equal `order` execute in registration order within a phase.
    pub fn add_system(&mut self, phase: Phase, name: &'static str, f: SystemFn<S>) {
        self.add_system_ordered(phase, u16::MAX, name, f);
    }

    /// Registers a per-tick system in `phase` in ascending `order` (ties preserve registration order).
    pub fn add_system_ordered(
        &mut self,
        phase: Phase,
        order: u16,
        name: &'static str,
        f: SystemFn<S>,
    ) {
        let entries = &mut self.schedule_mut().phases[phase.index()];
        let idx = entries.partition_point(|existing| existing.order <= order);
        entries.insert(idx, SystemEntry { order, name, run: f });
    }

    /// Registers a one-time window setup hook invoked with the window's `Waker`.
    pub fn add_setup(&mut self, name: &'static str, f: SetupFn<S>) {
        self.schedule_mut().setup.push(SetupEntry { name, run: f });
    }

    /// Registers an idle/rest memory-eviction hook receiving a predicate of session IDs to keep.
    pub fn add_rest(&mut self, name: &'static str, f: RestFn<S>) {
        self.schedule_mut().rest.push(RestEntry { name, run: f });
    }

    /// Registers a window visibility-change hook (`shown == true` when visible).
    pub fn add_visibility(&mut self, name: &'static str, f: VisibilityFn<S>) {
        self.schedule_mut().visibility.push(VisibilityEntry { name, run: f });
    }

    /// Registers a window close hook.
    pub fn add_close(&mut self, name: &'static str, f: CloseFn<S>) {
        self.schedule_mut().close.push(CloseEntry { name, run: f });
    }

    /// Runs every system registered in `phase`. Returns `true` if any system reported a change.
    pub fn run_phase(&self, phase: Phase, state: &mut S, now: f64, tx: &mut rune::Tx) -> bool {
        self.run_phase_with(phase, state, now, tx, |_, _| {})
    }

    /// Runs every system registered in `phase`, calling `on_step(state, system_changed)` after
    /// each system returns. Returns `true` if any system reported a change.
    pub fn run_phase_with(
        &self,
        phase: Phase,
        state: &mut S,
        now: f64,
        tx: &mut rune::Tx,
        mut on_step: impl FnMut(&mut S, bool),
    ) -> bool {
        let Some(inner) = self.inner.as_deref() else { return false };
        let mut changed = false;
        for system in &inner.phases[phase.index()] {
            let step_changed = (system.run)(state, now, tx);
            if step_changed {
                changed = true;
            }
            on_step(state, step_changed);
        }
        changed
    }

    /// Runs all four phases in order (`Ingest` -> `Derive` -> `React` -> `Persist`).
    pub fn run_all(&self, state: &mut S, now: f64, tx: &mut rune::Tx) -> bool {
        let mut changed = false;
        for phase in Phase::ALL {
            changed |= self.run_phase(phase, state, now, tx);
        }
        changed
    }

    /// Runs all registered setup hooks in registration order.
    pub fn run_setup(&self, state: &mut S, waker: rune::Waker) {
        let Some(inner) = self.inner.as_deref() else { return };
        for hook in &inner.setup {
            (hook.run)(state, waker.clone());
        }
    }

    /// Runs all registered rest/eviction hooks in registration order.
    pub fn run_rest(&self, state: &mut S, keep_session: &dyn Fn(&str) -> bool) {
        let Some(inner) = self.inner.as_deref() else { return };
        for hook in &inner.rest {
            (hook.run)(state, keep_session);
        }
    }

    /// Runs all registered visibility hooks in registration order.
    pub fn run_visibility(&self, state: &mut S, shown: bool, tx: &mut rune::Tx) {
        let Some(inner) = self.inner.as_deref() else { return };
        for hook in &inner.visibility {
            (hook.run)(state, shown, tx);
        }
    }

    /// Runs all registered close hooks in registration order.
    pub fn run_close(&self, state: &mut S) {
        let Some(inner) = self.inner.as_deref() else { return };
        for hook in &inner.close {
            (hook.run)(state);
        }
    }

    /// Iterates the names of systems registered in `phase`.
    pub fn system_names(&self, phase: Phase) -> impl Iterator<Item = &'static str> + '_ {
        self.inner.as_deref().into_iter().flat_map(move |s| s.phases[phase.index()].iter().map(|entry| entry.name))
    }

    /// Iterates the names of registered setup hooks.
    pub fn setup_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.inner.as_deref().into_iter().flat_map(|s| s.setup.iter().map(|entry| entry.name))
    }

    /// Iterates the names of registered rest hooks.
    pub fn rest_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.inner.as_deref().into_iter().flat_map(|s| s.rest.iter().map(|entry| entry.name))
    }

    /// Iterates the names of registered visibility hooks.
    pub fn visibility_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.inner.as_deref().into_iter().flat_map(|s| s.visibility.iter().map(|entry| entry.name))
    }

    /// Iterates the names of registered close hooks.
    pub fn close_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.inner.as_deref().into_iter().flat_map(|s| s.close.iter().map(|entry| entry.name))
    }

    /// Total number of tick systems across all phases.
    pub fn system_count(&self) -> usize {
        self.inner.as_deref().map_or(0, |s| s.phases.iter().map(Vec::len).sum())
    }
}

impl<S> fmt::Debug for LifecycleScheduler<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (ingest, derive, react, persist, setup, rest, visibility, close) = match self.inner.as_deref() {
            Some(s) => (
                s.phases[Phase::Ingest.index()].len(),
                s.phases[Phase::Derive.index()].len(),
                s.phases[Phase::React.index()].len(),
                s.phases[Phase::Persist.index()].len(),
                s.setup.len(),
                s.rest.len(),
                s.visibility.len(),
                s.close.len(),
            ),
            None => (0, 0, 0, 0, 0, 0, 0, 0),
        };
        f.debug_struct("LifecycleScheduler")
            .field("ingest", &ingest)
            .field("derive", &derive)
            .field("react", &react)
            .field("persist", &persist)
            .field("setup", &setup)
            .field("rest", &rest)
            .field("visibility", &visibility)
            .field("close", &close)
            .finish()
    }
}
