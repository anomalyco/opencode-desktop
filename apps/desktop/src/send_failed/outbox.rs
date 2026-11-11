//! Write-ahead journal for idempotent prompt admissions. The writer never sees a new ID
//! until its complete request and editable draft have been committed locally. One journal per
//! server, shared by its windows: a window that closes mid-send leaves its prompts to the others
//! and to the next launch.
use super::journal::{self, Journal};
use super::*;
use crate::Route;
use crate::pickers::PickerHost as _;
use opencode_kit::client::Write;
use std::time::Duration;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) enum Operation {
    #[default]
    Send,
    /// A reply to a subagent whose folder may be gone (`subagents/reply.rs`).
    MovingHome,
    Start(String),
    Worktree {
        project: String,
        from: String,
        branch: Option<String>,
    },
    Revert,
}
impl Operation {
    pub(super) fn creates(&self) -> bool {
        matches!(self, Self::Start(_) | Self::Worktree { .. })
    }
    fn write(&self, session: String, prompt: Prompt) -> Write {
        match self {
            Self::Send => Write::Send { session, prompt },
            Self::MovingHome => Write::SendMovingHome { session, prompt },
            Self::Start(directory) => Write::StartAs { session, directory: directory.clone(), prompt },
            Self::Worktree { project, from, branch } => {
                Write::StartInNewWorktree { session, project: project.clone(), from: from.clone(), branch: branch.clone(), prompt }
            }
            Self::Revert => Write::SendFromRevert { session, prompt },
        }
    }
}

impl Log {
    pub(super) fn save(&self) -> bool {
        match &self.journal {
            Journal::Memory => true,
            Journal::Blocked => false,
            Journal::File(path) => journal::write(path, &self.sent).is_ok(),
        }
    }

    /// Another window's read of the same server's journal, joining this one (`windows::peer`):
    /// prompts this copy doesn't hold yet come in (an older build's per-window journal).
    pub(crate) fn absorb(&mut self, from: Log) {
        let before = self.sent.len();
        for s in from.sent {
            if !self.sent.iter().any(|x| x.id == s.id) {
                self.sent.push(s);
            }
        }
        if self.journal == Journal::Memory {
            self.journal = from.journal;
        }
        if self.sent.len() != before {
            self.save();
        }
    }
}

/// The journal of server `base` (one file per server), and an older build's journal of this
/// window on it (one file per window and server), whose prompts move over.
fn paths(base: &str, window: &str) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    use sha2::{Digest, Sha256};
    let dir = crate::dirs::data()?;
    let file = |scope: String| dir.join(format!("outbox-{:x}.json", Sha256::digest(scope)));
    Some((file(format!("server\n{base}")), file(format!("{window}\n{base}"))))
}

impl App {
    pub(crate) fn open_outbox(&mut self) {
        let Source::Live { server, allow_send: true, .. } = &self.source else {
            return;
        };
        if !self.persists() {
            return;
        }
        let found = paths(&server.base, &self.window.id);
        self.unsent = Log::default();
        let Some((path, legacy)) = found else {
            self.unsent.journal = Journal::Blocked;
            return;
        };
        self.read_outbox(path, legacy);
    }

    pub(super) fn read_outbox(&mut self, path: std::path::PathBuf, legacy: std::path::PathBuf) {
        let (Ok(rows), Ok(old)) = (journal::read(&path), journal::read(&legacy)) else {
            // Never overwrite unreadable words.
            self.unsent.journal = Journal::Blocked;
            return;
        };
        self.unsent.journal = Journal::File(path);
        let moved = !old.is_empty();
        for mut s in rows.into_iter().chain(old) {
            machine::step(&mut s, Event::Reopened);
            if !self.unsent.sent.iter().any(|x| x.id == s.id) {
                self.unsent.note(s);
            }
        }
        if moved && self.unsent.save() {
            let _ = std::fs::remove_file(legacy);
        }
        self.show_outbox();
    }

    /// Each unsent prompt where it waits: a follow-up behind steers already waiting waits with
    /// them, as the server will deliver it after them (t1361); the rest show as their bubble.
    pub(crate) fn show_outbox(&mut self) {
        for s in self.unsent.sent.iter().filter(|s| s.journaled && s.phase != Phase::Accepted) {
            let inbox = self.inboxes.get(&s.session);
            if inbox.is_some_and(|i| i.items.iter().any(|i| i.id == s.id)) {
                continue;
            }
            if s.inbox && inbox.is_some_and(|i| i.steering().next().is_some()) {
                let delivery = s.prompt.as_ref().and_then(|p| p.delivery).unwrap_or(opencode_kit::model::Delivery::Steer);
                let item = opencode_kit::model::InboxItem { id: s.id.clone(), text: s.draft.text.clone(), files: s.files.clone(), delivery };
                self.inboxes.entry(s.session.clone()).or_default().upsert(item);
                continue;
            }
            let t = self
                .transcripts
                .entry(s.session.clone())
                .or_insert_with(|| opencode_kit::model::Transcript { session: s.session.clone(), messages: vec![] });
            if !t.messages.iter().any(|m| m.id() == s.id) {
                t.messages.push(Message::User { id: s.id.clone(), text: s.draft.text.clone() });
                opencode_kit::model::set_user_files(&s.id, s.files.clone());
                if let Some(p) = &s.prompt {
                    opencode_kit::model::set_user_comments(&s.id, p.comments.clone());
                }
            }
        }
    }

    /// Journals `sent`, to go out as `write`, then pumps. False (and a toast) when it couldn't be
    /// saved: the draft stays where it is.
    pub(crate) fn enqueue_prompt(&mut self, mut sent: Sent, write: Write) -> bool {
        sent.operation = match write {
            Write::Send { .. } => Operation::Send,
            Write::SendMovingHome { .. } => Operation::MovingHome,
            Write::StartAs { directory, .. } => Operation::Start(directory),
            Write::StartInNewWorktree { project, from, branch, .. } => Operation::Worktree { project, from, branch },
            Write::SendFromRevert { .. } => Operation::Revert,
            _ => return false,
        };
        sent.journaled = true;
        let offline = crate::reconnect::down(self) || matches!(self.source, Source::Live { writes: None, .. });
        sent.phase = Phase::Waiting { why: if offline { "waiting for connection".into() } else { String::new() } };
        let (id, session) = (sent.id.clone(), sent.session.clone());
        self.unsent.note(sent);
        if !self.unsent.save() {
            self.unsent.take(&id);
            crate::status_toast::fail(self, "Couldn't save message", "Your draft hasn't been sent.");
            return false;
        }
        crate::telemetry::send_receipt(&session, &id, "saved", 0);
        self.pump_outbox();
        true
    }

    pub(super) fn defer_outbox(&mut self) {
        let attempts = match self.unsent.retry {
            Retry::Ready => 0,
            Retry::Backoff { attempts, .. } => attempts,
        };
        let delay = (0.25 * 2_f64.powi(attempts.min(5) as i32)).min(5.0);
        self.unsent.retry = Retry::Backoff { attempts: attempts + 1, until: self.clock + delay };
    }

    pub(crate) fn tick_outbox(&mut self, tx: &mut rune::Tx) -> bool {
        if !self.unsent.waiting_for_link() {
            return false;
        }
        let mut changed = false;
        let due = match self.unsent.retry {
            Retry::Ready => self.clock,
            Retry::Backoff { until, .. } => until,
        };
        if self.clock >= due {
            changed = self.pump_outbox();
            self.defer_outbox();
        }
        let next = match self.unsent.retry {
            Retry::Ready => self.clock,
            Retry::Backoff { until, .. } => until,
        };
        tx.wake_after(Duration::from_secs_f64((next - self.clock).max(0.01)));
        changed
    }

    /// Sends the outbox's oldest prompt still on its way, through this window's writer, once the
    /// one before it is confirmed.
    pub(crate) fn pump_outbox(&mut self) -> bool {
        if crate::reconnect::down(self) {
            return false;
        }
        let Source::Live { writes: Some(writer), allow_send: true, .. } = &self.source else {
            return false;
        };
        let writer = writer.clone();
        let Some(s) = self.unsent.sent.iter().find(|s| s.journaled && !matches!(s.phase, Phase::Accepted | Phase::Failed { .. })) else {
            return false;
        };
        let (id, session) = (s.id.clone(), s.session.clone());
        match &s.phase {
            // A successful POST may have lost its SSE echo; check before releasing the next prompt.
            Phase::Unconfirmed { .. } => {
                if self.unsent.check == Check::Idle {
                    self.check_sends();
                }
                return false;
            }
            Phase::Sending { .. } => return false,
            _ => {}
        }
        let (Some(prompt), operation) = (s.prompt.clone(), s.operation.clone()) else {
            return false;
        };
        let was = s.why().to_owned();
        let by = crate::windows::peer::token(self);
        self.step_send(&id, Event::Send { by });
        let why = if !self.unsent.save() {
            "couldn’t save retry"
        } else if writer.send(operation.write(session, prompt)).is_err() {
            "waiting for connection"
        } else {
            return true;
        };
        self.step_send(&id, Event::NotSent { why });
        was != why
    }

    pub(super) fn restore_rejection(&mut self, id: &str) {
        let Some(s) = self.unsent.get(id).cloned() else { return };
        if self.route != Route::here(s.session.clone()) {
            // Its session's draft takes it back whole, files and quotes too, when that's empty.
            if self.capture_draft(&s.session).is_empty() {
                let caret = s.draft.text.len();
                let quotes = s.prompt.map(|p| p.comments).unwrap_or_default();
                self.apply_draft(&s.session, crate::rewind::SavedDraft { entry: s.draft, caret, files: s.files, paths: s.paths, quotes });
            }
            return;
        }
        if self.composer.text.is_empty() && !self.attach.any() && !self.quote.any() {
            for entry in self.unsent.sent.iter_mut().filter(|entry| entry.session == s.session && entry.id != id) {
                if let Phase::Failed { restored, .. } = &mut entry.phase {
                    *restored = false;
                }
            }
            self.step_send(id, Event::Restored);
        }
        self.restore_draft(s);
    }

    fn matches_restored(&self, s: &Sent) -> bool {
        self.route == Route::here(s.session.clone())
            && Entry::of(&self.composer) == s.draft
            && self.attach.files == s.files
            && self.attach.paths == s.paths
            && self.attach.context.is_empty()
            && self.attach.uploads.is_empty()
            && s.prompt.as_ref().is_some_and(|p| p.comments == self.quote.quotes())
    }

    /// Enter on the untouched restored draft is Retry, not a second independently retryable bubble.
    pub(crate) fn retry_restored(&mut self) -> bool {
        let Some(id) = self
            .unsent
            .sent
            .iter()
            .rev()
            .find(|s| {
                matches!(s.phase, Phase::Failed { restored: true, .. })
                    && self.matches_restored(s)
                    && s.prompt.as_ref().is_some_and(|p| (p.agent.clone(), p.model.clone()) == self.picked_choice())
            })
            .map(|s| s.id.clone())
        else {
            return false;
        };
        self.outbox_action(&id, "outbox-retry");
        true
    }

    /// An Edit or Delete the machine says to carry out now. An Edit waits while its session
    /// isn't the open one.
    pub(super) fn run_outbox_action(&mut self, id: &str, action: AfterCheck) {
        let elsewhere = self.unsent.get(id).is_some_and(|s| self.route != Route::here(s.session.clone()));
        if action == AfterCheck::Edit && elsewhere {
            self.step_send(id, Event::Parked);
            return;
        }
        self.remove_from_outbox(id, action);
    }

    /// The bubble's or its menu's Retry now, Edit in composer or Delete.
    pub(crate) fn outbox_action(&mut self, id: &str, action: &str) {
        let Some(s) = self.unsent.get(id).filter(|s| s.journaled).cloned() else {
            return;
        };
        if action == "outbox-retry" {
            if s.state() == State::Failed {
                self.step_send(id, Event::Retry);
                if !self.unsent.save() {
                    if let Some(entry) = self.unsent.sent.iter_mut().find(|x| x.id == id) {
                        entry.phase = s.phase.clone();
                    }
                    crate::status_toast::fail(self, "Couldn't save retry", "Your message hasn't changed.");
                    return;
                }
                if matches!(s.phase, Phase::Failed { restored: true, .. }) && self.matches_restored(&s) {
                    self.composer.clear();
                    self.attach.files.clear();
                    self.attach.paths.clear();
                    self.quote.take();
                }
            }
            if let Retry::Backoff { attempts, .. } = self.unsent.retry {
                self.unsent.retry = Retry::Backoff { attempts, until: 0. };
            }
            self.pump_outbox();
            return;
        }
        let act = if action == "outbox-edit" { AfterCheck::Edit } else { AfterCheck::Delete };
        if act == AfterCheck::Edit && matches!(s.phase, Phase::Waiting { .. } | Phase::Failed { .. }) && self.route != Route::here(s.session.clone()) {
            return;
        }
        self.step_send(id, Event::Act(act));
        if matches!(s.phase, Phase::Sending { .. } | Phase::Unconfirmed { .. }) && !self.unsent.save() {
            crate::status_toast::fail(self, "Couldn't save change", "Check this message again after reconnecting.");
        }
    }

    /// Takes prompt `id` out of the outbox and the timeline; an Edit puts its words in the
    /// composer, after anything already there.
    fn remove_from_outbox(&mut self, id: &str, action: AfterCheck) {
        let Some(removed) = self.unsent.take(id) else {
            return;
        };
        if !self.unsent.save() {
            self.unsent.note(removed);
            return;
        }
        let s = removed;
        if let Some(t) = self.transcripts.get_mut(&s.session) {
            t.messages.retain(|m| m.id() != id);
        }
        if let Some(i) = self.inboxes.get_mut(&s.session) {
            i.remove(id);
        }
        self.attach.echoes.retain(|e| e.1 != id);
        if action != AfterCheck::Edit {
            return;
        }
        let mut draft = Entry::of(&self.composer);
        if draft.text != s.draft.text {
            if !draft.text.is_empty() {
                draft.text.push_str("\n\n");
            }
            let offset = draft.text.len();
            draft.text.push_str(&s.draft.text);
            draft.pills.extend(s.draft.pills.into_iter().map(|(r, p)| (r.start + offset..r.end + offset, p)));
        }
        draft.put(&mut self.composer, draft.text.len());
        for f in s.files {
            self.attach.add(f);
        }
        for p in s.paths {
            self.attach.add_path(p);
        }
        if let Some(prompt) = s.prompt {
            for quote in prompt.comments {
                if !self.quote.quotes().contains(&quote) {
                    self.quote.push(quote);
                }
            }
        }
        self.composer.focused = true;
    }
}
