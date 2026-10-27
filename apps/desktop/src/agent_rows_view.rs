//! The ⌘⇧K Subagent rows styles: how a subagent call's row in the parent's timeline looks
//! while the agent runs and once it ends (`agent_rows.rs` has what each row knows).
//!
//! Every style shares the parts here, and with them the motion: a new step rises in from below
//! as the old one rises out, the time ticks in a fixed tabular slot, the mark crossfades and
//! scales between spinner, ✓ and ✕, cut-off words fade out at the edge, and agents that join a
//! group rise into place at the bottom (nothing above them moves). Hover and click are instant.

use rune::{Anim, El, Spring, SpringPair, TextStyle, Transition, Tx, el, icon, row, text};
use rune_kit::{Kit, RollingText};

use opencode_kit::model::{self, ToolStatus};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::agent_rows::{Run, State, Style, style};
use crate::palette::Pal;
use crate::turns::Block;
use crate::view::{UI, UI_MED, UI_SMALL, activity_ring_on};

const INSTANT: Anim = Anim::INSTANT;
/// The widest a task gets on a one-line row, so what the agent is doing always shows beside it.
pub(crate) const TITLE_W: f32 = 260.0;

/// One call, ready to draw.
pub(crate) struct Call<'a> {
    pub key: String,
    pub agent: &'a str,
    pub title: &'a str,
    pub child: Option<&'a str>,
    pub run: Run,
}

/// The row for `blocks[i]`, a subagent call, in the current style. Grouped styles draw the
/// calls that follow it too; `skip` says which rows they took.
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
    if style == Style::Status {
        return line(p, parent, &call(&blocks[i]).unwrap());
    }
    let calls: Vec<Call> = blocks[i..].iter().map_while(call).collect();
    let g = crate::agent_groups_view::Group { p, parent, calls: &calls, kit: Kit::neutral(p.dark) };
    match style {
        Style::Boxes => g.boxes(),
        Style::Stacked => g.stacked(),
        Style::Card => g.card(),
        Style::Summary => g.summary(s.agent_rows.open.contains(&calls[0].key)),
        _ => g.chips(),
    }
}

/// Grouped styles: whether `blocks[i]` is drawn inside the call above it.
pub fn skip(s: &App, blocks: &[Block], i: usize) -> bool {
    let agent = |b: &Block| matches!(b, Block::Agent { status, .. } if **status != ToolStatus::Pending);
    style(s).groups() && i > 0 && agent(&blocks[i]) && agent(&blocks[i - 1])
}

/// How a new line of words replaces the last: in from 8pt below, out 8pt up, both fading.
fn roll() -> Transition {
    let fade = |y: f32| Transition::offset(0.0, y).combined(Transition::opacity());
    Transition::asymmetric(fade(8.0), fade(-8.0)).springs(SpringPair::new(Spring::new(0.34, 0.0), Spring::new(0.26, 0.0)))
}

/// How an agent joins a group: it rises 6pt into its place at the bottom as it fades in.
pub(crate) fn arrive() -> Transition {
    Transition::offset(0.0, 6.0).combined(Transition::opacity()).springs(SpringPair::new(Spring::new(0.4, 0.0), Spring::new(0.3, 0.0)))
}

/// The 16pt mark: a spinner, a ✓ or a ✕, each scaling and fading into the last one's place.
pub(crate) fn mark(p: &Pal, state: State, on: rune::Color) -> El {
    let (key, glyph) = match state {
        State::Running => ("spin", activity_ring_on(p, on, 14.0)),
        State::Done => ("done", icon(&ic::CHECK_SMALL, 16.0).color(p.success)),
        State::Failed => ("failed", icon(&ic::CIRCLE_X, 16.0).color(p.danger)),
    };
    let swap = Transition::scale(0.6).allow_small_scale().springs(SpringPair::new(Spring::new(0.32, 0.0), Spring::new(0.22, 0.0)));
    el().key("mark").size(16.0, 16.0).child(el().key(key).absolute(0.0, 0.0).size(16.0, 16.0).center().child(glyph).transition(swap))
}

/// The agent type as a quiet tag.
pub(crate) fn tag(p: &Pal, agent: &str) -> El {
    row()
        .key("tag")
        .h(18.0)
        .px(6.0)
        .center()
        .radius(5.0)
        .border(0.5, p.border_muted)
        .child(text(model::capitalize(agent)).font(UI_SMALL.lh(16.0)).color(p.faint))
}

/// The run time in tabular digits, right-aligned in a slot wide enough for "59s", so ticking
/// never moves what sits beside it.
pub(crate) fn time(p: &Pal, t: Option<String>) -> Option<El> {
    t.map(|t| row().key("time").min_w(24.0).justify_end().child(text(t).key("t").font(UI_SMALL.lh(16.0).features("\"tnum\"")).color(p.faint)))
}

/// The words a task's title shows in, fading out where they're cut.
pub(crate) fn title(p: &Pal, c: &Call, font: TextStyle) -> El {
    text(c.title.to_owned()).key("title").font(font).color(p.text).truncate_fade().max_w(TITLE_W)
}

/// What the agent is doing now, or how it ended, in a slot `h` tall: each new line rolls in
/// over the last, keyed by its step so the same words never swap in place.
pub(crate) fn now(p: &Pal, r: &Run, font: TextStyle, h: f32) -> El {
    let (key, color) = match r.state {
        State::Running => (format!("step-{}", r.taken + r.steps.len()), p.faint),
        State::Done => ("result".to_owned(), p.faint),
        State::Failed => ("error".to_owned(), p.danger),
    };
    rolling_slot("now", h, text(r.line()).key(key).font(font).color(color).truncate_fade())
}

/// A slot `h` tall whose one child rolls in over the last; the fades sit in its spare height.
pub(crate) fn rolling_slot(key: &str, h: f32, child: El) -> El {
    row().key(key.to_owned()).h(h).grow(1.0).shrink().min_w(0.0).center_y().clip_fade(0.0, 5.0, 0.0, 5.0).child(child.transition(roll()))
}

/// "1 running · 1 done · 1 failed" as segments that come and go on their own, each count
/// rolling its digits; `lead` puts a "·" before the first.
pub(crate) fn tally(p: &Pal, k: &Kit, gk: &str, runs: &[&Run], lead: bool) -> El {
    let count = |st: State| runs.iter().filter(|r| r.state == st).count();
    let mut out = row().key("tally").gap(4.0).center_y().min_w(0.0);
    let parts = [("running", count(State::Running), p.faint), ("done", count(State::Done), p.faint), ("failed", count(State::Failed), p.danger)];
    let mut first = !lead;
    for (word, n, color) in parts {
        if n == 0 {
            continue;
        }
        let dot = if first { "" } else { "· " };
        first = false;
        let words = RollingText::new(format!("{gk}-{word}"), format!("{dot}{n} {word}")).font(UI.lh(20.0)).color(color).build(k);
        out = out.child(row().key(word).animate_layout().child(words).transition(Transition::opacity()));
    }
    out
}

/// "3 agents", its digit rolling as agents join.
pub(crate) fn count(p: &Pal, k: &Kit, gk: &str, n: usize) -> El {
    let words = if n == 1 { "1 agent".to_owned() } else { format!("{n} agents") };
    row().key("count").child(RollingText::new(format!("{gk}-count"), words).font(UI_MED.lh(20.0)).color(p.text).build(k))
}

/// Makes `surface` open the call's session.
pub(crate) fn opens(parent: &str, c: &Call, surface: El) -> El {
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
pub(crate) fn chevron(p: &Pal, c: &Call) -> Option<El> {
    c.child.map(|_| icon(&ic::CHEVRON_RIGHT, 16.0).key("open").color(p.icon_faint).group_opacity(0.0, 1.0))
}

/// One agent's live row, `h` tall: its mark, task, what it's doing now, type and time.
pub(crate) fn live_row(p: &Pal, c: &Call, h: f32, on: rune::Color) -> El {
    let r = &c.run;
    row()
        .w_full()
        .h(h)
        .gap(8.0)
        .center_y()
        .child(mark(p, r.state, on))
        .child(title(p, c, UI_MED.lh(20.0)))
        .child(now(p, r, UI.lh(20.0), h - 6.0))
        .child(tag(p, c.agent))
        .children(time(p, r.time.clone()))
        .children(chevron(p, c))
}

/// Live status: one line with the task, then what it's doing now (or its result), its type
/// and its time.
fn line(p: &Pal, parent: &str, c: &Call) -> El {
    let surface = live_row(p, c, 36.0, p.layer2).key("agent-line").px(12.0).radius(8.0).fill(p.layer2).animate_layout();
    row().w_full().child(opens(parent, c, surface))
}
