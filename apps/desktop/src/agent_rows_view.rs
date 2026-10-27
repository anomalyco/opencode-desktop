//! The ⌘⇧K Subagent rows styles: how a subagent call's row in the parent's timeline looks
//! while the agent runs and once it ends (`agent_rows.rs` has what each row knows).

use rune::{Anim, El, Transition, Tx, col, el, icon, row, spacer, text};

use opencode_kit::model::{self, ToolStatus};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::agent_rows::{Run, State, Style, style};
use crate::palette::Pal;
use crate::turns::Block;
use crate::view::{UI, UI_MED, UI_SMALL, activity_ring_on};

const INSTANT: Anim = Anim::INSTANT;
/// The widest a task gets on a one-line row, so what the agent is doing always shows beside it.
const TITLE_W: f32 = 260.0;

/// One call, ready to draw.
struct Call<'a> {
    key: String,
    agent: &'a str,
    title: &'a str,
    child: Option<&'a str>,
    run: Run,
}

/// The row for `blocks[i]`, a subagent call, in the current style. Grouped draws the calls
/// that follow it too; `skip` says which rows it took.
pub fn row_for<'a>(s: &'a App, p: &Pal, parent: &str, blocks: &'a [Block<'a>], i: usize) -> El {
    let Block::Agent { agent, title, status, session, background, .. } = &blocks[i] else { unreachable!() };
    let style = style(s);
    if style == Style::Today || **status == ToolStatus::Pending {
        return crate::subagents::card(s, p, parent, agent, title, status, *session, *background);
    }
    let call = |b: &'a Block<'a>| match b {
        Block::Agent { id, agent, title, status, session, .. } if **status != ToolStatus::Pending => {
            let child = crate::subagents::child_of(s, parent, *session, title);
            Some(Call { key: id.to_string(), agent, title, child, run: s.agent_run(child, status) })
        }
        _ => None,
    };
    match style {
        Style::Status => line(p, parent, &call(&blocks[i]).unwrap()),
        Style::Expanding => expanding(p, parent, &call(&blocks[i]).unwrap()),
        _ => {
            let calls: Vec<Call> = blocks[i..].iter().map_while(call).collect();
            if calls.len() == 1 { line(p, parent, &calls[0]) } else { grouped(p, parent, &calls) }
        }
    }
}

/// Grouped: whether `blocks[i]` is drawn inside the call above it.
pub fn skip(s: &App, blocks: &[Block], i: usize) -> bool {
    let agent = |b: &Block| matches!(b, Block::Agent { status, .. } if **status != ToolStatus::Pending);
    style(s) == Style::Grouped && i > 0 && agent(&blocks[i]) && agent(&blocks[i - 1])
}

/// The 16pt mark: a spinner, a ✓ or a ✗.
fn mark(p: &Pal, state: State, on: rune::Color) -> El {
    let m = el().key("mark").size(16.0, 16.0).center();
    match state {
        State::Running => m.child(activity_ring_on(p, on, 14.0)),
        State::Done => m.child(icon(&ic::CHECK_SMALL, 16.0).color(p.success)),
        State::Failed => m.child(icon(&ic::CIRCLE_X, 16.0).color(p.danger)),
    }
}

/// The agent type as a quiet tag.
fn tag(p: &Pal, agent: &str) -> El {
    row()
        .key("tag")
        .h(18.0)
        .px(6.0)
        .center()
        .radius(5.0)
        .border(0.5, p.border_muted)
        .child(text(model::capitalize(agent)).font(UI_SMALL.lh(16.0)).color(p.faint))
}

fn time(p: &Pal, run: &Run) -> Option<El> {
    run.time.clone().map(|t| text(t).key("time").font(UI_SMALL.lh(16.0).features("\"tnum\"")).color(p.faint))
}

fn line_color(p: &Pal, state: State) -> rune::Color {
    if state == State::Failed { p.danger } else { p.faint }
}

/// Makes `surface` open the call's session.
fn opens(parent: &str, c: &Call, surface: El) -> El {
    let Some(child) = c.child else { return surface };
    let (parent, child, title, agent) = (parent.to_owned(), child.to_owned(), c.title.to_owned(), c.agent.to_owned());
    surface
        .role("button")
        .label(format!("{} {}", model::capitalize(&agent), title))
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.open_child(&parent, &child, &title, &agent, tx);
        })
}

/// The chevron that shows on hover when there's a session to open.
fn chevron(p: &Pal, c: &Call) -> Option<El> {
    c.child.map(|_| icon(&ic::CHEVRON_RIGHT, 16.0).key("open").color(p.icon_faint).group_opacity(0.0, 1.0))
}

/// Live status: one line with the task, then what it's doing now (or its result), its type
/// and its time.
fn line(p: &Pal, parent: &str, c: &Call) -> El {
    let r = &c.run;
    let surface = row()
        .key("agent-line")
        .w_full()
        .h(36.0)
        .px(12.0)
        .gap(8.0)
        .center_y()
        .radius(8.0)
        .fill(p.layer2)
        .animate_layout()
        .child(mark(p, r.state, p.layer2))
        .child(text(c.title.to_owned()).key("title").font(UI_MED.lh(20.0)).color(p.text).truncate().shrink().max_w(TITLE_W))
        .child(text(r.line()).key("now").font(UI.lh(20.0)).color(line_color(p, r.state)).truncate().shrink().grow(1.0))
        .child(tag(p, c.agent))
        .children(time(p, r))
        .children(chevron(p, c));
    row().w_full().child(opens(parent, c, surface))
}

/// Expanding: the task over its last few steps while it runs, then over its result.
fn expanding(p: &Pal, parent: &str, c: &Call) -> El {
    let r = &c.run;
    let head = row()
        .key("head")
        .w_full()
        .h(36.0)
        .gap(8.0)
        .center_y()
        .child(mark(p, r.state, p.layer2))
        .child(text(c.title.to_owned()).key("title").font(UI_MED.lh(20.0)).color(p.text).truncate().shrink())
        .child(spacer())
        .child(tag(p, c.agent))
        .children(time(p, r))
        .children(chevron(p, c));
    let mut body = col().key("body").w_full().pl(24.0).pb(10.0).gap(2.0);
    match r.state {
        State::Running => {
            let n = r.steps.len();
            for (j, st) in r.steps.iter().enumerate() {
                let age = n - 1 - j;
                body = body.child(
                    text(st.clone())
                        .key(format!("step-{}", r.taken + j))
                        .font(UI_SMALL.lh(18.0))
                        .color(p.faint)
                        .opacity([1.0, 0.6, 0.35][age.min(2)])
                        .truncate()
                        .transition(Transition::offset(0.0, 6.0).combined(Transition::opacity())),
                );
            }
        }
        state => {
            body = body.child(
                text(r.line()).key("summary").font(UI_SMALL.lh(18.0)).color(line_color(p, state)).truncate().transition(Transition::opacity()),
            );
        }
    }
    let surface = col().key("agent-card").w_full().px(12.0).radius(8.0).fill(p.layer2).animate_layout().child(head).child(body);
    row().w_full().child(opens(parent, c, surface))
}

/// Grouped: agents started together share a card, a line each, under their overall progress.
fn grouped(p: &Pal, parent: &str, calls: &[Call]) -> El {
    let count = |st: State| calls.iter().filter(|c| c.run.state == st).count();
    let (running, done, failed) = (count(State::Running), count(State::Done), count(State::Failed));
    let n = calls.len();
    let mut parts = Vec::new();
    if running > 0 {
        parts.push(format!("{running} running"));
    }
    if done > 0 {
        parts.push(format!("{done} done"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    let overall = if running > 0 { State::Running } else if failed > 0 { State::Failed } else { State::Done };
    let head = row()
        .key("group-head")
        .w_full()
        .h(36.0)
        .px(12.0)
        .gap(8.0)
        .center_y()
        .child(mark(p, overall, p.layer2))
        .child(text(format!("{n} agents")).key("count").font(UI_MED.lh(20.0)).color(p.text))
        .child(text(parts.join(" · ")).key("tally").font(UI.lh(20.0)).color(p.faint).truncate().shrink())
        .child(spacer());
    let frac = (done + failed) as f32 / n as f32;
    let track = row()
        .key("progress")
        .w_full()
        .h(2.0)
        .radius(1.0)
        .fill(p.border_muted)
        .child(el().key("progress-fill").w_frac(frac).h(2.0).radius(1.0).fill(p.icon_faint).animate_layout());
    let mut card = col()
        .key("agent-group")
        .w_full()
        .radius(8.0)
        .fill(p.layer2)
        .pb(4.0)
        .animate_layout()
        .child(head)
        .child(row().w_full().px(12.0).pb(4.0).child(track));
    for c in calls {
        let r = &c.run;
        let line = row()
            .key(format!("member-{}", c.key))
            .w_full()
            .h(30.0)
            .px(12.0)
            .gap(8.0)
            .center_y()
            .radius(6.0)
            .hover(p.hover)
            .child(mark(p, r.state, p.layer2))
            .child(text(c.title.to_owned()).key("title").font(UI.lh(20.0)).color(p.text).truncate().shrink().max_w(TITLE_W))
            .child(text(r.line()).key("now").font(UI_SMALL.lh(18.0)).color(line_color(p, r.state)).truncate().shrink().grow(1.0))
            .child(tag(p, c.agent))
            .children(time(p, r))
            .children(chevron(p, c));
        card = card.child(row().w_full().px(4.0).child(opens(parent, c, line)));
    }
    row().w_full().child(card)
}
