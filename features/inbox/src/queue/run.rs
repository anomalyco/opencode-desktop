//! Running a queue change against the server (`queue.ts` `run`). A steer, move, removal or undo
//! is one tracked write, answered as `Incoming::Settled`. An edit or reorder is a chain of
//! requests (`client::rewrite`) that reports how far it got as it goes: the ids it re-admits
//! under, the admission of the edit, the step it can't take back, and what it withdrew. Once it
//! ends, the server's queue is read again, and only then does the machine hear the result, so
//! the rows keep their places until the server has caught up.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use opencode_kit::client::rewrite::{Progress, Replace};
use opencode_kit::client::{Server, Ticket, TicketKind, Write};
use opencode_kit::model::{Delivery, InboxItem};
use opencode_kit::wire_error::WireError;
use rune::machine::Attempt;

use super::QueueHost;
use super::machine::{EditOp, Event, Operation, Stage};

/// What a chain in flight has done so far (`MutationProgress`): how far it got, the ids it
/// withdrew or cancelled, and its stop.
#[derive(Default)]
pub struct Run {
    stage: Stage,
    hidden: Vec<String>,
    abort: Arc<AtomicBool>,
}

/// Each chain in flight, by session and attempt.
#[derive(Default)]
pub struct Runs(HashMap<(String, u32), Arc<Mutex<Run>>>);

impl Runs {
    /// How far the chain at `attempt` got, for its bound.
    pub fn progress(&self, session: &str, attempt: Attempt) -> (Stage, Vec<String>) {
        self.0.get(&(session.to_owned(), attempt.0)).map_or((Stage::Running, Vec::new()), |r| {
            let r = r.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            (r.stage, r.hidden.clone())
        })
    }

    /// Stops the chain at `attempt` at its next step.
    pub fn abort(&self, session: &str, attempt: Attempt) {
        if let Some(r) = self.0.get(&(session.to_owned(), attempt.0)) {
            r.lock().unwrap_or_else(std::sync::PoisonError::into_inner).abort.store(true, Ordering::SeqCst);
        }
    }

    pub(crate) fn insert(&mut self, session: String, attempt: Attempt, run: Arc<Mutex<Run>>) {
        self.0.insert((session, attempt.0), run);
    }

    pub(crate) fn remove(&mut self, session: &str, attempt: Attempt) {
        self.0.remove(&(session.to_owned(), attempt.0));
    }
}

/// Off-thread queue edit or reorder rewrite chain executed via [`execute_rewrite`].
pub struct RewriteTask {
    session: String,
    attempt: Attempt,
    operation: Operation,
    run: Arc<Mutex<Run>>,
    abort: Arc<AtomicBool>,
}

/// Background work or deferred reply scheduled by [`QueueHost::spawn_queue_job`].
pub enum QueueJob {
    /// Run a queue edit/reorder chain off the UI thread with [`execute_rewrite`], delivering
    /// intermediate [`QueueReply::Progress`] updates and the final [`QueueReply::Finished`] back
    /// on the UI thread via [`apply_reply`].
    Rewrite(RewriteTask),
    /// Deliver a read-only/disconnected failure on the next UI poll via [`apply_reply`].
    DeferFailed { session: String, attempt: Attempt, now: f64 },
}

/// Progress update or completion from a [`QueueJob`], applied on the UI thread by [`apply_reply`].
pub enum QueueReply {
    Progress { session: String, attempt: Attempt, progress: Progress },
    Finished { session: String, attempt: Attempt, ok: bool, hidden: Vec<String>, why: Option<WireError>, listed: Option<Vec<serde_json::Value>> },
    DeferredFailed { session: String, attempt: Attempt, now: f64 },
}

/// Executes a [`RewriteTask`] off the UI thread against `srv`, emitting intermediate progress
/// updates through `on_progress` and returning the terminal [`QueueReply::Finished`].
pub fn execute_rewrite(srv: &Server, task: RewriteTask, on_progress: impl Fn(QueueReply)) -> QueueReply {
    let RewriteTask { session, attempt, operation, run, abort } = task;
    let mut progress = |p: Progress| {
        {
            let mut r = run.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match &p {
                Progress::Committed => r.stage = Stage::Committed,
                Progress::Hidden(id) => r.hidden.push(id.clone()),
                Progress::Moved(_) | Progress::Admitted => {}
            }
        }
        if matches!(p, Progress::Moved(_) | Progress::Admitted) {
            on_progress(QueueReply::Progress { session: session.clone(), attempt, progress: p });
        }
    };
    let result = match &operation {
        Operation::Reorder { ids, .. } => srv.rewrite_queue(&session, Some(ids), None, &abort, &mut progress),
        Operation::Edit(e) if e.delivery == Delivery::Queue => {
            let replace = Replace { original: e.original.clone(), admission: e.admission.clone(), moved: e.moved.iter().cloned().collect() };
            srv.rewrite_queue(&session, None, Some(&replace), &abort, &mut progress)
        }
        Operation::Edit(e) => {
            let id = moved_id(e);
            srv.steer_edit(&session, &e.original, &id, &e.admission, &abort, &mut progress)
        }
        _ => Ok(()),
    };
    let why = result.err();
    let ok = why.is_none();
    if ok {
        run.lock().unwrap_or_else(std::sync::PoisonError::into_inner).stage = Stage::Settled;
    }
    let hidden = run.lock().unwrap_or_else(std::sync::PoisonError::into_inner).hidden.clone();
    // The rows keep their places until the server's queue has caught up.
    let listed = srv.inbox_items(&session).ok();
    QueueReply::Finished { session, attempt, ok, hidden, why, listed }
}

/// Applies a [`QueueReply`] back onto `host` on the UI thread.
pub fn apply_reply(host: &mut dyn QueueHost, reply: QueueReply) {
    match reply {
        QueueReply::Progress { session, attempt, progress } => queue_progress(host, &session, attempt, progress),
        QueueReply::Finished { session, attempt, ok, hidden, why, listed } => {
            host.queue_runs_mut().remove(&session, attempt);
            if let Some(items) = listed {
                super::inbox_listed(host, &session, &items);
            }
            let now = host.clock();
            let event = if ok { Event::Succeeded { attempt, now } } else { Event::Failed { attempt, hidden, why, now } };
            super::step_queue(host, &session, event);
        }
        QueueReply::DeferredFailed { session, attempt, now } => {
            super::step_queue(host, &session, Event::Failed { attempt, hidden: Vec::new(), why: None, now });
        }
    }
}

pub(super) fn run_queue_op(host: &mut dyn QueueHost, session: &str, attempt: Attempt, operation: Operation) {
    let (s, id) = (session.to_owned(), operation.id().to_owned());
    let write = match &operation {
        Operation::Steer { .. } => Write::Deliver { session: s, item: id, delivery: Delivery::Steer },
        Operation::Requeue { .. } => Write::Deliver { session: s, item: id, delivery: Delivery::Queue },
        Operation::Remove { .. } | Operation::Undo { .. } | Operation::Delete { .. } => Write::Cancel { session: s, item: id },
        Operation::Edit(_) | Operation::Reorder { .. } => return run_rewrite(host, session, attempt, operation),
    };
    // Fixture runs act locally, as if the server agreed at once.
    if host.is_fixture() {
        return super::queue_settled(host, session, attempt.0, Ok(()));
    }
    let ticket = Ticket { kind: TicketKind::Inbox, session: session.to_owned(), attempt: attempt.0 };
    if !host.write(Write::Tracked { ticket, write: Box::new(write) }) {
        super::queue_settled(host, session, attempt.0, Err(None));
    }
}

/// An edit or reorder: its chain runs off the app's thread and reports back here.
fn run_rewrite(host: &mut dyn QueueHost, session: &str, attempt: Attempt, operation: Operation) {
    if host.is_fixture() {
        fixture_rewrite(host, session, &operation);
        let now = host.clock();
        return super::step_queue(host, session, Event::Succeeded { attempt, now });
    }
    if !host.can_write() {
        let now = host.clock();
        host.spawn_queue_job(QueueJob::DeferFailed { session: session.to_owned(), attempt, now });
        return;
    }
    let run = Arc::new(Mutex::new(Run::default()));
    let abort = run.lock().unwrap_or_else(std::sync::PoisonError::into_inner).abort.clone();
    host.queue_runs_mut().insert(session.to_owned(), attempt, run.clone());
    host.spawn_queue_job(QueueJob::Rewrite(RewriteTask { session: session.to_owned(), attempt, operation, run, abort }));
}

/// A chain's progress: the ids it re-admits under, or the edit's admission.
fn queue_progress(host: &mut dyn QueueHost, session: &str, attempt: Attempt, progress: Progress) {
    match progress {
        Progress::Moved(moved) => super::step_queue(host, session, Event::Moved { attempt, moved }),
        Progress::Admitted => {
            // A steered edit moves the blocking work, as a steer does.
            let steered_id = host
                .queue_ops()
                .get(session)
                .and_then(|q| q.running())
                .and_then(|(a, o)| match o {
                    Operation::Edit(e) if a == attempt && e.delivery == Delivery::Steer => Some(moved_id(e)),
                    _ => None,
                });
            if let Some(id) = steered_id {
                host.on_steered(session, &id);
            }
            super::step_queue(host, session, Event::Admitted { attempt });
        }
        Progress::Committed | Progress::Hidden(_) => {}
    }
}

/// A fixture's server takes the change at once: the rows move, or the edit replaces its
/// prompt in place.
fn fixture_rewrite(host: &mut dyn QueueHost, session: &str, operation: &Operation) {
    let mut steered_id = None;
    {
        let inbox = host.inboxes_mut().entry(session.to_owned()).or_default();
        match operation {
            Operation::Reorder { ids, .. } => {
                let mut queued: Vec<InboxItem> = inbox.items.iter().filter(|i| i.delivery == Delivery::Queue).cloned().collect();
                inbox.items.retain(|i| i.delivery != Delivery::Queue);
                for id in ids {
                    if let Some(at) = queued.iter().position(|i| i.id == *id) {
                        inbox.items.push(queued.remove(at));
                    }
                }
                inbox.items.extend(queued);
            }
            Operation::Edit(e) => {
                if let Some(item) = inbox.items.iter_mut().find(|i| i.id == e.original) {
                    item.text.clone_from(&e.text);
                    item.files.clone_from(&e.prompt.files);
                    item.delivery = e.delivery;
                }
                if e.delivery == Delivery::Steer {
                    steered_id = Some(e.original.clone());
                }
            }
            _ => {}
        }
    }
    if let Some(id) = steered_id {
        host.on_steered(session, &id);
    }
}

/// The id a steered edit's replacement goes under.
fn moved_id(e: &EditOp) -> String {
    e.moved.iter().find(|(o, _)| *o == e.original).map_or_else(opencode_kit::client::message_id, |(_, n)| n.clone())
}
