//! Rewinding a session to a user message, undoing that rewind from its toast, and `/undo` +
//! `/redo`, matching OpenCode Desktop (`packages/app/src/session/revert.ts`,
//! `packages/session-ui/src/message/message-content.tsx`, `packages/app/src/composer/submit.ts`).
//! Each session's operations are `machine::SessionRevert` (`session/revert-machine.ts`).
//!
//! Acting updates the timeline and composer on the press: messages from the boundary onward
//! leave on the timeline's growth spring, the rewound prompt returns to the composer, and the
//! `"Reverted"` toast offers `"Undo"`. If the server refuses, or doesn't answer in 30 s, the
//! timeline, running state and composer draft roll back and [`crate::status_toast::fail`] reports
//! why. An undelivered prompt is withdrawn instead, its words back in the composer once the
//! server lets it go.

use std::collections::HashMap;
use std::ops::Range;

use opencode_kit::client::{PathAttachment, Ticket, TicketKind, Write};
use opencode_kit::mention::{self, MentionKind};
use opencode_kit::model::{self, Attachment, Message, PromptComment};
use rune::machine::Attempt;
use rune::{Align, El, LocalTime, Tx, icon};
use rune_theme_opencode::icons as ic;

use crate::palette::Pal;
use crate::prompt_history::Entry;
use crate::{App, Route, Source};

mod machine;
use machine::{Effect, Event, Operation, Rollback, SessionRevert, State};

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
#[derive(Clone, Debug)]
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
    pub(crate) fn busy(&self, session: &str) -> bool {
        self.sessions.get(session).is_some_and(SessionRevert::running)
    }

    pub fn menu_is(&self, session: &str, message_id: &str) -> bool {
        self.menu.as_ref().is_some_and(|(s, m)| s == session && m == message_id)
    }

    pub fn menu_open_on(&self, message_id: &str) -> bool {
        self.menu.as_ref().is_some_and(|(_, m)| m == message_id)
    }
}

impl App {
    /// Whether `session` is a root session (subagent sessions hide rewind and fork).
    pub fn can_rewind_session(&self, session: &str) -> bool {
        self.session(session).is_none_or(|i| i.parent.is_none())
    }

    /// Visible user messages (`id < revert`) in `session`, in chronological order.
    pub fn visible_user_ids(&self, session: &str) -> Vec<String> {
        let revert = self.session(session).and_then(|i| i.revert.as_deref());
        let Some(t) = self.transcripts.get(session) else { return Vec::new() };
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
    /// before the session's boundary, an undelivered one in the inbox included (which Undo
    /// withdraws).
    fn undo_target(&self, session: &str) -> Option<String> {
        let boundary = self.session(session).and_then(|i| i.revert.as_deref());
        let sent = self.transcripts.get(session).into_iter().flat_map(|t| &t.messages).filter_map(|m| match m {
            Message::User { id, .. } => Some(id.as_str()),
            _ => None,
        });
        let waiting = self.inboxes.get(session).into_iter().flat_map(|i| i.items.iter().map(|i| i.id.as_str()));
        sent.chain(waiting).filter(|id| boundary.is_none_or(|b| *id < b)).max().map(str::to_owned)
    }

    /// Whether `/undo` and `⌘K → Undo` are enabled on the open session.
    pub fn can_undo_session(&self) -> bool {
        let Route::Session { id, .. } = &*self.route else { return false };
        self.can_rewind_session(id) && !self.rewind.busy(id) && self.undo_target(id).is_some()
    }

    /// Whether `/redo` and `⌘K → Redo` are enabled on the open session.
    pub fn can_redo_session(&self) -> bool {
        let Route::Session { id, .. } = &*self.route else { return false };
        self.can_rewind_session(id) && !self.rewind.busy(id) && self.session(id).and_then(|i| i.revert.as_ref()).is_some()
    }

    /// Captures `session`'s current composer draft, attachments and quotes.
    pub fn capture_draft(&self, session: &str) -> SavedDraft {
        if self.composer_holds(session) {
            return SavedDraft {
                entry: Entry::of(&self.composer),
                caret: self.composer.range().1.min(self.composer.text.len()),
                files: self.attach.files.clone(),
                paths: self.attach.paths.clone(),
                quotes: self.quote.quotes(),
            };
        }
        let text = self.restore.drafts.get(session).cloned().unwrap_or_default();
        let parked = self.restore.parked.get(session).cloned().unwrap_or_default();
        let caret = parked.caret.filter(|&c| text.is_char_boundary(c)).unwrap_or(text.len());
        SavedDraft { entry: Entry { text, pills: parked.pills }, caret, files: parked.files, paths: parked.paths, quotes: parked.quotes }
    }

    /// Replaces `session`'s composer draft, attachments and quotes (now if open, or for when it opens).
    pub fn apply_draft(&mut self, session: &str, draft: SavedDraft) {
        if self.composer_holds(session) {
            return self.write_composer_draft(draft);
        }
        let parked = crate::restore::Parked::of(&draft);
        self.park_draft(session.to_owned(), draft.entry.text, parked);
    }

    fn write_composer_draft(&mut self, draft: SavedDraft) {
        let focused = self.composer.focused || (!self.stage.is_some_and(|st| st.watch()) && self.has_composer());
        draft.entry.put(&mut self.composer, draft.caret.min(draft.entry.text.len()));
        self.composer.focused = focused;
        self.attach.files = draft.files;
        self.attach.paths = draft.paths;
        let _ = self.quote.take();
        for q in draft.quotes {
            self.quote.push(q);
        }
    }

    /// Builds the composer draft to restore from user message `(id, body)`.
    pub fn draft_from_message(&self, id: &str, body: &str) -> SavedDraft {
        let paths = mention::user_path_attachments(id).to_vec();
        let mut clean = body;
        for p in paths.iter().rev() {
            let line = format!("Attached file: `{}`", p.path);
            if let Some(head) = clean.strip_suffix(&line) {
                clean = head.strip_suffix('\n').unwrap_or(head);
            }
        }
        let (shown, pills) = restore_pills(self, id, clean);
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
    pub fn rewind_to(&mut self, session: &str, message_id: &str, _tx: &mut Tx) {
        self.close_message_menu();
        if !self.can_rewind_session(session) || self.rewind.busy(session) {
            return;
        }
        // An undelivered inbox prompt has no history to rewind: withdraw it into the composer.
        if let Some(item) = self.inboxes.get(session).and_then(|i| i.items.iter().find(|x| x.id == message_id)).cloned() {
            let mut draft = self.draft_from_message(&item.id, &item.text);
            if draft.files.is_empty() {
                draft.files = item.files;
            }
            let operation = Operation::Withdraw { message: message_id.to_owned(), draft };
            self.step_revert(session, Event::Requested { operation, now: *self.clock });
            return;
        }
        let Some(body) = self.transcripts.get(session).and_then(|t| {
            t.messages.iter().find_map(|m| match m {
                Message::User { id, text } if id == message_id => Some(text.clone()),
                _ => None,
            })
        }) else {
            return;
        };

        let prior = self.session(session).and_then(|i| i.revert.clone());
        let prior_draft = self.capture_draft(session);
        let restored = self.draft_from_message(message_id, &body);
        let root = self.root_of(session).to_owned();
        let stopped: Vec<String> = self.running.iter().filter(|s| self.root_of(s) == root).cloned().collect();
        let attempt = self.rewind.sessions.entry(session.to_owned()).or_default().next_attempt();

        // Shown on the press: the rows leave, the prompt comes back, the toast offers Undo.
        for sid in &stopped {
            self.running.remove(sid);
        }
        if self.stage.is_some() || matches!(*self.source, Source::Fixture) {
            self.playback.retain(|(_, _, d)| d["sessionID"].as_str() != Some(session));
        }
        self.set_session_revert(session, Some(message_id.to_owned()));
        self.exit_shell();
        self.apply_draft(session, restored.clone());
        let toast = crate::status_toast::rewound(self, UndoRewind { session: session.to_owned(), attempt });

        let rollback = Rollback { revert: prior.clone(), stopped, draft: Some(prior_draft.clone()), toast: Some(toast) };
        let operation = Operation::Rewind { message: message_id.to_owned(), prior, draft: prior_draft, restored, rollback };
        self.step_revert(session, Event::Requested { operation, now: *self.clock });
    }

    /// `/undo` and `⌘K → Undo`: rewinds to the newest user message before the session's boundary,
    /// or withdraws it when it's still undelivered (`revert-machine.ts` `undo.requested`).
    pub fn undo_last(&mut self, tx: &mut Tx) -> bool {
        let Route::Session { id: session, .. } = (*self.route).clone() else { return false };
        if self.rewind.busy(&session) {
            return false;
        }
        let Some(last) = self.undo_target(&session) else { return false };
        self.rewind_to(&session, &last, tx);
        true
    }

    /// Alias for [`App::undo_last`].
    pub fn undo(&mut self, tx: &mut Tx) -> bool {
        self.undo_last(tx)
    }

    /// `/redo` and `⌘K → Redo`: restores every reverted message at once and leaves the composer alone.
    pub fn redo_revert(&mut self, _tx: &mut Tx) -> bool {
        let Route::Session { id: session, .. } = (*self.route).clone() else { return false };
        if !self.can_rewind_session(&session) || self.rewind.busy(&session) {
            return false;
        }
        let Some(prior) = self.session(&session).and_then(|i| i.revert.clone()) else {
            return false;
        };
        self.set_session_revert(&session, None);
        let rollback = Rollback { revert: Some(prior), ..Rollback::default() };
        self.step_revert(&session, Event::Requested { operation: Operation::Redo { rollback }, now: *self.clock });
        true
    }

    /// Alias for [`App::redo_revert`].
    pub fn redo(&mut self, tx: &mut Tx) -> bool {
        self.redo_revert(tx)
    }

    /// `Undo` on the `"Reverted"` toast: bound to its session and rewind, even after switching
    /// tabs, and puts back the unsent draft that was in the composer. An older toast's Undo, or
    /// one whose rewind another boundary replaced, does nothing.
    pub fn undo_rewind(&mut self, undo: UndoRewind, _tx: &mut Tx) {
        let (attempt, prior) = match self.rewind.sessions.get(&undo.session).map(|r| r.state.clone()) {
            Some(State::Restored { attempt, prior, .. }) if attempt == undo.attempt => (attempt, prior),
            Some(State::Running { attempt, operation: Operation::Rewind { prior, .. }, .. }) if attempt == undo.attempt => (attempt, prior),
            _ => return,
        };
        let rollback = Rollback { revert: self.session(&undo.session).and_then(|i| i.revert.clone()), draft: Some(self.capture_draft(&undo.session)), ..Rollback::default() };
        self.set_session_revert(&undo.session, prior);
        self.step_revert(&undo.session, Event::ToastUndo { attempt, now: *self.clock, rollback });
    }

    /// Steps `session`'s revert machine and carries out what it says.
    fn step_revert(&mut self, session: &str, event: Event) {
        let effects = self.rewind.sessions.entry(session.to_owned()).or_default().step(event);
        for effect in effects {
            self.revert_effect(session, effect);
        }
        crate::windows::peer::touch(self);
    }

    fn revert_effect(&mut self, session: &str, effect: Effect) {
        match effect {
            Effect::Run { attempt, operation } => {
                let write = match &operation {
                    Operation::Withdraw { message, .. } => Write::Cancel { session: session.to_owned(), item: message.clone() },
                    Operation::Rewind { message, .. } => Write::RevertStage { session: session.to_owned(), message: message.clone(), settle: true },
                    Operation::Redo { .. } | Operation::UndoRewind { prior: None, .. } => Write::RevertClear(session.to_owned()),
                    Operation::UndoRewind { prior: Some(prior), .. } => Write::RevertStage { session: session.to_owned(), message: prior.clone(), settle: false },
                };
                // The fixture and the storybook act as if the server agreed at once.
                if self.stage.is_some() || matches!(*self.source, Source::Fixture) {
                    return self.step_revert(session, Event::Succeeded { attempt });
                }
                let ticket = Ticket { kind: TicketKind::Revert, session: session.to_owned(), attempt: attempt.0 };
                if !self.write(Write::Tracked { ticket, write: Box::new(write) }) {
                    let (why, now) = ((*self.status).clone(), *self.clock);
                    self.step_revert(session, Event::Failed { attempt, why, now });
                }
            }
            Effect::Undo { rollback, why } => {
                if let Some(rollback) = rollback {
                    self.set_session_revert(session, rollback.revert);
                    for r in rollback.stopped {
                        self.running.insert(r);
                    }
                    if let Some(id) = rollback.toast {
                        crate::status_toast::dismiss_id(self, id);
                    }
                    if let Some(draft) = rollback.draft {
                        self.apply_draft(session, draft);
                    }
                }
                let why = why.split_once(": ").map_or(why.as_str(), |(_, why)| why).to_owned();
                crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), why);
            }
            // `revert.ts` `restore`: the withdrawn prompt replaces the draft.
            Effect::RestoreMessage { message, draft } => {
                if let Some(inbox) = self.inboxes.get_mut(session) {
                    inbox.remove(&message);
                }
                model::set_user_files(&message, Vec::new());
                self.exit_shell();
                self.apply_draft(session, draft);
            }
            Effect::RestoreDraft { draft } => self.apply_draft(session, draft),
            // The server holds the boundary of a rewind that was rolled back: it shows, and its
            // prompt comes back unless something was typed over the rolled-back draft since.
            Effect::Land { attempt, boundary, rolled_back, restored } => {
                self.set_session_revert(session, Some(boundary));
                if self.capture_draft(session).same_words(&rolled_back) {
                    self.exit_shell();
                    self.apply_draft(session, restored);
                }
                crate::status_toast::rewound(self, UndoRewind { session: session.to_owned(), attempt });
            }
            Effect::DismissToast => {
                if let Some(e) = self.toast.current()
                    && matches!(&e.action, Some(crate::status_toast::ToastAction::UndoRewind(u)) if u.session == session)
                {
                    crate::status_toast::dismiss(self);
                }
            }
        }
    }

    /// `Incoming::Settled` for a revert ticket.
    pub(crate) fn revert_settled(&mut self, session: &str, attempt: u32, result: Result<(), opencode_kit::wire_error::WireError>) {
        let attempt = Attempt(attempt);
        let event = match result {
            Ok(()) => Event::Succeeded { attempt },
            Err(e) => Event::Failed { attempt, why: crate::errors::said(self, "", &e), now: *self.clock },
        };
        self.step_revert(session, event);
    }

    /// Each frame: an operation past its 30 s bound fails as it got. True when one did.
    pub(crate) fn revert_timeouts(&mut self, tx: &mut Tx) -> bool {
        let now = *self.clock;
        let due: Vec<String> = self.rewind.sessions.iter().filter(|(_, r)| r.due().is_some_and(|d| now + 1e-9 >= d)).map(|(id, _)| id.clone()).collect();
        for session in &due {
            self.step_revert(session, Event::Due { now: now });
        }
        if let Some(next) = self.rewind.sessions.values().filter_map(SessionRevert::due).reduce(f64::min) {
            tx.wake_after(std::time::Duration::from_secs_f64((next - now).max(0.0)));
        }
        !due.is_empty()
    }

    pub(crate) fn set_session_revert(&mut self, session: &str, revert: Option<String>) {
        if let Some(info) = self.sessions.iter_mut().find(|i| i.id == session) {
            info.revert = revert;
        }
    }

    /// Commits a staged revert locally right before sending a new prompt (`composer/submit.ts`),
    /// so the reverted tail is dropped and the new prompt isn't hidden behind the old boundary.
    pub(crate) fn commit_staged_revert(&mut self, session: &str) -> bool {
        let Some(boundary) = self.session(session).and_then(|i| i.revert.clone()) else {
            return false;
        };
        self.set_session_revert(session, None);
        self.step_revert(session, Event::Committed);
        if let Some(inbox) = self.inboxes.get_mut(session) {
            inbox.items.retain(|i| i.id < boundary);
        }
        if let Some(t) = self.transcripts.get_mut(session) {
            let keep = t.visible_len(Some(&boundary));
            let gone: Vec<String> = t.messages[keep..].iter().map(|m| m.id().to_owned()).collect();
            self.glows.step(session_view::arrivals::Event::Gone(&gone));
            t.messages.truncate(keep);
        }
        if let Some(e) = self.toast.current()
            && matches!(&e.action, Some(crate::status_toast::ToastAction::UndoRewind(u)) if u.session == session)
        {
            crate::status_toast::dismiss(self);
        }
        true
    }

    /// Opens message `message_id`'s menu under its corner chevron, as Desktop's `Menu` opens from
    /// its trigger (`message-content.tsx:426-503`), or closes it when it's the one open.
    pub fn toggle_message_menu(&mut self, session: &str, message_id: &str, tx: &mut Tx) {
        if self.rewind.menu_is(session, message_id) && self.popup.open_at(&menu_key(message_id)) {
            tx.instant();
            crate::popup::dismiss(self, tx);
            return;
        }
        let Some(menu) = message_menu(self, session, message_id) else { return };
        self.rewind.menu = Some((session.to_owned(), message_id.to_owned()));
        let (sid, mid) = (session.to_owned(), message_id.to_owned());
        crate::popup::open(self, menu, menu_key(message_id), tx, move |s: &mut App, picked, tx| {
            s.close_message_menu();
            if let Some(id) = picked {
                run_message_item(s, &sid, &mid, id, tx);
            }
        });
    }

    pub fn close_message_menu(&mut self) {
        self.rewind.menu = None;
    }
}

/// What a `session.revert.*` event says of the session's boundary: `Some(Some(id))` staged at
/// `id`, `Some(None)` cleared or committed.
pub fn reported(ty: &str, data: &serde_json::Value) -> Option<Option<String>> {
    match ty {
        "session.revert.staged" => Some(data["revert"]["messageID"].as_str().map(str::to_owned)),
        "session.revert.cleared" | "session.revert.committed" => Some(None),
        _ => None,
    }
}

/// A `session.revert.*` event in this window: its machine takes the echo of its own request, or
/// a change from elsewhere. While its later requests are still out, the boundary it shows is
/// the last one's, which a stale echo mustn't overwrite: that boundary is returned.
pub fn landed(s: &mut App, ty: &str, data: &serde_json::Value, sid: &str) -> Option<Option<String>> {
    let reported = reported(ty, data)?;
    if !s.rewind.sessions.contains_key(sid) {
        return None;
    }
    let event = if ty == "session.revert.committed" { Event::Committed } else { Event::Reported(reported) };
    s.step_revert(sid, event);
    s.rewind.sessions.get(sid)?.expected().cloned()
}

/// A fresh read of the sessions (a reconnect's): a rolled-back rewind whose boundary the server
/// now holds landed after all.
pub(crate) fn read_sessions(s: &mut App) {
    let doubtful: Vec<(String, Option<String>)> = s.rewind.sessions.iter().filter(|(_, r)| r.doubtful()).map(|(id, _)| (id.clone(), s.session(id).and_then(|i| i.revert.clone()))).collect();
    for (sid, revert) in doubtful {
        s.step_revert(&sid, Event::Read(revert));
    }
}

/// `sid`'s inbox changed: a withdrawal whose cancel failed, whose prompt left the inbox without
/// reaching the transcript, was cancelled after all.
pub(crate) fn inbox_changed(s: &mut App, sid: &str) {
    let Some(machine::State::Doubtful { doubt: machine::Doubt::Withdraw { message, .. }, .. }) = s.rewind.sessions.get(sid).map(|r| &r.state) else { return };
    let message = message.clone();
    let queued = s.inboxes.get(sid).is_some_and(|i| i.items.iter().any(|i| i.id == message));
    let delivered = s.transcripts.get(sid).is_some_and(|t| t.messages.iter().any(|m| m.id() == message));
    if !queued && !delivered {
        s.step_revert(sid, Event::Withdrawn { message });
    }
}

/// The key of message `id`'s corner chevron, which its menu opens from.
fn menu_key(id: &str) -> String {
    format!("user-menu-{id}")
}

/// The chevron in a user message's top trailing corner (`user-message-menu`): `chevron-down` at
/// 14 pt, icon-muted, or icon-base on the pressed fill while its menu is open (`icon-button.css`'s
/// ghost-muted).
pub fn user_menu(s: &App, p: &Pal, session: &str, id: &str, pending: bool) -> Option<El> {
    if session.is_empty() || (pending && !s.can_rewind_session(session)) {
        return None;
    }
    let open = s.rewind.menu_is(session, id);
    let (sid, mid) = (session.to_owned(), id.to_owned());
    Some(
        session_view::timeline::user_menu_button(p, menu_key(id), opencode_i18n::t!("ui.message.moreActions"), open)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.toggle_message_menu(&sid, &mid, tx))
            .child(icon(&ic::CHEVRON_DOWN, 14.0).color(if open { p.icon } else { p.icon_muted })),
    )
}

/// Message `mid`'s menu under its chevron: [`message_items`] in a menu of their own.
fn message_menu(s: &App, session: &str, mid: &str) -> Option<crate::popup::Menu> {
    let menu = crate::popup::Menu::new(opencode_i18n::t!("ui.message.moreActions")).above().align(Align::End).min_width(160.0);
    message_items(s, session, mid, menu)
}

/// Message `mid`'s items after `menu`'s, as Desktop builds them (`message-content.tsx:440-498`):
/// when it was sent (or Pending), then Move to queue on a steer, Copy message, Fork from here
/// on a delivered message, and last, after a rule, Delete on a steer or Undo (the newest prompt)
/// or Revert to here where the session can rewind. A subagent's session can't fork or rewind. A
/// reply has when it was written and Copy message. The chevron's menu and a right-click's
/// ([`open_context_menu`]) both take them from here.
pub(crate) fn message_items(s: &App, session: &str, mid: &str, mut menu: crate::popup::Menu) -> Option<crate::popup::Menu> {
    use crate::popup::Item;
    if let Some(reply) = reply_body(s, session, mid) {
        if let Some(when) = sent_label(mid, *s.now_ms) {
            menu = menu.group(when).separator();
        }
        return Some(menu.item(Item::new("copy", opencode_i18n::t!("ui.message.copyMessage")).icon(&ic::OUTLINE_COPY).enabled(!reply.trim().is_empty())));
    }
    let delivered = message_body(s, session, mid);
    let pending = delivered.is_none();
    let body = delivered.or_else(|| pending_body(s, session, mid))?;
    let root = s.can_rewind_session(session);
    let when = if pending { opencode_i18n::t!("ui.message.pending").to_owned() } else { sent_label(mid, *s.now_ms).unwrap_or_default() };
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
    let latest = s.visible_user_ids(session).last().is_some_and(|l| l == mid);
    let revert = if latest { Item::new("revert", opencode_i18n::t!("ui.message.undoMessage")).icon(&ic::OUTLINE_UNDO) } else { Item::new("revert", opencode_i18n::t!("ui.message.revertMessage")).icon(&ic::RESET) };
    Some(menu.separator().item(revert))
}

/// Runs message item `id` ([`message_items`]) on message `mid`.
pub(crate) fn run_message_item(s: &mut App, sid: &str, mid: &str, id: &str, tx: &mut Tx) {
    match id {
        "queue" => s.queue_pending(sid, mid),
        "copy" => {
            if let Some(body) = message_body(s, sid, mid).or_else(|| pending_body(s, sid, mid)).or_else(|| reply_body(s, sid, mid)) {
                tx.copy(body);
            }
        }
        "fork" => {
            s.fork_from(sid, mid, tx);
        }
        "revert" => {
            // The menu closes in its frame, but the rows it reverts leave on their own exits, as
            // a revert from anywhere else does.
            tx.animated();
            s.rewind_to(sid, mid, tx);
        }
        "delete" => {
            s.withdraw(sid, mid);
        }
        _ => {}
    }
}

/// A reply's words: its text parts, a blank line apart.
fn reply_body(s: &App, session: &str, mid: &str) -> Option<String> {
    let t = s.transcripts.get(session)?;
    t.messages.iter().find_map(|m| match m {
        Message::Assistant(a) if a.id == mid => {
            let texts: Vec<&str> = a.parts.iter().filter_map(|p| match p {
                model::Part::Text { text } => Some(text.trim()),
                _ => None,
            }).filter(|t| !t.is_empty()).collect();
            Some(texts.join("\n\n"))
        }
        _ => None,
    })
}

/// An undelivered steer's words.
fn pending_body(s: &App, session: &str, mid: &str) -> Option<String> {
    s.inboxes.get(session)?.items.iter().find(|i| i.id == mid).map(|i| i.text.clone())
}

fn message_body(s: &App, session: &str, mid: &str) -> Option<String> {
    let revert = s.session(session).and_then(|i| i.revert.as_deref());
    let t = s.transcripts.get(session)?;
    let shown = t.visible_len(revert);
    t.messages[..shown].iter().find_map(|m| match m {
        Message::User { id, text } if id == mid => Some(text.clone()),
        _ => None,
    })
}

fn restore_pills(s: &App, id: &str, body: &str) -> (String, Vec<(Range<usize>, String)>) {
    enum SpanKind<'a> {
        Recorded(MentionKind),
        Session(&'a str),
    }
    let mentions = mention::user_mentions(id);
    let mut marks: Vec<(Range<usize>, SpanKind<'_>)> =
        session_view::user_parts::recorded_mentions(body, &mentions).map(|m| (m.range.clone(), SpanKind::Recorded(m.kind))).collect();
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
                match s.mention_title(sid) {
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
                        let sid = s.mentions.skills.iter().find(|sk| sk.id == val || sk.name == val).map_or(val, |sk| sk.id.as_str());
                        format!("skill:{sid}")
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
pub(crate) fn sent_label(id: &str, now: i64) -> Option<String> {
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

