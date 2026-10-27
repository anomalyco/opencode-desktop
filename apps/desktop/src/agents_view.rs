//! Background agents drawn in the ⌘⇧K Subagents style (`agents.rs`). Desktop's capsule, list and
//! card (`subagents.rs`) are the default; the live styles show each agent's step and run time:
//!
//! - Live stack: a row per agent docked over the composer.
//! - Inline cards: the timeline card itself goes live, and a chip over the composer jumps back
//!   to it once it's scrolled away.
//! - Live capsule: the capsule shows the newest agent's step, the others as "+2".
//!
//! Every live dock is always there, folded to nothing while it has nothing to show, so what it
//! shows rolls out of the composer's top edge as its height springs and rolls back in. A
//! finished agent holds its ✓ for [`agents::HOLD_MS`], then folds away the same way.

use rune::{Anim, El, Tx, col, icon, row, text};

use opencode_kit::model::{self, ToolStatus};
use rune_theme_opencode::icons as ic;

use crate::agents::{self, Agent, State, Style};
use crate::palette::Pal;
use crate::subagents::{self, ELAPSED};
use crate::view::{CONTENT_W, UI, UI_MED, UI_SMALL, activity_ring_on, composer_fill};
use crate::{App, Route};

const INSTANT: Anim = Anim::INSTANT;
const ROW_H: f32 = 28.0;
/// Rows the live stack shows before the rest wait behind "N more".
const STACK_ROWS: usize = 4;
/// The live capsule's width, so a new step doesn't resize it.
const ISLAND_W: f32 = 460.0;
/// Room the capsule leaves at the right for Jump to latest.
const JUMP_ROOM: f32 = 36.0;
const LABEL_MAX: f32 = 240.0;

/// What stands over the composer: Desktop's capsule, or the style's live dock.
pub fn dock(s: &App, p: &Pal, id: &str) -> Option<El> {
    let lane = |c: El| col().key("activity").w_full().center().px(24.0).child(c);
    let style = agents::style(s);
    if style == Style::Desktop {
        return subagents::capsule(s, p, id).map(lane);
    }
    let list = s.agents_of(id);
    let (content, open) = match style {
        Style::Stack => (stack(s, p, id, &list), list.iter().any(|a| !a.folded)),
        Style::Inline => chip(s, p, id, &list),
        _ => (island(s, p, id, &list), list.iter().any(|a| !a.folded)),
    };
    let mut lane = col().key("activity").w_full().center().px(24.0).clip_soft(0.0).children(content);
    if !open {
        lane = lane.h(0.0).inert();
    }
    Some(lane)
}

/// The list over the composer while it's open: Desktop's, or the live capsule's.
pub fn overlay(s: &App, p: &Pal) -> Option<El> {
    let Route::Session(id) = &s.route else { return None };
    let panel = match agents::style(s) {
        Style::Desktop => subagents::panel(s, p, id),
        Style::Island => island_panel(s, p, id),
        _ => None,
    }?;
    let dismiss = rune::el().key("activity-dismiss").absolute(0.0, 0.0).w_full().h_full().on_press_tx(|s: &mut App, tx: &mut Tx| {
        tx.instant();
        s.activity_open = false;
    });
    Some(rune::el().key("activity-overlay").absolute(0.0, 0.0).w_full().h_full().child(dismiss).child(panel))
}

/// Whether the dock over the composer is a capsule on Jump to latest's row (else the button
/// stands over the dock).
pub fn shares_lane(s: &App, id: &str) -> bool {
    match agents::style(s) {
        Style::Desktop => !s.running_subagents(id).is_empty(),
        Style::Stack => false,
        Style::Inline => jump_to(s, id, &s.agents_of(id)).is_some_and(|j| j.away),
        Style::Island => s.agents_of(id).iter().any(|a| !a.folded),
    }
}

fn time(a: &Agent) -> String {
    if a.run.started > 0 { subagents::elapsed(a.ran) } else { String::new() }
}

fn time_w(list: &[Agent]) -> f32 {
    list.iter().map(|a| rune::text::measure(&time(a), &[], ELAPSED, None).0).fold(0.0, f32::max).ceil()
}

/// The spinner while it runs, then a ✓ or ✕.
fn mark(p: &Pal, a: &Agent, bg: rune::Color) -> El {
    let m = rune::el().key("mark").size(16.0, 16.0).center();
    match a.state {
        State::Running => m.child(activity_ring_on(p, bg, 16.0)),
        State::Done => m.child(icon(&ic::CHECK_SMALL, 16.0).color(p.icon_muted)),
        State::Failed => m.child(icon(&ic::CIRCLE_X, 16.0).color(p.danger)),
    }
}

/// One agent on one line: its mark, its task, its step and its run time. Folded, its height
/// springs to nothing, the line pinned to its top so it rolls up rather than squeezing.
fn agent_row(p: &Pal, parent: &str, a: &Agent, bg: rune::Color, time_w: f32, folded: bool) -> El {
    let (parent, child, title, agent) = (parent.to_owned(), a.run.session.clone(), a.run.label.clone(), a.run.agent.clone());
    let line = row()
        .key("line")
        .role("button")
        .label(format!("{} {}", a.run.label, a.step))
        .w_full()
        .h(ROW_H)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.open_child(&parent, &child, &title, &agent, tx);
        })
        .child(mark(p, a, bg))
        .child(text(a.run.label.clone()).key("label").font(UI_MED).color(p.text).truncate_fade().max_w(LABEL_MAX))
        .child(text(a.step.clone()).key("step").font(UI).color(p.muted).truncate_fade().grow(1.0).shrink())
        .child(row().w(time_w).justify_end().child(text(time(a)).key("time").font(ELAPSED).color(p.faint)));
    let fold = col().key(format!("agent-{}", a.run.session)).w_full().h(if folded { 0.0 } else { ROW_H }).clip_soft(0.0).child(line);
    if folded { fold.inert() } else { fold }
}

/// Live stack: a card of rows over the composer, oldest first. Past [`STACK_ROWS`], the
/// newest wait folded behind "N more" until it's pressed.
fn stack(s: &App, p: &Pal, id: &str, list: &[Agent]) -> Option<El> {
    if list.is_empty() {
        return None;
    }
    let bg = composer_fill(p);
    let tw = time_w(list);
    let live = list.iter().filter(|a| !a.folded).count();
    let capped = live > STACK_ROWS && !s.agents.more;
    let mut card = col().key("agent-stack").w_full().px(4.0).py(2.0).radius(10.0).fill(bg).border(0.5, p.border_muted).shadow(1.0, 2.0, p.shadow);
    let mut shown = 0;
    for a in list {
        let hidden = capped && !a.folded && shown >= STACK_ROWS - 1;
        shown += usize::from(!a.folded);
        card = card.child(agent_row(p, id, a, bg, tw, a.folded || hidden));
    }
    if live > STACK_ROWS {
        let label = if capped { format!("{} more", live - (STACK_ROWS - 1)) } else { "Show fewer".to_owned() };
        card = card.child(
            row()
                .key("agent-more")
                .role("button")
                .label(label.clone())
                .w_full()
                .h(ROW_H)
                .px(6.0)
                .gap(8.0)
                .center_y()
                .radius(6.0)
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    s.agents.more = !s.agents.more;
                    tx.wake_after(std::time::Duration::ZERO);
                })
                .child(rune::el().size(16.0, 16.0))
                .child(text(label).font(UI).color(p.faint)),
        );
    }
    Some(row().key("activity-lane").w_full().max_w(CONTENT_W).pb(8.0).child(card))
}

/// The capsule's surface, as Desktop's (`subagents::capsule`) and its knobs have it.
fn capsule(s: &App, p: &Pal, key: &str) -> El {
    row()
        .key(key)
        .h(28.0)
        .center_y()
        .radius(s.knobs.get(&crate::knobs::CAPSULE_RADIUS))
        .fill(composer_fill(p).alpha(s.knobs.get(&crate::knobs::CAPSULE_FILL)))
        .border(0.5, p.border_muted.mix(p.ring, s.knobs.get(&crate::knobs::CAPSULE_RING)))
        .shadow(1.0, 2.0, p.shadow.alpha(p.shadow.a * s.knobs.get(&crate::knobs::CAPSULE_FILL)))
}

/// The capsule's agent: the newest still running, else the newest that hasn't folded.
fn lead(list: &[Agent]) -> Option<&Agent> {
    list.iter().rev().find(|a| a.state == State::Running && !a.folded).or_else(|| list.iter().rev().find(|a| !a.folded)).or(list.last())
}

/// Live capsule: the lead agent's step and run time, which open its session, and "+2" for the
/// others, which opens the list of all of them.
fn island(s: &App, p: &Pal, id: &str, list: &[Agent]) -> Option<El> {
    let a = lead(list)?;
    let others = list.iter().filter(|o| o.state == State::Running && !o.folded && o.run.session != a.run.session).count();
    let r = s.knobs.get(&crate::knobs::CAPSULE_RADIUS);
    let bg = composer_fill(p);
    let (parent, child, title, agent) = (id.to_owned(), a.run.session.clone(), a.run.label.clone(), a.run.agent.clone());
    let mut chip = capsule(s, p, "activity-chip").w(ISLAND_W).shrink().role("group").label("Subagents").child(
        row()
            .key("agent-lead")
            .role("button")
            .label(format!("{} {}", a.run.label, a.step))
            .h_full()
            .grow(1.0)
            .shrink()
            .pl(10.0)
            .pr(10.0)
            .gap(8.0)
            .center_y()
            .corners(if others > 0 { [r, 0.0, 0.0, r] } else { [r; 4] })
            .hover(p.hover)
            .press(p.pressed)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.open_child(&parent, &child, &title, &agent, tx);
            })
            .child(mark(p, a, bg))
            .child(text(a.run.label.clone()).key("label").font(UI_MED).color(p.text).truncate_fade().max_w(180.0))
            .child(text(a.step.clone()).key("step").font(UI).color(p.muted).truncate_fade().grow(1.0).shrink())
            .child(text(time(a)).key("time").font(ELAPSED).color(p.faint)),
    );
    if others > 0 {
        let mut more = row()
            .key("agent-others")
            .role("button")
            .label(format!("{others} more subagents"))
            .h_full()
            .pl(8.0)
            .pr(10.0)
            .center_y()
            .corners([0.0, r, r, 0.0])
            .hover(p.hover)
            .press(p.pressed)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.activity_open = !s.activity_open;
            })
            .child(text(format!("+{others}")).font(UI_MED.features("\"tnum\"")).color(p.muted));
        if s.activity_open {
            more = more.fill(p.pressed);
        }
        chip = chip.child(more);
    }
    Some(row().key("activity-lane").w_full().max_w(CONTENT_W).pr(JUMP_ROOM).pb(8.0).child(chip))
}

/// The live capsule's list: every agent as the live stack draws it, over the capsule.
fn island_panel(s: &App, p: &Pal, id: &str) -> Option<El> {
    let list = s.agents_of(id);
    if !s.activity_open || list.iter().all(|a| a.folded) {
        return None;
    }
    let tw = time_w(&list);
    let mut menu = col()
        .key("activity-panel")
        .role("menu")
        .label("Subagents")
        .above("activity-chip", 8.0)
        .w(ISLAND_W)
        .max_h(300.0)
        .padding(4.5)
        .radius(10.0)
        .fill(p.layer1)
        .border(0.5, p.ring)
        .shadow(8.0, 16.0, p.shadow_far)
        .shadow(4.0, 8.0, p.shadow)
        .transition(rune::Transition::opacity().springs(crate::view::no_motion()))
        .on_press(|_: &mut App| {});
    for a in &list {
        menu = menu.child(agent_row(p, id, a, p.layer1, tw, a.folded));
    }
    Some(menu)
}

struct Jump<'a> {
    agent: &'a Agent,
    row: String,
    /// Other running agents scrolled away.
    others: usize,
    /// The agent's card is scrolled away (else the chip folds, still naming it).
    away: bool,
}

/// What Inline cards' chip points at: the first agent still showing whose card is scrolled
/// away, else the newest (the chip folded, so it has words while it folds).
fn jump_to<'a>(s: &App, id: &str, list: &'a [Agent]) -> Option<Jump<'a>> {
    let cards: Vec<(&Agent, String)> = list.iter().filter_map(|a| s.call_of(id, &a.run.session).map(|c| (a, format!("a-{c}")))).collect();
    let away: Vec<&(&Agent, String)> = cards.iter().filter(|(a, k)| !a.folded && s.agents.off(k)).collect();
    let (agent, row) = away.first().copied().or(cards.last())?.clone();
    Some(Jump { agent, row, others: away.len().saturating_sub(1), away: !away.is_empty() })
}

/// Inline cards' chip, which scrolls back to its agent's card. It folds away while every live
/// card is on screen; true in `.1` while it shows.
fn chip(s: &App, p: &Pal, id: &str, list: &[Agent]) -> (Option<El>, bool) {
    let Some(j) = jump_to(s, id, list) else { return (None, false) };
    let a = j.agent;
    let target = j.row.clone();
    let mut c = capsule(s, p, "agent-jump")
        .role("button")
        .label(format!("Jump to {}", a.run.label))
        .shrink()
        .px(10.0)
        .gap(8.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |_: &mut App, tx: &mut Tx| tx.scroll_into_view(target.clone()))
        .child(mark(p, a, composer_fill(p)))
        .child(text(a.run.label.clone()).key("label").font(UI_MED).color(p.text).truncate_fade().max_w(LABEL_MAX))
        .child(text(a.step.clone()).key("step").font(UI).color(p.muted).truncate_fade().w(160.0))
        .child(text(time(a)).key("time").font(ELAPSED).color(p.faint));
    if j.others > 0 {
        c = c.child(text(format!("+{}", j.others)).font(UI_MED.features("\"tnum\"")).color(p.muted));
    }
    (Some(row().key("activity-lane").w_full().max_w(CONTENT_W).pr(JUMP_ROOM).pb(8.0).child(c)), j.away)
}

/// The call's card in the timeline. Inline cards make it live: a second line with what the
/// agent is doing, its run time at the end, and a ✓ once it's done.
#[allow(clippy::too_many_arguments)]
pub fn card(s: &App, p: &Pal, parent: &str, agent: &str, title: &str, status: &ToolStatus, named: Option<&str>, background: bool) -> El {
    if agents::style(s) != Style::Inline || *status == ToolStatus::Pending {
        return subagents::card(s, p, parent, agent, title, status, named, background);
    }
    let child = subagents::child_of(s, parent, named, title);
    let info = child.and_then(|c| s.session(c));
    let running = *status == ToolStatus::Running || child.is_some_and(|c| s.subtree_running(c));
    let ended = child.and_then(|c| s.agent_ended(c));
    let outcome = info.and_then(|i| i.outcome.as_deref());
    let (state, step) = if running {
        (State::Running, child.map_or_else(|| "Starting".to_owned(), |c| s.agent_step(c)))
    } else if matches!(ended, Some((true, _))) || matches!(outcome, Some("failed")) || *status == ToolStatus::Error {
        (State::Failed, "Failed".to_owned())
    } else {
        (State::Done, "Done".to_owned())
    };
    let ran = info.filter(|i| i.created > 0).map(|i| {
        let end = match ended {
            _ if running => s.now_ms,
            Some((_, at)) => at,
            None => i.idle.unwrap_or(i.updated),
        };
        subagents::elapsed(end - i.created)
    });
    let a = Agent { run: subagents::Running::default(), state, step: String::new(), ran: 0, folded: false };
    let mut surface = row()
        .key("chip")
        .role("button")
        .label(format!("{} {title}", model::capitalize(agent)))
        .w_full()
        .h(52.0)
        .px(12.0)
        .gap(10.0)
        .center_y()
        .radius(8.0)
        .fill(p.layer2)
        .hover(p.layer3)
        .press(p.pressed)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        // The ✓ fades in over the spinner's place; a run time that widens slides the rest over.
        .animate_layout()
        .child(if running { activity_ring_on(p, p.layer2, 16.0).key("spinner") } else { mark(p, &a, p.layer2) })
        .child(
            col()
                .key("lines")
                .grow(1.0)
                .shrink()
                .gap(2.0)
                .child(
                    row()
                        .gap(6.0)
                        .child(text(model::capitalize(agent)).key("agent").font(UI_MED.lh(18.0)).color(p.text))
                        .child(text(title.to_owned()).key("subtitle").font(UI.lh(18.0)).color(p.text).truncate_fade().shrink()),
                )
                .child(text(step).key("step").font(UI_SMALL).color(p.muted).truncate_fade()),
        )
        .children(ran.map(|t| text(t).key("time").font(ELAPSED).color(p.faint)));
    if let Some(c) = child {
        let (parent, c, title, agent) = (parent.to_owned(), c.to_owned(), title.to_owned(), agent.to_owned());
        surface = surface
            .child(icon(&ic::CHEVRON_RIGHT, 16.0).key("open").color(p.icon_faint).group_opacity(0.0, 1.0))
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.open_child(&parent, &c, &title, &agent, tx);
            });
    }
    row().w_full().child(surface)
}
