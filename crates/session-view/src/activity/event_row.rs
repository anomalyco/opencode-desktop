//! Work that runs on (`timeline-next/event-row.tsx`, as of anomalyco/opencode 0cfe2c2610,
//! #64017): a run of background shells read like a group's label, "◯ 2 shells · ✓ 1 shell",
//! opening to each command; and a subagent's event, a row of its own: a mark for what happened,
//! its agent, and its task, which opens its session. Marks only (t1582): each status is a bare
//! glyph whose word is its tooltip and accessible name, with no chip around it.

use rune::{Color, El, Icon, Tx, col, el, icon, row, text};

use super::events::{self, Event, Verb};
use crate::TimelineHost;
use crate::focus::FocusExt;
use crate::palette::Pal;
use crate::style::{INSTANT, ROW_GAP, TOOL_CODE, UI, UI_MED, arrow, readable};

/// The row of `events` in session `session`; `open` lists them, a press on the label runs `press`,
/// and clicking the nested rail runs `collapse` (which only collapses and never re-opens mid-fold).
/// Nothing when nothing in it has happened yet.
pub fn view<S: TimelineHost>(
    s: &S,
    p: &Pal,
    session: &str,
    events: &[Event],
    open: bool,
    press: impl Fn(&mut S, &mut Tx) + 'static,
    collapse: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
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
        // A bucket whose every shell failed takes the cross, so "Ran 2 shells" never ticks.
        let all_failed = events.iter().filter(|e| e.kind == events::Kind::Shell && e.verb == b.verb).all(|e| e.failed);
        let m = if all_failed { Mark::Failed } else { Mark::of_verb(b.verb) };
        words = words.child(row().key(format!("bucket-{k}")).gap(5.0).center_y().shrink().min_w(0.0)
            .child(mark(p, m, verb))
            .child(text(detail).key("detail").font(UI).color(p.text).shrink().min_w(0.0).truncate_fade()));
    }
    // Failures keep their count in words, in the danger colour; where every one failed it says
    // so without counting them again.
    if failed > 0 {
        let words_failed = if failed == events.len() { opencode_i18n::t!("ui.timeline.event.allFailed").to_owned() } else { opencode_i18n::t!("ui.timeline.event.failed", count = failed) };
        label.push(words_failed.clone());
        words = words.child(row().key("failed").gap(5.0).ml(4.0).center_y()
            .child(mark(p, Mark::Failed, opencode_i18n::t!("ui.toolErrorCard.failed")))
            .child(text(words_failed).key("count").font(UI).color(p.danger)));
    }
    // `self_start`: its hover fill hugs its words rather than the timeline's width.
    let mut trigger = row().key("trigger").h(28.0).gap(4.0).center_y().self_start().shrink().min_w(0.0).child(words);
    if items.is_empty() {
        trigger = trigger.role("group").label(label.join(" · "));
    } else {
        let a = el().key("arrow").w(16.0).h(24.0).ml(2.0).center().child(arrow(p, open));
        trigger = hoverable(p, trigger)
            .role("button")
            .label(label.join(" · "))
            .expanded(open)
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
    c.child(crate::disclosure::reveal(s.knobs(), crate::style::nested_collapsible(p, ROW_GAP, list, collapse)))
}

/// One shell an events row lists (`TimelineEventItem`): "Shell", its command on one line as a
/// Shell call's row shows it, and a cross when it failed. A command over several lines keeps
/// all of them in its tooltip and accessible name.
fn item<S: TimelineHost>(_s: &S, p: &Pal, _session: &str, k: usize, e: &Event) -> El {
    let shell = opencode_i18n::t!("ui.tool.shell");
    let command = e.title.clone().unwrap_or_default();
    let failed = opencode_i18n::t!("ui.toolErrorCard.failed");
    let said = if e.failed { format!("{shell} {command}, {failed}") } else { format!("{shell} {command}") };
    let line = text(crate::tools::plain::one_line(&command)).key("title").dir(rune::Direction::Ltr).font(TOOL_CODE.lh(16.0)).color(p.text).truncate_fade();
    row().key(format!("item-{k}")).h(28.0).gap(6.0).center_y().shrink().min_w(0.0).role("group").label(said).help(command)
        .child(text(shell).key("name").font(UI).color(p.muted))
        .child(readable(p, line).shrink().min_w(0.0))
        .children(e.failed.then(|| mark(p, Mark::Failed, failed)))
}

/// What an event's mark shows. The timeline is a set of events, so a Started ring never spins
/// while its agent runs, unlike official's (#64102).
#[derive(Clone, Copy, PartialEq)]
pub enum Mark {
    Started,
    Updated,
    Completed,
    Failed,
    Stopped,
    /// A session notice that only tells: a server restart, a job's report.
    Info,
}

impl Mark {
    fn of(e: &Event) -> Self {
        if e.failed { Mark::Failed } else { Mark::of_verb(e.verb) }
    }

    fn of_verb(v: Verb) -> Self {
        match v {
            Verb::Started => Mark::Started,
            Verb::Updated => Mark::Updated,
            Verb::Stopped => Mark::Stopped,
            Verb::Ran | Verb::Finished => Mark::Completed,
        }
    }
}

/// A bare status mark: a 14 pt glyph with no box, its `word` the tooltip and the name a screen
/// reader hears. Started a faint ring, Updated a quiet blue ↻, Completed a green ✓, Failed a red
/// ✕, Stopped a muted ■, Info a faint ⓘ.
pub fn mark(p: &Pal, m: Mark, word: &str) -> El {
    let (glyph, ink): (&'static Icon, Color) = match m {
        Mark::Started => (&RING, p.faint),
        Mark::Updated => (&AGAIN, mix_srgb(p.info, p.muted, 0.7)),
        Mark::Completed => (&CHECK, p.success),
        Mark::Failed => (&CROSS, p.danger),
        Mark::Stopped => (&SQUARE, p.muted),
        Mark::Info => (&INFO, p.faint),
    };
    el().key("mark").size(GLYPH, GLYPH).center().role("img").label(word.to_owned()).help(word.to_owned())
        .child(icon(glyph, GLYPH).key("glyph").color(ink))
}

/// An interactive event row's hover: a quiet fill that comes and goes at once, 6 past its words
/// each side with a 4 radius, and the shared ring 2 outside it while the keyboard has it. No underline.
fn hoverable(p: &Pal, row: El) -> El {
    row.ml(-6.0).px(6.0).radius(4.0).cursor(rune::Cursor::Pointer).hover(p.hover).hover_group_focus()
        .hover_motion(INSTANT, INSTANT)
        .focus_default(p)
}

/// `color-mix(in srgb, a n%, b)` for opaque `a` and `b`.
fn mix_srgb(a: Color, b: Color, n: f32) -> Color {
    use rune::color::{srgb_decode, srgb_encode};
    let m = |a: f32, b: f32| srgb_decode(srgb_encode(a) * n + srgb_encode(b) * (1.0 - n));
    Color { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: a.a * n + b.a * (1.0 - n) }
}

const GLYPH: f32 = 14.0;

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
static INFO: Icon = Icon {
    name: "event-info",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="5" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 7.4v3.4M8 5.3v.1" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};

/// State colour for a subagent status pill or mark (`timeline-next.css:309-328`).
fn state_ink(p: &Pal, m: Mark) -> Color {
    match m {
        Mark::Started => p.faint,
        Mark::Updated => mix_srgb(p.info, p.muted, 0.7),
        Mark::Completed => p.success,
        Mark::Failed => p.danger,
        Mark::Stopped => p.muted,
        Mark::Info => p.faint,
    }
}

/// A subagent's event, a row of its own (`TimelineEventItem`, anomalyco/opencode #65164):
/// a 3x10 rounded coloured bar and its status word in the state's colour, its agent's name in
/// muted ("Explore Agent"), and its task when it names one. 28 tall around its 16 pt line like
/// the timeline's other rows; no hover fill or title underline, keeping focus ring and click-to-open.
pub fn subagent<S: TimelineHost>(s: &S, p: &Pal, session: &str, e: &Event) -> El {
    let name = match e.agent.as_deref().filter(|a| !a.is_empty()) {
        Some(agent) => opencode_i18n::t!("ui.timeline.subagent.name", name = opencode_kit::model::capitalize(agent)),
        None => opencode_i18n::t!("ui.tool.agent.default").to_owned(),
    };
    let status = events::subagent_status(e);
    let ink = state_ink(p, Mark::of(e));
    let title = e.title.clone().unwrap_or_default();
    let child = s.resolve_subagent(session, e.session.as_deref(), &title).map(str::to_owned).filter(|_| s.can_open_subagent());
    let said = [status, name.as_str(), title.as_str()].iter().filter(|w| !w.is_empty()).copied().collect::<Vec<_>>().join(" ");
    let badge = row().key("status").gap(6.0).center_y()
        .child(el().key("bar").size(3.0, 10.0).radius(1.5).fill(ink))
        .child(text(status).key("word").font(UI_MED).color(ink));
    let line = row().key("subagent").h(28.0).gap(8.0).center_y().shrink().min_w(0.0).label(said)
        .child(badge)
        .child(text(name.clone()).key("subagent-agent").font(UI).color(p.muted))
        .children((!title.is_empty()).then(|| text(title.clone()).key("title").dir(rune::Direction::Auto).font(UI).color(p.text).truncate_fade().shrink().min_w(0.0)));
    let Some(child) = child else { return row().w_full().child(line.role("group")) };
    let (parent, agent) = (session.to_owned(), e.agent.clone().unwrap_or_default());
    let line = line.role("link").radius(4.0).cursor(rune::Cursor::Pointer).focus_default(p)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            // A click with a modifier opens nothing (`event-row.tsx`).
            if crate::plain_click(tx) {
                tx.instant();
                s.open_subagent(&parent, &child, &title, &agent, tx);
            }
        });
    row().w_full().child(line)
}
