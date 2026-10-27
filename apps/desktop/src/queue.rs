//! Prompts sent while a session is busy, as OpenCode Desktop shows them. Enter steers: the
//! prompt shows at the end of the timeline, muted and marked Pending, until the server delivers
//! it at the turn's next step boundary. ⌘⏎ queues: the prompt waits in a panel above the
//! composer until the turn ends, where it can be steered (Send once the turn has stopped),
//! taken back into the composer, or removed.

use rune::{El, Icon, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use opencode_kit::client::{Prompt, Write};
use opencode_kit::model::{self, Delivery, Inbox, InboxChange, InboxItem, Message};

use crate::palette::Pal;
use crate::view::{UI, UI_MED};
use crate::{App, Source};

const STEER: Icon = Icon {
    name: "outline-steer",
    view: 16.0,
    body: r#"<path d="M8 14C11.3137 14 14 11.3137 14 8C14 4.68634 11.3137 2 8 2C4.68626 2 2 4.68634 2 8C2 11.3137 4.68626 14 8 14Z" stroke-linecap="round" stroke-linejoin="round" stroke="currentColor"/><path d="M8 3.86328V12.1391C10.2854 12.1391 12.1379 10.2863 12.1379 8.00121C12.1379 5.71608 10.2854 3.86328 8 3.86328Z" fill="currentColor"/>"#,
};
const UNDO: Icon = Icon {
    name: "outline-undo",
    view: 16.0,
    body: r#"<path d="M2 10L4.25193 7.76999C7.29213 4.7594 12.4119 5.77372 14.0747 9.71606" stroke="currentColor"/><path d="M2 6V10H6" stroke="currentColor" stroke-linecap="square"/>"#,
};
const TO_QUEUE: Icon = Icon {
    name: "arrow-down-to-line",
    view: 20.0,
    body: r#"<path d="M15.2083 11.6667L10 16.875L4.79167 11.6667M10 16.25V3.125" stroke="currentColor" stroke-width="1.25" stroke-linecap="square"/>"#,
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
        self.inboxes.get(session).filter(|i| !i.items.is_empty())
    }

    /// Admits a prompt sent while the session is busy: shown at once, as Desktop does, and
    /// replaced by the server's copy when `session.inbox.enqueued` echoes it.
    pub fn admit(&mut self, session: &str, mut prompt: Prompt, delivery: Delivery, tx: &mut Tx) {
        prompt.delivery = Some(delivery);
        let id = prompt.id.clone().unwrap_or_default();
        let item = InboxItem { id: id.clone(), text: prompt.display_text.clone().unwrap_or_else(|| prompt.text.clone()), delivery, files: self.attach.files.clone() };
        if !self.inbox_write(Write::Send { session: session.to_owned(), prompt }) {
            return;
        }
        model::set_user_files(&id, std::mem::take(&mut self.attach.files));
        self.inboxes.entry(session.to_owned()).or_default().upsert(item);
        match delivery {
            Delivery::Steer => tx.fly_text("prompt", format!("text-{id}")),
            Delivery::Queue => tx.instant(),
        }
        self.composer.clear();
    }

    /// Steers a queued prompt (sends it now when the turn has stopped).
    pub fn steer_queued(&mut self, session: &str, id: &str) {
        self.redeliver(session, id, Delivery::Steer);
    }

    /// Moves a pending steer back to the queue.
    pub fn queue_pending(&mut self, session: &str, id: &str) {
        self.redeliver(session, id, Delivery::Queue);
    }

    fn redeliver(&mut self, session: &str, id: &str, delivery: Delivery) {
        let w = Write::Deliver { session: session.to_owned(), item: id.to_owned(), delivery };
        if self.inbox_write(w) {
            self.inboxes.entry(session.to_owned()).or_default().set_delivery(id, delivery);
        }
    }

    pub fn withdraw(&mut self, session: &str, id: &str) -> Option<InboxItem> {
        if !self.inbox_write(Write::Cancel { session: session.to_owned(), item: id.to_owned() }) {
            return None;
        }
        self.inboxes.get_mut(session)?.remove(id)
    }

    /// Withdraws a queued prompt into the composer, after anything already typed there.
    pub fn undo_queued(&mut self, session: &str, id: &str) {
        let Some(item) = self.withdraw(session, id) else { return };
        let draft = self.composer.text.trim_end();
        let merged = if draft.is_empty() { item.text } else { format!("{draft}\n\n{}", item.text) };
        self.composer.replace_text(merged);
        self.composer.focused = true;
        self.attach.files.extend(model::user_files(&item.id).iter().cloned());
        model::set_user_files(&item.id, Vec::new());
    }

    /// Fixture runs act locally, as if the server agreed.
    fn inbox_write(&mut self, w: Write) -> bool {
        matches!(self.source, Source::Fixture) || self.write(w)
    }

    /// `session.inbox.*`: a delivered prompt joins the end of the transcript, where the server
    /// puts it.
    pub fn apply_inbox(&mut self, ty: &str, data: &serde_json::Value, sid: &str) -> bool {
        match self.inboxes.entry(sid.to_owned()).or_default().apply(ty, data) {
            InboxChange::None => false,
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
        }
    }

    /// A fetched page that already has a prompt means its delivery event was missed.
    pub fn settle_inbox(&mut self, session: &str, fetched: &[String]) {
        if let Some(inbox) = self.inboxes.get_mut(session) {
            inbox.items.retain(|i| !fetched.contains(&i.id));
        }
    }
}

/// Pending steers, below the running turn: muted bubbles marked Pending, with Move to queue and
/// Delete.
pub fn pending_rows(s: &App, p: &Pal, session: &str) -> Vec<(String, El)> {
    let Some(inbox) = s.inbox_for(session) else { return Vec::new() };
    inbox.steering().map(|item| (format!("p-{}", item.id), pending_bubble(p, session, item))).collect()
}

fn pending_bubble(p: &Pal, session: &str, item: &InboxItem) -> El {
    let (sq, iq) = (session.to_owned(), item.id.clone());
    let (sd, id) = (session.to_owned(), item.id.clone());
    let files = model::user_files(&item.id);
    let label = if item.text.trim().is_empty() { attachments_label(files.len()) } else { item.text.clone() };
    col()
        .w_full()
        .items_end()
        .pt(20.0)
        .pb(16.0)
        .gap(4.0)
        .child(
            col()
                .max_w(460.0)
                .shrink()
                .px(12.0)
                .py(8.0)
                .radius(10.0)
                .fill(p.layer1)
                .child(text(label).key(format!("text-{}", item.id)).font(crate::view::BUBBLE).color(p.muted).wrap()),
        )
        .child(
            row()
                .key("pending-meta")
                .h(24.0)
                .gap(4.0)
                .center_y()
                .child(icon(&STEER, 14.0).color(p.icon_faint))
                .child(text("Pending").font(UI).color(p.faint))
                .child(el().w(4.0))
                .child(icon_button(p, "move-to-queue", "Move to queue", &TO_QUEUE, 16.0, move |s: &mut App| s.queue_pending(&sq, &iq)))
                .child(icon_button(p, "delete-pending", "Delete", &ic::TRASH, 16.0, move |s: &mut App| {
                    s.withdraw(&sd, &id);
                })),
        )
}

fn attachments_label(n: usize) -> String {
    if n == 1 { "1 attachment".into() } else { format!("{n} attachments") }
}

fn icon_button(p: &Pal, key: &str, label: &str, glyph: &'static Icon, size: f32, f: impl Fn(&mut App) + 'static) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .size(24.0, 24.0)
        .center()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            f(s);
        })
        .child(icon(glyph, size).color(p.icon_muted))
}

/// The queue: a panel tucked under the composer's top edge, one row per queued prompt.
pub fn panel(s: &App, p: &Pal, session: &str, busy: bool) -> Option<El> {
    let queued: Vec<&InboxItem> = s.inbox_for(session)?.queued().collect();
    if queued.is_empty() {
        return None;
    }
    let n = queued.len();
    let mut list = col().key("queue-list").w_full();
    for (k, item) in queued.iter().enumerate() {
        // While the turn is stopped the queue is parked: the first prompt shows its actions,
        // labelled Send, which is how the queue resumes.
        list = list.child(queue_row(p, session, item, busy, !busy && k == 0));
    }
    let list_h = (n as f32 * ROW_H).min(MAX_LIST_H);
    let mut panel = col()
        .key("queue")
        .role("list")
        .label("Queued prompts")
        .w_full()
        .max_w(crate::view::CONTENT_W)
        .px(6.0)
        .pt(6.0)
        .pb(if n > 4 { 12.0 } else { 18.0 })
        .mb(-12.0)
        .radius(12.0)
        .fill(p.bg_base)
        .border(0.5, p.border)
        .transition(rune::Transition::opacity().springs(crate::view::no_motion()));
    if n > 3 {
        panel = panel.child(row().key("queue-count").px(6.0).pt(4.0).pb(1.0).child(text(format!("{n} QUEUED")).font(LABEL).color(p.muted)));
    }
    let list = if n > 4 {
        col().key("queue-scroll").w_full().h(list_h).scroll(rune::ScrollSpec { stick_bottom: false }).clip_fade(0.0, 8.0, 0.0, 12.0).child(list)
    } else {
        list
    };
    Some(panel.child(list))
}

fn queue_row(p: &Pal, session: &str, item: &InboxItem, busy: bool, parked: bool) -> El {
    let files = model::user_files(&item.id).len().max(item.files.len());
    let label = if item.text.trim().is_empty() { attachments_label(files) } else { item.text.lines().next().unwrap_or("").to_owned() };
    let (ss, is) = (session.to_owned(), item.id.clone());
    let (su, iu) = (session.to_owned(), item.id.clone());
    let (sr, ir) = (session.to_owned(), item.id.clone());
    let reveal = |e: El| if parked { e } else { e.group_opacity(0.0, 1.0) };
    let steer = row()
        .key("steer")
        .role("button")
        .label(if busy { "Steer" } else { "Send" })
        .h(24.0)
        .px(6.0)
        .gap(4.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.steer_queued(&ss, &is);
        })
        .child(icon(&ic::ARROW_UP, 14.0).color(p.icon_faint))
        .child(text(if busy { "Steer" } else { "Send" }).font(UI_MED).color(p.faint));
    let mut left = row().grow(1.0).shrink().gap(16.0).center_y().pl(6.0).child(text(label).font(UI).color(p.text).truncate());
    if files > 0 && !item.text.trim().is_empty() {
        left = left.child(text(attachments_label(files)).font(UI).color(p.muted));
    }
    row()
        .key(format!("queued-{}", item.id))
        .role("listitem")
        .w_full()
        .h(32.0)
        .mb(1.0)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .child(left)
        .child(
            reveal(row().key("actions").gap(6.0).center_y())
                .child(steer)
                .child(icon_button(p, "undo", "Undo", &UNDO, 16.0, move |s: &mut App| s.undo_queued(&su, &iu)))
                .child(icon_button(p, "remove", "Remove", &XMARK, 16.0, move |s: &mut App| {
                    s.withdraw(&sr, &ir);
                })),
        )
}

/// "Queue ⌘⏎" (or "Steer ⌘⏎") beside the submit button while a turn runs and the composer holds
/// a prompt: submits with the delivery Enter doesn't use.
pub fn alternate_hint(p: &Pal) -> El {
    row()
        .key("alternate-delivery")
        .role("button")
        .label("Queue")
        .h(28.0)
        .px(6.0)
        .mr(4.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .transition(rune::Transition::opacity().springs(crate::view::no_motion()))
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.submit(true, tx))
        .child(text("Queue").font(UI_MED).color(p.faint))
        .child(keycap(p, "⌘"))
        .child(keycap(p, "⏎"))
}

fn keycap(p: &Pal, key: &str) -> El {
    row().h(16.0).min_w(16.0).px(3.0).center().radius(3.0).fill(p.layer2).child(text(key.to_owned()).font(UI).color(p.faint))
}


#[cfg(test)]
mod tests {
    use rune::{Env, Runtime, TextEdit};

    use crate::{App, Route};

    fn busy_app() -> Runtime<App> {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let Route::Session(id) = app.route.clone() else { panic!("no open session") };
        app.running.insert(id);
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        rt
    }

    fn texts(rt: &Runtime<App>, inside: &str) -> Vec<String> {
        rt.elements().into_iter().filter(|e| !e.exiting && e.path.contains(inside)).filter_map(|e| e.text).collect()
    }

    fn shows(rt: &Runtime<App>, s: &str) -> bool {
        rt.elements().into_iter().any(|e| !e.exiting && e.text.as_deref() == Some(s))
    }

    #[test]
    fn a_queued_prompt_waits_in_the_panel_not_the_timeline() {
        let mut rt = busy_app();
        let before = rt.state.transcripts.values().map(|t| t.messages.len()).sum::<usize>();
        rt.update(|s, tx| {
            s.composer = TextEdit::new("then update the docs");
            s.submit(true, tx);
        });
        rt.frame(1.0 / 120.0);
        assert!(rt.state.composer.text.is_empty());
        assert_eq!(rt.state.transcripts.values().map(|t| t.messages.len()).sum::<usize>(), before);
        assert!(texts(&rt, "queue").iter().any(|t| t == "then update the docs"), "{:?}", texts(&rt, "queue"));
        assert!(!shows(&rt, "Pending"));

        // Undo takes it back into the composer.
        let (sid, id) = {
            let (sid, inbox) = rt.state.inboxes.iter().find(|(_, i)| !i.items.is_empty()).unwrap();
            (sid.clone(), inbox.items[0].id.clone())
        };
        rt.update(|s, _| s.undo_queued(&sid, &id));
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.composer.text, "then update the docs");
        assert!(texts(&rt, "queue").is_empty());
    }

    #[test]
    fn a_steer_shows_pending_until_delivered() {
        let mut rt = busy_app();
        rt.update(|s, tx| {
            s.composer = TextEdit::new("use tabs, not spaces");
            s.send(tx);
        });
        rt.frame(1.0 / 120.0);
        assert!(shows(&rt, "Pending"));
        assert!(shows(&rt, "use tabs, not spaces"));
        assert!(texts(&rt, "queue").is_empty());

        let (sid, id) = {
            let (sid, inbox) = rt.state.inboxes.iter().find(|(_, i)| !i.items.is_empty()).unwrap();
            (sid.clone(), inbox.items[0].id.clone())
        };
        rt.update(|s, _| {
            s.apply_event("session.inbox.delivered", &serde_json::json!({"sessionID": sid, "inboxID": id}));
        });
        rt.frame(1.0 / 120.0);
        assert!(!shows(&rt, "Pending"));
        assert!(shows(&rt, "use tabs, not spaces"));
        assert!(rt.state.transcripts[&sid].messages.iter().any(|m| m.id() == id));
    }
}
