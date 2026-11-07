//! Rewinding a session to a user message, undoing that rewind from its toast, and `/undo` +
//! `/redo`, matching OpenCode Desktop (`packages/app/src/session/revert.ts`,
//! `packages/session-ui/src/message/message-content.tsx`, `packages/app/src/composer/submit.ts`).
//!
//! Acting updates the timeline and composer on the press: messages from the boundary onward
//! leave on the timeline's growth spring, the rewound prompt returns to the composer, and the
//! `"Rewound"` toast offers `"Undo"`. If the server refuses, the timeline, running state and
//! composer draft roll back and [`crate::status_toast::fail`] reports why.

use std::collections::HashMap;
use std::ops::Range;

use opencode_kit::client::{PathAttachment, Write};
use opencode_kit::mention::{self, MentionKind};
use opencode_kit::model::{self, Attachment, Message, Quote};
use rune::{Align, El, LocalTime, Mark, MarkRange, Tx, icon};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use crate::palette::Pal;
use crate::prompt_history::Entry;
use crate::{App, Route, Source};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedDraft {
    pub entry: Entry,
    pub caret: usize,
    pub files: Vec<Attachment>,
    pub paths: Vec<PathAttachment>,
    pub quotes: Vec<Quote>,
}

impl SavedDraft {
    pub fn is_empty(&self) -> bool {
        self.entry.text.trim().is_empty() && self.files.is_empty() && self.paths.is_empty() && self.quotes.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct UndoRewind {
    pub session: String,
    pub prior_revert: Option<String>,
    pub draft: SavedDraft,
}

#[derive(Clone, Debug)]
struct PendingRevert {
    target: Option<String>,
    prior_revert: Option<String>,
    stopped_running: Vec<String>,
    rollback_draft: Option<SavedDraft>,
    toast_id: Option<u64>,
}

#[derive(Default)]
pub struct RewindState {
    /// The message whose menu is open: `(session_id, message_id)`.
    pub menu: Option<(String, String)>,
    /// Optimistic revert writes waiting on the server, by session id.
    pending: HashMap<String, PendingRevert>,
    /// Per-session composer drafts (including pills, attachments and quotes) across tab switches.
    pub(crate) drafts: HashMap<String, SavedDraft>,
}

impl RewindState {
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

    /// Whether `/undo` and `⌘K → Undo` are enabled on the open session.
    pub fn can_undo_session(&self) -> bool {
        let Route::Session(id) = &self.route else { return false };
        self.can_rewind_session(id) && !self.visible_user_ids(id).is_empty()
    }

    /// Whether `/redo` and `⌘K → Redo` are enabled on the open session.
    pub fn can_redo_session(&self) -> bool {
        let Route::Session(id) = &self.route else { return false };
        self.can_rewind_session(id) && self.session(id).and_then(|i| i.revert.as_ref()).is_some()
    }

    /// Captures `session`'s current composer draft, attachments and quotes.
    pub fn capture_draft(&self, session: &str) -> SavedDraft {
        if self.route == Route::Session(session.to_owned()) {
            return SavedDraft {
                entry: Entry::of(&self.composer),
                caret: self.composer.range().1.min(self.composer.text.len()),
                files: self.attach.files.clone(),
                paths: self.attach.paths.clone(),
                quotes: self.quote.quotes(),
            };
        }
        if let Some(saved) = self.rewind.drafts.get(session) {
            return saved.clone();
        }
        let text = self.restore.drafts.get(session).cloned().unwrap_or_default();
        let caret = text.len();
        SavedDraft { entry: Entry { text, pills: Vec::new() }, caret, ..Default::default() }
    }

    /// Replaces `session`'s composer draft, attachments and quotes (now if open, or for when it opens).
    pub fn apply_draft(&mut self, session: &str, draft: SavedDraft) {
        if draft.entry.text.trim().is_empty() {
            self.restore.drafts.remove(session);
        } else {
            self.restore.drafts.insert(session.to_owned(), draft.entry.text.clone());
        }
        if draft.is_empty() {
            self.rewind.drafts.remove(session);
        } else {
            self.rewind.drafts.insert(session.to_owned(), draft.clone());
        }
        if self.route == Route::Session(session.to_owned()) {
            self.write_composer_draft(draft);
        }
    }

    fn write_composer_draft(&mut self, draft: SavedDraft) {
        let focused = self.composer.focused || (!self.stage.is_some_and(|st| st.watch()) && self.has_composer());
        self.composer.replace_text(draft.entry.text);
        self.composer.set_marks(
            draft
                .entry
                .pills
                .into_iter()
                .map(|(r, id)| MarkRange::new(r, Mark::Pill(id)))
                .collect(),
        );
        let caret = draft.caret.min(self.composer.text.len());
        self.composer.set_caret(caret);
        self.composer.focused = focused;
        self.attach.files = draft.files;
        self.attach.paths = draft.paths;
        let _ = self.quote.take();
        for q in draft.quotes {
            self.quote.push(q);
        }
    }

    /// Preserves per-session drafts created by rewind/fork across tab switches.
    pub(crate) fn sync_route_draft(&mut self, prev: &Route, next_session: &str) {
        if let Route::Session(old) = prev
            && old != next_session
            && (self.rewind.drafts.contains_key(old) || self.rewind.drafts.contains_key(next_session))
        {
            let cur = SavedDraft {
                entry: Entry::of(&self.composer),
                caret: self.composer.range().1.min(self.composer.text.len()),
                files: self.attach.files.clone(),
                paths: self.attach.paths.clone(),
                quotes: self.quote.quotes(),
            };
            if cur.is_empty() {
                self.rewind.drafts.remove(old);
            } else {
                self.rewind.drafts.insert(old.clone(), cur);
            }
            let target = self.rewind.drafts.get(next_session).cloned().unwrap_or_default();
            self.write_composer_draft(target);
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
            quotes: model::user_quotes(id).to_vec(),
        }
    }

    /// Rewinds `session` to user message `message_id` (the message menu's Undo or Revert to here).
    pub fn rewind_to(&mut self, session: &str, message_id: &str, _tx: &mut Tx) {
        self.close_message_menu();
        if !self.can_rewind_session(session) {
            return;
        }
        // An undelivered inbox prompt has no history to rewind: withdraw it into the composer.
        if self.inboxes.get(session).is_some_and(|i| i.items.iter().any(|x| x.id == message_id)) {
            self.undo_queued(session, message_id);
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

        let prior_revert = self.session(session).and_then(|i| i.revert.clone());
        let prior_draft = self.capture_draft(session);
        let restored = self.draft_from_message(message_id, &body);

        let fixture = self.stage.is_some() || matches!(self.source, Source::Fixture);
        if !fixture && !self.write(Write::RevertStage { session: session.to_owned(), message: message_id.to_owned(), settle: true }) {
            let why = self.status.clone();
            crate::status_toast::fail(self, "Request failed", why);
            return;
        }

        let root = self.root_of(session).to_owned();
        let stopped_running: Vec<String> = self.running.iter().filter(|s| self.root_of(s) == root).cloned().collect();
        for sid in &stopped_running {
            self.running.remove(sid);
        }
        if fixture {
            self.playback.retain(|(_, _, d)| d["sessionID"].as_str() != Some(session));
        }
        self.set_session_revert(session, Some(message_id.to_owned()));
        self.exit_shell();
        self.apply_draft(session, restored);

        let toast_id = crate::status_toast::rewound(
            self,
            UndoRewind {
                session: session.to_owned(),
                prior_revert: prior_revert.clone(),
                draft: prior_draft.clone(),
            },
        );
        if !fixture {
            self.rewind.pending.insert(
                session.to_owned(),
                PendingRevert {
                    target: Some(message_id.to_owned()),
                    prior_revert,
                    stopped_running,
                    rollback_draft: Some(prior_draft),
                    toast_id: Some(toast_id),
                },
            );
        }
    }

    /// `/undo` and `⌘K → Undo`: rewinds to the last visible user message before any current boundary.
    pub fn undo_last(&mut self, tx: &mut Tx) -> bool {
        let Route::Session(session) = self.route.clone() else { return false };
        let Some(last) = self.visible_user_ids(&session).pop() else { return false };
        self.rewind_to(&session, &last, tx);
        true
    }

    /// Alias for [`App::undo_last`].
    pub fn undo(&mut self, tx: &mut Tx) -> bool {
        self.undo_last(tx)
    }

    /// `/redo` and `⌘K → Redo`: restores every reverted message at once and leaves the composer alone.
    pub fn redo_revert(&mut self, _tx: &mut Tx) -> bool {
        let Route::Session(session) = self.route.clone() else { return false };
        if !self.can_rewind_session(&session) {
            return false;
        }
        let Some(prior_revert) = self.session(&session).and_then(|i| i.revert.clone()) else {
            return false;
        };
        let fixture = self.stage.is_some() || matches!(self.source, Source::Fixture);
        if !fixture && !self.write(Write::RevertClear(session.clone())) {
            let why = self.status.clone();
            crate::status_toast::fail(self, "Request failed", why);
            return false;
        }
        self.set_session_revert(&session, None);
        if let Some(e) = &self.toast.current
            && matches!(&e.action, Some(crate::status_toast::ToastAction::UndoRewind(u)) if u.session == session)
        {
            crate::status_toast::dismiss(self);
        }
        if !fixture {
            self.rewind.pending.insert(
                session,
                PendingRevert {
                    target: None,
                    prior_revert: Some(prior_revert),
                    stopped_running: Vec::new(),
                    rollback_draft: None,
                    toast_id: None,
                },
            );
        }
        true
    }

    /// Alias for [`App::redo_revert`].
    pub fn redo(&mut self, tx: &mut Tx) -> bool {
        self.redo_revert(tx)
    }

    /// `Undo` on the `"Rewound"` toast: bound to `undo.session` and `undo.prior_revert`, even
    /// after switching tabs, and puts back the unsent draft that was in the composer.
    pub fn undo_rewind(&mut self, undo: UndoRewind, _tx: &mut Tx) {
        let current_revert = self.session(&undo.session).and_then(|i| i.revert.clone());
        let current_draft = self.capture_draft(&undo.session);
        let fixture = self.stage.is_some() || matches!(self.source, Source::Fixture);
        if !fixture {
            let w = match &undo.prior_revert {
                Some(prior) => Write::RevertStage { session: undo.session.clone(), message: prior.clone(), settle: false },
                None => Write::RevertClear(undo.session.clone()),
            };
            if !self.write(w) {
                let why = self.status.clone();
                crate::status_toast::fail(self, "Request failed", why);
                return;
            }
        }
        self.set_session_revert(&undo.session, undo.prior_revert.clone());
        self.apply_draft(&undo.session, undo.draft);
        if !fixture {
            self.rewind.pending.insert(
                undo.session,
                PendingRevert {
                    target: undo.prior_revert,
                    prior_revert: current_revert,
                    stopped_running: Vec::new(),
                    rollback_draft: Some(current_draft),
                    toast_id: None,
                },
            );
        }
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
        self.rewind.pending.remove(session);
        if let Some(inbox) = self.inboxes.get_mut(session) {
            inbox.items.retain(|i| i.id < boundary);
        }
        if let Some(t) = self.transcripts.get_mut(session) {
            let keep = t.visible_len(Some(&boundary));
            for m in &t.messages[keep..] {
                self.glows.remove(m.id());
            }
            t.messages.truncate(keep);
        }
        if let Some(e) = &self.toast.current
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
            match picked {
                Some("queue") => s.queue_pending(&sid, &mid),
                Some("copy") => {
                    if let Some(body) = message_body(s, &sid, &mid).or_else(|| pending_body(s, &sid, &mid)) {
                        tx.copy(body);
                    }
                }
                Some("fork") => {
                    s.fork_from(&sid, &mid, tx);
                }
                Some("revert") => {
                    // The menu closes in its frame, but the rows it reverts leave on their own
                    // exits, as a revert from anywhere else does.
                    tx.animated();
                    s.rewind_to(&sid, &mid, tx);
                }
                Some("delete") => {
                    s.withdraw(&sid, &mid);
                }
                _ => {}
            }
        });
    }

    pub fn close_message_menu(&mut self) {
        self.rewind.menu = None;
    }
}

/// Reconciles `session.revert.*` SSE events with any in-flight optimistic write.
pub fn resolve_event(s: &mut App, ty: &str, data: &Value, sid: &str) -> Option<Option<String>> {
    let incoming = match ty {
        "session.revert.staged" => data["revert"]["messageID"].as_str().map(str::to_owned),
        "session.revert.cleared" | "session.revert.committed" => None,
        _ => return None,
    };
    if let Some(op) = s.rewind.pending.get(sid) {
        if ty == "session.revert.committed" || op.target == incoming {
            s.rewind.pending.remove(sid);
            return Some(incoming);
        }
        // A stale SSE from an earlier write landed while a newer Undo/Redo is in flight.
        return Some(op.target.clone());
    }
    Some(incoming)
}

/// Handles write failures from revert or fork routes: rolls back optimistic state and shows
/// [`crate::status_toast::fail`]. Returns `true` when `error` belonged to revert or fork.
pub fn failed(s: &mut App, error: &str) -> bool {
    let (head, why) = error.split_once(": ").unwrap_or((error, ""));
    let revert_sid = head
        .strip_prefix("POST /api/session/")
        .and_then(|p| {
            p.strip_suffix("/revert/stage")
                .or_else(|| p.strip_suffix("/revert/commit"))
                .or_else(|| p.strip_suffix("/interrupt").filter(|sid| s.rewind.pending.contains_key(*sid)))
        })
        .or_else(|| {
            head.strip_prefix("POST /api/experimental/session/")
                .and_then(|p| p.strip_suffix("/wait"))
                .filter(|sid| s.rewind.pending.contains_key(*sid))
        })
        .or_else(|| head.strip_prefix("DELETE /api/session/").and_then(|p| p.strip_suffix("/revert")));

    if let Some(sid) = revert_sid {
        let sid = sid.to_owned();
        if let Some(op) = s.rewind.pending.remove(&sid) {
            s.set_session_revert(&sid, op.prior_revert);
            for r in op.stopped_running {
                s.running.insert(r);
            }
            if let Some(tid) = op.toast_id {
                crate::status_toast::dismiss_id(s, tid);
            }
            if let Some(draft) = op.rollback_draft {
                s.apply_draft(&sid, draft);
            }
        }
        crate::status_toast::fail(s, "Request failed", why);
        return true;
    }

    if let Some(sid) = head.strip_prefix("POST /api/session/").and_then(|p| p.strip_suffix("/fork")) {
        s.fork.clear_session(sid);
        crate::status_toast::fail(s, "Request failed", why);
        return true;
    }
    false
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
        session_view::timeline::user_menu_button(p, menu_key(id), "More actions", open)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.toggle_message_menu(&sid, &mid, tx))
            .child(icon(&ic::CHEVRON_DOWN, 14.0).color(if open { p.icon } else { p.icon_muted })),
    )
}

/// Message `mid`'s menu, as Desktop builds it (`message-content.tsx:440-498`): when it was sent
/// (or Pending), then Move to queue on a steer, Copy message, Fork from here on a delivered
/// message, and last, after a rule, Delete on a steer or Undo (the newest prompt) or Revert to
/// here where the session can rewind. A subagent's session can't fork or rewind.
fn message_menu(s: &App, session: &str, mid: &str) -> Option<crate::popup::Menu> {
    use crate::popup::{Item, Menu};
    let delivered = message_body(s, session, mid);
    let pending = delivered.is_none();
    let body = delivered.or_else(|| pending_body(s, session, mid))?;
    let root = s.can_rewind_session(session);
    let when = if pending { "Pending".to_owned() } else { sent_label(mid, s.now_ms).unwrap_or_default() };
    let mut menu = Menu::new("More actions").above().align(Align::End).min_width(160.0);
    if !when.is_empty() {
        menu = menu.group(when).separator();
    }
    if pending {
        menu = menu.item(Item::new("queue", "Move to queue").icon(&ic::BULLET_LIST));
    }
    menu = menu.item(Item::new("copy", "Copy message").icon(&ic::OUTLINE_COPY).enabled(!body.trim().is_empty()));
    if pending {
        return Some(menu.separator().item(Item::new("delete", "Delete").icon(&ic::OUTLINE_TRASH).danger(true)));
    }
    menu = menu.item(Item::new("fork", "Fork from here").icon(&ic::FORK).enabled(root));
    if !root {
        return Some(menu);
    }
    let latest = s.visible_user_ids(session).last().is_some_and(|l| l == mid);
    let revert = if latest { Item::new("revert", "Undo").icon(&ic::OUTLINE_UNDO) } else { Item::new("revert", "Revert to here").icon(&ic::RESET) };
    Some(menu.separator().item(revert))
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
    let (sent, today) = (LocalTime::at(ms), LocalTime::at(now));
    let day = |t: &LocalTime| civil_days(t.year as i64, t.month as i64, t.day as i64);
    let time = session_view::turn_errors::short_time(sent);
    Some(match day(&today) - day(&sent) {
        0 => format!("Today, {time}"),
        1 => format!("Yesterday, {time}"),
        _ if sent.year == today.year => format!("{} {}, {time}", MONTHS[sent.month as usize - 1], sent.day),
        _ => session_view::turn_errors::date_time(sent),
    })
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Days since 1970-01-01 of a civil date (Howard Hinnant's `days_from_civil`).
fn civil_days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

#[cfg(test)]
#[path = "rewind_tests.rs"]
mod tests;
