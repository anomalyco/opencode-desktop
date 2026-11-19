//! The outbox's way to the server: idempotent prompt admissions, each kept as a durable row
//! (`store.rs`) before its write can leave. The writer never sees a new ID until its complete
//! request and editable draft are committed. One outbox per server, shared by its windows: a
//! window that closes mid-send leaves its prompts to the others and to the next launch.

use std::time::Duration;

use opencode_kit::client::Write;
use opencode_kit::model::{Delivery, InboxItem, Message, Transcript};
use session_view::composer::history::Entry;

pub(super) use super::machine::Operation;
use super::store::{self, Forward, Mail, Store};
use super::{AfterCheck, Check, Event, Log, Phase, Retry, SendFailedHost, Sent, State, machine};

impl Log {
    /// Another window's outbox of the same server, joining this one (`windows::peer`): prompts
    /// this copy doesn't hold yet come in.
    pub fn absorb(&mut self, from: Log) {
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

/// The window's server's outbox: its stored prompts are read on the storage thread, after
/// the first frame, and come in through [`super::poll_sends`].
pub fn open_outbox(host: &mut dyn SendFailedHost) {
    if !host.live_allow_send() {
        return;
    }
    let Some(server) = host.live_server() else { return };
    let base = server.base;
    *host.unsent_mut() = Log::default();
    if !host.persists() {
        return;
    }
    let waker = host.waker();
    let window = host.window_id().to_owned();
    store::listen(&base, waker);
    host.with_storage(store::read_op(&base, &window));
    host.unsent_mut().store = Store::Reading { server: base };
}

/// This window is woken when the storage thread answers its server's outbox.
pub fn listen_outbox(host: &dyn SendFailedHost) {
    if let Some(server) = host.unsent().store.server() {
        store::listen(server, host.waker());
    }
}

/// What the storage thread sent back for this server: its stored prompts, a row that couldn't
/// be saved, a writer that was gone.
pub(super) fn take_mail(host: &mut dyn SendFailedHost) -> bool {
    let Some(server) = host.unsent().store.server().map(str::to_owned) else { return false };
    store::listen(&server, host.waker());
    let mail = store::take(&server);
    let changed = !mail.is_empty();
    for m in mail {
        match m {
            Mail::Read(rows) => stored_prompts(host, &server, rows),
            Mail::NotSaved { id, handoff } => {
                if let Some(s) = host.unsent_mut().sent.iter_mut().find(|s| s.id == id) {
                    s.row = store::Row::None;
                }
                super::step_send(host, &id, Event::NotSaved { handoff });
            }
            Mail::NotSent { id, handoff } => {
                super::step_send(host, &id, Event::NotSent { why: "waiting for connection", handoff });
            }
        }
    }
    changed
}

/// The server's stored prompts are in: each takes its place before the ones sent since
/// (it's older), a write out when the app quit checks first, and the outbox may pump.
fn stored_prompts(host: &mut dyn SendFailedHost, server: &str, rows: Result<Vec<Sent>, String>) {
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            // Storage that can't keep them: this run's outbox lives in memory.
            eprintln!("opencode-rune: outbox not readable, kept in memory: {e}");
            host.unsent_mut().store = Store::Memory;
            return;
        }
    };
    {
        let unsent = host.unsent_mut();
        if !matches!(unsent.store, Store::Reading { .. }) && unsent.store != (Store::Open { server: server.to_owned() }) {
            return;
        }
        unsent.store = Store::Open { server: server.to_owned() };
        for mut s in rows {
            if unsent.sent.iter().any(|x| x.id == s.id) {
                continue;
            }
            machine::step(&mut s, Event::Reopened);
            unsent.note(s);
        }
        unsent.sent.sort_by_key(|s| s.seq);
    }
    show_outbox(host);
    pump_outbox(host);
}

/// Each unsent prompt where it waits: a follow-up behind steers already waiting waits with
/// them, as the server will deliver it after them (t1361); the rest show as their bubble.
pub fn show_outbox(host: &mut dyn SendFailedHost) {
    let pending: Vec<Sent> = host
        .unsent()
        .sent
        .iter()
        .filter(|s| s.journaled && s.phase != Phase::Accepted)
        .cloned()
        .collect();
    for s in pending {
        let already_in_inbox = host.inboxes().get(&s.session).is_some_and(|i| i.items.iter().any(|i| i.id == s.id));
        if already_in_inbox {
            continue;
        }
        let has_steering = host.inboxes().get(&s.session).is_some_and(|i| i.steering().next().is_some());
        if s.inbox && has_steering {
            let delivery = s.prompt.as_ref().and_then(|p| p.delivery).unwrap_or(Delivery::Steer);
            let item = InboxItem { id: s.id.clone(), text: s.draft.text.clone(), files: s.files.clone(), delivery };
            host.inboxes_mut().entry(s.session.clone()).or_default().upsert(item);
            continue;
        }
        let t = host
            .transcripts_mut()
            .entry(s.session.clone())
            .or_insert_with(|| Transcript { session: s.session.clone(), messages: vec![] });
        if !t.messages.iter().any(|m| m.id() == s.id) {
            t.messages.push(Message::User { id: s.id.clone(), text: s.draft.text.clone() });
            opencode_kit::model::set_user_files(&s.id, s.files.clone());
            if let Some(p) = &s.prompt {
                opencode_kit::model::set_user_comments(&s.id, p.comments.clone());
            }
        }
    }
}

/// A new session's first prompt that must wait for its worktree or its folder's MCP servers
/// (`new_session`): its row is written now, Preparing, so a kill while it waits loses
/// nothing, and it goes once [`release_prepared`] says so. False when the outbox can't
/// take it (a read-only window, the storage in memory still takes it for the run).
pub fn enqueue_prepared(host: &mut dyn SendFailedHost, sent: Sent, write: Write) -> bool {
    enqueue_prompt_as(host, sent, write, true)
}

/// Prompt `id`, Preparing, may go: on to `directory` (its new worktree, made now) when given.
pub fn release_prepared(host: &mut dyn SendFailedHost, id: &str, directory: Option<String>) {
    if let Some(dir) = directory
        && let Some(s) = host.unsent_mut().sent.iter_mut().find(|s| s.id == id)
    {
        s.operation = Operation::Start(dir);
    }
    super::step_send(host, id, Event::Prepared);
    pump_outbox(host);
}

/// Takes `sent`, to go out as `write`: its row is committed on the storage thread and, when
/// it's next in line, the write leaves from there once the row is durable. The UI thread
/// writes nothing. False only for a write the outbox doesn't keep.
pub fn enqueue_prompt(host: &mut dyn SendFailedHost, sent: Sent, write: Write) -> bool {
    enqueue_prompt_as(host, sent, write, false)
}

fn enqueue_prompt_as(host: &mut dyn SendFailedHost, mut sent: Sent, write: Write, preparing: bool) -> bool {
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
    if host.is_session_here(&sent.session)
        && !host.composer().text.is_empty()
        && Entry::of(host.composer()) == sent.draft
        && host.unsent().store != Store::Memory
        && let Some(key) = host.active_draft_document()
    {
        let rev = host.hand_off_draft_doc(&key);
        sent.left_draft = super::LeftDraft::Pending { key, rev };
    }
    let offline = host.is_reconnect_down() || host.live_writer().is_none();
    sent.phase = if preparing {
        Phase::Preparing
    } else {
        Phase::Waiting { why: if offline { "waiting for connection".into() } else { String::new() } }
    };
    let (id, session) = (sent.id.clone(), sent.session.clone());
    host.unsent_mut().note(sent);
    opencode_platform::telemetry::send_receipt(&session, &id, "saved", 0);
    pump_outbox(host);
    // Not its turn (offline, or behind another): its row holds it until it is.
    if host.unsent().state(&id) == Some(State::Waiting)
        && let Some(op) = host.unsent_mut().persist_op(&id, None)
    {
        host.with_storage(op);
    }
    true
}

pub(super) fn defer_outbox(host: &mut dyn SendFailedHost) {
    let now = host.clock();
    let unsent = host.unsent_mut();
    let attempts = match unsent.retry {
        Retry::Ready => 0,
        Retry::Backoff { attempts, .. } => attempts,
    };
    let delay = (0.25 * 2_f64.powi(attempts.min(5) as i32)).min(5.0);
    unsent.retry = Retry::Backoff { attempts: attempts + 1, until: now + delay };
}

/// Each frame: marks slow sends past [`super::SLOW_SEND`], pumps the outbox when backoff allows,
/// and arms the next wakeup timer.
pub fn tick_outbox(host: &mut dyn SendFailedHost, tx: &mut rune::Tx) -> bool {
    // A prompt sending past `SLOW_SEND` says so whether or not the link is up: a server that
    // took the POST and holds it is the case it's for.
    let now = host.clock();
    // Most polls change nothing: asked first, they leave the outbox unwritten, so the regions
    // that read it (the timeline's footers) keep.
    let (mut changed, slow_at) = if host.unsent_untracked().slow_writes(now) {
        host.unsent_mut().tell_slow(now)
    } else {
        host.unsent_untracked().slow_next()
    };
    if let Some(at) = slow_at {
        tx.wake_at(at);
    }
    if !host.unsent().waiting_for_link() {
        return changed;
    }
    let due = match host.unsent().retry {
        Retry::Ready => now,
        Retry::Backoff { until, .. } => until,
    };
    if now >= due {
        changed |= pump_outbox(host);
        defer_outbox(host);
    }
    let next = match host.unsent().retry {
        Retry::Ready => host.clock(),
        Retry::Backoff { until, .. } => until,
    };
    tx.wake_after(Duration::from_secs_f64((next - host.clock()).max(0.01)));
    changed
}

/// Sends the outbox's oldest prompt still on its way, through this window's writer, once the
/// one before it is confirmed: its `Sending` row commits on the storage thread, which hands
/// the write to the writer once it's durable.
pub fn pump_outbox(host: &mut dyn SendFailedHost) -> bool {
    if host.is_reconnect_down() || matches!(host.unsent().store, Store::Reading { .. }) || !host.live_allow_send() {
        return false;
    }
    let Some(writer) = host.live_writer() else {
        return false;
    };
    let next = host
        .unsent()
        .sent
        .iter()
        .find(|s| s.journaled && !matches!(s.phase, Phase::Accepted | Phase::Failed { .. } | Phase::Preparing))
        .map(|s| (s.id.clone(), s.session.clone(), s.phase.clone(), s.prompt.clone(), s.operation.clone()));
    let Some((id, session, phase, prompt, operation)) = next else {
        return false;
    };
    match phase {
        // A successful POST may have lost its SSE echo; check before releasing the next prompt.
        Phase::Unconfirmed { .. } => {
            if host.unsent().check == Check::Idle {
                super::check_sends(host);
            }
            return false;
        }
        Phase::Sending { .. } => return false,
        _ => {}
    }
    let Some(prompt) = prompt else {
        return false;
    };
    let by = host.peer_token();
    let now = host.clock();
    super::step_send(host, &id, Event::Send { by, now });
    let Some(Phase::Sending { handoff, .. }) = host.unsent().get(&id).map(|s| s.phase.clone()) else { return true };
    let write = operation.write(session, prompt);
    if host.unsent().store == Store::Memory {
        if writer.send(write).is_err() {
            super::step_send(host, &id, Event::NotSent { why: "waiting for connection", handoff });
        }
        return true;
    }
    match hand_over(&writer, write, handoff) {
        Some(forward) => {
            if let Some(op) = host.unsent_mut().persist_op(&id, Some(forward)) {
                host.with_storage(op);
            }
        }
        None => {
            super::step_send(host, &id, Event::NotSent { why: "waiting for connection", handoff });
        }
    }
    true
}

/// Puts a prompt that never went back in the composer, when that's empty.
pub fn restore_draft(host: &mut dyn SendFailedHost, sent: Sent) {
    host.retain_echoes_without(&sent.id);
    if host.composer().text.is_empty() && !host.composer_has_attachments() && !host.composer_has_quotes() {
        let caret = sent.draft.text.len();
        sent.draft.put(host.composer_mut(), caret);
        for f in sent.files {
            host.add_composer_file(f);
        }
        for p in sent.paths {
            host.add_composer_path(p);
        }
        if let Some(prompt) = sent.prompt {
            for quote in prompt.comments {
                host.push_composer_quote(quote);
            }
        }
    }
}

pub(super) fn restore_rejection(host: &mut dyn SendFailedHost, id: &str) {
    let Some(s) = host.unsent().get(id).cloned() else { return };
    if !host.is_session_here(&s.session) {
        // Its session's draft takes it back whole, files and quotes too, when that's empty.
        let quotes = s.prompt.map(|p| p.comments).unwrap_or_default();
        host.restore_inactive_draft(&s.session, s.draft, s.files, s.paths, quotes);
        return;
    }
    if host.composer().text.is_empty() && !host.composer_has_attachments() && !host.composer_has_quotes() {
        for entry in host.unsent_mut().sent.iter_mut().filter(|entry| entry.session == s.session && entry.id != id) {
            if let Phase::Failed { restored, .. } = &mut entry.phase {
                *restored = false;
            }
        }
        super::step_send(host, id, Event::Restored);
    }
    restore_draft(host, s);
}

fn matches_restored(host: &dyn SendFailedHost, s: &Sent) -> bool {
    host.is_session_here(&s.session)
        && Entry::of(host.composer()) == s.draft
        && host.composer_files() == s.files.as_slice()
        && host.composer_paths() == s.paths.as_slice()
        && !host.composer_has_context_or_uploads()
        && s.prompt.as_ref().is_some_and(|p| p.comments == host.composer_quotes())
}

/// Enter on the untouched restored draft is Retry, not a second independently retryable bubble.
pub fn retry_restored(host: &mut dyn SendFailedHost) -> bool {
    let picked = host.picked_choice();
    let Some(id) = host
        .unsent()
        .sent
        .iter()
        .rev()
        .find(|s| {
            matches!(s.phase, Phase::Failed { restored: true, .. })
                && matches_restored(host, s)
                && s.prompt.as_ref().is_some_and(|p| (p.agent.clone(), p.model.clone()) == picked)
        })
        .map(|s| s.id.clone())
    else {
        return false;
    };
    outbox_action(host, &id, "outbox-retry");
    true
}

/// An Edit or Delete the machine says to carry out now. An Edit waits while its session
/// isn't the open one.
pub(super) fn run_outbox_action(host: &mut dyn SendFailedHost, id: &str, action: AfterCheck) {
    let elsewhere = host.unsent().get(id).is_some_and(|s| !host.is_session_here(&s.session));
    if action == AfterCheck::Edit && elsewhere {
        super::step_send(host, id, Event::Parked);
        return;
    }
    remove_from_outbox(host, id, action);
}

/// The bubble's or its menu's Retry now, Edit in composer or Delete.
pub fn outbox_action(host: &mut dyn SendFailedHost, id: &str, action: &str) {
    let Some(s) = host.unsent().get(id).filter(|s| s.journaled).cloned() else {
        return;
    };
    if action == "outbox-retry" {
        if s.state() == State::Failed {
            super::step_send(host, id, Event::Retry);
            if matches!(s.phase, Phase::Failed { restored: true, .. }) && matches_restored(host, &s) {
                host.composer_mut().clear();
                host.clear_composer_attachments_and_quotes();
            }
        }
        if let Retry::Backoff { attempts, .. } = host.unsent().retry {
            host.unsent_mut().retry = Retry::Backoff { attempts, until: 0.0 };
        }
        pump_outbox(host);
        return;
    }
    let act = if action == "outbox-edit" { AfterCheck::Edit } else { AfterCheck::Delete };
    if act == AfterCheck::Edit && matches!(s.phase, Phase::Waiting { .. } | Phase::Failed { .. }) && !host.is_session_here(&s.session) {
        return;
    }
    super::step_send(host, id, Event::Act(act));
}

/// Takes prompt `id` out of the outbox and the timeline; an Edit puts its words in the
/// composer, after anything already there.
fn remove_from_outbox(host: &mut dyn SendFailedHost, id: &str, action: AfterCheck) {
    let (s, delete_op) = {
        let unsent = host.unsent_mut();
        let Some(at) = unsent.sent.iter().position(|s| s.id == id) else { return };
        let mut s = unsent.sent.remove(at);
        let op = unsent.forget_op(&mut s);
        (s, op)
    };
    if let Some(op) = delete_op {
        host.with_storage(op);
    }
    if let Some(t) = host.transcripts_mut().get_mut(&s.session) {
        t.messages.retain(|m| m.id() != id);
    }
    if let Some(i) = host.inboxes_mut().get_mut(&s.session) {
        i.remove(id);
    }
    host.retain_echoes_without(id);
    if action != AfterCheck::Edit {
        return;
    }
    let mut draft = Entry::of(host.composer());
    if draft.text != s.draft.text {
        if !draft.text.is_empty() {
            draft.text.push_str("\n\n");
        }
        let offset = draft.text.len();
        draft.text.push_str(&s.draft.text);
        draft.pills.extend(s.draft.pills.into_iter().map(|(r, p)| (r.start + offset..r.end + offset, p)));
    }
    let caret = draft.text.len();
    draft.put(host.composer_mut(), caret);
    for f in s.files {
        host.add_composer_file(f);
    }
    for p in s.paths {
        host.add_composer_path(p);
    }
    if let Some(prompt) = s.prompt {
        for quote in prompt.comments {
            if !host.composer_quotes().contains(&quote) {
                host.push_composer_quote(quote);
            }
        }
    }
    if !session_view::style::touch_controls() {
        host.composer_mut().focused = true;
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
    Some(Forward::After { writer: writer.clone(), write: Box::new(write), handoff })
}
