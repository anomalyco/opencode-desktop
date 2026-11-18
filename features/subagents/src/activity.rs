//! The running-work list's machine, as Desktop's `shell-panel-machine.ts`: shut, open on the
//! list, or open on one shell, whose phase runs from running through ending (it left the
//! running list and the panel waits to hear how) to ended (exited, timed out or killed). Stops
//! in flight are shared by the list's rows and the shell panel. `step` returns the effects
//! (`stop.run`, `ended.load`, timers, the shown shell's output reads) for the caller to run.

use std::collections::BTreeMap;

use crate::shells::tail::{self, Tail, TailEffect, TailEvent};
use crate::shells::Shell;

/// A Stop that hasn't settled by then counts as failed (`SHELL_STOP_TIMEOUT_MS`).
pub const STOP_TIMEOUT_MS: i64 = 10_000;
/// How long a shown shell that left the list waits to hear how it ended (`SHELL_END_TIMEOUT_MS`).
pub const END_TIMEOUT_MS: i64 = 5_000;

/// Localized parts of the activity capsule's label ("2 subagents · 1 shell").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub subagents: String,
    pub shells: String,
    pub between: String,
    pub label: String,
}

/// Formats the localized subagent and shell counts, the separator between them, and the combined
/// accessible label (`session.activity.summary`).
pub fn summary(subs: usize, sh: usize) -> Summary {
    let subagents = opencode_i18n::plural!("session.activity.subagents", subs);
    let shells = opencode_i18n::plural!("session.activity.shells", sh);
    // The summary's words between its two counts, in the language's order.
    let template = opencode_i18n::t!("session.activity.summary", subagents = "\u{1}", shells = "\u{2}");
    let between = template.split(['\u{1}', '\u{2}']).nth(1).unwrap_or("").trim().to_owned();
    let label = match (subs, sh) {
        (0, _) => shells.clone(),
        (_, 0) => subagents.clone(),
        _ => opencode_i18n::t!("session.activity.summary", subagents = subagents.clone(), shells = shells.clone()).to_string(),
    };
    Summary { subagents, shells, between, label }
}

/// How a shell ended (`ShellEnd`): `completed` freezes its run time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct End {
    pub status: EndStatus,
    pub exit: Option<i64>,
    pub completed: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndStatus {
    Exited,
    Timeout,
    Killed,
}

/// What `GET /api/shell/:id` says of a shell that left the list (`ShellEndInfo`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EndInfo {
    pub status: EndStatus,
    pub exit: Option<i64>,
    pub completed: Option<i64>,
}

/// `ShellPanelPhase`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Running,
    Ending { attempt: u64 },
    Ended(End),
}

/// `ShellPanelState`, less what Rune keeps elsewhere (focus moves, the capsule's fade-in).
/// `back_lit`: Back holds the menu's highlight, focus having moved there, until the pointer
/// leaves it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Panel {
    #[default]
    Closed,
    List,
    Shell { shell: Shell, phase: Phase, back_lit: bool, tail: Tail },
}

/// The machine: the panel, the counter its attempts come from, and the Stops in flight by row
/// key (a shell's id, a subagent's session) with their attempts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    pub panel: Panel,
    issued: u64,
    pub stopping: BTreeMap<String, u64>,
}

/// What runs in the background now (`ShellPanelWork`): row keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Work {
    pub enabled: bool,
    pub subagents: Vec<String>,
    pub shells: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    WorkSynced,
    Toggled(bool),
    Selected(Shell),
    Back,
    StopRequested { key: String, subagent: bool },
    StopSettled { attempt: u64, stopped: bool, now: i64 },
    StopTimedOut { attempt: u64 },
    EndedSettled { attempt: u64, info: Option<EndInfo>, now: i64 },
    EndedTimedOut { attempt: u64, now: i64 },
    /// The pointer left Back.
    BackLeft,
    Tail(TailEvent),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Timer {
    Stopping,
    Ending,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    RunStop { attempt: u64, key: String, subagent: bool },
    LoadEnded { attempt: u64, id: String },
    TimerStart { attempt: u64, timer: Timer },
    /// `None` cancels every timer.
    TimerCancel(Option<u64>),
    Tail(TailEffect),
}

impl Activity {
    pub fn is_open(&self) -> bool {
        !matches!(self.panel, Panel::Closed)
    }

    /// The shown shell, its phase, and the output read so far.
    pub fn shown(&self) -> Option<(&Shell, Phase, &tail::Text, bool)> {
        match &self.panel {
            Panel::Shell { shell, phase, tail, back_lit } => Some((shell, *phase, tail.text(), *back_lit)),
            _ => None,
        }
    }
}

/// A shell that ended while its panel is open still counts until the list closes
/// (`selectShellPanelView`'s `held`).
pub fn held(state: &Activity, work: &Work) -> bool {
    matches!(&state.panel, Panel::Shell { shell, .. } if !work.shells.contains(&shell.id))
}

/// `transitionShellPanel`.
pub fn step(state: Activity, event: Event, work: &Work) -> (Activity, Vec<Effect>) {
    let before = tail_running(&state);
    let (mut next, mut effects) = transition(state, event, work);
    // Output stops polling once the shown shell leaves the running list; its final read lands.
    let after = tail_running(&next);
    if before != after
        && after.is_some()
        && let Panel::Shell { tail, .. } = &mut next.panel
    {
        let (t, e) = tail::step(std::mem::replace(tail, Tail::Complete { text: Default::default() }), TailEvent::Running(after == Some(true)));
        *tail = t;
        effects.extend(e.into_iter().map(Effect::Tail));
    }
    (next, effects)
}

fn transition(state: Activity, event: Event, work: &Work) -> (Activity, Vec<Effect>) {
    match event {
        Event::WorkSynced => work_synced(state, work),
        Event::StopRequested { key, subagent } => request_stop(state, key, subagent, work),
        Event::StopSettled { attempt, stopped, now } => settle_stop(state, attempt, Some((stopped, now)), work),
        Event::StopTimedOut { attempt } => settle_stop(state, attempt, None, work),
        Event::Toggled(false) if state.panel == Panel::Closed => (state, Vec::new()),
        Event::Toggled(false) => close(state),
        Event::Toggled(true) if state.panel != Panel::Closed || !work.enabled || count(work) == 0 => (state, Vec::new()),
        Event::Toggled(true) => (Activity { panel: Panel::List, issued: state.issued + 1, ..state }, Vec::new()),
        Event::Selected(shell) => {
            let issued = state.issued + 1;
            let mut effects = leave(&state);
            let running = state.stopping.contains_key(&shell.id) || work.shells.contains(&shell.id);
            let (tail, reads) = tail::start(running);
            effects.extend(reads.into_iter().map(Effect::Tail));
            // A shell whose Stop is in flight keeps its panel running until the Stop settles.
            let phase = if running { Phase::Running } else { Phase::Ending { attempt: issued } };
            if !running {
                effects.extend(ending(issued, &shell.id));
            }
            (Activity { panel: Panel::Shell { shell, phase, back_lit: true, tail }, issued, ..state }, effects)
        }
        Event::Back => match state.panel {
            Panel::Shell { .. } if count(work) == 0 => close(state),
            Panel::Shell { .. } => {
                let effects = leave(&state);
                (Activity { panel: Panel::List, issued: state.issued + 1, ..state }, effects)
            }
            _ => (state, Vec::new()),
        },
        Event::EndedSettled { attempt, info, now } => ended(state, attempt, info, now),
        Event::EndedTimedOut { attempt, now } => ended(state, attempt, None, now),
        Event::BackLeft => match state.panel {
            Panel::Shell { shell, phase, tail, .. } => (Activity { panel: Panel::Shell { shell, phase, back_lit: false, tail }, ..state }, Vec::new()),
            _ => (state, Vec::new()),
        },
        Event::Tail(e) => match state.panel {
            Panel::Shell { shell, phase, back_lit, tail } => {
                let (tail, effects) = tail::step(tail, e);
                (Activity { panel: Panel::Shell { shell, phase, back_lit, tail }, ..state }, effects.into_iter().map(Effect::Tail).collect())
            }
            _ => (state, Vec::new()),
        },
    }
}

fn ended(state: Activity, a: u64, info: Option<EndInfo>, now: i64) -> (Activity, Vec<Effect>) {
    match state.panel {
        Panel::Shell { shell, phase: Phase::Ending { attempt }, back_lit, tail } if attempt == a => {
            // Removing a running shell ends it as killed, and the server forgets it.
            let end = End { status: info.map_or(EndStatus::Killed, |i| i.status), exit: info.and_then(|i| i.exit), completed: info.and_then(|i| i.completed).unwrap_or(now) };
            (Activity { panel: Panel::Shell { shell, phase: Phase::Ended(end), back_lit, tail }, ..state }, vec![Effect::TimerCancel(Some(a))])
        }
        panel => (Activity { panel, ..state }, Vec::new()),
    }
}

fn work_synced(state: Activity, work: &Work) -> (Activity, Vec<Effect>) {
    match &state.panel {
        Panel::Closed => (state, Vec::new()),
        Panel::List if active(&state, work) => (state, Vec::new()),
        Panel::List => close(state),
        Panel::Shell { .. } if !work.enabled => close(state),
        // A shell being stopped waits for its Stop to report how it ended.
        Panel::Shell { shell, phase, .. } if *phase != Phase::Running || state.stopping.contains_key(&shell.id) || work.shells.contains(&shell.id) => (state, Vec::new()),
        Panel::Shell { .. } => to_ending(state),
    }
}

fn request_stop(state: Activity, key: String, subagent: bool, work: &Work) -> (Activity, Vec<Effect>) {
    let stoppable = if subagent { work.subagents.contains(&key) } else { work.shells.contains(&key) };
    if state.stopping.contains_key(&key) || !stoppable {
        return (state, Vec::new());
    }
    // A shown shell that already left the list is ending on its own; stopping it would report it stopped.
    if matches!(&state.panel, Panel::Shell { shell, phase, .. } if shell.id == key && *phase != Phase::Running) {
        return (state, Vec::new());
    }
    let issued = state.issued + 1;
    let mut stopping = state.stopping;
    stopping.insert(key.clone(), issued);
    (Activity { issued, stopping, ..state }, vec![Effect::RunStop { attempt: issued, key, subagent }, Effect::TimerStart { attempt: issued, timer: Timer::Stopping }])
}

/// A Stop settled (`Some((stopped, now))`) or timed out.
fn settle_stop(state: Activity, attempt: u64, settled: Option<(bool, i64)>, work: &Work) -> (Activity, Vec<Effect>) {
    let Some(key) = state.stopping.iter().find(|(_, a)| **a == attempt).map(|(k, _)| k.clone()) else { return (state, Vec::new()) };
    let mut stopping = state.stopping.clone();
    stopping.remove(&key);
    let cancel = Effect::TimerCancel(Some(attempt));
    let shown = matches!(&state.panel, Panel::Shell { shell, phase: Phase::Running, .. } if shell.id == key);
    let state = Activity { stopping, ..state };
    if !shown {
        return (state, vec![cancel]);
    }
    if let Some((true, now)) = settled
        && let Panel::Shell { shell, back_lit, tail, .. } = state.panel
    {
        let end = End { status: EndStatus::Killed, exit: None, completed: now };
        return (Activity { panel: Panel::Shell { shell, phase: Phase::Ended(end), back_lit, tail }, ..state }, vec![cancel]);
    }
    // A Stop that failed leaves the shell running, unless it left the list meanwhile and ends on its own.
    if work.shells.contains(&key) {
        return (state, vec![cancel]);
    }
    let (state, mut effects) = to_ending(state);
    effects.insert(0, cancel);
    (state, effects)
}

fn to_ending(state: Activity) -> (Activity, Vec<Effect>) {
    let issued = state.issued + 1;
    match state.panel {
        Panel::Shell { shell, back_lit, tail, .. } => {
            let effects = ending(issued, &shell.id);
            (Activity { panel: Panel::Shell { shell, phase: Phase::Ending { attempt: issued }, back_lit, tail }, issued, ..state }, effects)
        }
        panel => (Activity { panel, ..state }, Vec::new()),
    }
}

/// The menu always opens on the list, and closing forgets the shell so nothing follows it while
/// the menu is shut. Stops in flight carry on.
fn close(state: Activity) -> (Activity, Vec<Effect>) {
    let effects = leave(&state);
    (Activity { panel: Panel::Closed, issued: state.issued + 1, ..state }, effects)
}

/// Leaving a view drops the shown shell's end lookup.
fn leave(state: &Activity) -> Vec<Effect> {
    match &state.panel {
        Panel::Shell { phase: Phase::Ending { attempt }, .. } => vec![Effect::TimerCancel(Some(*attempt))],
        _ => Vec::new(),
    }
}

fn ending(attempt: u64, id: &str) -> Vec<Effect> {
    vec![Effect::LoadEnded { attempt, id: id.to_owned() }, Effect::TimerStart { attempt, timer: Timer::Ending }]
}

/// Whether the shown shell's output is followed as running (`tailRunning`), if one is shown.
fn tail_running(state: &Activity) -> Option<bool> {
    match &state.panel {
        Panel::Shell { phase, .. } => Some(*phase == Phase::Running),
        _ => None,
    }
}

fn active(state: &Activity, work: &Work) -> bool {
    work.enabled && count(work) + usize::from(held(state, work)) > 0
}

fn count(work: &Work) -> usize {
    work.subagents.len() + work.shells.len()
}
