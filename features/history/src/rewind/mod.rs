//! Rewinding a session to a user message, undoing that rewind from its toast, and `/undo` +
//! `/redo`, matching OpenCode Desktop (`packages/app/src/session/revert.ts`,
//! `packages/session-ui/src/message/message-content.tsx`, `packages/app/src/composer/submit.ts`).
//! Each session's operations are [`machine::SessionRevert`] (`session/revert-machine.ts`).
//!
//! Acting updates the timeline and composer on the press: messages from the boundary onward
//! leave on the timeline's growth spring, the rewound prompt returns to the composer, and the
//! `"Reverted"` toast offers `"Undo"`. If the server refuses, or doesn't answer in 30 s, the
//! timeline, running state and composer draft roll back and a failure toast reports why. An
//! undelivered prompt is withdrawn instead, its words back in the composer once the server lets
//! it go.

pub mod machine;

use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use opencode_kit::client::{PathAttachment, Ticket, TicketKind, Write};
use opencode_kit::mention::{self, MentionKind};
use opencode_kit::model::{self, Attachment, Inbox, Message, PromptComment, SessionInfo, Transcript};
use opencode_kit::wire_error::WireError;
use opencode_ui::popup::{Align, Item, Menu, MenuButton as _, PopupHost};
use rune::machine::Attempt;
use rune::{El, LocalTime, Tx, icon};
use rune_theme_opencode::icons as ic;
use serde_json::Value;
use session_view::palette::Pal;

pub use session_view::composer::history::Entry;

use machine::{Doubt, Effect, Event, Operation, Rollback, SessionRevert, State};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedDraft {
    pub entry: Entry,
    pub caret: usize,
    pub files: Vec<Attachment>,
    pub paths: Vec<PathAttachment>,
    pub quotes: Vec<PromptComment>,
}

impl SavedDraft {
    /// The same words, pills, files, paths and quotes, wherever the caret is.
    pub fn same_words(&self, other: &SavedDraft) -> bool {
        self.entry == other.entry && self.files == other.files && self.paths == other.paths && self.quotes == other.quotes
    }

    pub fn is_empty(&self) -> bool {
        self.entry.text.trim().is_empty() && self.files.is_empty() && self.paths.is_empty() && self.quotes.is_empty()
    }
}

/// The Rewound toast's Undo: the session and the rewind (`attempt`) it undoes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoRewind {
    pub session: String,
    pub attempt: Attempt,
}

#[derive(Default)]
pub struct RewindState {
    /// The message whose menu is open: `(session_id, message_id)`.
    pub menu: Option<(String, String)>,
    /// Each session's rewinds, undos and redos (`machine.rs`).
    sessions: HashMap<String, SessionRevert>,
}

impl RewindState {
    /// A rewind, undo or redo of `session` is on its way: another waits for it.
    pub fn busy(&self, session: &str) -> bool {
        self.sessions.get(session).is_some_and(SessionRevert::running)
    }

    pub fn menu_is(&self, session: &str, message_id: &str) -> bool {
        self.menu.as_ref().is_some_and(|(s, m)| s == session && m == message_id)
    }

    pub fn menu_open_on(&self, message_id: &str) -> bool {
        self.menu.as_ref().is_some_and(|(_, m)| m == message_id)
    }
}

/// Object-safe host boundary for session timeline Rewind (`/undo`, `/redo`, `"Reverted"` toast,
/// staged revert commits, and user/assistant message actions menus).
pub trait RewindHost: opencode_kernel::AppEnv + opencode_kernel::AppFeedback {
    fn rewind(&self) -> &RewindState;
    fn rewind_mut(&mut self) -> &mut RewindState;

    /// Metadata for `session`, if known.
    fn session(&self, session: &str) -> Option<&SessionInfo>;
    /// Sets `SessionInfo::revert` for `session`.
    fn set_session_revert(&mut self, session: &str, revert: Option<String>);

    /// Loaded transcript for `session`, if any.
    fn transcript(&self, session: &str) -> Option<&Transcript>;
    /// Mutable loaded transcript for `session` (used when committing a staged revert locally).
    fn transcript_mut(&mut self, session: &str) -> Option<&mut Transcript>;
    /// Undelivered inbox for `session`, if any.
    fn inbox(&self, session: &str) -> Option<&Inbox>;
    /// Mutable undelivered inbox for `session` (used when withdrawing or committing a revert).
    fn inbox_mut(&mut self, session: &str) -> Option<&mut Inbox>;

    /// Captures `session`'s current composer draft, attachments, and quotes (live composer or parked).
    fn capture_draft(&self, session: &str) -> SavedDraft;
    /// Applies `draft` to `session`'s composer (live if active, or parked for when it opens).
    fn apply_draft(&mut self, session: &str, draft: SavedDraft);
    /// Exits shell mode in the composer when restoring a non-shell prompt draft.
    fn exit_shell(&mut self) {}
    /// Display title for a `@session:<id>` mention pill when restoring a prompt draft.
    fn mention_title(&self, _session_id: &str) -> Option<String> {
        None
    }
    /// Resolves a skill name or id to its canonical skill id when restoring `@skill` pills.
    fn resolve_skill_id(&self, _name_or_id: &str) -> Option<&str> {
        None
    }

    /// Removes all running session IDs belonging to `session`'s root family and returns them for rollback.
    fn stop_running_family(&mut self, _session: &str) -> Vec<String> {
        Vec::new()
    }
    /// Restores previously stopped session IDs on rollback.
    fn restore_running(&mut self, _sessions: Vec<String>) {}
    /// Whether the host runs in fixture or storybook mode (server writes succeed immediately locally).
    fn is_simulated(&self) -> bool {
        false
    }
    /// Drops queued local playback events for `session` in fixture/storybook mode.
    fn clear_playback(&mut self, _session: &str) {}

    /// Dispatches a tracked server write; returns `false` when the write could not be sent.
    fn send_write(&mut self, _write: Write) -> bool {
        false
    }
    /// Current status line text used as the fallback reason when [`Self::send_write`] returns `false`.
    fn status_text(&self) -> &str {
        ""
    }
    /// Formats a server [`WireError`] into a user-facing message.
    fn format_wire_error(&self, err: &WireError) -> String {
        err.to_string()
    }
    /// Notifies peer windows that shared session/revert state changed.
    fn touch_peer(&mut self) {}

    /// Shows the `"Reverted"` toast with an `"Undo"` action bound to `undo`, returning its toast ID.
    fn toast_rewound(&mut self, undo: UndoRewind) -> u64;
    /// Dismisses a specific toast by ID (on rollback).
    fn toast_dismiss_id(&mut self, id: u64);
    /// Dismisses the current toast if it is an `UndoRewind` toast for `session`.
    fn dismiss_rewound_toast(&mut self, session: &str);

    /// Whether a popup menu is currently open at `anchor`.
    fn popup_open_at(&self, _anchor: &str) -> bool {
        false
    }
    /// Dismisses the currently open popup menu.
    fn dismiss_popup(&mut self, _tx: &mut Tx) {}
    /// Opens `menu` anchored at `anchor` for `(session, message_id)`, calling
    /// [`on_message_menu_picked`] when the popup settles.
    fn open_message_popup(&mut self, _menu: Menu, _anchor: String, _session: String, _message_id: String, _tx: &mut Tx) {}

    /// Message action: moves an undelivered steer to the queue (`"queue"`).
    fn queue_pending(&mut self, _session: &str, _message_id: &str) {}
    /// Message action: withdraws/deletes an undelivered steer (`"delete"`).
    fn withdraw_pending(&mut self, _session: &str, _message_id: &str) {}
    /// Message action: forks `session` from delivered user message `message_id` (`"fork"`).
    fn fork_from_message(&mut self, _session: &str, _message_id: &str, _tx: &mut Tx) {}
}

/// Whether `session` is a root session (subagent sessions hide rewind and fork).
pub fn can_rewind_session(host: &dyn RewindHost, session: &str) -> bool {
    host.session(session).is_none_or(|i| i.parent.is_none())
}

/// Visible user messages (`id < revert`) in `session`, in chronological order.
pub fn visible_user_ids(host: &dyn RewindHost, session: &str) -> Vec<String> {
    let revert = host.session(session).and_then(|i| i.revert.as_deref());
    let Some(t) = host.transcript(session) else { return Vec::new() };
    let shown = t.visible_len(revert);
    t.messages[..shown]
        .iter()
        .filter_map(|m| match m {
            Message::User { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// What `/undo` rewinds to (`revert-machine.ts` `undo.requested`): the newest user message
/// before the session's boundary, an undelivered one in the inbox included (which Undo withdraws).
fn undo_target(host: &dyn RewindHost, session: &str) -> Option<String> {
    let boundary = host.session(session).and_then(|i| i.revert.as_deref());
    let sent = host.transcript(session).into_iter().flat_map(|t| &t.messages).filter_map(|m| match m {
        Message::User { id, .. } => Some(id.as_str()),
        _ => None,
    });
    let waiting = host.inbox(session).into_iter().flat_map(|i| i.items.iter().map(|i| i.id.as_str()));
    sent.chain(waiting).filter(|id| boundary.is_none_or(|b| *id < b)).max().map(str::to_owned)
}

/// Whether `/undo` and `⌘K → Undo` are enabled on the open session.
pub fn can_undo_session(host: &dyn RewindHost) -> bool {
    let Some(id) = host.active_session_id() else { return false };
    can_rewind_session(host, id) && !host.rewind().busy(id) && undo_target(host, id).is_some()
}

/// Whether `/redo` and `⌘K → Redo` are enabled on the open session.
pub fn can_redo_session(host: &dyn RewindHost) -> bool {
    let Some(id) = host.active_session_id() else { return false };
    can_rewind_session(host, id) && !host.rewind().busy(id) && host.session(id).and_then(|i| i.revert.as_ref()).is_some()
}

/// Builds the composer draft to restore from user message `(id, body)` using `host` for
/// session-mention titles and skill ID resolution.
pub fn draft_from_message(host: &dyn RewindHost, id: &str, body: &str) -> SavedDraft {
    draft_from_message_with(id, body, |sid| host.mention_title(sid), |val| host.resolve_skill_id(val).map(str::to_owned))
}

/// Pure builder for restoring a [`SavedDraft`] from user message `(id, body)`.
pub fn draft_from_message_with(
    id: &str,
    body: &str,
    mention_title: impl Fn(&str) -> Option<String>,
    resolve_skill_id: impl Fn(&str) -> Option<String>,
) -> SavedDraft {
    let paths = mention::user_path_attachments(id).to_vec();
    let mut clean = body;
    for p in paths.iter().rev() {
        let line = format!("Attached file: `{}`", p.path);
        if let Some(head) = clean.strip_suffix(&line) {
            clean = head.strip_suffix('\n').unwrap_or(head);
        }
    }
    let (shown, pills) = restore_pills(id, clean, mention_title, resolve_skill_id);
    let caret = shown.len();
    SavedDraft {
        entry: Entry { text: shown, pills },
        caret,
        files: model::user_files(id).to_vec(),
        paths,
        quotes: model::user_comments(id).to_vec(),
    }
}

/// Rewinds `session` to user message `message_id` (the message menu's Undo or Revert to here),
/// or withdraws it when it's still in the inbox.
pub fn rewind_to(host: &mut dyn RewindHost, session: &str, message_id: &str, _tx: &mut Tx) {
    close_message_menu(host);
    if !can_rewind_session(host, session) || host.rewind().busy(session) {
        return;
    }
    // An undelivered inbox prompt has no history to rewind: withdraw it into the composer.
    if let Some(item) = host.inbox(session).and_then(|i| i.items.iter().find(|x| x.id == message_id)).cloned() {
        let mut draft = draft_from_message(host, &item.id, &item.text);
        if draft.files.is_empty() {
            draft.files = item.files;
        }
        let operation = Operation::Withdraw { message: message_id.to_owned(), draft };
        let now = host.clock();
        step_revert(host, session, Event::Requested { operation, now });
        return;
    }
    let Some(body) = host.transcript(session).and_then(|t| {
        t.messages.iter().find_map(|m| match m {
            Message::User { id, text } if id == message_id => Some(text.clone()),
            _ => None,
        })
    }) else {
        return;
    };

    let prior = host.session(session).and_then(|i| i.revert.clone());
    let prior_draft = host.capture_draft(session);
    let restored = draft_from_message(host, message_id, &body);
    let stopped = host.stop_running_family(session);
    let attempt = host.rewind_mut().sessions.entry(session.to_owned()).or_default().next_attempt();

    // Shown on the press: the rows leave, the prompt comes back, the toast offers Undo.
    if host.is_simulated() {
        host.clear_playback(session);
    }
    host.set_session_revert(session, Some(message_id.to_owned()));
    host.exit_shell();
    host.apply_draft(session, restored.clone());
    let toast = host.toast_rewound(UndoRewind { session: session.to_owned(), attempt });

    let rollback = Rollback { revert: prior.clone(), stopped, draft: Some(prior_draft.clone()), toast: Some(toast) };
    let operation = Operation::Rewind { message: message_id.to_owned(), prior, draft: prior_draft, restored, rollback };
    let now = host.clock();
    step_revert(host, session, Event::Requested { operation, now });
}

/// `/undo` and `⌘K → Undo`: rewinds to the newest user message before the session's boundary,
/// or withdraws it when it's still undelivered (`revert-machine.ts` `undo.requested`).
pub fn undo_last(host: &mut dyn RewindHost, tx: &mut Tx) -> bool {
    let Some(session) = host.active_session_id().map(str::to_owned) else { return false };
    if host.rewind().busy(&session) {
        return false;
    }
    let Some(last) = undo_target(host, &session) else { return false };
    rewind_to(host, &session, &last, tx);
    true
}

/// Alias for [`undo_last`].
pub fn undo(host: &mut dyn RewindHost, tx: &mut Tx) -> bool {
    undo_last(host, tx)
}

/// `/redo` and `⌘K → Redo`: restores every reverted message at once and leaves the composer alone.
pub fn redo_revert(host: &mut dyn RewindHost, _tx: &mut Tx) -> bool {
    let Some(session) = host.active_session_id().map(str::to_owned) else { return false };
    if !can_rewind_session(host, &session) || host.rewind().busy(&session) {
        return false;
    }
    let Some(prior) = host.session(&session).and_then(|i| i.revert.clone()) else {
        return false;
    };
    host.set_session_revert(&session, None);
    let rollback = Rollback { revert: Some(prior), ..Rollback::default() };
    let now = host.clock();
    step_revert(host, &session, Event::Requested { operation: Operation::Redo { rollback }, now });
    true
}

/// Alias for [`redo_revert`].
pub fn redo(host: &mut dyn RewindHost, tx: &mut Tx) -> bool {
    redo_revert(host, tx)
}

/// `Undo` on the `"Reverted"` toast: bound to its session and rewind, even after switching
/// tabs, and puts back the unsent draft that was in the composer. An older toast's Undo, or
/// one whose rewind another boundary replaced, does nothing.
pub fn undo_rewind(host: &mut dyn RewindHost, undo: UndoRewind, _tx: &mut Tx) {
    let (attempt, prior) = match host.rewind().sessions.get(&undo.session).map(|r| r.state.clone()) {
        Some(State::Restored { attempt, prior, .. }) if attempt == undo.attempt => (attempt, prior),
        Some(State::Running { attempt, operation: Operation::Rewind { prior, .. }, .. }) if attempt == undo.attempt => (attempt, prior),
        _ => return,
    };
    let rollback = Rollback {
        revert: host.session(&undo.session).and_then(|i| i.revert.clone()),
        draft: Some(host.capture_draft(&undo.session)),
        ..Rollback::default()
    };
    host.set_session_revert(&undo.session, prior);
    let now = host.clock();
    step_revert(host, &undo.session, Event::ToastUndo { attempt, now, rollback });
}

/// Steps `session`'s revert machine and carries out what it says.
fn step_revert(host: &mut dyn RewindHost, session: &str, event: Event) {
    let effects = host.rewind_mut().sessions.entry(session.to_owned()).or_default().step(event);
    for effect in effects {
        revert_effect(host, session, effect);
    }
    host.touch_peer();
}

fn revert_effect(host: &mut dyn RewindHost, session: &str, effect: Effect) {
    match effect {
        Effect::Run { attempt, operation } => {
            let write = match &operation {
                Operation::Withdraw { message, .. } => Write::Cancel { session: session.to_owned(), item: message.clone() },
                Operation::Rewind { message, .. } => Write::RevertStage { session: session.to_owned(), message: message.clone(), settle: true },
                Operation::Redo { .. } | Operation::UndoRewind { prior: None, .. } => Write::RevertClear(session.to_owned()),
                Operation::UndoRewind { prior: Some(prior), .. } => Write::RevertStage { session: session.to_owned(), message: prior.clone(), settle: false },
            };
            // The fixture and the storybook act as if the server agreed at once.
            if host.is_simulated() {
                step_revert(host, session, Event::Succeeded { attempt });
                return;
            }
            let ticket = Ticket { kind: TicketKind::Revert, session: session.to_owned(), attempt: attempt.0 };
            if !host.send_write(Write::Tracked { ticket, write: Box::new(write) }) {
                let why = host.status_text().to_owned();
                let now = host.clock();
                step_revert(host, session, Event::Failed { attempt, why, now });
            }
        }
        Effect::Undo { rollback, why } => {
            if let Some(rollback) = rollback {
                host.set_session_revert(session, rollback.revert);
                host.restore_running(rollback.stopped);
                if let Some(id) = rollback.toast {
                    host.toast_dismiss_id(id);
                }
                if let Some(draft) = rollback.draft {
                    host.apply_draft(session, draft);
                }
            }
            let why = why.split_once(": ").map_or(why.as_str(), |(_, why)| why).to_owned();
            host.toast_fail(opencode_i18n::t!("common.requestFailed").to_owned(), why);
        }
        // `revert.ts` `restore`: the withdrawn prompt replaces the draft.
        Effect::RestoreMessage { message, draft } => {
            if let Some(inbox) = host.inbox_mut(session) {
                inbox.remove(&message);
            }
            model::set_user_files(&message, Vec::new());
            host.exit_shell();
            host.apply_draft(session, draft);
        }
        Effect::RestoreDraft { draft } => host.apply_draft(session, draft),
        // The server holds the boundary of a rewind that was rolled back: it shows, and its
        // prompt comes back unless something was typed over the rolled-back draft since.
        Effect::Land { attempt, boundary, rolled_back, restored } => {
            host.set_session_revert(session, Some(boundary));
            if host.capture_draft(session).same_words(&rolled_back) {
                host.exit_shell();
                host.apply_draft(session, restored);
            }
            let _ = host.toast_rewound(UndoRewind { session: session.to_owned(), attempt });
        }
        Effect::DismissToast => host.dismiss_rewound_toast(session),
    }
}

/// `Incoming::Settled` for a revert ticket.
pub fn revert_settled(host: &mut dyn RewindHost, session: &str, attempt: u32, result: Result<(), WireError>) {
    let attempt = Attempt(attempt);
    let event = match result {
        Ok(()) => Event::Succeeded { attempt },
        Err(e) => Event::Failed { attempt, why: host.format_wire_error(&e), now: host.clock() },
    };
    step_revert(host, session, event);
}

/// Each frame: an operation past its 30 s bound fails as it got. True when one did.
pub fn revert_timeouts(host: &mut dyn RewindHost, tx: &mut Tx) -> bool {
    let now = host.clock();
    let due: Vec<String> = host
        .rewind()
        .sessions
        .iter()
        .filter(|(_, r)| r.due().is_some_and(|d| now + 1e-9 >= d))
        .map(|(id, _)| id.clone())
        .collect();
    for session in &due {
        step_revert(host, session, Event::Due { now });
    }
    if let Some(next) = host.rewind().sessions.values().filter_map(SessionRevert::due).reduce(f64::min) {
        tx.wake_after(Duration::from_secs_f64((next - now).max(0.0)));
    }
    !due.is_empty()
}

pub fn set_session_revert(host: &mut dyn RewindHost, session: &str, revert: Option<String>) {
    host.set_session_revert(session, revert);
}

/// Commits a staged revert locally right before sending a new prompt (`composer/submit.ts`),
/// so the reverted tail is dropped and the new prompt isn't hidden behind the old boundary.
pub fn commit_staged_revert(host: &mut dyn RewindHost, session: &str) -> bool {
    let Some(boundary) = host.session(session).and_then(|i| i.revert.clone()) else {
        return false;
    };
    host.set_session_revert(session, None);
    step_revert(host, session, Event::Committed);
    if let Some(inbox) = host.inbox_mut(session) {
        inbox.items.retain(|i| i.id < boundary);
    }
    if let Some(t) = host.transcript_mut(session) {
        let keep = t.visible_len(Some(&boundary));
        t.messages.truncate(keep);
    }
    host.dismiss_rewound_toast(session);
    true
}

/// Opens message `message_id`'s menu under its corner chevron, as Desktop's `Menu` opens from
/// its trigger (`message-content.tsx:426-503`), or closes it when it's the one open.
pub fn toggle_message_menu(host: &mut dyn RewindHost, session: &str, message_id: &str, tx: &mut Tx) {
    let key = menu_key(message_id);
    if host.rewind().menu_is(session, message_id) && host.popup_open_at(&key) {
        tx.instant();
        host.dismiss_popup(tx);
        return;
    }
    let Some(menu) = message_menu(host, session, message_id) else { return };
    host.rewind_mut().menu = Some((session.to_owned(), message_id.to_owned()));
    host.open_message_popup(menu, key, session.to_owned(), message_id.to_owned(), tx);
}

/// Callback invoked by the host when a message popup opened via
/// [`RewindHost::open_message_popup`] closes or selects an item.
pub fn on_message_menu_picked(host: &mut dyn RewindHost, session: &str, message_id: &str, picked: Option<&str>, tx: &mut Tx) {
    close_message_menu(host);
    if let Some(id) = picked {
        run_message_item(host, session, message_id, id, tx);
    }
}

pub fn close_message_menu(host: &mut dyn RewindHost) {
    host.rewind_mut().menu = None;
}

/// What a `session.revert.*` event says of the session's boundary: `Some(Some(id))` staged at
/// `id`, `Some(None)` cleared or committed.
pub fn reported(ty: &str, data: &Value) -> Option<Option<String>> {
    match ty {
        "session.revert.staged" => Some(data["revert"]["messageID"].as_str().map(str::to_owned)),
        "session.revert.cleared" | "session.revert.committed" => Some(None),
        _ => None,
    }
}

/// A `session.revert.*` event in this window: its machine takes the echo of its own request, or
/// a change from elsewhere. While its later requests are still out, the boundary it shows is
/// the last one's, which a stale echo mustn't overwrite: that boundary is returned.
pub fn landed(host: &mut dyn RewindHost, ty: &str, data: &Value, sid: &str) -> Option<Option<String>> {
    let reported = reported(ty, data)?;
    if !host.rewind().sessions.contains_key(sid) {
        return None;
    }
    let event = if ty == "session.revert.committed" { Event::Committed } else { Event::Reported(reported) };
    step_revert(host, sid, event);
    host.rewind().sessions.get(sid)?.expected().cloned()
}

/// A fresh read of the sessions (a reconnect's): a rolled-back rewind whose boundary the server
/// now holds landed after all.
pub fn read_sessions(host: &mut dyn RewindHost) {
    let h: &dyn RewindHost = host;
    let doubtful: Vec<(String, Option<String>)> = h
        .rewind()
        .sessions
        .iter()
        .filter(|(_, r)| r.doubtful())
        .map(|(id, _)| (id.clone(), h.session(id).and_then(|i| i.revert.clone())))
        .collect();
    for (sid, revert) in doubtful {
        step_revert(host, &sid, Event::Read(revert));
    }
}

/// `sid`'s inbox changed: a withdrawal whose cancel failed, whose prompt left the inbox without
/// reaching the transcript, was cancelled after all.
pub fn inbox_changed(host: &mut dyn RewindHost, sid: &str) {
    let Some(State::Doubtful { doubt: Doubt::Withdraw { message, .. }, .. }) = host.rewind().sessions.get(sid).map(|r| &r.state) else {
        return;
    };
    let message = message.clone();
    let queued = host.inbox(sid).is_some_and(|i| i.items.iter().any(|i| i.id == message));
    let delivered = host.transcript(sid).is_some_and(|t| t.messages.iter().any(|m| m.id() == message));
    if !queued && !delivered {
        step_revert(host, sid, Event::Withdrawn { message });
    }
}

/// The key of message `id`'s corner chevron, which its menu opens from.
fn menu_key(id: &str) -> String {
    format!("user-menu-{id}")
}

fn user_menu_shell(host: &dyn RewindHost, p: &Pal, session: &str, id: &str, pending: bool) -> Option<El> {
    if session.is_empty() || (pending && !can_rewind_session(host, session)) {
        return None;
    }
    let open = host.rewind().menu_is(session, id);
    Some(
        session_view::timeline::user_menu_button(p, menu_key(id), opencode_i18n::t!("ui.message.moreActions"), open)
            .child(icon(&ic::CHEVRON_DOWN, 14.0).color(if open { p.icon } else { p.icon_muted })),
    )
}

/// The chevron in a user message's top trailing corner (`user-message-menu`): `chevron-down` at
/// 14 pt, icon-muted, or icon-base on the pressed fill while its menu is open (`icon-button.css`'s
/// ghost-muted).
pub fn user_menu<H: RewindHost + PopupHost + 'static>(host: &H, p: &Pal, session: &str, id: &str, pending: bool) -> Option<El> {
    let button = user_menu_shell(host, p, session, id, pending)?;
    let (sid, mid) = (session.to_owned(), id.to_owned());
    Some(button.menu_button(menu_key(id), move |s: &mut H, tx: &mut Tx| toggle_message_menu(s, &sid, &mid, tx)))
}

/// Message `mid`'s menu under its chevron: [`message_items`] in a menu of their own.
fn message_menu(host: &dyn RewindHost, session: &str, mid: &str) -> Option<Menu> {
    let menu = Menu::new(opencode_i18n::t!("ui.message.moreActions")).above().align(Align::End).min_width(160.0);
    message_items(host, session, mid, menu)
}

/// Message `mid`'s items after `menu`'s, as Desktop builds them (`message-content.tsx:440-498`):
/// when it was sent (or Pending), then Move to queue on a steer, Copy message, Fork from here
/// on a delivered message, and last, after a rule, Delete on a steer or Undo (the newest prompt)
/// or Revert to here where the session can rewind. A subagent's session can't fork or rewind. A
/// reply has when it was written and Copy message. The chevron's menu and a right-click's both
/// take them from here.
pub fn message_items(host: &dyn RewindHost, session: &str, mid: &str, mut menu: Menu) -> Option<Menu> {
    let now_ms = host.now_ms();
    if let Some(reply) = reply_body(host, session, mid) {
        if let Some(when) = sent_label(mid, now_ms) {
            menu = menu.group(when).separator();
        }
        return Some(menu.item(Item::new("copy", opencode_i18n::t!("ui.message.copyMessage")).icon(&ic::OUTLINE_COPY).enabled(!reply.trim().is_empty())));
    }
    let delivered = message_body(host, session, mid);
    let pending = delivered.is_none();
    let body = delivered.or_else(|| pending_body(host, session, mid))?;
    let root = can_rewind_session(host, session);
    let when = if pending { opencode_i18n::t!("ui.message.pending").to_owned() } else { sent_label(mid, now_ms).unwrap_or_default() };
    if !when.is_empty() {
        menu = menu.group(when).separator();
    }
    if pending {
        menu = menu.item(Item::new("queue", opencode_i18n::t!("ui.message.moveToQueue")).icon(&ic::BULLET_LIST));
    }
    menu = menu.item(Item::new("copy", opencode_i18n::t!("ui.message.copyMessage")).icon(&ic::OUTLINE_COPY).enabled(!body.trim().is_empty()));
    if pending {
        return Some(menu.separator().item(Item::new("delete", opencode_i18n::t!("ui.message.deletePending")).icon(&ic::OUTLINE_TRASH).danger(true)));
    }
    menu = menu.item(Item::new("fork", opencode_i18n::t!("ui.message.forkMessage")).icon(&ic::FORK).enabled(root));
    if !root {
        return Some(menu);
    }
    let latest = visible_user_ids(host, session).last().is_some_and(|l| l == mid);
    let revert = if latest {
        Item::new("revert", opencode_i18n::t!("ui.message.undoMessage")).icon(&ic::OUTLINE_UNDO)
    } else {
        Item::new("revert", opencode_i18n::t!("ui.message.revertMessage")).icon(&ic::RESET)
    };
    Some(menu.separator().item(revert))
}

/// Runs message item `id` ([`message_items`]) on message `mid`.
pub fn run_message_item(host: &mut dyn RewindHost, sid: &str, mid: &str, id: &str, tx: &mut Tx) {
    match id {
        "queue" => host.queue_pending(sid, mid),
        "copy" => {
            if let Some(body) = message_body(host, sid, mid).or_else(|| pending_body(host, sid, mid)).or_else(|| reply_body(host, sid, mid)) {
                tx.copy(body);
            }
        }
        "fork" => host.fork_from_message(sid, mid, tx),
        "revert" => {
            // The menu closes in its frame, but the rows it reverts leave on their own exits, as
            // a revert from anywhere else does.
            tx.animated();
            rewind_to(host, sid, mid, tx);
        }
        "delete" => host.withdraw_pending(sid, mid),
        _ => {}
    }
}

/// A reply's words: its text parts, a blank line apart.
fn reply_body(host: &dyn RewindHost, session: &str, mid: &str) -> Option<String> {
    let t = host.transcript(session)?;
    t.messages.iter().find_map(|m| match m {
        Message::Assistant(a) if a.id == mid => {
            let texts: Vec<&str> = a
                .parts
                .iter()
                .filter_map(|p| match p {
                    model::Part::Text { text } => Some(text.trim()),
                    _ => None,
                })
                .filter(|t| !t.is_empty())
                .collect();
            Some(texts.join("\n\n"))
        }
        _ => None,
    })
}

/// An undelivered steer's words.
fn pending_body(host: &dyn RewindHost, session: &str, mid: &str) -> Option<String> {
    host.inbox(session)?.items.iter().find(|i| i.id == mid).map(|i| i.text.clone())
}

fn message_body(host: &dyn RewindHost, session: &str, mid: &str) -> Option<String> {
    let revert = host.session(session).and_then(|i| i.revert.as_deref());
    let t = host.transcript(session)?;
    let shown = t.visible_len(revert);
    t.messages[..shown].iter().find_map(|m| match m {
        Message::User { id, text } if id == mid => Some(text.clone()),
        _ => None,
    })
}

fn restore_pills(
    id: &str,
    body: &str,
    mention_title: impl Fn(&str) -> Option<String>,
    resolve_skill_id: impl Fn(&str) -> Option<String>,
) -> (String, Vec<(Range<usize>, String)>) {
    enum SpanKind<'b> {
        Recorded(MentionKind),
        Session(&'b str),
    }
    let mentions = mention::user_mentions(id);
    let mut marks: Vec<(Range<usize>, SpanKind<'_>)> = session_view::user_parts::recorded_mentions(body, &mentions)
        .map(|m| (m.range.clone(), SpanKind::Recorded(m.kind)))
        .collect();
    marks.extend(mention::session_tokens(body).into_iter().map(|(r, sid)| (r, SpanKind::Session(sid))));
    marks.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(body.len());
    let mut pills = Vec::new();
    let mut at = 0;
    for (r, kind) in marks {
        if r.start < at {
            continue;
        }
        out.push_str(&body[at..r.start]);
        let start = out.len();
        let raw = &body[r.clone()];
        let pill_id = match kind {
            SpanKind::Session(sid) => {
                match mention_title(sid) {
                    Some(title) => out.push_str(&format!("@{title}")),
                    None => out.push_str(raw),
                }
                format!("session:{sid}")
            }
            SpanKind::Recorded(mk) => {
                out.push_str(raw);
                let val = raw.strip_prefix('@').unwrap_or(raw);
                match mk {
                    MentionKind::Agent => format!("agent:{val}"),
                    MentionKind::Skill => {
                        let sid = resolve_skill_id(val);
                        format!("skill:{}", sid.as_deref().unwrap_or(val))
                    }
                    MentionKind::File | MentionKind::Directory => format!("file:{val}"),
                    MentionKind::Session => format!("session:{val}"),
                }
            }
        };
        pills.push((start..out.len(), pill_id));
        at = r.end;
    }
    out.push_str(&body[at..]);
    (out, pills)
}

/// When a user message was sent, as its menu names it (`message-content.tsx:313-336`): "Today,
/// 6:03 AM" or "Yesterday, …" by the local calendar, then "Nov 7, 6:03 AM" this year, then
/// "Nov 7, 2025, 6:03 AM". `now` is the app's clock (ms).
pub fn sent_label(id: &str, now: i64) -> Option<String> {
    let ms = mention::user_created(id).or_else(|| {
        let hex = id.strip_prefix("msg_")?.get(..12)?;
        u64::from_str_radix(hex, 16).ok().map(|v| (v >> 12) as i64).filter(|&v| v > 1_500_000_000_000)
    })?;
    use opencode_i18n::date::{self, Style};
    let (sent, today) = (session_view::turn_errors::moment(LocalTime::at(ms)), session_view::turn_errors::moment(LocalTime::at(now)));
    let days = today.days() - sent.days();
    // Today and Yesterday as `RelativeTimeFormat` names them, capitalised.
    if let Some(day) = date::relative_day(-days).filter(|_| days >= 0) {
        let mut word = day.chars();
        let first = word.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
        return Some(format!("{first}{}, {}", word.as_str(), date::format(Style::Time, sent)));
    }
    Some(date::format(if sent.year == today.year { Style::MonthDayTime } else { Style::DateTime }, sent))
}
