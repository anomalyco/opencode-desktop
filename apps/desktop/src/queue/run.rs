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
use opencode_kit::client::{Ticket, TicketKind, Write};
use opencode_kit::model::{Delivery, InboxItem};
use rune::machine::Attempt;

use super::machine::{EditOp, Event, Operation, Stage};
use crate::{App, Source};

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
            let r = r.lock().unwrap();
            (r.stage, r.hidden.clone())
        })
    }

    /// Stops the chain at `attempt` at its next step.
    pub fn abort(&self, session: &str, attempt: Attempt) {
        if let Some(r) = self.0.get(&(session.to_owned(), attempt.0)) {
            r.lock().unwrap().abort.store(true, Ordering::SeqCst);
        }
    }
}

impl App {
    pub(super) fn run_queue_op(&mut self, session: &str, attempt: Attempt, operation: Operation) {
        let (s, id) = (session.to_owned(), operation.id().to_owned());
        let write = match &operation {
            Operation::Steer { .. } => Write::Deliver { session: s, item: id, delivery: Delivery::Steer },
            Operation::Requeue { .. } => Write::Deliver { session: s, item: id, delivery: Delivery::Queue },
            Operation::Remove { .. } | Operation::Undo { .. } | Operation::Delete { .. } => Write::Cancel { session: s, item: id },
            Operation::Edit(_) | Operation::Reorder { .. } => return self.run_rewrite(session, attempt, operation),
        };
        // Fixture runs act locally, as if the server agreed at once.
        if matches!(*self.source, Source::Fixture) {
            return self.queue_settled(session, attempt.0, true);
        }
        let ticket = Ticket { kind: TicketKind::Inbox, session: session.to_owned(), attempt: attempt.0 };
        if !self.write(Write::Tracked { ticket, write: Box::new(write) }) {
            self.queue_settled(session, attempt.0, false);
        }
    }

    /// An edit or reorder: its chain runs off the app's thread and reports back here.
    fn run_rewrite(&mut self, session: &str, attempt: Attempt, operation: Operation) {
        if matches!(*self.source, Source::Fixture) {
            self.fixture_rewrite(session, &operation);
            let now = *self.clock;
            return self.step_queue(session, Event::Succeeded { attempt, now: now });
        }
        if !self.can_write() {
            let (s, now) = (session.to_owned(), *self.clock);
            return self.later(Box::new(move |app: &mut App, _| app.step_queue(&s, Event::Failed { attempt, hidden: Vec::new(), now: now })));
        }
        let run = Arc::new(Mutex::new(Run::default()));
        let abort = run.lock().unwrap().abort.clone();
        self.queue_runs.0.insert((session.to_owned(), attempt.0), run.clone());
        let key = crate::servers::runtime::active(self).clone();
        let deliver = self.deliverer();
        let session = session.to_owned();
        self.job(move |srv| {
            let mut progress = |p: Progress| {
                {
                    let mut r = run.lock().unwrap();
                    match &p {
                        Progress::Committed => r.stage = Stage::Committed,
                        Progress::Hidden(id) => r.hidden.push(id.clone()),
                        Progress::Moved(_) | Progress::Admitted => {}
                    }
                }
                if matches!(p, Progress::Moved(_) | Progress::Admitted) {
                    let (key, session) = (key.clone(), session.clone());
                    deliver(Box::new(move |app: &mut App, _| {
                        crate::servers::runtime::with_server(app, &key, |app| app.queue_progress(&session, attempt, p));
                    }));
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
            let ok = result.is_ok();
            if ok {
                run.lock().unwrap().stage = Stage::Settled;
            }
            let hidden = run.lock().unwrap().hidden.clone();
            // The rows keep their places until the server's queue has caught up.
            let listed = srv.inbox_items(&session).ok();
            Box::new(move |app: &mut App, _| {
                crate::servers::runtime::with_server(app, &key, |app| {
                    app.queue_runs.0.remove(&(session.clone(), attempt.0));
                    if let Some(items) = listed {
                        app.apply_event("session.inbox.listed", &serde_json::json!({ "sessionID": session, "items": items }));
                    }
                    let now = *app.clock;
                    app.step_queue(&session, if ok { Event::Succeeded { attempt, now: now } } else { Event::Failed { attempt, hidden, now: now } });
                });
            })
        });
    }

    /// A chain's progress: the ids it re-admits under, or the edit's admission.
    fn queue_progress(&mut self, session: &str, attempt: Attempt, progress: Progress) {
        match progress {
            Progress::Moved(moved) => self.step_queue(session, Event::Moved { attempt, moved }),
            Progress::Admitted => {
                // A steered edit moves the blocking work, as a steer does.
                if let Some((a, Operation::Edit(e))) = self.queue_ops.get(session).and_then(|q| q.running()).map(|(a, o)| (a, o.clone()))
                    && a == attempt
                    && e.delivery == Delivery::Steer
                {
                    self.steered(session, &moved_id(&e));
                }
                self.step_queue(session, Event::Admitted { attempt });
            }
            Progress::Committed | Progress::Hidden(_) => {}
        }
    }

    /// A fixture's server takes the change at once: the rows move, or the edit replaces its
    /// prompt in place.
    fn fixture_rewrite(&mut self, session: &str, operation: &Operation) {
        let inbox = self.inboxes.entry(session.to_owned()).or_default();
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
                    let id = e.original.clone();
                    self.steered(session, &id);
                }
            }
            _ => {}
        }
    }
}

/// The id a steered edit's replacement goes under.
fn moved_id(e: &EditOp) -> String {
    e.moved.iter().find(|(o, _)| *o == e.original).map_or_else(opencode_kit::client::message_id, |(_, n)| n.clone())
}
