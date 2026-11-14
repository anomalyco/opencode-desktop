//! The outbox's way to the server: idempotent prompt admissions, each kept as a durable row
//! (`store.rs`) before its write can leave. The writer never sees a new ID until its complete
//! request and editable draft are committed. One outbox per server, shared by its windows: a
//! window that closes mid-send leaves its prompts to the others and to the next launch.
use super::store::{self, Forward, Mail, Store};
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
    /// Another window's outbox of the same server, joining this one (`windows::peer`): prompts
    /// this copy doesn't hold yet come in.
    pub(crate) fn absorb(&mut self, from: Log) {
        for s in from.sent {
            if !self.sent.iter().any(|x| x.id == s.id) {
                self.sent.push(s);
            }
        }
        self.sent.sort_by_key(|s| s.seq);
        if self.store == Store::Memory {
            self.store = from.store;
        }
    }
}

impl App {
    /// The window's server's outbox: its stored prompts are read on the storage thread, after
    /// the first frame, and come in through `poll_sends`.
    pub(crate) fn open_outbox(&mut self) {
        let Source::Live { server, allow_send: true, .. } = &*self.source else {
            return;
        };
        let base = server.base.clone();
        *self.unsent = Log::default();
        if !self.persists() {
            return;
        }
        store::listen(&base, (*self.waker).clone());
        store::read(&base, &self.window.id);
        self.unsent.store = Store::Reading { server: base };
    }

    /// This window is woken when the storage thread answers its server's outbox.
    pub(crate) fn listen_outbox(&self) {
        if let Some(server) = self.unsent.store.server() {
            store::listen(server, (*self.waker).clone());
        }
    }

    /// What the storage thread sent back for this server: its stored prompts, a row that couldn't
    /// be saved, a writer that was gone.
    pub(super) fn take_mail(&mut self) -> bool {
        let Some(server) = self.unsent.store.server().map(str::to_owned) else { return false };
        store::listen(&server, (*self.waker).clone());
        let mail = store::take(&server);
        let changed = !mail.is_empty();
        for m in mail {
            match m {
                Mail::Read(rows) => self.stored_prompts(&server, rows),
                Mail::NotSaved { id, handoff } => {
                    if let Some(s) = self.unsent.sent.iter_mut().find(|s| s.id == id) {
                        s.row = store::Row::None;
                    }
                    self.step_send(&id, Event::NotSaved { handoff });
                }
                Mail::NotSent { id, handoff } => {
                    self.step_send(&id, Event::NotSent { why: "waiting for connection", handoff });
                }
            }
        }
        changed
    }

    /// The server's stored prompts are in: each takes its place before the ones sent since
    /// (it's older), a write out when the app quit checks first, and the outbox may pump.
    fn stored_prompts(&mut self, server: &str, rows: Result<Vec<Sent>, String>) {
        let rows = match rows {
            Ok(rows) => rows,
            Err(e) => {
                // Storage that can't keep them: this run's outbox lives in memory.
                eprintln!("opencode-rune: outbox not readable, kept in memory: {e}");
                self.unsent.store = Store::Memory;
                return;
            }
        };
        if !matches!(self.unsent.store, Store::Reading { .. }) && self.unsent.store != (Store::Open { server: server.to_owned() }) {
            return;
        }
        self.unsent.store = Store::Open { server: server.to_owned() };
        for mut s in rows {
            if self.unsent.sent.iter().any(|x| x.id == s.id) {
                continue;
            }
            machine::step(&mut s, Event::Reopened);
            self.unsent.note(s);
        }
        self.unsent.sent.sort_by_key(|s| s.seq);
        self.show_outbox();
        self.pump_outbox();
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

    /// Takes `sent`, to go out as `write`: its row is committed on the storage thread and, when
    /// it's next in line, the write leaves from there once the row is durable. The UI thread
    /// writes nothing. False only for a write the outbox doesn't keep.
    /// A new session's first prompt that must wait for its worktree or its folder's MCP servers
    /// (`new_session`): its row is written now, Preparing, so a kill while it waits loses
    /// nothing, and it goes once [`Self::release_prepared`] says so. False when the outbox can't
    /// take it (a read-only window, the storage in memory still takes it for the run).
    pub(crate) fn enqueue_prepared(&mut self, sent: Sent, write: Write) -> bool {
        self.enqueue_prompt_as(sent, write, true)
    }

    /// Prompt `id`, Preparing, may go: on to `directory` (its new worktree, made now) when given.
    pub(crate) fn release_prepared(&mut self, id: &str, directory: Option<String>) {
        if let Some(dir) = directory
            && let Some(s) = self.unsent.sent.iter_mut().find(|s| s.id == id)
        {
            s.operation = Operation::Start(dir);
        }
        self.step_send(id, Event::Prepared);
        self.pump_outbox();
    }

    pub(crate) fn enqueue_prompt(&mut self, sent: Sent, write: Write) -> bool {
        self.enqueue_prompt_as(sent, write, false)
    }

    fn enqueue_prompt_as(&mut self, mut sent: Sent, write: Write, preparing: bool) -> bool {
        sent.operation = match write {
            Write::Send { .. } => Operation::Send,
            Write::SendMovingHome { .. } => Operation::MovingHome,
            Write::StartAs { directory, .. } => Operation::Start(directory),
            Write::StartInNewWorktree { project, from, branch, .. } => Operation::Worktree { project, from, branch },
            Write::SendFromRevert { .. } => Operation::Revert,
            _ => return false,
        };
        sent.journaled = true;
        sent.seq = store::next_seq();
        // Words that leave the composer leave its saved draft in the same transaction as their row.
        if self.route == Route::here(sent.session.clone()) && !self.composer.text.is_empty() && Entry::of(&self.composer) == sent.draft && self.unsent.store != Store::Memory {
            sent.left_draft = crate::restore::draft_key(&self.route).and_then(|k| self.draft_document_of(&k));
            if let Some(key) = &sent.left_draft {
                crate::storage::set_doc(key, None);
            }
        }
        let offline = crate::reconnect::down(self) || matches!(*self.source, Source::Live { writes: None, .. });
        sent.phase = if preparing { Phase::Preparing } else { Phase::Waiting { why: if offline { "waiting for connection".into() } else { String::new() } } };
        let (id, session) = (sent.id.clone(), sent.session.clone());
        self.unsent.note(sent);
        crate::telemetry::send_receipt(&session, &id, "saved", 0);
        self.pump_outbox();
        // Not its turn (offline, or behind another): its row holds it until it is.
        if self.unsent.state(&id) == Some(State::Waiting) {
            self.unsent.persist(&id, None);
        }
        true
    }

    pub(super) fn defer_outbox(&mut self) {
        let attempts = match self.unsent.retry {
            Retry::Ready => 0,
            Retry::Backoff { attempts, .. } => attempts,
        };
        let delay = (0.25 * 2_f64.powi(attempts.min(5) as i32)).min(5.0);
        self.unsent.retry = Retry::Backoff { attempts: attempts + 1, until: *self.clock + delay };
    }

    pub(crate) fn tick_outbox(&mut self, tx: &mut rune::Tx) -> bool {
        // A prompt sending past `SLOW_SEND` says so whether or not the link is up: a server that
        // took the POST and holds it is the case it's for.
        let now = *self.clock;
        // Most polls change nothing: asked first, they leave the outbox unwritten, so the regions
        // that read it (the timeline's footers) keep.
        let log = rune::Tracked::untracked(&self.unsent);
        let (mut changed, slow_at) = if log.slow_writes(now) { self.unsent.tell_slow(now) } else { log.slow_next() };
        if let Some(at) = slow_at {
            tx.wake_at(at);
        }
        if !self.unsent.waiting_for_link() {
            return changed;
        }
        let due = match self.unsent.retry {
            Retry::Ready => *self.clock,
            Retry::Backoff { until, .. } => until,
        };
        if *self.clock >= due {
            changed |= self.pump_outbox();
            self.defer_outbox();
        }
        let next = match self.unsent.retry {
            Retry::Ready => *self.clock,
            Retry::Backoff { until, .. } => until,
        };
        tx.wake_after(Duration::from_secs_f64((next - *self.clock).max(0.01)));
        changed
    }

    /// Sends the outbox's oldest prompt still on its way, through this window's writer, once the
    /// one before it is confirmed: its `Sending` row commits on the storage thread, which hands
    /// the write to the writer once it's durable.
    pub(crate) fn pump_outbox(&mut self) -> bool {
        if crate::reconnect::down(self) || matches!(self.unsent.store, Store::Reading { .. }) {
            return false;
        }
        let Source::Live { writes: Some(writer), allow_send: true, .. } = &*self.source else {
            return false;
        };
        let writer = writer.clone();
        let Some(s) = self.unsent.sent.iter().find(|s| s.journaled && !matches!(s.phase, Phase::Accepted | Phase::Failed { .. } | Phase::Preparing)) else {
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
        let by = crate::windows::peer::token(self);
        self.step_send(&id, Event::Send { by });
        let Some(Phase::Sending { handoff, .. }) = self.unsent.get(&id).map(|s| s.phase.clone()) else { return true };
        let write = operation.write(session, prompt);
        if self.unsent.store == Store::Memory {
            if writer.send(write).is_err() {
                self.step_send(&id, Event::NotSent { why: "waiting for connection", handoff });
            }
            return true;
        }
        match hand_over(&writer, write, handoff) {
            Some(forward) => self.unsent.persist(&id, Some(forward)),
            None => {
                self.step_send(&id, Event::NotSent { why: "waiting for connection", handoff });
            }
        }
        true
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
    }

    /// Takes prompt `id` out of the outbox and the timeline; an Edit puts its words in the
    /// composer, after anything already there.
    fn remove_from_outbox(&mut self, id: &str, action: AfterCheck) {
        let Some(s) = self.unsent.take(id) else {
            return;
        };
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

/// The write of hand-off `handoff`, waiting for its row: on the lane already, making its request
/// while the row commits (natively), or to go once it has (a page, whose one thread can't wait).
/// None when the lane is gone.
fn hand_over(writer: &std::sync::mpsc::Sender<Write>, write: Write, handoff: u32) -> Option<Forward> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (gate, held) = std::sync::mpsc::sync_channel(1);
        writer.send(Write::Gated { gate: held, write: Box::new(write) }).ok()?;
        Some(Forward::Gate { gate, handoff })
    }
    #[cfg(target_arch = "wasm32")]
    Some(Forward::After { writer: writer.clone(), write, handoff })
}
