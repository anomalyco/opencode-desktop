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
    /// replaced by the server's copy when `session.inbox.enqueued` echoes it. A steer rises whole
    /// into the timeline as a send does, and the queue panel rises from behind the composer.
    pub fn admit(&mut self, session: &str, mut prompt: Prompt, delivery: Delivery) {
        prompt.delivery = Some(delivery);
        let id = prompt.id.clone().unwrap_or_default();
        let item = InboxItem { id: id.clone(), text: prompt.display_text.clone().unwrap_or_else(|| prompt.text.clone()), delivery, files: self.attach.files.clone() };
        if !self.inbox_write(Write::Send { session: session.to_owned(), prompt }) {
            return;
        }
        model::set_user_files(&id, std::mem::take(&mut self.attach.files));
        self.inboxes.entry(session.to_owned()).or_default().upsert(item);
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
        // An item listed by the server has its files only on the item.
        let files = model::user_files(&item.id);
        self.attach.files.extend(if files.is_empty() { item.files } else { files.to_vec() });
        model::set_user_files(&item.id, Vec::new());
    }

    /// Fixture runs act locally, as if the server agreed.
    fn inbox_write(&mut self, w: Write) -> bool {
        matches!(self.source, Source::Fixture) || self.write(w)
    }

    /// `session.inbox.*`: a delivered prompt joins the end of the transcript, where the server
    /// puts it.
    pub fn apply_inbox(&mut self, ty: &str, data: &serde_json::Value, sid: &str) -> bool {
        let inbox = self.inboxes.entry(sid.to_owned()).or_default();
        let change = inbox.apply(ty, data);
        // The server admits every prompt to its inbox, a send to an idle session too. That one
        // already shows as its prompt, which is where it stays, so it never waits as a steer.
        if let Some(t) = self.transcripts.get(sid) {
            inbox.items.retain(|i| !t.messages.iter().any(|m| m.id() == i.id));
        }
        match change {
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

/// What a pending steer's bubble says: its words, or its attachment count when it has none.
pub fn pending_label(item: &InboxItem) -> String {
    let files = model::user_files(&item.id).len().max(item.files.len());
    if item.text.trim().is_empty() { attachments_label(files) } else { item.text.clone() }
}

/// A pending steer's Move to queue and Delete, where a prompt's copy button goes (see
/// `view::user_bubble`): beside the bubble while hovered inside a run (`beside`), or on the
/// line under the run's last steer, marked Pending.
pub fn pending_actions(p: &Pal, session: &str, id: &str, beside: bool) -> El {
    let (sq, iq) = (session.to_owned(), id.to_owned());
    let (sd, id) = (session.to_owned(), id.to_owned());
    let buttons = row()
        .key("pending-actions")
        .gap(4.0)
        .center_y()
        .child(icon_button(p, "move-to-queue", "Move to queue", &TO_QUEUE, 20.0, 14.0, move |s: &mut App| s.queue_pending(&sq, &iq)))
        .child(icon_button(p, "delete-pending", "Delete", &ic::TRASH, 20.0, 14.0, move |s: &mut App| {
            s.withdraw(&sd, &id);
        }));
    if beside {
        return buttons.group_opacity(0.0, 1.0);
    }
    row()
        .key("pending-meta")
        .h(20.0)
        .gap(4.0)
        .center_y()
        .child(icon(&STEER, 14.0).color(p.icon_faint))
        .child(text("Pending").font(UI).color(p.faint))
        .child(buttons)
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
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            f(s);
        })
        .child(icon(glyph, size).color(p.icon_muted))
}

/// How far the queue tucks under the composer's top edge. Its shadow, like the composer's,
/// shows through the composer's translucent ring; 8 pt is where the queue's shadow fading out
/// meets the composer's, so the sides keep one tone across the divider.
const TUCK: f32 = 8.0;
/// How far below the composer's top edge the closed queue's face rests.
const SINK: f32 = 6.0;

/// The queue: one shape with the composer, its square bottom tucked under the composer's top
/// edge. Empty, it is a line along that edge, so it opens and closes as its height springs and
/// every queued prompt comes and goes through the composer.
///
/// The composer's ring is translucent and shows what's behind it, so the queue's own ring
/// stops at the composer's top edge (`queue-clip` cuts its sides and drops its bottom edge)
/// and only its fill tucks under, inset clear of the composer's ring. It casts the composer's
/// shadow too, so both rings sit over the same backdrop. The two read as one outline, with the
/// composer's top edge as the one divider.
pub fn panel(s: &App, p: &Pal, session: &str, busy: bool) -> El {
    let queued: Vec<&InboxItem> = s.inbox_for(session).map(|i| i.queued().collect()).unwrap_or_default();
    let shadow = if queued.is_empty() { p.shadow.alpha(0.0) } else { p.shadow };
    let shell = col().key("queue").w_full().max_w(crate::view::CONTENT_W).corners([12.0, 12.0, 0.0, 0.0]).shadow(1.0, 2.0, shadow);
    let face = col().key("queue-face").w_full().mb(-1.0).corners([12.0, 12.0, 0.0, 0.0]).fill(crate::view::composer_fill(p)).border(0.5, p.ring);
    let clip = col().key("queue-clip").w_full().clip_soft(0.0);
    let tuck = el().key("queue-tuck").mx(0.5).fill(crate::view::composer_fill(p));
    if queued.is_empty() {
        // Closed, the face rests below the composer's top edge, so closing it passes behind
        // that edge rather than creeping up to it as a second line.
        return shell.h(0.0).child(clip.h(0.0).child(face.mt(SINK).h(1.0))).child(tuck.h(0.0));
    }
    let n = queued.len();
    let mut list = col().key("queue-list").w_full();
    for (k, item) in queued.iter().enumerate() {
        // While the turn is stopped the queue is parked: the first prompt shows its actions,
        // labelled Send, which is how the queue resumes.
        list = list.child(queue_row(p, session, item, busy, !busy && k == 0));
    }
    let list_h = (n as f32 * ROW_H).min(MAX_LIST_H);
    // 1 pt of the face hangs below the clip, so its padding is 1 pt more than what shows.
    let mut face = face.px(6.0).pt(6.0).pb(if n > 4 { 1.0 } else { 7.0 });
    if n > 3 {
        face = face.child(row().key("queue-count").px(6.0).pt(4.0).pb(1.0).child(text(format!("{n} QUEUED")).font(LABEL).color(p.muted)));
    }
    let list = if n > 4 {
        col().key("queue-scroll").w_full().h(list_h).scroll(rune::ScrollSpec { stick_bottom: false }).clip_fade(0.0, 8.0, 0.0, 12.0).child(list)
    } else {
        list
    };
    shell.role("list").label("Queued prompts").mb(-TUCK).child(clip.child(face.child(list))).child(tuck.h(TUCK))
}

/// Whether the open session has prompts in the queue, which squares the composer's top corners.
pub fn docked(s: &App) -> bool {
    matches!(&s.route, crate::Route::Session(id) if s.inbox_for(id).is_some_and(|i| i.queued().next().is_some()))
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
    let mut left = row().grow(1.0).shrink().gap(16.0).center_y().pl(6.0).child(text(label).key("label").font(UI).color(p.text).truncate());
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
        // It leaves at once, and the queue closes up on its height spring.
        .transition(rune::Transition::opacity().springs(crate::view::no_motion()))
        .child(left)
        .child(
            reveal(row().key("actions").gap(6.0).center_y())
                .child(steer)
                .child(icon_button(p, "undo", "Undo", &UNDO, 24.0, 16.0, move |s: &mut App| s.undo_queued(&su, &iu)))
                .child(icon_button(p, "remove", "Remove", &XMARK, 24.0, 16.0, move |s: &mut App| {
                    s.withdraw(&sr, &ir);
                })),
        )
}

/// "Queue ⌘⏎" (or "Steer ⌘⏎") beside the submit button while a turn runs and the composer holds
/// a prompt: submits with the delivery Enter doesn't use. It sits in the toolbar's free `room`,
/// which it never widens, as the word and the keys, the keys alone, or nothing: in a narrow
/// window it can't push the mic and Send aside.
pub fn alternate_hint(p: &Pal, room: f32) -> Option<El> {
    // The two keycaps and the gap between them, its padding and its right margin.
    let caps: f32 = ["⌘", "⏎"].iter().map(|k| (rune::text::measure(k, &[], UI, None).0 + 6.0).max(16.0)).sum();
    let keys = caps + 6.0 + 12.0 + 4.0;
    let word = rune::text::measure("Queue", &[], UI_MED, None).0 + 6.0;
    if room < keys {
        return None;
    }
    let hint = row()
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
        .transition(rune::Transition::opacity())
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.submit(true, tx))
        .children((room >= keys + word).then(|| text("Queue").font(UI_MED).color(p.faint)))
        .child(keycap(p, "⌘"))
        .child(keycap(p, "⏎"));
    Some(hint)
}

fn keycap(p: &Pal, key: &str) -> El {
    row().h(16.0).min_w(16.0).px(3.0).center().radius(3.0).fill(p.layer2).child(text(key.to_owned()).font(UI).color(p.faint))
}

/// The `queue*` scenes: the open session busy with `n` prompts queued, over the question dock
/// when `ask`.
pub fn scene(s: &mut App, n: usize, ask: bool) {
    crate::scenes::busy_scene(s);
    let crate::Route::Session(id) = s.route.clone() else { return };
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

#[cfg(test)]
#[path = "queue_edges_tests.rs"]
mod edges_tests;

#[cfg(test)]
mod tests {
    use rune::{Env, Key, KeyEvent, Mods, Runtime, TextEdit};

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

    fn settled_busy_app() -> Runtime<App> {
        let mut rt = busy_app();
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    fn tap(rt: &mut Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    }

    /// Types `body` into the composer through real input and presses Enter (⌘⏎ when `queue`).
    fn type_and_submit(rt: &mut Runtime<App>, body: &str, queue: bool) -> String {
        tap(rt, "prompt");
        rt.frame(1.0 / 60.0);
        rt.key(KeyEvent { key: Key::Text(body.into()), mods: Default::default() });
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.composer.text, body, "typed into the composer");
        rt.key(KeyEvent { key: Key::Enter, mods: Mods { cmd: queue, ..Default::default() } });
        rt.frame(1.0 / 120.0);
        assert!(rt.state.composer.text.is_empty(), "the composer clears: {:?} {}", rt.state.composer.text, rt.state.status);
        let inbox = rt.state.inboxes.values().find(|i| !i.items.is_empty()).expect("the prompt waits in the inbox");
        inbox.items.last().unwrap().id.clone()
    }

    /// The drawn rect of the text keyed `key`; exactly one copy shows.
    fn text_rect(rt: &Runtime<App>, key: &str) -> Option<([f32; 4], [f32; 4])> {
        let found: Vec<_> = rt.elements().into_iter().filter(|e| !e.exiting && e.key.as_deref() == Some(key)).collect();
        assert!(found.len() <= 1, "{key} shows {} times", found.len());
        found.first().map(|e| (e.rect, e.target))
    }

    /// Frames each prompt's text through `frames` frames of 120 Hz: every frame at its own
    /// full size, and one monotonic path (never back, never a one-frame snap).
    fn trace(rt: &mut Runtime<App>, keys: &[String], frames: usize) -> Vec<Vec<[f32; 4]>> {
        let mut drawn = vec![Vec::new(); keys.len()];
        for f in 0..frames {
            // Frame 0 is the one Enter committed in.
            if f > 0 {
                rt.frame(1.0 / 120.0);
            }
            for (k, key) in keys.iter().enumerate() {
                let (rect, target) = text_rect(rt, key).unwrap_or_else(|| panic!("frame {f}: {key} shows"));
                assert!(
                    (rect[2] - target[2]).abs() < 0.01 && (rect[3] - target[3]).abs() < 0.01,
                    "frame {f}: {key} drawn at {:.0}% size: {rect:?} vs {target:?}",
                    100.0 * rect[3] / target[3]
                );
                drawn[k].push(rect);
            }
        }
        for (key, rects) in keys.iter().zip(&drawn) {
            for axis in 0..2 {
                let at: Vec<f32> = rects.iter().map(|r| r[axis]).collect();
                let total = at[at.len() - 1] - at[0];
                for w in at.windows(2) {
                    let step = w[1] - w[0];
                    assert!(step * total.signum() >= -0.01, "{key} turns back: {at:?}");
                    assert!(step.abs() <= total.abs().max(20.0) / 10.0, "{key} snaps {step} pt in a frame: {at:?}");
                }
            }
        }
        drawn
    }

    /// Where the composer's words start.
    fn typed_at(rt: &Runtime<App>) -> [f32; 4] {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("prompt")).unwrap().rect
    }

    /// The first frame of a flight out of the composer: its words a frame from the typed ones.
    fn leaves_composer(rt: &Runtime<App>, rects: &[[f32; 4]]) {
        let (typed, first) = (typed_at(rt), rects[0]);
        assert!((first[0] - typed[0]).abs() < 5.0 && (first[1] - typed[1]).abs() < 5.0, "leaves from the composer: {first:?} vs {typed:?}");
        let last = rects[rects.len() - 1];
        let land = rects.iter().position(|r| (r[0] - last[0]).abs() < 0.5 && (r[1] - last[1]).abs() < 0.5).unwrap();
        assert!(land >= 24, "flies over {land} frames");
    }

    #[test]
    fn a_steer_rises_whole_at_full_size_and_keeps_its_identity_when_delivered() {
        let mut rt = settled_busy_app();
        let first = type_and_submit(&mut rt, "Use tabs, not spaces, everywhere in the parser module please", false);
        let a = format!("text-{first}");
        let drawn = trace(&mut rt, std::slice::from_ref(&a), 90);
        leaves_composer(&rt, &drawn[0]);
        let ys: Vec<f32> = drawn[0].iter().map(|r| r[1]).collect();
        assert!(ys[0] > ys[ys.len() - 1] + 20.0, "it rises into place: {ys:?}");

        // A second Enter stacks a second steer under the first; both travel on one path.
        let second = type_and_submit(&mut rt, "And keep the cap at 30s.", false);
        let b = format!("text-{second}");
        let drawn = trace(&mut rt, &[a.clone(), b.clone()], 90);
        leaves_composer(&rt, &drawn[1]);
        let ys: Vec<f32> = drawn[1].iter().map(|r| r[1]).collect();
        assert!(ys[0] > ys[ys.len() - 1] + 20.0, "the second rises into place: {ys:?}");

        // The server takes the first: it stays the same element, where it was, at full size.
        let sid = rt.state.inboxes.iter().find(|(_, i)| !i.items.is_empty()).unwrap().0.clone();
        let before = text_rect(&rt, &a).unwrap().0;
        rt.update(|s, _| {
            s.apply_event("session.inbox.delivered", &serde_json::json!({"sessionID": sid, "inboxID": first}));
        });
        let drawn = trace(&mut rt, &[a.clone(), b], 90);
        let travel = drawn[0].iter().map(|r| (r[1] - before[1]).abs()).fold(0.0, f32::max);
        assert!(travel < 1.0, "the delivered prompt stays put: moved {travel} pt");
    }

    #[test]
    fn stacked_steers_share_borders_and_keep_their_own_actions() {
        let mut rt = settled_busy_app();
        let a = type_and_submit(&mut rt, "Use tabs, not spaces.", false);
        let b = type_and_submit(&mut rt, "And keep the cap at 30s.", false);
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
        }
        let els = rt.elements();
        let inside = |row: &str, key: &str| {
            els.iter().find(|e| !e.exiting && e.path.contains(&format!("u-{row}/")) && e.key.as_deref() == Some(key)).cloned()
        };
        let (ba, bb) = (inside(&a, "bubble").unwrap(), inside(&b, "bubble").unwrap());
        assert!((bb.rect[1] - (ba.rect[1] + ba.rect[3]) - 4.0).abs() < 0.5, "4 apart: {:?} {:?}", ba.rect, bb.rect);
        assert!(inside(&a, "pending-meta").is_none() && inside(&b, "pending-meta").is_some(), "only the run's last says Pending");
        let working = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some("working")).unwrap();
        assert!(working.rect[1] > bb.rect[1] + bb.rect[3], "Working stays under the steers");

        // Hovering the first shows its own actions beside it; its Delete removes only it.
        let actions = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| !e.exiting && e.path.contains(&format!("u-{a}/")) && e.key.as_deref() == Some("pending-actions")).unwrap();
        assert!(actions(&rt).opacity < 0.01, "hidden until hovered");
        let (x, y) = (ba.rect[0] + ba.rect[2] / 2.0, ba.rect[1] + ba.rect[3] / 2.0);
        rt.pointer_move(x, y);
        for _ in 0..30 {
            rt.frame(1.0 / 120.0);
        }
        assert!(actions(&rt).opacity > 0.99, "shown while hovered");
        let delete = rt.elements().into_iter().find(|e| !e.exiting && e.path.contains(&format!("u-{a}/")) && e.key.as_deref() == Some("delete-pending")).unwrap();
        let (x, y) = (delete.rect[0] + delete.rect[2] / 2.0, delete.rect[1] + delete.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 120.0);
        let left: Vec<String> = rt.state.inboxes.values().flat_map(|i| i.items.iter().map(|i| i.id.clone())).collect();
        assert_eq!(left, vec![b]);
    }

    #[test]
    fn a_queued_prompt_shows_at_full_size() {
        let mut rt = settled_busy_app();
        let id = type_and_submit(&mut rt, "then update the docs", true);
        let row = format!("queued-{id}");
        let mut rects = Vec::new();
        for f in 0..90 {
            if f > 0 {
                rt.frame(1.0 / 120.0);
            }
            let e = rt.elements().into_iter().find(|e| !e.exiting && e.path.contains(&row) && e.text.as_deref() == Some("then update the docs"));
            let e = e.unwrap_or_else(|| panic!("frame {f}: the queued row shows"));
            assert!((e.rect[3] - e.target[3]).abs() < 0.01 && e.scale == 1.0, "frame {f}: full size: {:?} {}", e.rect, e.scale);
            rects.push(e.rect);
        }
        leaves_composer(&rt, &rects);
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

    fn element(rt: &Runtime<App>, key: &str) -> rune::ElementInfo {
        rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
    }

    /// The queue's top edge, frame by frame, with the composer's top-left radius.
    fn dock_edges(rt: &mut Runtime<App>, frames: usize) -> Vec<(f32, f32)> {
        (0..frames)
            .map(|f| {
                if f > 0 {
                    rt.frame(1.0 / 120.0);
                }
                let queue = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("queue"));
                let composer = element(rt, "composer");
                (queue.map_or(composer.rect[1], |q| q.rect[1]), composer.radius)
            })
            .collect()
    }

    fn monotonic(at: &[f32], what: &str) {
        let total = at[at.len() - 1] - at[0];
        for w in at.windows(2) {
            assert!((w[1] - w[0]) * total.signum() >= -0.01, "{what} turns back: {at:?}");
            assert!((w[1] - w[0]).abs() <= total.abs().max(20.0) / 8.0, "{what} jumps: {at:?}");
        }
    }

    #[test]
    fn the_queue_and_the_composer_are_one_shape() {
        let mut rt = settled_busy_app();
        type_and_submit(&mut rt, "then update the docs", true);
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
        }
        let (queue, composer) = (element(&rt, "queue"), element(&rt, "composer"));
        assert!((queue.rect[0] - composer.rect[0]).abs() < 0.5 && (queue.rect[2] - composer.rect[2]).abs() < 0.5, "same edges: {:?} {:?}", queue.rect, composer.rect);
        assert!(queue.rect[1] + queue.rect[3] >= composer.rect[1] + super::TUCK - 0.5, "tucked under the composer's top edge: {:?} {:?}", queue.rect, composer.rect);
        assert_eq!(element(&rt, "queue-face").fill, composer.fill, "the composer's surface");
        assert!(composer.radius < 0.5, "the composer's top corners meet the queue square: {}", composer.radius);
    }

    #[test]
    fn the_queue_rises_from_behind_the_composer() {
        let mut rt = settled_busy_app();
        let top = element(&rt, "composer").rect[1];
        type_and_submit(&mut rt, "then update the docs", true);
        let edges: Vec<f32> = dock_edges(&mut rt, 90).iter().map(|e| e.0).collect();
        assert!(edges[0] >= top - 1.0, "starts hidden behind the composer: {edges:?}");
        assert!(edges[edges.len() - 1] < top - 30.0, "opens: {edges:?}");
        monotonic(&edges, "the queue's edge");
        let land = edges.iter().position(|y| (y - edges[edges.len() - 1]).abs() < 0.5).unwrap();
        assert!(land >= 24, "opens over {land} frames: {edges:?}");
    }

    #[test]
    fn the_last_delivery_closes_the_queue_back_behind_the_composer() {
        let mut rt = settled_busy_app();
        let top = element(&rt, "composer").rect[1];
        let id = type_and_submit(&mut rt, "then update the docs", true);
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
        }
        let sid = rt.state.inboxes.iter().find(|(_, i)| !i.items.is_empty()).unwrap().0.clone();
        let at = rt.state.clock;
        rt.state.playback.push_back((at, "session.inbox.delivered".into(), serde_json::json!({"sessionID": sid, "inboxID": id})));
        let closing = dock_edges(&mut rt, 90);
        let edges: Vec<f32> = closing.iter().map(|e| e.0).collect();
        assert!(edges[0] < top - 30.0, "still open the frame it's delivered: {edges:?}");
        monotonic(&edges, "the queue's edge");
        let land = edges.iter().position(|y| (y - edges[edges.len() - 1]).abs() < 0.5).unwrap();
        assert!(land >= 24, "closes over {land} frames: {edges:?}");
        assert!(edges[edges.len() - 1] >= top - 0.5, "closes behind the composer: {edges:?}");
        assert!((closing[closing.len() - 1].1 - 12.0).abs() < 0.5, "the composer's corners round again: {closing:?}");
        assert!(!rt.elements().iter().any(|e| e.path.contains(&format!("queued-{id}")) && e.opacity > 0.01), "its row doesn't linger");
    }

    #[test]
    fn a_delivered_queued_prompt_flies_from_its_row_into_the_timeline() {
        let mut rt = settled_busy_app();
        let id = type_and_submit(&mut rt, "then update the docs", true);
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
        }
        let row = rt.elements().into_iter().find(|e| !e.exiting && e.path.contains(&format!("queued-{id}")) && e.text.as_deref() == Some("then update the docs")).expect("its row").rect;
        let sid = rt.state.inboxes.iter().find(|(_, i)| !i.items.is_empty()).unwrap().0.clone();
        rt.update(|s, _| s.playback.push_back((s.clock, "session.inbox.delivered".into(), serde_json::json!({"sessionID": sid, "inboxID": id}))));
        rt.frame(1.0 / 120.0);
        let drawn = trace(&mut rt, &[format!("text-{id}")], 90);
        let (first, last) = (drawn[0][0], drawn[0][89]);
        assert!((first[0] - row[0]).abs() < 5.0 && (first[1] - row[1]).abs() < 5.0, "leaves from its row: {first:?} vs {row:?}");
        assert!(first[1] > last[1] + 20.0, "rises into the timeline: {first:?} to {last:?}");
        let land = drawn[0].iter().position(|r| (r[0] - last[0]).abs() < 0.5 && (r[1] - last[1]).abs() < 0.5).unwrap();
        assert!(land >= 24, "flies over {land} frames");
    }
}
