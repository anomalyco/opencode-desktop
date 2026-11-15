//! Work that runs on (`timeline-next/event-row.tsx`, as of anomalyco/opencode 0cfe2c2610,
//! #64017): a run of background shells read like a group's label, "Started 2 shells · Finished 1
//! shell", opening to each command; and a subagent's event, a row of its own: what happened as
//! a chip, its agent, and its task, which opens its session.

use rune::{Color, El, Icon, Tx, col, el, icon, row, text};

use super::events::{self, Event, Verb};
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

/// What a subagent row's status chip shows. The timeline is a set of events, so a Started chip
/// never spins while its agent runs, unlike official's (#64102).
#[derive(Clone, Copy, PartialEq)]
enum Chip {
    Started,
    Updated,
    Completed,
    Failed,
    Stopped,
}

impl Chip {
    fn of(e: &Event) -> Self {
        match e.verb {
            _ if e.failed => Chip::Failed,
            Verb::Started => Chip::Started,
            Verb::Updated => Chip::Updated,
            Verb::Stopped => Chip::Stopped,
            Verb::Ran | Verb::Finished => Chip::Completed,
        }
    }
}

/// The status chip leading a subagent row (`timeline-event-item-status` in `timeline-next.css`,
/// e3ed63c7de): 18 pt tall, 6 pt in, 12/530, radius 4, a 1 pt border, a 12 pt glyph 4 before
/// its word, styled by state. Started a faint outline with a ring, Updated a quiet blue dashed outline with ↻, Completed a soft green fill with ✓,
/// Failed the danger tag with ✕, Stopped a gray outline with ■.
fn status_chip(p: &Pal, chip: Chip, word: &str) -> El {
    // `color-mix(in srgb, c n%, transparent)`, as it should paint over the timeline here.
    let tint = |c: Color, n: f32| rune::theme::for_blend(c.alpha(c.a * n), p.bg_base);
    let (fill, border, ink) = match chip {
        Chip::Started { .. } => (None, Some(tint(p.muted, 0.35)), p.faint),
        Chip::Updated => (None, Some(tint(p.info, 0.5)), mix_srgb(p.info, p.muted, 0.7)),
        Chip::Completed => (Some(tint(p.success, 0.16)), None, p.success),
        Chip::Failed => (Some(p.danger_bg), None, p.danger),
        Chip::Stopped => (None, Some(tint(p.muted, 0.55)), p.muted),
    };
    let glyph = match chip {
        Chip::Started => icon(&RING, GLYPH).key("glyph").color(ink),
        Chip::Updated => icon(&AGAIN, GLYPH).key("glyph").color(ink),
        Chip::Completed => icon(&CHECK, GLYPH).key("glyph").color(ink),
        Chip::Failed => icon(&CROSS, GLYPH).key("glyph").color(ink),
        Chip::Stopped => icon(&SQUARE, GLYPH).key("glyph").color(ink),
    };
    let mut c = row().key("status").h(18.0).px(6.0).gap(4.0).center().radius(4.0)
        .child(el().ml(-1.0).size(GLYPH, GLYPH).child(glyph))
        .child(text(word.to_owned()).font(super::TAG).color(ink));
    if let Some(f) = fill {
        c = c.fill(f);
    }
    if let Some(b) = border {
        c = c.border(1.0, b);
    }
    if chip == Chip::Updated { c.border_dash(3.0, 3.0) } else { c }
}

/// `color-mix(in srgb, a n%, b)` for opaque `a` and `b`.
fn mix_srgb(a: Color, b: Color, n: f32) -> Color {
    use rune::color::{srgb_decode, srgb_encode};
    let m = |a: f32, b: f32| srgb_decode(srgb_encode(a) * n + srgb_encode(b) * (1.0 - n));
    Color { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: a.a * n + b.a * (1.0 - n) }
}

const GLYPH: f32 = 12.0;

static RING: Icon = Icon {
    name: "subagent-started",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="4.5" fill="none" stroke="currentColor" stroke-width="1.4"/>"#,
};
static AGAIN: Icon = Icon {
    name: "subagent-updated",
    view: 16.0,
    body: r#"<path d="M12.5 8a4.5 4.5 0 1 1-1.4-3.25M12.5 3v2.5H10" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
static CHECK: Icon = Icon {
    name: "subagent-completed",
    view: 16.0,
    body: r#"<path d="M3.8 8.3l2.7 2.7 5.7-6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
static CROSS: Icon = Icon {
    name: "subagent-failed",
    view: 16.0,
    body: r#"<path d="M5 5l6 6M11 5l-6 6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};
static SQUARE: Icon = Icon {
    name: "subagent-stopped",
    view: 16.0,
    body: r#"<rect x="4.75" y="4.75" width="6.5" height="6.5" rx="1.2" fill="currentColor"/>"#,
};

/// A subagent's event, a row of its own (`TimelineEventItem`, anomalyco/opencode e3ed63c7de,
/// #64102): a status chip saying what happened (see [`status_chip`]), its agent's name in muted
/// ("Explore Agent"), and its task when it names one. 28 tall
/// around its 16 pt line like the timeline's other rows; under the pointer it takes a faint fill
/// 6 past its words each side, its task underlines, and a press opens its session.
pub fn subagent<S: TimelineHost>(s: &S, p: &Pal, session: &str, e: &Event) -> El {
    let name = match e.agent.as_deref().filter(|a| !a.is_empty()) {
        Some(agent) => opencode_i18n::t!("ui.timeline.subagent.name", name = opencode_kit::model::capitalize(agent)),
        None => opencode_i18n::t!("ui.tool.agent.default").to_owned(),
    };
    let status = events::subagent_status(e);
    let title = e.title.clone().unwrap_or_default();
    let chip = Chip::of(e);
    let child = s.resolve_subagent(session, e.session.as_deref(), &title).map(str::to_owned).filter(|_| s.can_open_subagent());
    let words = |underline: bool| {
        let t = text(title.clone()).key("title").font(UI).color(p.text).truncate_fade();
        let t = el().key("task").shrink().min_w(0.0).child(t);
        if underline { t.child(el().key("underline").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(p.text.alpha(0.4)).group_opacity(0.0, 1.0)) } else { t }
    };
    let line = row().key("subagent").h(28.0).ml(-6.0).px(6.0).gap(8.0).center_y().radius(4.0).shrink().min_w(0.0)
        .child(status_chip(p, chip, status))
        .child(text(name.clone()).key("agent").font(UI).color(p.muted))
        .children((!title.is_empty()).then(|| words(child.is_some())));
    let Some(child) = child else { return row().w_full().child(line) };
    let (parent, agent) = (session.to_owned(), e.agent.clone().unwrap_or_default());
    let said = [status, name.as_str(), title.as_str()].iter().filter(|w| !w.is_empty()).copied().collect::<Vec<_>>().join(" ");
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
