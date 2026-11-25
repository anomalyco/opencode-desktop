//! Queue tray above the composer and alternate-delivery hint (`queue-panel.tsx`, `editor.tsx`).
//!
//! All state queries and pointer/keyboard handlers operate on `&dyn QueueHost` / `&mut dyn QueueHost`.
//! Generic `H: QueueHost + 'static` bounds appear strictly on `rune::El` view wrappers and delegate
//! 1-line to non-generic functions.

use std::rc::Rc;

use opencode_kit::model::{self, InboxItem};
use opencode_ui::keymap::{Bind, LABEL as KEYMAP_LABEL, chips as keymap_chips};
use rune::{El, Icon, Key, KeyEvent, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use opencode_ui::odometer::odometer;
use opencode_ui::palette::Pal;
use opencode_ui::style::{UI, UI_MED};

use super::QueueHost;
use super::drag;
use super::machine::{Queue, Row as QueueRow};

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
/// How far the tray runs under the composer card (`-mb-3`, `queue-panel.tsx:93`).
const TUCK: f32 = 12.0;
/// Past this many rows the queue builds only those in sight of its scroll.
const VIRTUAL_PAST: usize = 12;
/// The handle's width: its padding either side of two dot columns 2 apart.
const HANDLE_W: f32 = 4.0 + 2.0 + 2.0 + 2.0 + 4.0;

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

/// A queued prompt's words as its row shows them: on one line, as a `truncate` button collapses
/// its whitespace.
pub(crate) fn row_text(item: &InboxItem) -> String {
    item.text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How many attachments a queued prompt carries: its files, the files it names by path, and its
/// quotes (`queuedPromptRows`).
pub(crate) fn row_files(item: &InboxItem) -> usize {
    let files = model::inbox_payload(&item.id).and_then(|p| p["files"].as_array().map(Vec::len)).unwrap_or(0);
    let files = files.max(model::user_files(&item.id).len()).max(item.files.len());
    files + opencode_kit::mention::user_path_attachments(&item.id).len() + model::user_comments(&item.id).len()
}

fn attachments_label(n: usize) -> String {
    if n == 1 { "1 attachment".into() } else { format!("{n} attachments") }
}

/// The queue: Desktop's tray above the composer (`queue-panel.tsx:88-158`). The tray is the
/// page's own colour (`bg-v2-background-bg-base`) inside a 0.5 pt `border-border-base` ring with
/// 10 pt corners (`rounded-xl`: the app's `--radius-xl` is 0.625rem), 6 pt in on the sides and
/// top; its bottom runs 12 pt under the composer, which stays its own rounded card on top
/// (`relative z-10`), so 6 pt of tray show under the last row (`pb-[18px]`), or none past four
/// rows, where the list scrolls (`pb-3`). Empty, it closes to nothing on its height spring,
/// clipped as it goes (`Grow`), and the composer comes down to where it was.
///
/// A question or permission in the composer's place closes it, as Desktop hides its queue while
/// a dock blocks the composer (`SessionQueuePanel hidden={blocked}`); it opens again after.
pub fn panel<H: QueueHost + 'static>(host: &H, p: &Pal, session: &str, busy: bool) -> El {
    let rows = shown_rows(host, session);
    let shell = col().key("queue").w_full().max_w(opencode_ui::style::dock_w()).clip_soft(0.0);
    if rows.is_empty() {
        return shell.h(0.0);
    }
    // The frame belongs to the room, not the tray: as a delivery takes the tray's height the
    // rows animate inside a frame that keeps its shape.
    let shell = shell.radius(10.0).fill(p.bg_base).box_shadow(Pal::ring_in(p.border));
    let n = rows.len();
    let ops = host.queue_ops().get(session);
    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let list = if n > VIRTUAL_PAST {
        // A long queue builds the rows in sight of its scroll (a virtual list). Each list row is a
        // box around the queue's row, which keeps its key and the 1 pt under it (a list row's own
        // margin isn't part of its height), so a drag and a send find it as before.
        let keys: Rc<Vec<String>> = Rc::new(rows.iter().map(|r| format!("{}-row", row_key(ops, r))).collect());
        let (rows, ids, session_id, pal) = (Rc::new(rows), Rc::new(ids), session.to_owned(), *p);
        let version = keys.clone();
        col()
            .key("queue-list")
            .w_full()
            .growth(rune::GrowthSpec::default())
            .virtual_rows(
                n,
                ROW_H,
                move |_: &H, k| keys[k].clone().into(),
                move |s: &H, k| el().w_full().child(queue_row::<H>(&pal, &session_id, &rows[k], &ids, row_state(s, &session_id, &rows[k], k, busy))),
            )
            .virtual_rows_version(version)
    } else {
        let mut list = col().key("queue-list").w_full();
        for (k, r) in rows.iter().enumerate() {
            list = list.child(queue_row::<H>(p, session, r, &ids, row_state(host, session, r, k, busy)));
        }
        list
    };
    let list_h = (n as f32 * ROW_H).min(MAX_LIST_H);
    let mut tray = col()
        .key("queue-tray")
        .w_full()
        .px(6.0)
        .pt(6.0)
        .pb(if n > 4 { TUCK } else { TUCK + 6.0 });
    if n > 3 {
        tray = tray.child(row().key("queue-count").px(6.0).pt(4.0).pb(1.0).child(odometer(
            format!("queue-count-{session}"),
            opencode_i18n::plural!("session.queue.count", n).to_uppercase(),
            n as f64,
            LABEL,
            p.muted,
        )));
    }
    let list = if n > 4 {
        col()
            .key("queue-scroll")
            .w_full()
            .h(list_h)
            .scroll(rune::ScrollSpec { stick_bottom: false })
            .clip_fade_linear(0.0, 8.0, 0.0, 12.0)
            .child(list)
    } else {
        list
    };
    shell.role("list").label_static(opencode_i18n::t!("rune.queue.label")).mb(-TUCK).child(tray.child(list))
}

/// "Queue ⌘⏎" (or "Steer ⌘⏎") beside the submit button while a turn runs and the composer holds
/// a prompt: submits with the delivery Enter doesn't use. The keys show only in a window at least
/// 640 wide (`sm`; `hidden sm:block`, `editor.tsx:1456-1458`). It sits in the toolbar's free
/// `room`, which it never widens, as the word and the keys, the word alone, or nothing: in a
/// narrow window it can't push the mic and Send aside.
///
/// When `has_queue` (queued prompts exist and none is open for edit), its tooltip lists both the
/// alternate shortcut and `Alt+Enter` "Queue first" (`editor.tsx:1471-1495`, `#63109`).
pub fn alternate_hint<H: QueueHost + 'static>(p: &Pal, room: f32, sm: bool, steer: bool, has_queue: bool) -> Option<El> {
    let (word, labels) = alternate_hint_spec(room, sm, steer)?;
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
        .on_press_tx(|s: &mut H, tx: &mut Tx| on_alternate_press(s, tx))
        .child(text(word).font(UI_MED).color(p.faint))
        // The keys sit in a block `span` (`hidden sm:block`) whose line box sets the inline-flex
        // `Keybind` on its baseline, 2 pt below the button's centre (measured at a true 1x).
        .children((!labels.is_empty()).then(|| keymap_chips(p, labels).offset(0.0, 2.0)));
    // `inactive={delivery !== "steer" && !view.submit.queue?.count()}` (`editor.tsx:1474`).
    let hint = if has_queue {
        hint.help(word)
            .help_keys(Bind::SubmitAlternate.keycaps())
            .help_row(opencode_i18n::t!("session.queue.first"))
            .help_keys(Bind::SubmitFirst.keycaps())
            .help_above()
    } else if steer {
        hint.help(opencode_i18n::t!("ui.promptInput.steerHint")).help_above()
    } else {
        hint
    };
    Some(hint)
}

fn alternate_hint_spec(room: f32, sm: bool, steer: bool) -> Option<(&'static str, Vec<String>)> {
    let word = if steer { opencode_i18n::t!("ui.promptInput.steer") } else { opencode_i18n::t!("ui.promptInput.queue") };
    // Its padding and right margin, the word, and the keycaps 6 after it (`gap-1.5`), each
    // Desktop's neutral `Keybind` chip 2 apart (`keymap::chips`).
    let labels: Vec<String> = Bind::SubmitAlternate.keycaps().iter().map(|k| k.to_uppercase()).collect();
    let caps: f32 = labels.iter().map(|k| (rune::text::measure(k, &[], KEYMAP_LABEL, None).0 + 8.0).max(14.0)).sum();
    let keys = caps + 6.0 + 2.0 * labels.len().saturating_sub(1) as f32;
    let width = rune::text::measure(word, &[], UI_MED, None).0 + 12.0 + 10.0;
    if room < width {
        return None;
    }
    let labels = if sm && room >= width + keys { labels } else { Vec::new() };
    Some((word, labels))
}

fn on_alternate_press(host: &mut dyn QueueHost, tx: &mut Tx) {
    host.submit_alternate(tx);
}

fn on_edit_press(host: &mut dyn QueueHost, session: &str, id: &str, tx: &mut Tx) {
    tx.instant();
    super::edit_queued(host, session, id);
}

fn on_handle_drag(host: &mut dyn QueueHost, session: &str, id: &str, ids: &[String], d: &mut rune::Drag, tx: &mut Tx) {
    let effect = match d.phase {
        rune::Phase::Down => host.queue_drag_mut().step(drag::Event::Down { session, id }),
        rune::Phase::Move => {
            tx.animated();
            host.queue_drag_mut().step(drag::Event::Move { dx: d.dx, dy: d.dy, ids })
        }
        rune::Phase::Up => {
            tx.instant();
            host.queue_drag_mut().step(drag::Event::Up { cancelled: d.cancelled() })
        }
    };
    if let Some(drag::Effect::Reorder { session, ids }) = effect {
        super::reorder_queued(host, &session, ids);
    }
}

fn on_handle_key(host: &mut dyn QueueHost, session: &str, id: &str, ids: &[String], ev: &KeyEvent, tx: &mut Tx) -> bool {
    let lifted = host.queue_drag().active(session);
    let event = match ev.key {
        Key::Text(ref t) if t == " " && !lifted => drag::Event::Pick { session, id, ids },
        Key::Enter if !lifted => drag::Event::Pick { session, id, ids },
        Key::Text(ref t) if t == " " => drag::Event::Drop,
        Key::Enter | Key::Tab if lifted => drag::Event::Drop,
        Key::Up if lifted => drag::Event::Step(-1),
        Key::Down if lifted => drag::Event::Step(1),
        Key::Escape if lifted => drag::Event::Cancel,
        _ => return false,
    };
    tx.animated();
    if let Some(drag::Effect::Reorder { session, ids }) = host.queue_drag_mut().step(event) {
        tx.instant();
        super::reorder_queued(host, &session, ids);
    }
    true
}

/// A row's key: its prompt's, as it was first queued while a change moves it.
fn row_key(ops: Option<&Queue>, r: &QueueRow) -> String {
    format!("queued-{}", ops.map_or_else(|| r.id.clone(), |q| q.origin(&r.id)))
}

/// Row `k`'s state as it stands. While the turn is stopped the queue is parked: the first prompt
/// shows its actions, labelled Send, which is how the queue resumes.
fn row_state(host: &dyn QueueHost, session: &str, r: &QueueRow, k: usize, busy: bool) -> RowState {
    let ops = host.queue_ops().get(session);
    RowState {
        busy,
        parked: !busy && k == 0,
        editing: ops.and_then(|q| q.editing()) == Some(r.id.as_str()),
        changing: ops.is_some_and(Queue::busy),
        lifted: host.queue_drag().lifted(session, &r.id),
        dragging: host.queue_drag().active(session),
        offset: host.queue_drag().offset(session, k),
        key: row_key(ops, r),
        index: k,
    }
}

/// The queue's rows (`queue-machine.ts` `selectQueueRows`); none while a dock covers the composer.
fn shown_rows(host: &dyn QueueHost, session: &str) -> Vec<QueueRow> {
    if host.dock_shown() {
        return Vec::new();
    }
    super::rows(host, session)
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

/// When a row's actions show: always (parked, open in the composer, pending, or a pointer that
/// can't hover), with the row hovered or focused, or never (lifted by a drag).
#[derive(Clone, Copy, PartialEq)]
enum Shown {
    Always,
    OnHover,
    Never,
}

/// The rows a drag passes step aside on dnd-kit's sortable transition (`250ms
/// cubic-bezier(0.25, 1, 0.5, 1)`).
fn sort_motion() -> rune::Anim {
    rune::Anim::new(rune::Curve::tween(0.25, rune::Easing::cubic(0.25, 1.0, 0.5, 1.0)))
}

fn icon_button<H: QueueHost + 'static>(p: &Pal, key: &str, label: &str, glyph: &'static Icon, side: f32, size: f32, f: impl Fn(&mut H) + 'static) -> El {
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
        .on_press(move |s: &mut H| f(s))
        .child(icon(glyph, size).color(p.icon_muted))
}

/// A row (`queue-panel.tsx:194-300`): the drag handle, the words (a click opens them in the
/// composer), the attachment count, and the actions, which show on hover, always on the parked
/// first row and while the row is open in the composer. While the actions hide, the words run on
/// to the row's end, where Desktop stops them at the hidden actions. A row whose change is on its
/// way shows its words faint and a loader in place of its actions.
fn queue_row<H: QueueHost + 'static>(p: &Pal, session: &str, r: &QueueRow, ids: &[String], st: RowState) -> El {
    let busy = st.busy;
    let label = if r.text.is_empty() { attachments_label(r.files) } else { r.text.clone() };
    let (ss, is) = (session.to_owned(), r.id.clone());
    let (su, iu) = (session.to_owned(), r.id.clone());
    let (sr, ir) = (session.to_owned(), r.id.clone());
    let (se, ie) = (session.to_owned(), r.id.clone());
    // A lifted row takes no hover: dnd-kit turns off the pointer for what it drags.
    let shown = match (r.pending || st.parked || st.editing, st.lifted) {
        (true, _) => Shown::Always,
        (false, true) => Shown::Never,
        (false, false) if opencode_ui::style::hover_none() => Shown::Always,
        (false, false) => Shown::OnHover,
    };
    let reveal = |e: El| match shown {
        Shown::Always => e,
        Shown::Never => e.opacity(0.0),
        Shown::OnHover => e.group_opacity(0.0, 1.0),
    };
    let color = if st.editing || r.pending { p.faint } else { p.text };
    let steer_label = if busy { opencode_i18n::t!("session.queue.steer") } else { opencode_i18n::t!("session.queue.send") };
    let steer = row()
        .key("steer")
        .role("button")
        .label(steer_label)
        // `button-v2` small, ghost-faint, with `gap-0.5 ps-1`.
        .h(24.0)
        .pl(4.0)
        .pr(9.0)
        .gap(2.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_press(move |s: &mut H| super::steer_queued(s, &ss, &is))
        .child(icon(&ic::ARROW_UP, 16.0).color(p.icon_muted))
        .child(text(steer_label).font(UI_MED).color(p.faint));
    // Its tooltip shows only while the turn runs (`inactive={!working}`); row 0 also shows the
    // empty-composer `Mod+Enter` shortcut (`queue-panel.tsx:256-267`, `#63111`).
    let steer = if busy {
        let s = steer.help(opencode_i18n::t!("session.queue.steerTooltip")).help_above();
        if st.index == 0 { s.help_keys(Bind::SubmitAlternate.keycaps()) } else { s }
    } else {
        steer
    };
    let words = row()
        .key("edit")
        .role("button")
        .label(label.clone())
        .self_start()
        .shrink()
        .min_w(0.0)
        // `rounded-sm`: the app's `--radius-sm` is 0.25rem.
        .radius(4.0)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| on_edit_press(s, &se, &ie, tx))
        .child(text(label.clone()).key("label").font(UI).color(color).truncate_fade())
        .child(marker(format!("edit-{}", st.index)));
    let words = if st.editing || r.pending { words } else { words.hover(p.hover).cursor(rune::Cursor::Text) };
    // While the actions hide they keep their room, so the words beside them would end short of
    // the row. A copy of the words draws in their place, running to the row's right padding; it
    // takes no input and says nothing to a screen reader, and gives way to the real words (bound
    // by the actions) on the same hover track that shows the actions.
    let full = (shown != Shown::Always).then(|| {
        let words = row().self_start().shrink().min_w(0.0).child(text(label).font(UI).color(color).truncate_fade());
        let full = row().key("full").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).pl(2.0).pr(4.0).gap(8.0).center_y().inert().a11y_hidden().child(el().w(HANDLE_W)).child(line(p, r, words));
        if shown == Shown::OnHover { full.group_opacity(1.0, 0.0) } else { full }
    });
    let words = reveal(line(p, r, words));
    let left = row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().child(handle::<H>(p, session, &r.id, ids, st.index, st.changing || r.pending)).child(words);
    let actions = if r.pending {
        row().key("actions").gap(4.0).center_y().child(opencode_ui::style::activity_ring_sized(p, 14.0))
    } else {
        let mut actions = reveal(row().key("actions").gap(4.0).center_y());
        if !st.editing {
            actions = actions
                .child(steer.child(marker(format!("steer-{}", st.index))))
                .child(icon_button::<H>(p, "undo", opencode_i18n::t!("session.queue.undo"), &UNDO, 24.0, 16.0, move |s| super::undo_queued(s, &su, &iu)).child(marker(format!("undo-{}", st.index))));
        }
        let actions = actions.child(icon_button::<H>(p, "remove", opencode_i18n::t!("session.queue.remove"), &XMARK, 24.0, 16.0, move |s| super::withdraw(s, &sr, &ir)).child(marker(format!("remove-{}", st.index))));
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
        .children(full)
        .child(left)
        .child(actions);
    // The lifted row follows the pointer at 60% over the others, which step aside for it.
    if st.lifted {
        row_el.opacity(0.6).raise().offset(0.0, st.offset).offset_motion(rune::Anim::INSTANT)
    } else {
        row_el.offset(0.0, st.offset).offset_motion(sort_motion())
    }
}

/// A row's words and, after them, its attachment count when it has words too.
fn line(p: &Pal, r: &QueueRow, words: El) -> El {
    let line = row().shrink().min_w(0.0).gap(16.0).center_y().child(words);
    if r.files > 0 && !r.text.is_empty() { line.child(text(attachments_label(r.files)).font(UI).color(p.muted)) } else { line }
}

/// An empty box over its parent, keyed by the row's place in the strip so scripts can find the
/// row; its parent keeps a key of its own, so a row that moves up keeps its elements.
fn marker(key: String) -> El {
    el().key(key).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
}

/// The drag handle (`queue-panel.tsx:206-215`): six 2 pt squares in two columns, 2 apart and
/// 2.25 down, inside 4 of padding, in `bg-layer-04`. The pointer drags it; the keys lift it from
/// focus.
fn handle<H: QueueHost + 'static>(p: &Pal, session: &str, id: &str, ids: &[String], index: usize, disabled: bool) -> El {
    let dot = || el().size(2.0, 2.0).fill(p.q.layer4);
    let column = || col().gap(2.25).child(dot()).child(dot()).child(dot());
    let h = row().key("reorder").role("button").label_static(opencode_i18n::t!("session.queue.reorder")).padding(4.0).gap(2.0).child(column()).child(column()).child(marker(format!("reorder-{index}")));
    if disabled {
        return h;
    }
    let (sd, idd, idsd) = (session.to_owned(), id.to_owned(), ids.to_vec());
    let (sk, idk, idsk) = (session.to_owned(), id.to_owned(), ids.to_vec());
    // No grab cursor: Desktop forces `.cursor-grab` back to the arrow (`packages/desktop/src/renderer/styles.css:4`).
    h.focusable(true)
        .on_drag(move |s: &mut H, d: &mut rune::Drag, tx: &mut Tx| on_handle_drag(s, &sd, &idd, &idsd, d, tx))
        .on_key(move |s: &mut H, ev: &KeyEvent, tx: &mut Tx| on_handle_key(s, &sk, &idk, &idsk, ev, tx))
}
