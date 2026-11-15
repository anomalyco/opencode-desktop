//! Work that runs on (`timeline-next/event-row.tsx`, as of anomalyco/opencode 0cfe2c2610,
//! #64017): a run of background shells read like a group's label, "Started 2 shells · Finished 1
//! shell", opening to each command; and a subagent's event, a row of its own: its agent's chip,
//! its task, and what happened, which opens its session.

use rune::{El, Tx, col, el, row, text};

use super::events::{self, Event};
use super::tag;
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{HOVER_FADE, ROW_GAP, UI, arrow};

/// The row of `events` in session `session`; `open` lists them, and a press on the
/// label runs `press`. Nothing when nothing in it has happened yet.
pub fn view<S: TimelineHost>(s: &S, p: &Pal, session: &str, events: &[Event], open: bool, press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let (buckets, failed) = events::summarize_shells(events);
    if buckets.is_empty() {
        return col();
    }
    let items: Vec<&Event> = events.iter().filter(|e| e.title.is_some()).collect();
    let mut words = row().key("words").gap(6.0).center_y().shrink().min_w(0.0);
    let mut label = Vec::new();
    for (k, b) in buckets.iter().enumerate() {
        if k > 0 {
            words = words.child(text("\u{00b7}").key(format!("sep-{k}")).font(UI).color(p.muted));
        }
        let detail = opencode_i18n::plural!("ui.timeline.event.shells", b.count);
        let verb = events::verb_word(b.verb);
        label.push(format!("{verb} {detail}"));
        words = words.child(text(verb).key(format!("verb-{k}")).font(UI).color(p.muted))
            .child(text(detail).key(format!("detail-{k}")).font(UI).color(p.text).shrink().min_w(0.0).truncate_fade());
    }
    // Where every one failed, the tag says so without counting them again.
    if failed > 0 {
        let words_failed = if failed == events.len() { opencode_i18n::t!("ui.timeline.event.allFailed").to_owned() } else { opencode_i18n::t!("ui.timeline.event.failed", count = failed) };
        words = words.child(tag(p, &words_failed, true));
    }
    let mut trigger = row().key("trigger").h(28.0).gap(4.0).center_y().shrink().min_w(0.0).child(words);
    if !items.is_empty() {
        let a = el().key("arrow").w(16.0).h(24.0).ml(2.0).center().child(arrow(p, open));
        trigger = trigger
            .role("button")
            .label(label.join(" · "))
            .expanded(open)
            .hover_group()
            .hover_motion(HOVER_FADE, HOVER_FADE)
            .on_press_tx(press)
            .child(if open || crate::style::hover_none() { a } else { a.group_opacity(0.0, 1.0) });
    }
    // The trigger is the row's own `trigger`, which a fold holds in place (`disclosure::toggle_group`).
    let c = col().w_full().child(trigger);
    if !open || items.is_empty() {
        return c;
    }
    // Rows 28 tall around a 16 pt line, 4 apart: 16 from text to text (`--timeline-gap-s`).
    let mut list = col().key("items").w_full().gap(ROW_GAP);
    for (k, e) in items.into_iter().enumerate() {
        list = list.child(item(s, p, session, k, e));
    }
    c.child(crate::disclosure::reveal(s.knobs(), crate::style::nested(p, ROW_GAP, list)))
}

/// One shell an events row lists (`TimelineEventItem`): "Shell", its command, and a Failed tag.
fn item<S: TimelineHost>(_s: &S, p: &Pal, _session: &str, k: usize, e: &Event) -> El {
    row().key(format!("item-{k}")).h(28.0).gap(6.0).center_y().shrink().min_w(0.0)
        .child(text(opencode_i18n::t!("ui.tool.shell")).key("name").font(UI).color(p.muted))
        .child(text(e.title.clone().unwrap_or_default()).key("title").font(UI).color(p.text).shrink().min_w(0.0).truncate_fade())
        .children(e.failed.then(|| tag(p, opencode_i18n::t!("ui.toolErrorCard.failed"), true)))
}

/// A subagent's event, a row of its own (`timeline-subagent`, `TimelineEventItem`): a neutral
/// chip naming its agent ("Explore Agent"), its task, and what happened right after it, muted
/// (Started, Updated, Completed, Stopped) or Failed in the danger colour. 28 tall around its
/// 16 pt line like the timeline's other rows; under the pointer it takes a faint fill 6 past
/// its words each side, its task underlines, and a press opens its session.
pub fn subagent<S: TimelineHost>(s: &S, p: &Pal, session: &str, e: &Event) -> El {
    let name = match e.agent.as_deref().filter(|a| !a.is_empty()) {
        Some(agent) => opencode_i18n::t!("ui.timeline.subagent.name", name = opencode_kit::model::capitalize(agent)),
        None => opencode_i18n::t!("ui.tool.agent.default").to_owned(),
    };
    let status = events::subagent_status(e);
    let title = e.title.clone().unwrap_or_default();
    let child = s.resolve_subagent(session, e.session.as_deref(), &title).map(str::to_owned).filter(|_| s.can_open_subagent());
    let words = |underline: bool| {
        let t = text(title.clone()).key("title").font(UI).color(p.text).truncate_fade();
        let t = el().key("task").shrink().min_w(0.0).child(t);
        if underline { t.child(el().key("underline").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(p.text.alpha(0.4)).group_opacity(0.0, 1.0)) } else { t }
    };
    let line = row().key("subagent").h(28.0).ml(-6.0).px(6.0).gap(8.0).center_y().radius(4.0).shrink().min_w(0.0)
        .child(tag(p, &name, false).key("agent"))
        .children((!title.is_empty()).then(|| words(child.is_some())))
        .child(text(status).key("status").font(UI).color(if e.failed { p.danger } else { p.muted }));
    let Some(child) = child else { return row().w_full().child(line) };
    let (parent, agent) = (session.to_owned(), e.agent.clone().unwrap_or_default());
    let said = [name.as_str(), title.as_str(), status].iter().filter(|w| !w.is_empty()).copied().collect::<Vec<_>>().join(" ");
    let line = line.role("link").label(said).cursor(rune::Cursor::Pointer).hover(p.hover).hover_group()
        .hover_motion(crate::style::INSTANT, crate::style::INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            // A click with a modifier opens nothing (`event-row.tsx`).
            if crate::plain_click(tx) {
                tx.instant();
                s.open_subagent(&parent, &child, &title, &agent, tx);
            }
        });
    row().w_full().child(line)
}
