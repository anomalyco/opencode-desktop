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
use rune::{
    Align, Color, El, Icon, Key, KeyEvent, Mark, MarkRange, Side, Spring, Transition, Tx, col, el,
    icon, row, text,
};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use crate::palette::Pal;
use crate::prompt_history::Entry;
use crate::view::{INSTANT, UI, UI_SMALL};
use crate::{App, Route, Source};

/// `@opencode/ui`'s default `reset` icon (`additional-icons.ts`).
pub static RESET: Icon = Icon {
    name: "reset",
    view: 20.0,
    body: r#"<path d="M5.83333 4.16406L2.5 7.4974L5.83333 10.8307M3.33333 7.4974H17.9167V15.4141H10" stroke="currentColor" stroke-linecap="square"/>"#,
};

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
    /// Open `⋯` message menu: `(session_id, message_id)` and highlighted item index.
    pub menu: Option<(String, String)>,
    pub menu_active: Option<usize>,
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

    /// Rewinds `session` to user message `message_id` (`Rewind to here`).
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

    pub fn toggle_message_menu(&mut self, session: &str, message_id: &str) {
        if self.rewind.menu_is(session, message_id) {
            self.close_message_menu();
        } else {
            self.rewind.menu = Some((session.to_owned(), message_id.to_owned()));
            self.rewind.menu_active = None;
        }
    }

    pub fn close_message_menu(&mut self) {
        self.rewind.menu = None;
        self.rewind.menu_active = None;
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

/// Hover actions for a delivered user message (`user_bubble`): `Rewind to here` (on root sessions)
/// and `More actions` (`⋯`), plus the pinned horizontal offset when placed beside a folded run row.
pub fn user_actions(s: &App, p: &Pal, id: &str, _body: &str) -> (El, f32) {
    let session = match &s.route {
        Route::Session(sid) => sid.as_str(),
        _ => "",
    };
    let can_revert = !session.is_empty() && s.can_rewind_session(session);
    let menu_open = s.rewind.menu_is(session, id);
    let mut actions = row().key("user-actions").gap(6.0).center_y();
    if can_revert {
        let (sid, mid) = (session.to_owned(), id.to_owned());
        actions = actions.child(
            row()
                .key(format!("rewind-{id}"))
                .role("button")
                .label("Rewind to here")
                .help("Rewind to here")
                .size(20.0, 20.0)
                .center()
                .radius(4.0)
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.rewind_to(&sid, &mid, tx))
                .child(icon(&RESET, 14.0).color(p.icon_muted)),
        );
    }
    let (sid, mid) = (session.to_owned(), id.to_owned());
    let mut more = row()
        .key(format!("more-{id}"))
        .role("button")
        .label("More actions")
        .help("More actions")
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            s.toggle_message_menu(&sid, &mid);
            crate::popover::motion::toggled(&s.knobs, tx, s.rewind.menu.is_some());
        })
        .child(icon(&ic::OUTLINE_DOTS, 14.0).color(p.icon_muted));
    if menu_open {
        more = more.fill(p.hover);
    }
    actions = actions.child(more);
    let actions = if menu_open {
        actions.opacity(1.0).opacity_motion(INSTANT)
    } else {
        actions.group_opacity(0.0, 1.0)
    };
    let pin_x = if can_revert { -50.0 } else { -24.0 };
    (actions, pin_x)
}

/// Keys while the `⋯` message menu is open.
pub fn menu_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some((session, mid)) = s.rewind.menu.clone() else { return false };
    let Some(body) = message_body(s, &session, &mid) else {
        s.close_message_menu();
        return false;
    };
    let can_fork = s.can_rewind_session(&session);
    let has_copy = !body.trim().is_empty();
    let enabled: Vec<usize> = [can_fork.then_some(0), has_copy.then_some(1)].into_iter().flatten().collect();
    match ev.key {
        Key::Escape => {
            tx.instant();
            s.close_message_menu();
            true
        }
        Key::Down | Key::Up => {
            tx.instant();
            if !enabled.is_empty() {
                let d: isize = if ev.key == Key::Down { 1 } else { -1 };
                let next = match s.rewind.menu_active.and_then(|a| enabled.iter().position(|&x| x == a)) {
                    Some(pos) => enabled[(pos as isize + d).rem_euclid(enabled.len() as isize) as usize],
                    None => if d > 0 { enabled[0] } else { *enabled.last().unwrap() },
                };
                s.rewind.menu_active = Some(next);
            }
            true
        }
        Key::Enter => {
            tx.instant();
            match s.rewind.menu_active {
                Some(0) if can_fork => {
                    s.close_message_menu();
                    s.fork_from(&session, &mid, tx);
                }
                Some(1) if has_copy => {
                    tx.copy(body);
                    s.close_message_menu();
                }
                _ => {}
            }
            true
        }
        _ => true,
    }
}

/// The `⋯` ("More actions") menu anchored below a user message's `more-<id>` button.
pub fn menu_view(s: &App, p: &Pal) -> Option<El> {
    let (session, mid) = s.rewind.menu.as_ref()?;
    let body = message_body(s, session, mid)?;
    let can_fork = s.can_rewind_session(session);
    let has_copy = !body.trim().is_empty();
    let active = s.rewind.menu_active;

    let mut menu = col()
        .key("message-menu")
        .role("menu")
        .label("More actions")
        .anchored(format!("more-{mid}"), Side::Bottom, Align::End, 4.0)
        .w(188.0)
        .padding(4.0)
        .gap(1.0)
        .radius(8.0)
        .fill(p.layer1)
        .border(0.5, p.border_muted)
        .shadow(4.0, 12.0, p.shadow)
        .transition(crate::popover::motion::transition(&s.knobs, Side::Bottom, Align::End))
        .motion(Spring::NONE)
        // What it holds comes in with it, not on fades of its own.
        .instant_changes()
        .on_press(|_: &mut App| {});

    if let Some(stamp) = sent_label(mid) {
        menu = menu
            .child(row().key("message-menu-time").w_full().h(24.0).px(8.0).center_y().child(text(stamp).font(UI_SMALL).color(p.faint)))
            .child(el().key("message-menu-sep").w_full().h(0.5).my(2.0).fill(p.border_muted));
    }

    let (fork_sid, fork_mid) = (session.clone(), mid.clone());
    let fork_item = row()
        .key("message-fork")
        .role("menuitem")
        .label("Fork from here")
        .w_full()
        .h(28.0)
        .px(8.0)
        .center_y()
        .radius(5.0)
        .fill(if active == Some(0) && can_fork { p.hover } else { Color::TRANSPARENT })
        .hover(if can_fork { p.hover } else { Color::TRANSPARENT })
        .press(if can_fork { p.pressed } else { Color::TRANSPARENT })
        .hover_motion(INSTANT, INSTANT)
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside && can_fork {
                tx.instant();
                s.rewind.menu_active = Some(0);
            }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if !can_fork {
                return;
            }
            tx.instant();
            s.close_message_menu();
            s.fork_from(&fork_sid, &fork_mid, tx);
        })
        .child(text("Fork from here").font(UI).color(if can_fork { p.text } else { p.faint }));

    let copy_words = body;
    let copy_item = row()
        .key("message-copy")
        .role("menuitem")
        .label("Copy message")
        .w_full()
        .h(28.0)
        .px(8.0)
        .center_y()
        .radius(5.0)
        .fill(if active == Some(1) && has_copy { p.hover } else { Color::TRANSPARENT })
        .hover(if has_copy { p.hover } else { Color::TRANSPARENT })
        .press(if has_copy { p.pressed } else { Color::TRANSPARENT })
        .hover_motion(INSTANT, INSTANT)
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside && has_copy {
                tx.instant();
                s.rewind.menu_active = Some(1);
            }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if !has_copy {
                return;
            }
            tx.instant();
            tx.copy(copy_words.clone());
            s.close_message_menu();
        })
        .child(text("Copy message").font(UI).color(if has_copy { p.text } else { p.faint }));

    menu = menu.child(fork_item).child(copy_item);

    Some(
        el().key("message-menu-dismiss")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .transition(Transition::none())
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.close_message_menu();
            })
            .child(menu)
            .raise(),
    )
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

/// Formats a user message's creation timestamp (`Oct 23, 2026, 18:38`) when known.
pub(crate) fn sent_label(id: &str) -> Option<String> {
    let ms = mention::user_created(id).or_else(|| {
        let hex = id.strip_prefix("msg_")?.get(..12)?;
        u64::from_str_radix(hex, 16).ok().map(|v| (v >> 12) as i64).filter(|&v| v > 1_500_000_000_000)
    })?;
    Some(format_epoch_ms(ms))
}

pub(crate) fn format_epoch_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    // Howard Hinnant's civil_from_days (UTC).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mon = MONTHS[(m as usize).saturating_sub(1).min(11)];
    format!("{mon} {d}, {year}, {hour:02}:{min:02}")
}

#[cfg(test)]
#[path = "rewind_tests.rs"]
mod tests;
