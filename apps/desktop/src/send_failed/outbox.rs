//! Write-ahead journal for idempotent prompt admissions. The writer never sees a new ID
//! until its complete request and editable draft have been committed locally.
use super::*;
use crate::Route;
use crate::pickers::PickerHost as _;
use opencode_kit::client::Write;
use std::time::Duration;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) enum Operation {
    #[default]
    Send,
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
        if self.blocked {
            return false;
        }
        let Some(path) = &self.path else { return true };
        let pending: Vec<_> = self.sent.iter().filter(|s| s.durable && s.state != State::Accepted).collect();
        write_journal(path, &pending).is_ok()
    }
}

fn write_journal(path: &std::path::Path, rows: &[&Sent]) -> std::io::Result<()> {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = std::io::BufWriter::new(options.open(&tmp)?);
    serde_json::to_writer(&mut file, rows)?;
    file.flush()?;
    file.get_ref().sync_data()?;
    drop(file);
    fs::rename(tmp, path)
}

impl App {
    pub(crate) fn open_outbox(&mut self) {
        use sha2::{Digest, Sha256};
        let Source::Live { server, allow_send: true, .. } = &self.source else {
            return;
        };
        if !self.persists() {
            return;
        }
        let scope = format!("{}\n{}", self.window.id, server.base);
        let path = crate::dirs::data().map(|p| p.join(format!("outbox-{:x}.json", Sha256::digest(scope))));
        self.unsent = Log::default();
        self.unsent.path = path;
        self.read_outbox();
    }

    pub(super) fn read_outbox(&mut self) {
        let Some(path) = &self.unsent.path else {
            self.unsent.blocked = true;
            return;
        };
        let result = std::fs::read(path).and_then(|b| Ok(serde_json::from_slice::<Vec<Sent>>(&b)?));
        match result {
            Ok(rows) => {
                for mut s in rows {
                    if s.state == State::Sending {
                        s.state = State::Unconfirmed;
                    }
                    self.unsent.note(s);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => self.unsent.blocked = true, // Never overwrite unreadable words.
        }
        self.show_outbox();
    }

    pub(crate) fn show_outbox(&mut self) {
        for s in self.unsent.sent.iter().filter(|s| s.durable && s.state != State::Accepted) {
            if self.inboxes.get(&s.session).is_some_and(|i| i.items.iter().any(|i| i.id == s.id)) {
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
                    opencode_kit::model::set_user_quotes(&s.id, p.quotes.clone());
                }
            }
        }
    }

    pub(crate) fn enqueue_prompt(&mut self, mut sent: Sent, write: Write) -> bool {
        sent.operation = match write {
            Write::Send { .. } => Operation::Send,
            Write::SendMovingHome { .. } => Operation::MovingHome,
            Write::StartAs { directory, .. } => Operation::Start(directory),
            Write::StartInNewWorktree { project, from, branch, .. } => Operation::Worktree { project, from, branch },
            Write::SendFromRevert { .. } => Operation::Revert,
            _ => return false,
        };
        sent.durable = true;
        sent.state = State::Waiting;
        if crate::reconnect::down(self) || matches!(self.source, Source::Live { writes: None, .. }) {
            sent.why = "waiting for connection".into();
        }
        let id = sent.id.clone();
        self.unsent.note(sent);
        if !self.unsent.save() {
            self.unsent.take(&id);
            crate::status_toast::fail(self, "Couldn't save message", "Your draft hasn't been sent.");
            return false;
        }
        crate::telemetry::send_receipt(&self.unsent.sent.last().unwrap().session, &id, "saved", 0);
        self.pump_outbox();
        true
    }

    pub(super) fn defer_outbox(&mut self) {
        let delay = (0.25 * 2_f64.powi(self.unsent.attempts.min(5) as i32)).min(5.0);
        self.unsent.attempts += 1;
        self.unsent.retry_at = self.clock + delay;
    }

    pub(crate) fn tick_outbox(&mut self, tx: &mut rune::Tx) -> bool {
        if !self.unsent.waiting_for_link() {
            return false;
        }
        let mut changed = false;
        if self.clock >= self.unsent.retry_at {
            changed = self.pump_outbox();
            self.defer_outbox();
        }
        tx.wake_after(Duration::from_secs_f64((self.unsent.retry_at - self.clock).max(0.01)));
        changed
    }

    pub(crate) fn pump_outbox(&mut self) -> bool {
        if crate::reconnect::down(self) {
            return false;
        }
        let Source::Live { writes: Some(writer), allow_send: true, .. } = &self.source else {
            return false;
        };
        let writer = writer.clone();
        let Some(s) = self.unsent.sent.iter_mut().find(|s| s.durable && !matches!(s.state, State::Accepted | State::Failed)) else {
            return false;
        };
        if s.state == State::Sending || s.state == State::Unconfirmed {
            // A successful POST may have lost its SSE echo; check before releasing the next prompt.
            if s.state == State::Unconfirmed && !self.unsent.checking {
                self.check_sends();
            }
            return false;
        }
        let (session, Some(prompt), id) = (s.session.clone(), s.prompt.clone(), s.id.clone()) else {
            return false;
        };
        let write = s.operation.write(session, prompt);
        if s.after_check.is_some() {
            return self.finish_outbox_action(&id);
        }
        s.state = State::Sending;
        let why = if !self.unsent.save() { "couldn’t save retry" } else if writer.send(write).is_err() { "waiting for connection" } else { return true };
        if let Some(s) = self.unsent.sent.iter_mut().find(|s| s.id == id) {
            s.state = State::Waiting;
            if s.why != why { s.why=why.into(); return true; }
        }
        false
    }

    pub(super) fn restore_rejection(&mut self, id: &str) {
        if let Some(s) = self.unsent.sent.iter().find(|s| s.id == id).cloned() {
            if self.route == Route::Session(s.session.clone()) {
                if self.composer.text.is_empty() && !self.attach.any() && !self.quote.any() {
                    for entry in self.unsent.sent.iter_mut().filter(|entry| entry.session == s.session) {
                        entry.restored_to_composer = entry.id == id;
                    }
                }
                self.restore_draft(s);
            } else {
                self.restore.drafts.entry(s.session).or_insert(s.draft.text);
            }
        }
    }

    fn matches_restored(&self, s: &Sent) -> bool {
        self.route == Route::Session(s.session.clone())
            && Entry::of(&self.composer) == s.draft
            && self.attach.files == s.files
            && self.attach.paths == s.paths
            && self.attach.context.is_empty()
            && self.attach.uploading.is_empty()
            && s.prompt.as_ref().is_some_and(|p| p.quotes == self.quote.quotes())
    }

    /// Enter on the untouched restored draft is Retry, not a second independently retryable bubble.
    pub(crate) fn retry_restored(&mut self) -> bool {
        let Some(id) = self
            .unsent
            .sent
            .iter()
            .rev()
            .find(|s| {
                s.restored_to_composer
                    && s.state == State::Failed
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

    pub(super) fn finish_outbox_action(&mut self, id: &str) -> bool {
        let Some(s) = self.unsent.sent.iter_mut().find(|s| s.id == id && matches!(s.state, State::Failed | State::Waiting)) else { return false };
        let Some(action) = s.after_check.take() else { return false };
        if action == AfterCheck::Edit && self.route != Route::Session(s.session.clone()) {
            s.state = State::Failed;
            s.why = "Ready to edit".into();
            self.unsent.save();
            return true;
        }
        self.outbox_action(id, if action == AfterCheck::Edit { "outbox-edit" } else { "outbox-delete" });
        true
    }

    pub(crate) fn outbox_action(&mut self, id: &str, action: &str) {
        let Some(s) = self.unsent.sent.iter().find(|s| s.id == id && s.durable).cloned() else {
            return;
        };
        if action == "outbox-retry" {
            if s.state == State::Failed {
                self.unsent.sent.iter_mut().find(|s| s.id == id).unwrap().state = State::Waiting;
                if !self.unsent.save() {
                    self.unsent.sent.iter_mut().find(|s| s.id == id).unwrap().state = State::Failed;
                    crate::status_toast::fail(self, "Couldn't save retry", "Your message hasn't changed.");
                    return;
                }
                if s.restored_to_composer && self.matches_restored(&s) {
                    self.composer.clear();
                    self.attach.files.clear();
                    self.attach.paths.clear();
                    self.quote.take();
                }
            }
            self.unsent.retry_at = 0.;
            self.pump_outbox();
            return;
        }
        if !matches!(s.state, State::Waiting | State::Failed) {
            if matches!(s.state, State::Sending | State::Unconfirmed) {
                let Some(entry) = self.unsent.sent.iter_mut().find(|s| s.id == id) else { return };
                entry.after_check = Some(if action == "outbox-edit" { AfterCheck::Edit } else { AfterCheck::Delete });
                if !self.unsent.save() {
                    crate::status_toast::fail(self, "Couldn't save change", "Check this message again after reconnecting.");
                }
                if s.state == State::Unconfirmed {
                    self.check_sends();
                }
            }
            return;
        }
        if action == "outbox-edit" && self.route != Route::Session(s.session.clone()) {
            return;
        }
        let Some(removed) = self.unsent.take(id) else {
            return;
        };
        if !self.unsent.save() {
            self.unsent.note(removed);
            return;
        }
        if let Some(t) = self.transcripts.get_mut(&s.session) {
            t.messages.retain(|m| m.id() != id);
        }
        if let Some(i) = self.inboxes.get_mut(&s.session) {
            i.remove(id);
        }
        self.attach.echoes.retain(|e| e.1 != id);
        if action == "outbox-edit" {
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
                for quote in prompt.quotes {
                    if !self.quote.quotes().contains(&quote) {
                        self.quote.push(quote);
                    }
                }
            }
            self.composer.focused = true;
        }
    }
}
