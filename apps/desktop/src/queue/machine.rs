//! One session's queue, after Desktop's `session/composer/queue-machine.ts`. A queued prompt can
//! open in the composer for an edit (`editing`); changes run one at a time, in the order they
//! were asked for (`active`, then `pending`), each with its attempt, and an edit, steer or
//! removal leaves the composer free, so a row can be edited while one runs
//! (`editing-while-mutating`). A steered or removed row leaves the strip at once and comes back
//! if the server refuses; an edit shows its words, pending, in the original's place; a reorder
//! keeps its rows where they were dropped until the whole rewrite settles; an undone row stays,
//! pending, until the server lets the prompt go, and only then do its words go into the
//! composer. Rows the server confirmed gone stay hidden until its events catch up (`gone`), each
//! for at most `GONE_SECS`.

use opencode_kit::client::PathAttachment;
use opencode_kit::model::{Attachment, Delivery, InboxItem};
use rune::machine::{Attempt, Effects, Seq};
use session_view::composer::history::Entry;

/// `QUEUE_MUTATION_TIMEOUT_MS`: bounds an edit, reorder or undo (a steer or removal is one
/// request, bounded by the request itself).
pub const MUTATION_TIMEOUT_SECS: f64 = 60.0;
/// How long a confirmed row stays hidden waiting for the server's event (`queue.ts:181`).
pub const GONE_SECS: f64 = 30.0;

/// What the composer holds, or held before an edit opened (`EditStash`): its words and pills,
/// caret, files, paths and mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub entry: Entry,
    pub caret: usize,
    pub files: Vec<Attachment>,
    pub paths: Vec<PathAttachment>,
    pub shell: bool,
}

impl Draft {
    /// Nothing to send: no words, files or paths.
    pub fn is_empty(&self) -> bool {
        self.entry.text.trim().is_empty() && self.files.is_empty() && self.paths.is_empty()
    }
}

/// The queued prompt open in the composer (`ActiveEdit`). `stash` is the draft the composer held
/// before the first row opened; every way out of the edit puts it back. `loaded` is what the row
/// put in the composer.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub id: String,
    pub loaded: Draft,
    pub stash: Draft,
}

/// An edit confirmed from the composer (`EditOperation`). `moved` maps each inbox id the edit
/// replaces to the id admitted in its place: the original's and, for a queued edit, the prompts
/// after it, re-admitted to keep their order.
#[derive(Clone, Debug, PartialEq)]
pub struct EditOp {
    pub original: String,
    pub delivery: Delivery,
    /// The composer's content, which a failure hands back.
    pub prompt: Draft,
    /// The replacement's admission, as the server takes it.
    pub admission: serde_json::Value,
    pub text: String,
    pub files: usize,
    pub moved: Vec<(String, String)>,
    /// The server admitted every replacement, so a later failure keeps the edit.
    pub admitted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// A queued prompt steers now (Send once the turn stopped).
    Steer { id: String },
    /// A queued prompt goes.
    Remove { id: String },
    /// A queued prompt comes back to the composer, after what's typed there.
    Undo { id: String, text: String, files: Vec<Attachment> },
    /// A pending steer moves to the queue (its menu's Move to queue).
    Requeue { id: String },
    /// A pending steer goes (its menu's Delete).
    Delete { id: String },
    Edit(Box<EditOp>),
    /// The queue rewritten in `ids`' order. Its rows keep their keys and order until the whole
    /// rewrite settles, so admissions and cancellations can't grow or shrink the strip.
    Reorder { ids: Vec<String>, rows: Vec<Row>, moved: Vec<(String, String)> },
}

impl Operation {
    pub fn id(&self) -> &str {
        match self {
            Operation::Steer { id } | Operation::Remove { id } | Operation::Undo { id, .. } | Operation::Requeue { id } | Operation::Delete { id } => id,
            Operation::Edit(e) => &e.original,
            Operation::Reorder { .. } => "",
        }
    }

    /// The strip hides its row while it runs or waits.
    fn hides(&self) -> bool {
        matches!(self, Operation::Steer { .. } | Operation::Remove { .. })
    }

    /// Leaves the composer free, so a row can be edited while it runs (`isConcurrent`).
    fn concurrent(&self) -> bool {
        !matches!(self, Operation::Undo { .. } | Operation::Reorder { .. })
    }

    /// Has a time bound (`bounded`).
    fn bounded(&self) -> bool {
        matches!(self, Operation::Undo { .. } | Operation::Reorder { .. } | Operation::Edit(_))
    }

    fn moved(&self) -> &[(String, String)] {
        match self {
            Operation::Edit(e) => &e.moved,
            Operation::Reorder { moved, .. } => moved,
            _ => &[],
        }
    }
}

/// The change in flight, with its attempt and, for a bounded one, when it times out.
#[derive(Clone, Debug, PartialEq)]
pub struct Active {
    pub attempt: Attempt,
    pub operation: Operation,
    due: Option<f64>,
}

/// A row of the strip (`QueueRow`).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub text: String,
    pub files: usize,
    pub pending: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    Editing { edit: Edit },
    Mutating { active: Active, pending: Vec<Operation> },
    /// `active` leaves the composer free ([`Operation::concurrent`]).
    EditingWhileMutating { edit: Edit, active: Active, pending: Vec<Operation> },
}

/// How far a timed-out change got (`QueueMutationStage`): `Committed`, a step it can't take back
/// is in flight, so it waits for that; `Settled`, the server holds the whole change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Running,
    Committed,
    Settled,
}

#[derive(Clone, Debug, Default)]
pub struct Queue {
    pub state: State,
    /// Rows the server confirmed gone, until their expiry.
    gone: Vec<(String, f64)>,
    /// Each inbox id a finished edit or reorder replaced, with the id admitted in its place.
    moved: Vec<(String, String)>,
    seq: Seq,
}

pub enum Event {
    /// The server's queued prompts, in order (`inbox.synced`).
    Synced { ids: Vec<String> },
    EditStart { id: String, reverted: bool, draft: Draft, loaded: Draft },
    EditCancel,
    /// `ids`: fresh ids, one per queued prompt, for the replacements an edit admits.
    EditConfirm { delivery: Delivery, reverted: bool, prompt: Draft, admission: serde_json::Value, ids: Vec<String>, now: f64 },
    ReorderRequested { ids: Vec<String>, reverted: bool, rows: Vec<Row>, now: f64 },
    Requested { operation: Operation, now: f64 },
    UndoRequested { id: String, text: String, files: Vec<Attachment>, shell: bool, now: f64 },
    Moved { attempt: Attempt, moved: Vec<(String, String)> },
    Admitted { attempt: Attempt },
    Succeeded { attempt: Attempt, now: f64 },
    /// `hidden`: the inbox ids the failed change already withdrew or cancelled.
    Failed { attempt: Attempt, hidden: Vec<String>, now: f64 },
    /// The clock: expired gone rows show again, and a change past its bound ends as it got.
    Tick { now: f64, stage: Stage, hidden: Vec<String> },
    /// The composer goes to another page: an open edit gives the composer its draft back.
    Leave,
}

/// An edit confirmed from the composer, as [`Event::EditConfirm`] carries it.
struct Confirm {
    delivery: Delivery,
    reverted: bool,
    prompt: Draft,
    admission: serde_json::Value,
    ids: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// The composer takes the queued prompt, in normal mode, with the caret at its end.
    Load(Draft),
    /// The composer gets its earlier draft back; with `keep`, what was typed into the edit of
    /// that prompt joins it.
    Restore { stash: Draft, focus: bool, keep: Option<Draft> },
    /// Words join what the composer holds: an undone prompt's, or a failed edit's.
    Merge { text: String, files: Vec<Attachment>, paths: Vec<PathAttachment> },
    Run { attempt: Attempt, operation: Operation },
    /// Stops the change at its next step; a request already sent still finishes.
    Abort { attempt: Attempt },
    /// "Request failed".
    Failed,
    /// "Redo the revert before you reorder or edit queued prompts".
    Reverted,
    /// "Leave shell mode before undoing a queued prompt".
    UndoShell,
}

impl Queue {
    /// Whether a change runs: the strip's actions wait.
    pub fn busy(&self) -> bool {
        self.active().is_some()
    }

    /// The row open in the composer.
    pub fn editing(&self) -> Option<&str> {
        self.edit().map(|e| e.id.as_str())
    }

    pub fn edit(&self) -> Option<&Edit> {
        match &self.state {
            State::Editing { edit } | State::EditingWhileMutating { edit, .. } => Some(edit),
            _ => None,
        }
    }

    /// Whether an undo runs, which holds the composer's submit.
    pub fn undoing(&self) -> bool {
        matches!(&self.state, State::Mutating { active: Active { operation: Operation::Undo { .. }, .. }, .. })
    }

    /// Whether `id` has a change running or waiting, which its menu's actions wait for.
    pub fn acting(&self, id: &str) -> bool {
        self.operations().any(|o| o.id() == id)
    }

    fn active(&self) -> Option<&Active> {
        match &self.state {
            State::Mutating { active, .. } | State::EditingWhileMutating { active, .. } => Some(active),
            _ => None,
        }
    }

    /// The change in flight, which `Incoming` results must match.
    pub fn running(&self) -> Option<(Attempt, &Operation)> {
        self.active().map(|a| (a.attempt, &a.operation))
    }

    /// When the next timer is due: a change's bound or a gone row's expiry.
    pub fn due(&self) -> Option<f64> {
        self.gone.iter().map(|(_, e)| *e).chain(self.active().and_then(|a| a.due)).reduce(f64::min)
    }

    fn operations(&self) -> impl Iterator<Item = &Operation> {
        let (active, pending) = match &self.state {
            State::Mutating { active, pending } | State::EditingWhileMutating { active, pending, .. } => (Some(&active.operation), pending.as_slice()),
            _ => (None, &[][..]),
        };
        active.into_iter().chain(pending)
    }

    /// Inbox ids replaced by an edit or reorder, finished or in flight, with their replacements.
    pub fn moved(&self) -> Vec<(String, String)> {
        let mut out = self.moved.clone();
        if let Some(a) = self.active() {
            out.extend(a.operation.moved().iter().cloned());
        }
        out
    }

    /// The id a row first had before edits and reorders replaced it, which keys its element.
    pub fn origin(&self, id: &str) -> String {
        let moved = self.moved();
        let mut at = id.to_owned();
        for _ in 0..moved.len() {
            match moved.iter().find(|(_, next)| *next == at) {
                Some((old, _)) => at.clone_from(old),
                None => break,
            }
        }
        at
    }

    /// The strip's rows (`selectQueueRows`): the queued prompts, less those a change took out,
    /// with edits in flight showing their words, pending, and a reorder's rows where they were
    /// dropped.
    pub fn rows(&self, items: &[&InboxItem], label: impl Fn(&InboxItem) -> (String, usize)) -> Vec<Row> {
        let operations: Vec<&Operation> = self.operations().collect();
        let hidden: Vec<&str> = self.gone.iter().map(|(g, _)| g.as_str()).chain(operations.iter().filter(|o| o.hides()).map(|o| o.id())).collect();
        let edits: Vec<&EditOp> = operations.iter().filter_map(|o| if let Operation::Edit(e) = o { Some(&**e) } else { None }).collect();
        let queued: Vec<&&InboxItem> = items.iter().filter(|i| !hidden.contains(&i.id.as_str())).collect();
        let listed = |id: &str| queued.iter().any(|i| i.id == id);
        let moves: Vec<&(String, String)> = edits.iter().flat_map(|e| e.moved.iter()).collect();
        let rows: Vec<Row> = queued
            .iter()
            .filter_map(|item| {
                let id = item.id.as_str();
                // A re-admitted copy stays hidden while the row it replaces is listed, so the strip
                // keeps its order; a steered edit leaves the strip once the server admits it.
                if moves.iter().any(|(old, next)| next == id && listed(old)) {
                    return None;
                }
                if edits.iter().any(|e| e.original == id && e.admitted && e.delivery == Delivery::Steer) {
                    return None;
                }
                let edit = edits.iter().rev().find(|e| e.original == id || e.moved.iter().any(|(o, n)| *o == e.original && n == id));
                let (text, files) = edit.map_or_else(|| label(item), |e| (e.text.clone(), e.files));
                Some(Row { id: id.to_owned(), text, files, pending: moves.iter().any(|(old, next)| old == id || next == id) })
            })
            .collect();
        match self.active().map(|a| &a.operation) {
            Some(Operation::Undo { id, .. }) => rows.into_iter().map(|r| Row { pending: r.pending || r.id == *id, ..r }).collect(),
            Some(Operation::Reorder { rows: kept, moved, .. }) => {
                // Fresh prompts still appear while the reordered rows keep their places.
                let covered = |id: &str| kept.iter().any(|r| r.id == id) || moved.iter().any(|(_, n)| n == id);
                kept.iter().filter(|r| !hidden.contains(&r.id.as_str())).cloned().chain(rows.into_iter().filter(|r| !covered(&r.id))).collect()
            }
            _ => rows,
        }
    }

    pub fn step(&mut self, event: Event, rows: &[Row]) -> Effects<Effect> {
        match event {
            Event::Synced { ids } => self.synced(&ids),
            Event::EditStart { id, reverted, draft, loaded } => self.start_edit(id, reverted, draft, loaded, rows),
            Event::EditCancel => match self.close_edit() {
                Some(edit) => Effects::one(Effect::Restore { stash: edit.stash, focus: true, keep: None }),
                None => Effects::none(),
            },
            Event::Leave => match self.close_edit() {
                Some(edit) => Effects::one(Effect::Restore { stash: edit.stash, focus: false, keep: None }),
                None => Effects::none(),
            },
            Event::EditConfirm { delivery, reverted, prompt, admission, ids, now } => self.confirm(Confirm { delivery, reverted, prompt, admission, ids }, rows, now),
            Event::ReorderRequested { ids, reverted, rows: current, now } => self.reorder(ids, reverted, current, now),
            Event::Requested { operation, now } => self.request(operation, rows, now),
            Event::UndoRequested { id, text, files, shell, now } => {
                if !matches!(self.state, State::Idle) || !rows.iter().any(|r| r.id == id && !r.pending) {
                    return Effects::none();
                }
                if shell {
                    return Effects::one(Effect::UndoShell);
                }
                self.enqueue(Operation::Undo { id, text, files }, now)
            }
            Event::Moved { attempt, moved } => {
                if let Some(Operation::Edit(e)) = self.current_mut(attempt) {
                    e.moved = moved;
                } else if let Some(Operation::Reorder { moved: m, .. }) = self.current_mut(attempt) {
                    *m = moved;
                }
                Effects::none()
            }
            Event::Admitted { attempt } => {
                if let Some(Operation::Edit(e)) = self.current_mut(attempt) {
                    e.admitted = true;
                }
                Effects::none()
            }
            Event::Succeeded { attempt, now } => self.succeeded(attempt, now),
            Event::Failed { attempt, hidden, now } => self.failed(attempt, hidden, now),
            Event::Tick { now, stage, hidden } => {
                self.gone.retain(|(_, expiry)| *expiry > now);
                let Some(attempt) = self.active().filter(|a| a.due.is_some_and(|d| now + 1e-9 >= d)).map(|a| a.attempt) else { return Effects::none() };
                // A change past its bound ends as it got: one whose irreversible step is in
                // flight waits for that, a finished one succeeds, and anything else fails.
                if stage == Stage::Committed {
                    if let State::Mutating { active, .. } | State::EditingWhileMutating { active, .. } = &mut self.state {
                        active.due = None;
                    }
                    return Effects::none();
                }
                let mut effects = Effects::one(Effect::Abort { attempt });
                effects.extend(if stage == Stage::Settled { self.succeeded(attempt, now) } else { self.failed(attempt, hidden, now) });
                effects
            }
        }
    }

    fn current_mut(&mut self, attempt: Attempt) -> Option<&mut Operation> {
        match &mut self.state {
            State::Mutating { active, .. } | State::EditingWhileMutating { active, .. } if active.attempt == attempt => Some(&mut active.operation),
            _ => None,
        }
    }

    fn synced(&mut self, ids: &[String]) -> Effects<Effect> {
        self.gone.retain(|(g, _)| ids.contains(g));
        // A replacement in flight can continue a finished one's chain.
        let chains = self.moved();
        let reaches = |start: &str| {
            let mut at = start;
            for _ in 0..chains.len() {
                match chains.iter().find(|(o, _)| o == at) {
                    Some((_, next)) if ids.contains(next) => return true,
                    Some((_, next)) => at = next,
                    None => return false,
                }
            }
            false
        };
        let kept: Vec<(String, String)> = self.moved.iter().filter(|(_, next)| ids.contains(next) || reaches(next)).cloned().collect();
        self.moved = kept;
        let Some(edit) = self.edit().filter(|e| !ids.contains(&e.id)) else { return Effects::none() };
        // The server delivered or cancelled the prompt being edited: the composer gets its
        // earlier draft back, and keeps whatever was typed into the edit.
        let keep = Some(edit.loaded.clone());
        let edit = self.close_edit().expect("an edit is open");
        Effects::one(Effect::Restore { stash: edit.stash, focus: true, keep })
    }

    fn start_edit(&mut self, id: String, reverted: bool, draft: Draft, loaded: Draft, rows: &[Row]) -> Effects<Effect> {
        // Other rows stay editable while an edit, steer or removal runs; the row it's changing
        // ignores clicks and shows pending until the server catches up.
        if self.active().is_some_and(|a| !a.operation.concurrent()) || !rows.iter().any(|r| r.id == id && !r.pending) {
            return Effects::none();
        }
        // Re-admitting a queued prompt commits a staged revert, which drops every prompt queued
        // after its boundary.
        if reverted {
            return Effects::one(Effect::Reverted);
        }
        if self.editing() == Some(id.as_str()) {
            return Effects::none();
        }
        let stash = self.edit().map_or(draft, |e| e.stash.clone());
        let edit = Edit { id, loaded: loaded.clone(), stash };
        self.state = match std::mem::take(&mut self.state) {
            State::Idle | State::Editing { .. } => State::Editing { edit },
            State::Mutating { active, pending } | State::EditingWhileMutating { active, pending, .. } => State::EditingWhileMutating { edit, active, pending },
        };
        Effects::one(Effect::Load(loaded))
    }

    fn close_edit(&mut self) -> Option<Edit> {
        let (edit, next) = match std::mem::take(&mut self.state) {
            State::Editing { edit } => (edit, State::Idle),
            State::EditingWhileMutating { edit, active, pending } => (edit, State::Mutating { active, pending }),
            other => {
                self.state = other;
                return None;
            }
        };
        self.state = next;
        Some(edit)
    }

    fn confirm(&mut self, c: Confirm, rows: &[Row], now: f64) -> Effects<Effect> {
        let Confirm { delivery, reverted, prompt, admission, ids } = c;
        let Some(edit) = self.edit().cloned() else { return Effects::none() };
        if reverted {
            return Effects::one(Effect::Reverted);
        }
        self.close_edit();
        // The composer returns to the draft it held before the edit and the strip shows the
        // edited prompt as pending; a failure hands the edit back.
        let mut effects = Effects::one(Effect::Restore { stash: edit.stash.clone(), focus: true, keep: None });
        if prompt.is_empty() {
            return effects;
        }
        let pristine = prompt.entry.text.trim() == edit.loaded.entry.text.trim() && prompt.files == edit.loaded.files && prompt.paths == edit.loaded.paths;
        if pristine && delivery == Delivery::Queue {
            return effects;
        }
        // An unchanged prompt steers in place instead of being replaced.
        if pristine {
            effects.extend(self.enqueue(Operation::Steer { id: edit.id }, now));
            return effects;
        }
        // A queued edit re-admits the prompts after it, so they're pending too.
        let position = rows.iter().position(|r| r.id == edit.id);
        let after: Vec<&Row> = match (delivery, position) {
            (Delivery::Queue, Some(at)) => rows[at + 1..].iter().collect(),
            _ => Vec::new(),
        };
        let moved = std::iter::once(edit.id.clone()).chain(after.iter().map(|r| r.id.clone())).zip(ids).collect();
        let text = prompt.entry.text.trim().to_owned();
        let files = prompt.files.len() + prompt.paths.len();
        let op = EditOp { original: edit.id, delivery, prompt, admission, text, files, moved, admitted: false };
        effects.extend(self.enqueue(Operation::Edit(Box::new(op)), now));
        effects
    }

    fn reorder(&mut self, ids: Vec<String>, reverted: bool, current: Vec<Row>, now: f64) -> Effects<Effect> {
        if !matches!(self.state, State::Idle | State::Editing { .. }) {
            return Effects::none();
        }
        if reverted {
            return Effects::one(Effect::Reverted);
        }
        let rows: Vec<Row> = ids.iter().filter_map(|id| current.iter().find(|r| r.id == *id).cloned()).collect();
        if rows.is_empty() {
            return Effects::none();
        }
        // The rewrite can replace the row being edited, so the edit closes first and keeps what
        // was typed into it.
        let mut effects = Effects::none();
        if let Some(edit) = self.close_edit() {
            effects.push(Effect::Restore { stash: edit.stash, focus: true, keep: Some(edit.loaded) });
        }
        effects.extend(self.enqueue(Operation::Reorder { ids, rows, moved: Vec::new() }, now));
        effects
    }

    fn request(&mut self, operation: Operation, rows: &[Row], now: f64) -> Effects<Effect> {
        // Pending steers (Requeue, Delete) have no row in the strip; a row already changing
        // ignores another press.
        let strip = matches!(operation, Operation::Steer { .. } | Operation::Remove { .. });
        if (strip && !rows.iter().any(|r| r.id == operation.id() && !r.pending)) || self.acting(operation.id()) || self.gone.iter().any(|(g, _)| g == operation.id()) {
            return Effects::none();
        }
        let mut effects = Effects::none();
        if self.editing() == Some(operation.id()) {
            let edit = self.close_edit().expect("an edit is open");
            effects.push(Effect::Restore { stash: edit.stash, focus: true, keep: None });
        }
        effects.extend(self.enqueue(operation, now));
        effects
    }

    fn enqueue(&mut self, operation: Operation, now: f64) -> Effects<Effect> {
        match &mut self.state {
            State::Mutating { pending, .. } | State::EditingWhileMutating { pending, .. } => {
                pending.push(operation);
                Effects::none()
            }
            State::Idle | State::Editing { .. } => self.start(operation, Vec::new(), now),
        }
    }

    /// `operation` runs, with `pending` waiting behind it.
    fn start(&mut self, operation: Operation, pending: Vec<Operation>, now: f64) -> Effects<Effect> {
        let attempt = self.seq.next();
        let due = operation.bounded().then_some(now + MUTATION_TIMEOUT_SECS);
        let active = Active { attempt, operation: operation.clone(), due };
        self.state = match std::mem::take(&mut self.state) {
            State::Editing { edit } | State::EditingWhileMutating { edit, .. } => State::EditingWhileMutating { edit, active, pending },
            State::Idle | State::Mutating { .. } => State::Mutating { active, pending },
        };
        Effects::one(Effect::Run { attempt, operation })
    }

    fn succeeded(&mut self, attempt: Attempt, now: f64) -> Effects<Effect> {
        let Some(active) = self.active().filter(|a| a.attempt == attempt).cloned() else { return Effects::none() };
        let confirmed: Vec<String> = match &active.operation {
            Operation::Edit(e) => e.moved.iter().map(|(o, _)| o.clone()).collect(),
            Operation::Reorder { moved, .. } => moved.iter().map(|(o, _)| o.clone()).collect(),
            other => vec![other.id().to_owned()],
        };
        self.hide(confirmed, now);
        let moved = active.operation.moved().to_vec();
        self.moved.extend(moved.iter().cloned());
        let mut effects = self.advance(&moved, now);
        if let Operation::Undo { text, files, .. } = active.operation {
            effects.push(Effect::Merge { text, files, paths: Vec::new() });
        }
        effects
    }

    fn failed(&mut self, attempt: Attempt, hidden: Vec<String>, now: f64) -> Effects<Effect> {
        let Some(active) = self.active().filter(|a| a.attempt == attempt).cloned() else { return Effects::none() };
        self.hide(hidden, now);
        let mut effects = Effects::one(Effect::Failed);
        // Nothing typed is lost: an edit that never reached the server returns to the composer,
        // merged into what it holds now, or into the draft an open edit stashed.
        let returned = match active.operation {
            Operation::Edit(e) if !e.admitted => Some(e.prompt),
            _ => None,
        };
        match (returned, &mut self.state) {
            (Some(prompt), State::EditingWhileMutating { edit, .. }) => merge(&mut edit.stash, prompt),
            (Some(prompt), _) => effects.push(Effect::Merge { text: prompt.entry.text, files: prompt.files, paths: prompt.paths }),
            (None, _) => {}
        }
        effects.extend(self.advance(&[], now));
        effects
    }

    fn hide(&mut self, ids: Vec<String>, now: f64) {
        self.gone.extend(ids.into_iter().map(|id| (id, now + GONE_SECS)));
    }

    /// The next waiting change starts. Prompts the finished one re-admitted carry over to their
    /// new ids, so a change asked for meanwhile never targets an id the rewrite cancelled.
    fn advance(&mut self, moved: &[(String, String)], now: f64) -> Effects<Effect> {
        let remap = |id: &mut String| {
            if let Some((_, next)) = moved.iter().find(|(o, _)| o == id) {
                id.clone_from(next);
            }
        };
        let (edit, pending) = match std::mem::take(&mut self.state) {
            State::Mutating { pending, .. } => (None, pending),
            State::EditingWhileMutating { edit, pending, .. } => (Some(edit), pending),
            other => {
                self.state = other;
                return Effects::none();
            }
        };
        let mut rest = pending.into_iter().map(|mut op| {
            match &mut op {
                Operation::Steer { id } | Operation::Remove { id } | Operation::Requeue { id } | Operation::Delete { id } | Operation::Undo { id, .. } => remap(id),
                Operation::Edit(e) => {
                    remap(&mut e.original);
                    for (old, _) in &mut e.moved {
                        remap(old);
                    }
                }
                Operation::Reorder { .. } => {}
            }
            op
        });
        self.state = match &edit {
            Some(edit) => State::Editing { edit: edit.clone() },
            None => State::Idle,
        };
        match rest.next() {
            None => Effects::none(),
            Some(next) => self.start(next, rest.collect(), now),
        }
    }
}

/// A failed edit's words join the draft an open edit stashed (`mergePrompt`).
fn merge(into: &mut Draft, prompt: Draft) {
    into.entry.text = merged_text(&into.entry.text, &prompt.entry.text);
    into.caret = into.entry.text.len();
    into.files.extend(prompt.files);
    into.paths.extend(prompt.paths);
}

/// `appendPrompt`: a paragraph break between two prompts with words; a prompt of only files adds
/// none.
pub fn merged_text(draft: &str, prompt: &str) -> String {
    match (draft.trim_end().is_empty(), prompt.trim().is_empty()) {
        (true, _) => prompt.to_owned(),
        (false, true) => draft.to_owned(),
        (false, false) => format!("{}\n\n{prompt}", draft.trim_end()),
    }
}
