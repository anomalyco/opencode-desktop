//! Prompts sent while a session is busy, as OpenCode Desktop shows them. Enter steers: the
//! prompt shows at the end of the timeline, muted and marked Pending, until the server delivers
//! it at the turn's next step boundary. ⌘⏎ queues: the prompt waits in a panel above the
//! composer until the turn ends, where it can be steered (Send once the turn has stopped),
//! taken back into the composer, or removed.

use rune::{El, Icon, Key, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use opencode_kit::client::{Prompt, Write};
use opencode_kit::model::{self, Delivery, Inbox, InboxChange, InboxItem, Message};
use rune::machine::Attempt;

pub mod drag;
mod edit;
pub mod machine;
pub mod run;
use machine::{Effect, Event, Operation, Row as QueueRow, Stage};

use crate::odometer::odometer;
use crate::palette::Pal;
use crate::view::{UI, UI_MED};
use crate::{App, Source};

const UNDO: Icon = Icon {
    name: "outline-undo",
    view: 16.0,
    body: r#"<path d="M2 10L4.25193 7.76999C7.29213 4.7594 12.4119 5.77372 14.0747 9.71606" stroke="currentColor"/><path d="M2 6V10H6" stroke="currentColor" stroke-linecap="square"/>"#,
};
const XMARK: Icon = Icon {
    name: "outline-xmark",
    view: 16.0,
    body: r#"<path fill-rule="evenodd" clip-rule="evenodd" d="M4.99487 5.70186L7.29297 7.99995L4.99487 10.2981L5.70198 11.0052L8.00008 8.70706L10.2982 11.0052L11.0053 10.2981L8.70718 7.99995L11.0053 5.70186L10.2982 4.99475L8.00008 7.29285L5.70198 4.99475L4.99487 5.70186Z" fill="currentColor"/>"#,
};
const LABEL: rune::TextStyle = rune::TextStyle::new(11.0, 530.0).lh(14.0).tracking(0.05);
/// Four rows show; more scroll, with half a row peeking.
const ROW_H: f32 = 33.0;
const MAX_LIST_H: f32 = 148.0;

impl App {
    /// The open session's undelivered prompts.
    pub fn inbox_for(&self, session: &str) -> Option<&Inbox> {
        self.inboxes.get(session).filter(|i| !i.items.is_empty() || i.waiting_notices().next().is_some())
    }

    /// Admits a prompt sent while the session is busy: shown at once, as Desktop does, and
    /// replaced by the server's copy when `session.inbox.enqueued` echoes it. A steer shows at
    /// once where it rests, flying nowhere, and the queue panel rises from behind the composer.
    pub fn admit(&mut self, session: &str, mut prompt: Prompt, delivery: Delivery) {
        prompt.delivery = Some(delivery);
        let id = prompt.id.clone().unwrap_or_default();
        let item = InboxItem { id: id.clone(), text: prompt.display_text.clone().unwrap_or_else(|| prompt.text.clone()), delivery, files: self.attach.files.clone() };
        let sent = crate::send_failed::Sent::new(session, &id, crate::prompt_history::Entry::of(&self.composer), self.attach.files.clone(), prompt.attachments.clone(), true).with_prompt(&prompt);
        let write = self.send_write(session, prompt);
        if matches!(*self.source, Source::Live {allow_send: true,..}) && self.stage.is_none() {
            let paths=sent.paths.clone();
            if !self.enqueue_prompt(sent,write) { self.attach.paths.extend(paths); return; }
        } else {
            if !self.inbox_write(write) { return; }
            self.unsent.note(sent);
        }
        model::set_user_files(&id, std::mem::take(&mut self.attach.files));
        model::set_user_comments(&id, self.quote.take());
        self.clear_line_comments(session);
        if self.unsent.state(&id) == Some(crate::send_failed::State::Waiting) { self.show_outbox(); }
        else { self.inboxes.entry(session.to_owned()).or_default().upsert(item); }
        self.composer.clear();
        if delivery == Delivery::Steer {
            self.steered(session, &id);
        }
    }

    /// Steers a queued prompt (sends it now when the turn has stopped).
    pub fn steer_queued(&mut self, session: &str, id: &str) {
        self.queue_op(session, Operation::Steer { id: id.to_owned() });
    }

    /// Moves a pending steer back to the queue (its menu's Move to queue).
    pub fn queue_pending(&mut self, session: &str, id: &str) {
        self.queue_op(session, Operation::Requeue { id: id.to_owned() });
    }

    /// Removes a queued prompt.
    pub fn withdraw(&mut self, session: &str, id: &str) {
        let queued = self.inboxes.get(session).is_some_and(|i| i.queued().any(|i| i.id == id));
        let id = id.to_owned();
        self.queue_op(session, if queued { Operation::Remove { id } } else { Operation::Delete { id } });
    }

    /// Undo on a removed queued prompt's toast: its words and files come back into `session`'s
    /// composer after anything typed there (its saved draft when another page shows), ready to
    /// queue again. Re-queued by itself it would go with today's agent and model, not its own.
    pub(crate) fn unwithdraw(&mut self, session: &str, text: String, files: Vec<opencode_kit::model::Attachment>) {
        let _ = self.queue_draft_effect(session, Effect::Merge { text, files, paths: Vec::new() });
    }

    /// Withdraws a queued prompt into the composer, after anything already typed there, once the
    /// server lets it go; until then its row shows it pending.
    pub fn undo_queued(&mut self, session: &str, id: &str) {
        let Some(item) = self.inboxes.get(session).and_then(|i| i.items.iter().find(|i| i.id == id)).cloned() else { return };
        // An item listed by the server has its files only on the item.
        let files = model::user_files(&item.id);
        let files = if files.is_empty() { item.files } else { files.to_vec() };
        let (shell, now) = (self.add.shell(), *self.clock);
        self.step_queue(session, Event::UndoRequested { id: item.id, text: item.text, files, shell, now: now });
    }

    /// A row dropped in a new place: the queue is rewritten in `ids`' order (`reorder.requested`).
    pub fn reorder_queued(&mut self, session: &str, ids: Vec<String>) {
        let reverted = self.session(session).and_then(|i| i.revert.as_ref()).is_some();
        let (rows, now) = (self.queue_rows(session), *self.clock);
        self.step_queue(session, Event::ReorderRequested { ids, reverted, rows, now: now });
    }

    /// Fixture runs act locally, as if the server agreed.
    fn inbox_write(&mut self, w: Write) -> bool {
        matches!(*self.source, Source::Fixture) || self.write(w)
    }

    /// Asks `session`'s queue machine for `operation`.
    fn queue_op(&mut self, session: &str, operation: Operation) {
        let now = *self.clock;
        self.step_queue(session, Event::Requested { operation, now: now });
    }

    pub(crate) fn step_queue(&mut self, session: &str, event: Event) {
        let rows = self.queue_rows(session);
        let effects = self.queue_ops.entry(session.to_owned()).or_default().step(event, &rows);
        for effect in effects {
            let Some(effect) = self.queue_draft_effect(session, effect) else { continue };
            match effect {
                Effect::Run { attempt, operation } => self.run_queue_op(session, attempt, operation),
                Effect::Abort { attempt } => self.queue_runs.abort(session, attempt),
                Effect::Failed => crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), ""),
                Effect::Reverted => crate::status_toast::say(self, opencode_i18n::t!("session.queue.reverted"), ""),
                Effect::UndoShell => crate::status_toast::say(self, opencode_i18n::t!("session.queue.undoShell"), ""),
                Effect::Load(_) | Effect::Restore { .. } | Effect::Merge { .. } => {}
            }
        }
    }

    /// `Incoming::Settled` for a queue change of `session`. A change the server took shows at
    /// once, as its event will say.
    pub(crate) fn queue_settled(&mut self, session: &str, attempt: u32, ok: bool) {
        let (attempt, now) = (Attempt(attempt), *self.clock);
        let Some((a, active)) = self.queue_ops.get(session).and_then(|q| q.running()).map(|(a, o)| (a, o.clone())) else { return };
        if a != attempt {
            return;
        }
        if ok {
            let inbox = self.inboxes.entry(session.to_owned()).or_default();
            match &active {
                Operation::Steer { id } => {
                    inbox.set_delivery(id, Delivery::Steer);
                }
                Operation::Requeue { id } => {
                    inbox.set_delivery(id, Delivery::Queue);
                }
                Operation::Remove { id } | Operation::Undo { id, .. } | Operation::Delete { id } => {
                    let item = inbox.items.iter().find(|i| i.id == *id).cloned();
                    inbox.remove(id);
                    // Removed (not taken back into the composer, which keeps it anyway): offered back.
                    if let (Some(item), false) = (item, matches!(active, Operation::Undo { .. })) {
                        let files = model::user_files(&item.id);
                        let files = if files.is_empty() { item.files } else { files.to_vec() };
                        crate::undo::offer(self, opencode_i18n::t!("rune.undo.queueRemoved"), crate::undo::Undoable::Unqueued { session: session.to_owned(), text: item.text, files });
                    }
                }
                Operation::Edit(_) | Operation::Reorder { .. } => {}
            }
            if let Operation::Undo { id, .. } = &active {
                model::set_user_files(id, Vec::new());
            }
            if let Operation::Steer { id } = &active {
                self.steered(session, id);
            }
            crate::windows::peer::touch(self);
            return self.step_queue(session, Event::Succeeded { attempt, now: now });
        }
        self.step_queue(session, Event::Failed { attempt, hidden: Vec::new(), now: now });
    }

    /// Each frame: confirmed rows' hiding expires and a change past its bound ends as it got.
    pub(crate) fn queue_timers(&mut self, tx: &mut Tx) {
        let now = *self.clock;
        let due: Vec<String> = self.queue_ops.iter().filter(|(_, q)| q.due().is_some_and(|d| now + 1e-9 >= d)).map(|(s, _)| s.clone()).collect();
        for session in due {
            let (stage, hidden) = match self.queue_ops.get(&session).and_then(|q| q.running()) {
                // An undo's cancel goes out at once, so it waits for its answer (`committed`).
                Some((_, Operation::Undo { .. })) => (Stage::Committed, Vec::new()),
                Some((attempt, _)) => self.queue_runs.progress(&session, attempt),
                None => (Stage::Running, Vec::new()),
            };
            self.step_queue(&session, Event::Tick { now: now, stage, hidden });
        }
        if let Some(next) = self.queue_ops.values().filter_map(machine::Queue::due).reduce(f64::min) {
            tx.wake_after(std::time::Duration::from_secs_f64((next - now).max(0.0)));
        }
    }

    /// `session.inbox.*` in the server's copy (`App::apply_shared`): a delivered prompt joins
    /// the end of the transcript, where the server puts it. What a listing dropped goes with the
    /// event, so each window keeps the steers it sent that the server hasn't confirmed.
    pub fn apply_inbox(&mut self, ty: &str, data: &serde_json::Value, sid: &str) -> crate::events::InboxDone {
        let inbox = self.inboxes.entry(sid.to_owned()).or_default();
        let old = if ty == "session.inbox.listed" { inbox.items.clone() } else { vec![] };
        let change = inbox.apply(ty, data);
        let dropped = old.into_iter().filter(|o| !inbox.items.iter().any(|i| i.id == o.id)).collect();
        // The server admits every prompt to its inbox, a send to an idle session too. That one
        // already shows as its prompt, which is where it stays, so it never waits as a steer.
        if let Some(t) = self.transcripts.get(sid) {
            inbox.items.retain(|i| !t.messages.iter().any(|m| m.id() == i.id));
            inbox.settle_notices(|id| t.messages.iter().any(|m| m.id() == id));
        }
        let delivered = inbox.take_notice_effects();
        // A background shell or subagent's notice the server delivered keeps its row as a message.
        let notices = !delivered.is_empty();
        for model::NoticeEffect::ToTranscript(n) in delivered {
            if let Some(t) = self.transcripts.get_mut(sid)
                && !t.messages.iter().any(|m| m.id() == n.id)
            {
                t.messages.push(Message::Notice(n));
            }
        }
        let changed = match change {
            InboxChange::None => notices,
            InboxChange::Changed => true,
            InboxChange::Delivered(item) => {
                if !item.files.is_empty() {
                    model::set_user_files(&item.id, item.files);
                }
                if let Some(t) = self.transcripts.get_mut(sid)
                    && !t.messages.iter().any(|m| m.id() == item.id)
                {
                    t.messages.push(Message::User { id: item.id, text: item.text });
                }
                true
            }
        };
        crate::events::InboxDone { changed, dropped }
    }

    /// After `apply_inbox`, in each window: its own sends the event admits or delivers, and its
    /// unconfirmed steers a listing dropped, which come back.
    pub fn inbox_landed(&mut self, ty: &str, data: &serde_json::Value, sid: &str, done: &crate::events::InboxDone) -> bool {
        let id = data["inboxID"].as_str().unwrap_or("");
        let mut admitted = match ty {
            "session.inbox.enqueued" => self.admitted_send(id),
            "session.inbox.delivered" => self.accept_send(id),
            _ => false,
        };
        if ty == "session.inbox.listed" {
            let inbox = self.inboxes.entry(sid.to_owned()).or_default();
            let before = inbox.items.len();
            self.unsent.preserve_inbox(sid, inbox, done.dropped.clone());
            if let Some(t) = self.transcripts.get(sid) {
                inbox.items.retain(|i| !t.messages.iter().any(|m| m.id() == i.id));
            }
            if inbox.items.len() != before {
                crate::windows::peer::touch(self);
                admitted = true;
            }
            let ids: Vec<String> = data["items"].as_array().into_iter().flatten().filter_map(|i| i["id"].as_str().map(str::to_owned)).collect();
            admitted |= self.judge_sends_by_inbox(sid, &ids);
            self.check_sends();
        }
        // The queue hears the server's queued prompts whenever they change (`inbox.synced`).
        if let Some(queue) = self.queue_ops.get(sid) {
            let ids = self.inboxes.get(sid).map(|i| i.queued().map(|i| i.id.clone()).collect()).unwrap_or_default();
            let delivered = queue.doubted().is_some_and(|d| self.transcripts.get(sid).is_some_and(|t| t.messages.iter().any(|m| m.id() == d)));
            self.step_queue(sid, Event::Synced { ids, delivered });
        }
        crate::rewind::inbox_changed(self, sid);
        admitted || done.changed
    }

    /// A fetched page that already has a prompt means its delivery event was missed.
    pub fn settle_inbox(&mut self, session: &str, fetched: &[String]) {
        if let Some(inbox) = self.inboxes.get_mut(session) {
            inbox.items.retain(|i| !fetched.contains(&i.id));
            inbox.settle_notices(|id| fetched.iter().any(|f| f == id));
            inbox.take_notice_effects();
        }
    }
}

/// What a pending steer's bubble says: its words, or its attachment count when it has none.
pub fn pending_label(item: &InboxItem) -> String {
    let files = model::user_files(&item.id).len().max(item.files.len());
    let quotes = model::user_comments(&item.id).len();
    match (item.text.trim().is_empty(), quotes) {
        (false, _) => item.text.clone(),
        (true, 0) => attachments_label(files),
        (true, 1) if files == 0 => "1 quote".into(),
        (true, n) if files == 0 => format!("{n} quotes"),
        (true, _) => attachments_label(files + quotes),
    }
}

fn attachments_label(n: usize) -> String {
    if n == 1 { "1 attachment".into() } else { format!("{n} attachments") }
}

fn icon_button(p: &Pal, key: &str, label: &str, glyph: &'static Icon, side: f32, size: f32, f: impl Fn(&mut App) + 'static) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .size(side, side)
        .center()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        // The row it takes away goes at once (rows enter and leave on `SpringPair::NONE`), and the
        // tray closes over it on its height spring, as Desktop's `Grow` does.
        .on_press(move |s: &mut App| f(s))
        .child(icon(glyph, size).color(p.icon_muted))
}

/// How far the tray runs under the composer card (`-mb-3`, `queue-panel.tsx:93`).
const TUCK: f32 = 12.0;

/// The queue: Desktop's tray above the composer (`queue-panel.tsx:88-158`). The tray is the
/// page's own colour (`bg-v2-background-bg-base`) inside a 0.5 pt `border-border-base` ring with
/// 10 pt corners (`rounded-xl`: the app's `--radius-xl` is 0.625rem), 6 pt in on the sides and top; its bottom runs 12 pt under the composer, which
/// stays its own rounded card on top (`relative z-10`), so 6 pt of tray show under the last row
/// (`pb-[18px]`), or none past four rows, where the list scrolls (`pb-3`). Empty, it closes to
/// nothing on its height spring, clipped as it goes (`Grow`), and the composer comes down to
/// where it was.
///
/// A question or permission in the composer's place closes it, as Desktop hides its queue while
/// a dock blocks the composer (`SessionQueuePanel hidden={blocked}`); it opens again after.
pub fn panel(s: &App, p: &Pal, session: &str, busy: bool) -> El {
    let rows = shown_rows(s, session);
    let shell = col().key("queue").w_full().max_w(session_view::style::dock_w()).clip_soft(0.0);
    if rows.is_empty() {
        return shell.h(0.0);
    }
    let n = rows.len();
    let ops = s.queue_ops.get(session);
    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut list = col().key("queue-list").w_full();
    for (k, r) in rows.iter().enumerate() {
        // While the turn is stopped the queue is parked: the first prompt shows its actions,
        // labelled Send, which is how the queue resumes.
        let st = RowState {
            busy,
            parked: !busy && k == 0,
            editing: ops.and_then(|q| q.editing()) == Some(r.id.as_str()),
            changing: ops.is_some_and(machine::Queue::busy),
            lifted: s.queue_drag.lifted(session, &r.id),
            dragging: s.queue_drag.active(session),
            offset: s.queue_drag.offset(session, k),
            key: format!("queued-{}", ops.map_or_else(|| r.id.clone(), |q| q.origin(&r.id))),
            index: k,
        };
        list = list.child(queue_row(p, session, r, &ids, st));
    }
    let list_h = (n as f32 * ROW_H).min(MAX_LIST_H);
    let mut tray = col()
        .key("queue-tray")
        .w_full()
        .radius(10.0)
        .fill(p.bg_base)
        .box_shadow(Pal::ring_in(p.border))
        .px(6.0)
        .pt(6.0)
        .pb(if n > 4 { TUCK } else { TUCK + 6.0 });
    if n > 3 {
        tray = tray.child(row().key("queue-count").px(6.0).pt(4.0).pb(1.0).child(odometer(format!("queue-count-{session}"), opencode_i18n::plural!("session.queue.count", n).to_uppercase(), n as f64, LABEL, p.muted)));
    }
    let list = if n > 4 {
        col().key("queue-scroll").w_full().h(list_h).scroll(rune::ScrollSpec { stick_bottom: false }).clip_fade_linear(0.0, 8.0, 0.0, 12.0).child(list)
    } else {
        list
    };
    shell.role("list").label_static(opencode_i18n::t!("rune.queue.label")).mb(-TUCK).child(tray.child(list))
}
/// The queue's rows (`queue-machine.ts` `selectQueueRows`); none while a dock covers the composer.
fn shown_rows(s: &App, session: &str) -> Vec<QueueRow> {
    if s.dock_shown() {
        return Vec::new();
    }
    s.queue_rows(session)
}

/// A queued prompt's words as its row shows them: on one line, as a `truncate` button collapses
/// its whitespace.
fn row_text(item: &InboxItem) -> String {
    item.text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How many attachments a queued prompt carries: its files, the files it names by path, and its
/// quotes (`queuedPromptRows`).
fn row_files(item: &InboxItem) -> usize {
    let files = model::inbox_payload(&item.id).and_then(|p| p["files"].as_array().map(Vec::len)).unwrap_or(0);
    let files = files.max(model::user_files(&item.id).len()).max(item.files.len());
    files + opencode_kit::mention::user_path_attachments(&item.id).len() + model::user_comments(&item.id).len()
}

/// How a row stands: the turn running (`busy`), the queue parked on it, open in the composer,
/// any change on its way (`changing`: the actions wait), lifted by a drag or beside one, and
/// how far the drag has moved it.
struct RowState {
    busy: bool,
    parked: bool,
    editing: bool,
    changing: bool,
    lifted: bool,
    dragging: bool,
    offset: f32,
    key: String,
    /// Its place in the strip, which keys its words and handle for scripts.
    index: usize,
}

/// The rows a drag passes step aside on dnd-kit's sortable transition (`250ms
/// cubic-bezier(0.25, 1, 0.5, 1)`).
fn sort_motion() -> rune::Anim {
    rune::Anim::new(rune::Curve::tween(0.25, rune::Easing::cubic(0.25, 1.0, 0.5, 1.0)))
}

/// A row (`queue-panel.tsx:194-300`): the drag handle, the words (a click opens them in the
/// composer), the attachment count, and the actions, which show on hover, always on the parked
/// first row and while the row is open in the composer. A row whose change is on its way shows
/// its words faint and a loader in place of its actions.
fn queue_row(p: &Pal, session: &str, r: &QueueRow, ids: &[String], st: RowState) -> El {
    let busy = st.busy;
    let label = if r.text.is_empty() { attachments_label(r.files) } else { r.text.clone() };
    let (ss, is) = (session.to_owned(), r.id.clone());
    let (su, iu) = (session.to_owned(), r.id.clone());
    let (sr, ir) = (session.to_owned(), r.id.clone());
    let (se, ie) = (session.to_owned(), r.id.clone());
    // A lifted row takes no hover: dnd-kit turns off the pointer for what it drags.
    let reveal = |e: El| match (st.parked || st.editing, st.lifted) {
        (true, _) => e,
        (false, true) => e.opacity(0.0),
        (false, false) => e.group_opacity(0.0, 1.0),
    };
    let steer = row()
        .key("steer")
        .role("button")
        .label(if busy { opencode_i18n::t!("session.queue.steer") } else { opencode_i18n::t!("session.queue.send") })
        // `button-v2` small, ghost-faint, with `gap-0.5 ps-1`.
        .h(24.0)
        .pl(4.0)
        .pr(9.0)
        .gap(2.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_press(move |s: &mut App| s.steer_queued(&ss, &is))
        .child(icon(&ic::ARROW_UP, 16.0).color(p.icon_muted))
        .child(text(if busy { opencode_i18n::t!("session.queue.steer") } else { opencode_i18n::t!("session.queue.send") }).font(UI_MED).color(p.faint));
    // Its tooltip shows only while the turn runs (`inactive={!working}`).
    let steer = if busy { steer.help(opencode_i18n::t!("session.queue.steerTooltip")) } else { steer };
    let words = row()
        .key("edit")
        .role("button")
        .label(label.clone())
        .self_start()
        .shrink()
        .min_w(0.0)
        // `rounded-sm`: the app's `--radius-sm` is 0.25rem.
        .radius(4.0)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.edit_queued(&se, &ie);
        })
        .child(text(label).key("label").font(UI).color(if st.editing || r.pending { p.faint } else { p.text }).truncate_fade())
        .child(marker(format!("edit-{}", st.index)));
    let words = if st.editing || r.pending { words } else { words.hover(p.hover).cursor(rune::Cursor::Text) };
    let mut inner = row().shrink().min_w(0.0).gap(16.0).center_y().child(words);
    if r.files > 0 && !r.text.is_empty() {
        inner = inner.child(text(attachments_label(r.files)).font(UI).color(p.muted));
    }
    let left = row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().child(handle(p, session, &r.id, ids, st.index, st.changing || r.pending)).child(inner);
    let actions = if r.pending {
        row().key("actions").gap(4.0).center_y().child(session_view::style::activity_ring_sized(p, 14.0))
    } else {
        let mut actions = reveal(row().key("actions").gap(4.0).center_y());
        if !st.editing {
            actions = actions.child(steer.child(marker(format!("steer-{}", st.index)))).child(icon_button(p, "undo", opencode_i18n::t!("session.queue.undo"), &UNDO, 24.0, 16.0, move |s: &mut App| s.undo_queued(&su, &iu)).child(marker(format!("undo-{}", st.index))));
        }
        let actions = actions.child(icon_button(p, "remove", opencode_i18n::t!("session.queue.remove"), &XMARK, 24.0, 16.0, move |s: &mut App| s.withdraw(&sr, &ir)).child(marker(format!("remove-{}", st.index))));
        // While another change runs or a row is dragged, the actions wait
        // (`pointer-events-none`, `disabled`).
        if st.changing || st.dragging { actions.inert() } else { actions }
    };
    let row_el = row()
        .key(st.key)
        .role("listitem")
        .w_full()
        .h(32.0)
        .mb(1.0)
        .pl(2.0)
        .pr(4.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .fill(if st.editing { p.hover } else { rune::Color::TRANSPARENT })
        // It leaves at once, and the queue closes up on its height spring.
        .transition(rune::Transition::opacity().springs(rune::SpringPair::NONE))
        .child(left)
        .child(actions);
    // The lifted row follows the pointer at 60% over the others, which step aside for it.
    if st.lifted {
        row_el.opacity(0.6).raise().offset(0.0, st.offset).offset_motion(rune::Anim::INSTANT)
    } else {
        row_el.offset(0.0, st.offset).offset_motion(sort_motion())
    }
}

/// An empty box over its parent, keyed by the row's place in the strip so scripts can find the
/// row; its parent keeps a key of its own, so a row that moves up keeps its elements.
fn marker(key: String) -> El {
    el().key(key).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
}

/// The drag handle (`queue-panel.tsx:206-215`): six 2 pt squares in two columns, 2 apart and
/// 2.25 down, inside 4 of padding, in `bg-layer-04`. The pointer drags it; the keys lift it from
/// focus.
fn handle(p: &Pal, session: &str, id: &str, ids: &[String], index: usize, disabled: bool) -> El {
    let dot = || el().size(2.0, 2.0).fill(p.q.layer4);
    let column = || col().gap(2.25).child(dot()).child(dot()).child(dot());
    let h = row().key("reorder").role("button").label_static(opencode_i18n::t!("session.queue.reorder")).padding(4.0).gap(2.0).child(column()).child(column()).child(marker(format!("reorder-{index}")));
    if disabled {
        return h;
    }
    let (sd, idd, idsd) = (session.to_owned(), id.to_owned(), ids.to_vec());
    let (sk, idk, idsk) = (session.to_owned(), id.to_owned(), ids.to_vec());
    h.cursor(rune::Cursor::Grab)
        .focusable(true)
        .on_drag(move |s: &mut App, d: &mut rune::Drag, tx: &mut Tx| {
            let effect = match d.phase {
                rune::Phase::Down => s.queue_drag.step(drag::Event::Down { session: &sd, id: &idd }),
                rune::Phase::Move => {
                    tx.animated();
                    s.queue_drag.step(drag::Event::Move { dx: d.dx, dy: d.dy, ids: &idsd })
                }
                rune::Phase::Up => {
                    tx.instant();
                    s.queue_drag.step(drag::Event::Up { cancelled: d.cancelled() })
                }
            };
            if let Some(drag::Effect::Reorder { session, ids }) = effect {
                s.reorder_queued(&session, ids);
            }
        })
        .on_key(move |s: &mut App, ev, tx| {
            let lifted = s.queue_drag.active(&sk);
            let event = match ev.key {
                Key::Text(ref t) if t == " " && !lifted => drag::Event::Pick { session: &sk, id: &idk, ids: &idsk },
                Key::Enter if !lifted => drag::Event::Pick { session: &sk, id: &idk, ids: &idsk },
                Key::Text(ref t) if t == " " => drag::Event::Drop,
                Key::Enter | Key::Tab if lifted => drag::Event::Drop,
                Key::Up if lifted => drag::Event::Step(-1),
                Key::Down if lifted => drag::Event::Step(1),
                Key::Escape if lifted => drag::Event::Cancel,
                _ => return false,
            };
            tx.animated();
            if let Some(drag::Effect::Reorder { session, ids }) = s.queue_drag.step(event) {
                tx.instant();
                s.reorder_queued(&session, ids);
            }
            true
        })
}

/// "Queue ⌘⏎" (or "Steer ⌘⏎") beside the submit button while a turn runs and the composer holds
/// a prompt: submits with the delivery Enter doesn't use. The keys show only in a window at least
/// 640 wide (`sm`; `hidden sm:block`, `editor.tsx:1456-1458`). It sits in the toolbar's free
/// `room`, which it never widens, as the word and the keys, the word alone, or nothing: in a
/// narrow window it can't push the mic and Send aside.
pub fn alternate_hint(p: &Pal, room: f32, sm: bool, steer: bool) -> Option<El> {
    let word = if steer { opencode_i18n::t!("ui.promptInput.steer") } else { opencode_i18n::t!("ui.promptInput.queue") };
    // Its padding and right margin, the word, and the keycaps 6 after it (`gap-1.5`), each
    // Desktop's neutral `Keybind` chip 2 apart (`keymap::view::chips`).
    let labels: Vec<String> = crate::keymap::Bind::SubmitAlternate.keycaps().iter().map(|k| k.to_uppercase()).collect();
    let caps: f32 = labels.iter().map(|k| (rune::text::measure(k, &[], crate::keymap::view::LABEL, None).0 + 8.0).max(14.0)).sum();
    let keys = caps + 6.0 + 2.0 * labels.len().saturating_sub(1) as f32;
    let width = rune::text::measure(word, &[], UI_MED, None).0 + 12.0 + 10.0;
    if room < width {
        return None;
    }
    let labels = if sm && room >= width + keys { labels } else { Vec::new() };
    let hint = row()
        .key("alternate-delivery")
        .role("button")
        .label(word)
        // `button-v2` ghost-faint at size small (`editor.tsx:1448-1455`, `button.css:46-50`):
        // 24 tall, 6 in, 4 pt corners, 12 before Send (`me-3`, the toolbar's 2 pt gap and 10).
        .h(24.0)
        .px(6.0)
        .mr(10.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .transition(rune::Transition::opacity())
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.submit(true, tx))
        .child(text(word).font(UI_MED).color(p.faint))
        // The keys sit in a block `span` (`hidden sm:block`) whose line box sets the inline-flex
        // `Keybind` on its baseline, 2 pt below the button's centre (measured at a true 1x).
        .children((!labels.is_empty()).then(|| crate::keymap::view::chips(p, labels).offset(0.0, 2.0)));
    // Only the steer has a tooltip (`inactive={delivery !== "steer"}`).
    Some(if steer { hint.help(opencode_i18n::t!("ui.promptInput.steerHint")) } else { hint })
}

/// The `queue*` scenes: the open session busy with `n` prompts queued, over the question dock
/// when `ask`.
pub fn scene(s: &mut App, n: usize, ask: bool) {
    crate::scenes::busy_scene(s);
    let crate::Route::Session { id, .. } = (*s.route).clone() else { return };
    let words = ["Then write up what changed in the PR description", "And keep the retry budget as it is", "Bump the version and tag it"];
    for k in 0..n {
        let item = InboxItem { id: format!("msg_zzqueued{k}"), text: words[k % words.len()].into(), delivery: Delivery::Queue, files: Vec::new() };
        s.inboxes.entry(id.clone()).or_default().upsert(item);
    }
    if ask {
        let q = s.fixture["question"].clone();
        s.apply_event("form.created", &q);
    }
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    // Busy with prompts queued above the composer or above the question dock;
    // `queue-question-morph` answers the question at 1.5 s and asks again at 2.5 s.
    ("queue", |s| scene(s, 1, false)),
    ("queue-3", |s| scene(s, 3, false)),
    ("queue-question", |s| scene(s, 1, true)),
    ("queue-question-3", |s| scene(s, 3, true)),
    ("queue-question-morph", |s| {
        scene(s, 1, true);
        let q = s.fixture["question"].clone();
        let replied = serde_json::json!({ "id": q["id"], "sessionID": q["sessionID"] });
        *s.playback = [(1.5, "form.replied".into(), replied), (2.5, "form.created".into(), q)].into();
    }),
]);

/// Timeline › Sends behind waiting steers: two steers wait, the run stops, and two more sends
/// join them under Working, in the order the server delivers them all.
mod behind_story {
    use rune::Play;
    use serde_json::json;

    use crate::App;
    use crate::stories::{Trigger, emit, replay, sid};

    const WORDS: [&str; 4] = ["or did he give me stuff?", "can you check", "this is really weird", "what"];

    const KEYS: &[Trigger] = &[
        replay(open),
        Trigger { key: 's', label: "Send", run: send },
        Trigger { key: 'x', label: "Stop the run", run: stop },
        Trigger { key: 'd', label: "Deliver the oldest", run: deliver },
    ];

    fn open(s: &mut App, tx: &mut Play<App>) {
        s.finish_stream();
        let id = sid(s);
        s.running.insert(id);
        tx.after(0.4, send);
        tx.after(1.0, send);
        tx.after(1.8, stop);
        tx.after(2.4, send);
        tx.after(3.0, send);
        for k in 0..4 {
            tx.after(4.2 + 0.1 * k as f64, deliver);
        }
    }

    fn send(s: &mut App, tx: &mut Play<App>) {
        let sent = s.inbox_for(&sid(s)).map_or(0, |i| i.items.len());
        *s.composer = rune::TextEdit::new(WORDS[sent % WORDS.len()]);
        s.send(tx);
    }

    fn stop(s: &mut App, tx: &mut Play<App>) {
        emit(s, "session.execution.interrupted", json!({ "reason": "user" }), tx);
    }

    fn deliver(s: &mut App, tx: &mut Play<App>) {
        let Some(id) = s.inbox_for(&sid(s)).and_then(|i| i.steering().next()).map(|i| i.id.clone()) else { return };
        emit(s, "session.inbox.delivered", json!({ "inboxID": id }), tx);
    }

    rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
        crate::stories::Story::session("Timeline/Sends behind waiting steers", KEYS),
    ]);
}

