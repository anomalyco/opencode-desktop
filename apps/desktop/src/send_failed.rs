//! The outbox: every prompt this app sent that the server hasn't confirmed, one copy for all of a
//! server's windows (`windows::peer`). Each prompt's way is the machine in `machine.rs`. An absent
//! HTTP reply isn't a rejected prompt: check inbox admission and the exact message before
//! replaying a journaled request with its original ID.
use opencode_kit::client::{PathAttachment, Prompt};
use opencode_kit::model::{Attachment, Message};
use opencode_kit::plain_error::failed_for_sure;

use crate::prompt_history::Entry;
use crate::{App, Source};

mod receipt;
mod reason;
pub(crate) mod machine;
mod journal;
mod outbox;
mod view;
pub(crate) mod stories;
pub use machine::Phase;
pub use view::footer;

use machine::{Effect, Event, Found, Verdict};

const KEEP: usize = 32;

/// A prompt's phase without its data, as the timeline and tests ask about it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum State {
    #[default]
    Sending,
    Unconfirmed,
    Failed,
    Accepted,
    Waiting,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub enum AfterCheck { Edit, Delete }

/// How a send ended, for whoever waits on it (the quote comment editor's send): the server has
/// it, it failed for good, or the outbox keeps it to send when it can (waiting for the link, or
/// for a receipt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendResult {
    Accepted,
    Failed,
    Queued,
}

/// One prompt in the outbox: what was typed, what went out, and where it is (`phase`).
#[derive(Clone)]
pub struct Sent {
    pub session: String,
    pub id: String,
    pub draft: Entry,
    pub files: Vec<Attachment>,
    pub paths: Vec<PathAttachment>,
    /// Sent while its session was busy: once the server has it, it waits in the inbox.
    pub inbox: bool,
    pub prompt: Option<Prompt>,
    pub phase: Phase,
    /// Kept in the journal across launches and sent again until the server has it; a fixture's
    /// or story's local send only rolls back.
    journaled: bool,
    operation: outbox::Operation,
}

/// Whether a receipt check is out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Check {
    #[default]
    Idle,
    /// Asked; `again` when an unconfirmed prompt joined while it was out.
    Checking { again: bool },
}

/// When the outbox may pump again after a failed send.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Retry {
    #[default]
    Ready,
    Backoff { attempts: u32, until: f64 },
}

#[derive(Default)]
pub struct Log {
    journal: journal::Journal,
    retry: Retry,
    sent: Vec<Sent>,
    receipts: rune::Latest<Vec<(String, Result<receipt::Receipt, String>)>>,
    check: Check,
}

impl Log {
    /// A journaled prompt is still on its way: the outbox keeps pumping.
    pub(crate) fn waiting_for_link(&self) -> bool {
        self.sent.iter().any(|s| s.journaled && matches!(s.phase, Phase::Sending { .. } | Phase::Waiting { .. } | Phase::Unconfirmed { .. }))
    }

    pub fn note(&mut self, s: Sent) {
        self.sent.retain(|x| x.id != s.id);
        self.sent.push(s);
        if self.sent.len() > KEEP {
            // Never discard words the user still needs to recover.
            if let Some(i) = self.sent.iter().position(|x| x.phase == Phase::Accepted) { self.sent.remove(i); }
        }
    }

    pub fn state(&self, id: &str) -> Option<State> {
        self.get(id).map(|s| s.state())
    }

    fn get(&self, id: &str) -> Option<&Sent> {
        self.sent.iter().find(|s| s.id == id)
    }

    pub(crate) fn durable(&self, id: &str) -> Option<&Sent> { self.sent.iter().find(|s| s.journaled && s.id == id && s.phase != Phase::Accepted) }
    pub(crate) fn journaled(&self, id: &str) -> bool { self.sent.iter().any(|s| s.id == id && s.journaled) }

    pub fn sending(&self, id: &str) -> bool { self.state(id) == Some(State::Unconfirmed) }

    pub(crate) fn take(&mut self, id: &str) -> Option<Sent> {
        let at = self.sent.iter().position(|s| s.id == id)?;
        Some(self.sent.remove(at))
    }

    /// A listing dropped `old` items: those still on their way from any of the server's windows
    /// come back, as the server hasn't seen them yet (`multi-2-review.md` #10).
    pub(crate) fn preserve_inbox(&self, session: &str, inbox: &mut opencode_kit::model::Inbox, old: Vec<opencode_kit::model::InboxItem>) {
        for item in old {
            if self.sent.iter().any(|s| s.session == session && s.id == item.id && matches!(s.phase, Phase::Sending { .. } | Phase::Unconfirmed { .. } | Phase::Waiting { .. }))
                && !inbox.items.iter().any(|i| i.id == item.id) { inbox.upsert(item); }
        }
    }

    /// Window `by` closed or left the server with writes out: their replies won't come to any
    /// window, so a receipt check settles them.
    pub(crate) fn release(&mut self, by: u64) -> bool {
        let mut released = false;
        for s in &mut self.sent {
            released |= !machine::step(s, Event::Released { by }).is_empty();
        }
        if released {
            self.save();
        }
        released
    }
}

impl Sent {
    pub fn new(session: &str, id: &str, draft: Entry, files: Vec<Attachment>, paths: Vec<PathAttachment>, inbox: bool) -> Self {
        Sent { session: session.into(), id: id.into(), draft, files, paths, inbox, prompt: None, phase: Phase::Sending { by: 0, then: None }, journaled: false, operation: Default::default() }
    }

    pub fn with_prompt(mut self, prompt: &Prompt) -> Self { self.prompt = Some(prompt.clone()); self }

    pub fn state(&self) -> State {
        match self.phase {
            Phase::Waiting { .. } => State::Waiting,
            Phase::Sending { .. } => State::Sending,
            Phase::Unconfirmed { .. } => State::Unconfirmed,
            Phase::Failed { .. } => State::Failed,
            Phase::Accepted => State::Accepted,
        }
    }

    /// What the phase says, for the bubble's status line.
    pub fn why(&self) -> &str {
        match &self.phase {
            Phase::Waiting { why } | Phase::Unconfirmed { why, .. } | Phase::Failed { why, .. } => why,
            Phase::Sending { .. } | Phase::Accepted => "",
        }
    }

    /// How it ended so far; `None` while it's on its way for the first time.
    pub fn result(&self) -> Option<SendResult> {
        match &self.phase {
            Phase::Accepted => Some(SendResult::Accepted),
            Phase::Failed { .. } => Some(SendResult::Failed),
            Phase::Unconfirmed { .. } => Some(SendResult::Queued),
            Phase::Waiting { why } if !why.is_empty() => Some(SendResult::Queued),
            Phase::Waiting { .. } | Phase::Sending { .. } => None,
        }
    }

    /// The Edit or Delete waiting on the server's answer.
    pub fn then(&self) -> Option<AfterCheck> {
        match self.phase {
            Phase::Sending { then, .. } | Phase::Unconfirmed { then, .. } => then,
            _ => None,
        }
    }
}

impl App {
    /// Steps prompt `id`'s machine and carries out what it says.
    pub(crate) fn step_send(&mut self, id: &str, event: Event) -> bool {
        let Some(s) = self.unsent.sent.iter_mut().find(|s| s.id == id) else { return false };
        let before = s.result();
        let effects = machine::step(s, event);
        let after = s.result();
        let changed = !effects.is_empty();
        for effect in effects {
            self.send_effect(id, effect);
        }
        if let Some(result) = after.filter(|r| before != Some(*r)) {
            crate::quote::send_result(self, id, result);
        }
        changed
    }

    /// How prompt `id`'s send ended so far.
    pub(crate) fn send_result(&self, id: &str) -> Option<SendResult> {
        self.unsent.get(id).and_then(Sent::result)
    }

    /// The prompt the outbox took last, if it took one since it held `before`.
    pub(crate) fn newest_send(&self, before: Option<&str>) -> Option<String> {
        self.unsent.sent.last().map(|s| s.id.clone()).filter(|id| Some(id.as_str()) != before)
    }

    fn send_effect(&mut self, id: &str, effect: Effect) {
        match effect {
            Effect::Save => { if self.unsent.journaled(id) { self.unsent.save(); } }
            Effect::Check => self.check_sends(),
            Effect::Defer => self.defer_outbox(),
            Effect::Outbox => {
                if let Some(s) = self.unsent.get(id) && let Some(i) = self.inboxes.get_mut(&s.session) { i.remove(id); }
                self.show_outbox();
            }
            Effect::ShowFailed => self.fail_send(id),
            Effect::Restore => self.restore_rejection(id),
            Effect::Run(action) => self.run_outbox_action(id, action),
            Effect::Accepted { late } => {
                if let Some(s) = self.unsent.get(id).filter(|s| s.journaled) {
                    crate::telemetry::send_receipt(&s.session, id, "accepted", 0);
                    self.unsent.retry = Retry::Ready;
                }
                if late { crate::status_toast::say(self, "Message already sent", "The server received it before your change."); }
            }
        }
    }

    /// A prompt write's error (`Incoming::SendFailed`).
    pub fn send_failed(&mut self, id: &str, error: &str) -> bool {
        let Some(sent) = self.unsent.get(id) else { return false };
        if sent.phase == Phase::Accepted { return true; }
        eprintln!("opencode-rune: prompt send failed: {error}");
        let (why, verdict) = if sent.journaled {
            let code = reason::status(error);
            crate::telemetry::send_receipt(&sent.session, id, "write failed", code.unwrap_or(0));
            let not_started = match sent.operation {
                outbox::Operation::Start(_) => error.starts_with("POST /api/session:"),
                outbox::Operation::Worktree { .. } | outbox::Operation::Revert => false,
                _ => true,
            };
            let verdict = if code.is_some_and(|n| (400..500).contains(&n) && !matches!(n, 408 | 425 | 429)) {
                Verdict::Rejected
            } else if code.is_none() && failed_for_sure(error) && not_started {
                Verdict::Unsent
            } else {
                Verdict::Lost
            };
            (reason::words(error).to_owned(), verdict)
        } else {
            let why = error.split_once(": ").map_or(error, |(_, why)| why);
            (crate::status_toast::plain(why), if failed_for_sure(error) { Verdict::Unsent } else { Verdict::Lost })
        };
        self.step_send(id, Event::WriteFailed { why, verdict });
        true
    }

    /// A successful read or reconnect retries receipts, with at most one bounded batch in flight.
    pub fn check_sends(&mut self) {
        if let Check::Checking { again } = &mut self.unsent.check { *again = true; return; }
        let Source::Live { server, .. } = &self.source else { return };
        let server = server.clone();
        let fetch: crate::session_pages::Fetch = std::sync::Arc::new(move |path| server.read(path));
        let ask: Vec<(String, String)> = self.unsent.sent.iter().filter(|s| s.state() == State::Unconfirmed).map(|s| (s.session.clone(), s.id.clone())).collect();
        if ask.is_empty() { return; }
        self.unsent.check = Check::Checking { again: false };
        self.unsent.receipts = rune::Latest::new(self.waker.clone().unwrap_or_else(|| std::sync::Arc::new(|| {})));
        self.unsent.receipts.request(ask, move |ask| read_receipts(&*fetch, ask));
    }

    pub(crate) fn poll_sends(&mut self) -> bool {
        let Some(receipts) = self.unsent.receipts.take() else { return false };
        self.apply_receipts(receipts);
        true
    }

    fn apply_receipts(&mut self, receipts: Vec<(String, Result<receipt::Receipt, String>)>) {
        let again = matches!(std::mem::take(&mut self.unsent.check), Check::Checking { again: true });
        for (id, result) in receipts {
            // A newer SSE receipt always wins over an older absent read.
            if self.unsent.state(&id) != Some(State::Unconfirmed) { continue; }
            let found = match result {
                Ok(receipt::Receipt::Delivered(message)) => {
                    let session = self.unsent.get(&id).map(|s| s.session.clone()).unwrap_or_default();
                    self.transcripts.entry(session.clone()).or_insert_with(|| opencode_kit::model::Transcript { session: session.clone(), messages: vec![] }).merge(vec![message]);
                    if let Some(inbox) = self.inboxes.get_mut(&session) { inbox.remove(&id); }
                    Found::Arrived
                }
                Ok(receipt::Receipt::Admitted) => {
                    self.admitted_send(&id);
                    continue;
                }
                Ok(receipt::Receipt::Missing) => Found::Missing,
                Err(error) if error.ends_with("http status: 404") => Found::NoSession,
                Err(error) => {
                    eprintln!("opencode-rune: prompt receipt unavailable: {error}");
                    Found::Unknown(reason::words(&error))
                }
            };
            self.step_send(&id, Event::Checked(found));
        }
        if again { self.check_sends(); }
    }

    /// Positive evidence is usable even without a preceding reconnect, and from older pages.
    /// Absence on a limited latest page is NOT evidence that an older prompt never arrived.
    pub fn judge_sends_by_page(&mut self, _session: &str, messages: &[Message]) {
        for message in messages { self.accept_send(message.id()); }
    }

    pub fn judge_sends_by_inbox(&mut self, _session: &str, ids: &[String]) -> bool {
        let mut changed = false;
        for id in ids { changed |= self.admitted_send(id); }
        changed
    }

    /// An accepted follow-up still waits for the busy session. Move its offline echo
    /// into the inbox until delivery; an already delivered message must never move back.
    pub(crate) fn admitted_send(&mut self, id: &str) -> bool {
        if let Some(s) = self.unsent.durable(id).filter(|s| s.inbox) {
            if let Some(t) = self.transcripts.get_mut(&s.session) { t.messages.retain(|m| m.id() != id); }
            let inbox = self.inboxes.entry(s.session.clone()).or_default();
            if !inbox.items.iter().any(|item| item.id == id) {
                inbox.upsert(opencode_kit::model::InboxItem { id: id.into(), text: s.draft.text.clone(), files: s.files.clone(),
                    delivery: s.prompt.as_ref().and_then(|p| p.delivery).unwrap_or(opencode_kit::model::Delivery::Steer) });
            }
        }
        self.accept_send(id)
    }

    pub(crate) fn accept_send(&mut self, id: &str) -> bool {
        self.step_send(id, Event::Admitted)
    }

    /// Shows prompt `id` as failed where it was: out of the inbox, its bubble in the timeline.
    fn fail_send(&mut self, id: &str) {
        let Some(sent) = self.unsent.get(id).cloned() else { return };
        if let Some(i) = self.inboxes.get_mut(&sent.session) { i.remove(id); }
        let t = self.transcripts.entry(sent.session.clone()).or_insert_with(|| opencode_kit::model::Transcript { session: sent.session.clone(), messages: vec![] });
        if !t.messages.iter().any(|m| m.id() == id) {
            t.messages.push(Message::User { id: id.into(), text: sent.draft.text.clone() });
        }
        self.attach.echoes.retain(|e| e.1 != id);
        if !sent.journaled { crate::status_toast::fail(self, opencode_i18n::t!("prompt.toast.promptSendFailed.title"), sent.why()); }
    }

    /// Retry on a local prompt that failed: the same request, under its own id.
    pub(crate) fn retry_send(&mut self, id: &str) {
        let Some(sent) = self.unsent.get(id).filter(|s| s.phase_is(State::Failed)).cloned() else { return };
        let Some(prompt) = sent.prompt.clone() else { return };
        let write = self.send_write(&sent.session, prompt);
        if !matches!(self.source, Source::Fixture) && !self.write(write) { return; }
        if sent.inbox {
            if let Some(t) = self.transcripts.get_mut(&sent.session) { t.messages.retain(|m| m.id() != id); }
            self.inboxes.entry(sent.session.clone()).or_default().upsert(opencode_kit::model::InboxItem {
                id: id.into(), text: sent.draft.text.clone(), files: sent.files.clone(),
                delivery: sent.prompt.as_ref().and_then(|p| p.delivery).unwrap_or(opencode_kit::model::Delivery::Steer),
            });
        }
        self.unsent.note(Sent { phase: Phase::Sending { by: crate::windows::peer::token(self), then: None }, ..sent });
    }

    pub(crate) fn edit_failed_send(&mut self, id: &str) {
        let Some(sent) = self.unsent.get(id).filter(|s| s.phase_is(State::Failed)).cloned() else { return };
        if self.route != crate::Route::here(sent.session.clone()) || !self.composer.text.is_empty() || self.attach.any() { return; }
        self.restore_draft(sent);
        self.composer.focused = true;
    }
}

impl Sent {
    fn phase_is(&self, state: State) -> bool { self.state() == state }
}

/// Each prompt's receipt in turn, stopping at the first the server can't answer.
fn read_receipts(fetch: &dyn Fn(&str) -> Result<serde_json::Value, String>, ask: Vec<(String, String)>) -> Vec<(String, Result<receipt::Receipt, String>)> {
    let mut results = Vec::new();
    for (session, id) in ask {
        let result = receipt::read(fetch, &session, &id);
        let unavailable = result.is_err();
        results.push((id, result));
        if unavailable { break; }
    }
    results
}

