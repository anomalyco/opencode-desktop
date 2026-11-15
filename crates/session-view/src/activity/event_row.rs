//! A run of subagent and background shell events, read like a group's label: "Started 2
//! subagents · Finished 1 shell" (`timeline-next/event-row.tsx`). Opening it lists each one, the
//! agent's name muted and its task in base ink; a subagent opens its session.

use rune::{El, Tx, col, el, row, text};

use super::events::{self, Event, Kind};
use super::tag;
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{HOVER_FADE, ROW_GAP, UI, arrow};

/// The row of `events` in session `session`; `open` lists them, and a press on the
/// label runs `press`. Nothing when nothing in it has happened yet.
pub fn view<S: TimelineHost>(s: &S, p: &Pal, session: &str, events: &[Event], open: bool, press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let (buckets, failed) = events::summarize(events);
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
        let detail = b.kinds.iter().map(|(kind, n)| events::kind_count(*kind, *n)).collect::<Vec<_>>().join(", ");
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

/// One subagent or shell an events row lists (`TimelineEventItem`): who did it and what it was.
fn item<S: TimelineHost>(s: &S, p: &Pal, session: &str, k: usize, e: &Event) -> El {
    let name = match e.kind {
        Kind::Shell => opencode_i18n::t!("ui.tool.shell").to_owned(),
        Kind::Subagent => e.agent.as_deref().map(opencode_kit::model::capitalize).unwrap_or_else(|| opencode_i18n::t!("ui.tool.agent.default").to_owned()),
    };
    let title = e.title.clone().unwrap_or_default();
    let line = row().key(format!("item-{k}")).h(28.0).gap(6.0).center_y().shrink().min_w(0.0)
        .child(text(name.clone()).key("name").font(UI).color(p.muted))
        .child(text(title.clone()).key("title").font(UI).color(p.text).shrink().min_w(0.0).truncate_fade())
        .children(e.failed.then(|| tag(p, opencode_i18n::t!("ui.toolErrorCard.failed"), true)));
    let child = s.resolve_subagent(session, e.session.as_deref(), &title).map(str::to_owned);
    let Some(child) = child.filter(|_| e.kind == Kind::Subagent && s.can_open_subagent()) else { return line };
    let (parent, agent) = (session.to_owned(), e.agent.clone().unwrap_or_default());
    line.role("link")
        .label(format!("{name} {title}"))
        .hover_group()
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            // A click with a modifier opens nothing (`event-row.tsx`).
            if crate::plain_click(tx) {
                tx.instant();
                s.open_subagent(&parent, &child, &title, &agent, tx);
            }
        })
}
