//! Explicit four-phase tick scheduler (named [`Stage`]s + `after` dependency DAG) and window
//! lifecycle hooks (`setup`, `rest`, `visibility`, `close`).

use std::collections::{BTreeSet, HashMap, HashSet};
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

/// A named execution stage within a [`Phase`].
///
/// Hosts declare stages in execution order via [`LifecycleScheduler::add_stage`] /
/// [`crate::KernelBuilder::add_stages`]. Systems target a `Stage` and may declare explicit
/// `after` edges instead of numeric slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stage {
    pub phase: Phase,
    pub name: &'static str,
}

impl Stage {
    /// Creates a stage descriptor for `phase`.
    pub const fn new(phase: Phase, name: &'static str) -> Self {
        Self { phase, name }
    }
}

pub type SystemFn<S> = fn(&mut S, f64, &mut rune::Tx) -> bool;
pub type SetupFn<S> = fn(&mut S, rune::Waker);
pub type RestFn<S> = fn(&mut S, &dyn Fn(&str) -> bool);
pub type VisibilityFn<S> = fn(&mut S, bool, &mut rune::Tx);
pub type CloseFn<S> = fn(&mut S);

/// Declarative per-tick system registration: target [`Stage`], unique name, optional `after`
/// dependencies, and system callback.
pub struct System<S> {
    stage: Stage,
    name: &'static str,
    after: Vec<&'static str>,
    run: SystemFn<S>,
}

impl<S> Clone for System<S> {
    fn clone(&self) -> Self {
        Self {
            stage: self.stage,
            name: self.name,
            after: self.after.clone(),
            run: self.run,
        }
    }
}

impl<S> System<S> {
    /// Creates a system targeting `stage` with unique identifier `name`.
    pub fn new(stage: Stage, name: &'static str, run: SystemFn<S>) -> Self {
        Self { stage, name, after: Vec::new(), run }
    }

    /// Declares that this system must execute after system `dep`.
    ///
    /// `dep` must exist in the same or an earlier `(Phase, Stage)`; missing targets, later-stage
    /// targets, and cycles panic when the schedule is finalized via [`LifecycleScheduler::resolve`].
    pub fn after(mut self, dep: &'static str) -> Self {
        self.after.push(dep);
        self
    }

    /// Declares that this system must execute after every system in `deps`.
    pub fn after_all(mut self, deps: &[&'static str]) -> Self {
        self.after.extend_from_slice(deps);
        self
    }
}

/// Builder handle for attaching ordering constraints to a just-registered system.
pub struct Scheduled<'a, S> {
    system: &'a mut System<S>,
}

impl<S> Scheduled<'_, S> {
    /// Declares that this system must execute after system `dep`.
    ///
    /// `dep` must exist in the same or an earlier `(Phase, Stage)`; missing targets, later-stage
    /// targets, and cycles panic when the schedule is finalized via [`LifecycleScheduler::resolve`].
    pub fn after(self, dep: &'static str) -> Self {
        self.system.after.push(dep);
        self
    }

    /// Declares that this system must execute after every system in `deps`.
    pub fn after_all(self, deps: &[&'static str]) -> Self {
        self.system.after.extend_from_slice(deps);
        self
    }
}

struct SystemEntry<S> {
    stage: &'static str,
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
    stages: [Vec<&'static str>; 4],
    pending: Vec<System<S>>,
    phases: [Vec<SystemEntry<S>>; 4],
    resolved: bool,
    setup: Vec<SetupEntry<S>>,
    rest: Vec<RestEntry<S>>,
    visibility: Vec<VisibilityEntry<S>>,
    close: Vec<CloseEntry<S>>,
}

impl<S> Default for Schedule<S> {
    fn default() -> Self {
        Self {
            stages: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            pending: Vec::new(),
            phases: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            resolved: true,
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
            stages: self.stages.clone(),
            pending: self.pending.clone(),
            phases: self.phases.clone(),
            resolved: self.resolved,
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

    /// Declares a named execution stage inside `stage.phase`.
    ///
    /// Stages within each phase execute in declaration order. Duplicate stage names panic when
    /// the schedule is finalized via [`Self::resolve`].
    pub fn add_stage(&mut self, stage: Stage) {
        let sched = self.schedule_mut();
        sched.stages[stage.phase.index()].push(stage.name);
        sched.resolved = false;
    }

    /// Declares multiple execution stages in order.
    pub fn add_stages(&mut self, stages: &[Stage]) {
        for &stage in stages {
            self.add_stage(stage);
        }
    }

    /// Registers a per-tick system in its declared [`Stage`], returning a [`Scheduled`] handle
    /// for chaining explicit `after` edges.
    pub fn schedule(&mut self, stage: Stage, name: &'static str, run: SystemFn<S>) -> Scheduled<'_, S> {
        let sched = self.schedule_mut();
        let idx = sched.pending.len();
        sched.pending.push(System::new(stage, name, run));
        sched.resolved = false;
        Scheduled { system: &mut sched.pending[idx] }
    }

    /// Registers a pre-built [`System`] in its declared [`Stage`].
    pub fn schedule_system(&mut self, system: System<S>) {
        let sched = self.schedule_mut();
        sched.pending.push(system);
        sched.resolved = false;
    }

    /// Validates all declared stages, system names, and `after` edges, and resolves every phase
    /// into a deterministic, registration-order-independent execution list.
    ///
    /// Panics if:
    /// - any stage name is declared more than once,
    /// - any system targets an undeclared stage,
    /// - any system name is registered more than once,
    /// - any `after` dependency names an unknown system,
    /// - any `after` dependency points to a later phase or later stage, or
    /// - any `after` dependency forms a cycle (including self-dependencies).
    pub fn resolve(&mut self) {
        let Some(inner) = self.inner.as_mut() else { return };
        let sched = Arc::make_mut(inner);
        if sched.resolved {
            return;
        }

        // 1. Validate unique stage names and assign a monotonic global rank to every stage.
        let mut seen_stages: HashSet<&'static str> = HashSet::new();
        let mut stage_rank: HashMap<(Phase, &'static str), usize> = HashMap::new();
        let mut next_rank = 0usize;
        for phase in Phase::ALL {
            for &stage_name in &sched.stages[phase.index()] {
                assert!(
                    seen_stages.insert(stage_name),
                    "scheduler: duplicate stage '{stage_name}'"
                );
                stage_rank.insert((phase, stage_name), next_rank);
                next_rank += 1;
            }
        }

        // 2. Validate unique system names and declared stages.
        let mut by_name: HashMap<&'static str, usize> = HashMap::with_capacity(sched.pending.len());
        let mut ranks: Vec<usize> = Vec::with_capacity(sched.pending.len());
        for (idx, sys) in sched.pending.iter().enumerate() {
            let Some(&rank) = stage_rank.get(&(sys.stage.phase, sys.stage.name)) else {
                panic!(
                    "scheduler: system '{}' targets undeclared stage '{}' in phase {:?}",
                    sys.name, sys.stage.name, sys.stage.phase
                );
            };
            assert!(
                by_name.insert(sys.name, idx).is_none(),
                "scheduler: duplicate system name '{}'",
                sys.name
            );
            ranks.push(rank);
        }

        // 3. Validate all `after` edges and build intra-stage DAG adjacency lists.
        let n = sched.pending.len();
        let mut in_degree: Vec<usize> = vec![0; n];
        let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (idx, sys) in sched.pending.iter().enumerate() {
            let mut seen_deps: HashSet<&'static str> = HashSet::new();
            for &dep in &sys.after {
                assert!(
                    dep != sys.name,
                    "scheduler: cycle detected: system '{}' cannot run after itself",
                    sys.name
                );
                let Some(&dep_idx) = by_name.get(dep) else {
                    panic!(
                        "scheduler: system '{}' depends on missing system '{dep}'",
                        sys.name
                    );
                };
                if ranks[dep_idx] > ranks[idx] {
                    let target = &sched.pending[dep_idx];
                    panic!(
                        "scheduler: system '{}' ({:?}/{}) cannot run after '{}' in later stage ({:?}/{})",
                        sys.name,
                        sys.stage.phase,
                        sys.stage.name,
                        dep,
                        target.stage.phase,
                        target.stage.name
                    );
                }
                if ranks[dep_idx] == ranks[idx] && seen_deps.insert(dep) {
                    outgoing[dep_idx].push(idx);
                    in_degree[idx] += 1;
                }
            }
        }

        // 4. Resolve each `(phase, stage)` via Kahn's algorithm with lexicographical name tie-break.
        let mut stage_buckets: Vec<Vec<usize>> = vec![Vec::new(); next_rank];
        for (idx, &rank) in ranks.iter().enumerate() {
            stage_buckets[rank].push(idx);
        }
        for phase in Phase::ALL {
            let out = &mut sched.phases[phase.index()];
            out.clear();
            for &stage_name in &sched.stages[phase.index()] {
                let rank = stage_rank[&(phase, stage_name)];
                let bucket = &stage_buckets[rank];
                let mut ready: BTreeSet<&'static str> = BTreeSet::new();
                for &idx in bucket {
                    if in_degree[idx] == 0 {
                        ready.insert(sched.pending[idx].name);
                    }
                }
                let mut emitted = 0usize;
                while let Some(name) = ready.pop_first() {
                    let idx = by_name[name];
                    let sys = &sched.pending[idx];
                    out.push(SystemEntry { stage: stage_name, name: sys.name, run: sys.run });
                    emitted += 1;
                    for &next_idx in &outgoing[idx] {
                        in_degree[next_idx] -= 1;
                        if in_degree[next_idx] == 0 {
                            ready.insert(sched.pending[next_idx].name);
                        }
                    }
                }
                if emitted < bucket.len() {
                    let mut stuck: Vec<&'static str> = bucket
                        .iter()
                        .copied()
                        .filter(|&idx| in_degree[idx] > 0)
                        .map(|idx| sched.pending[idx].name)
                        .collect();
                    stuck.sort_unstable();
                    panic!(
                        "scheduler: dependency cycle in {:?}/{stage_name} involving: {}",
                        phase,
                        stuck.join(", ")
                    );
                }
            }
        }
        sched.resolved = true;
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
        let Some(inner) = self.inner.as_deref() else { return false };
        assert!(
            inner.resolved,
            "LifecycleScheduler: schedule has unresolved registrations; call resolve() or KernelBuilder::build() before running"
        );
        let mut changed = false;
        for system in &inner.phases[phase.index()] {
            if (system.run)(state, now, tx) {
                changed = true;
            }
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

    /// Iterates the resolved names of systems registered in `phase`.
    pub fn system_names(&self, phase: Phase) -> impl Iterator<Item = &'static str> + '_ {
        if let Some(inner) = self.inner.as_deref() {
            assert!(inner.resolved, "LifecycleScheduler: call resolve() before inspecting systems");
        }
        self.inner.as_deref().into_iter().flat_map(move |s| s.phases[phase.index()].iter().map(|entry| entry.name))
    }

    /// Iterates the resolved `(stage_name, system_name)` pairs in execution order for `phase`.
    pub fn resolved_systems(&self, phase: Phase) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        if let Some(inner) = self.inner.as_deref() {
            assert!(inner.resolved, "LifecycleScheduler: call resolve() before inspecting systems");
        }
        self.inner
            .as_deref()
            .into_iter()
            .flat_map(move |s| s.phases[phase.index()].iter().map(|entry| (entry.stage, entry.name)))
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
        if let Some(inner) = self.inner.as_deref() {
            assert!(inner.resolved, "LifecycleScheduler: call resolve() before inspecting systems");
        }
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
