//! A subagent call's row in the parent's timeline: agents started together share one card,
//! a live row each (`agent_rows.rs` has what each row knows).
//!
//! A new step rises in from below as the old one rises out, the time ticks at once each second
//! (`agent_rows::poll`), the mark crossfades and scales between spinner, ✓, ✕ and the dash of a
//! call a later one steered (`agent_rows/calls.rs`), cut-off words fade out at the edge, and
//! agents that join the card rise into place at the bottom (nothing above them moves). Hover and
//! click are instant. A call still being written, or whose agent hasn't started, wakes in place
//! when it does (`agent_start.rs`).

use rune::{El, Spring, SpringPair, TextStyle, Transition, Tx, col, el, icon, row, text};

use opencode_kit::model::{self, ToolStatus};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::agent_rows::{Run, State};
use crate::palette::Pal;
use crate::subagents::run_time;
use crate::turns::Block;
use crate::view::{INSTANT, UI, UI_MED, UI_SMALL};

/// The widest a task gets, so what the agent is doing always shows beside it.
const TITLE_W: f32 = 260.0;

/// One call, ready to draw.
struct Call<'a> {
    key: String,
    agent: &'a str,
    title: &'a str,
    child: Option<&'a str>,
    run: Run,
    /// Still being written: "Delegating agent…" in place of its task.
    pending: bool,
    /// Its agent hasn't started yet: the spinner shows the starting dots.
    starting: bool,
}

/// The row for `blocks[i]`, a subagent call: one card holding it and the calls that follow it
/// (`skip` says which rows they took). A call still being written is Desktop's "Delegating
/// agent…" line, in the card it will run in.
pub fn row_for<'a>(s: &'a App, p: &Pal, parent: &str, blocks: &'a [Block<'a>], i: usize) -> El {
    let call = |b: &'a Block<'a>| match b {
        Block::Agent { id, agent, title, status, .. } if **status == ToolStatus::Pending => {
            let run = Run { state: State::Running, steps: Vec::new(), taken: 0, ms: None, result: None, error: None };
            Some(Call { key: id.to_string(), agent, title, child: None, run, pending: true, starting: true })
        }
        Block::Agent { id, agent, title, status, session, .. } => {
            let child = crate::subagents::child_of(s, parent, *session, title);
            let run = s.agent_run(parent, id, child, status);
            let starting = child.is_none() && run.state == State::Running;
            Some(Call { key: id.to_string(), agent, title, child, run, pending: false, starting })
        }
        _ => None,
    };
    let calls: Vec<Call> = blocks[i..].iter().map_while(call).collect();
    let mut card = col().key("agent-card").w_full().px(4.0).py(4.0).radius(10.0).fill(p.layer2);
    for c in &calls {
        let line = live_row(p, c, 32.0).key("agent-line").px(8.0).radius(6.0).hover(p.hover);
        card = card.child(row().key(format!("member-{}", c.key)).w_full().child(opens(parent, c, line)).transition(arrive()));
    }
    row().w_full().child(card)
}

/// Whether `blocks[i]` is drawn inside the card of the call above it.
pub fn skip(blocks: &[Block], i: usize) -> bool {
    let agent = |b: &Block| matches!(b, Block::Agent { .. });
    i > 0 && agent(&blocks[i]) && agent(&blocks[i - 1])
}

/// How a new line of words replaces the last: in from 8pt below, out 8pt up, both fading.
fn roll() -> Transition {
    let fade = |y: f32| Transition::offset(0.0, y).combined(Transition::opacity());
    Transition::asymmetric(fade(8.0), fade(-8.0)).springs(SpringPair::new(Spring::new(0.34, 0.0), Spring::new(0.26, 0.0)))
}

/// How an agent joins the card: it rises 6pt into its place at the bottom as it fades in.
fn arrive() -> Transition {
    Transition::offset(0.0, 6.0).combined(Transition::opacity()).springs(SpringPair::new(Spring::new(0.4, 0.0), Spring::new(0.3, 0.0)))
}

/// The 16pt mark: a spinner, a ✓, a ✕ or a steered call's dash, each scaling and fading into the
/// last one's place. The spinner wakes from its starting dots in place (`agent_start::spinner`).
fn mark(p: &Pal, c: &Call) -> El {
    let (key, glyph) = match c.run.state {
        State::Running => ("spin", crate::agent_start::spinner(p, c.starting)),
        State::Done => ("done", icon(&ic::CHECK_SMALL, 16.0).color(p.success)),
        State::Failed => ("failed", icon(&ic::CIRCLE_X, 16.0).color(p.danger)),
        State::Steered => ("steered", icon(&ic::DASH, 16.0).color(p.icon_faint)),
    };
    let swap = Transition::scale(0.6).allow_small_scale().springs(SpringPair::new(Spring::new(0.32, 0.0), Spring::new(0.22, 0.0)));
    el().key("mark").size(16.0, 16.0).child(el().key(key).absolute(0.0, 0.0).size(16.0, 16.0).center().child(glyph).transition(swap))
}

/// The agent type as a quiet tag.
fn tag(p: &Pal, agent: &str) -> El {
    row()
        .key("tag")
        .h(18.0)
        .px(6.0)
        .center()
        .radius(5.0)
        .fill(p.layer3)
        .child(text(model::capitalize(agent)).font(UI_SMALL.lh(16.0)).color(p.faint))
}

/// The run time, right-aligned in a slot wide enough for "59s", so ticking never moves what
/// sits beside it.
fn time(p: &Pal, call: &str, ms: Option<i64>) -> Option<El> {
    let ms = ms?;
    let t = run_time(format!("time-{call}"), ms, UI_SMALL.lh(16.0), p.faint);
    Some(row().key("time").min_w(24.0).justify_end().child(t))
}

/// The words a task's title shows in, fading out where they're cut. "Delegating agent…" holds
/// its place until the call is written, then rolls up as the task rises in.
fn title(p: &Pal, c: &Call, font: TextStyle) -> El {
    if c.pending {
        return row().key("delegating").child(crate::agent_start::words(p, font)).transition(focus(0.5));
    }
    text(c.title.to_owned()).key("title").font(font).color(p.text).truncate_fade().max_w(TITLE_W).transition(focus(0.5))
}

/// How the row comes into focus when its agent starts: each part sharpens in place from a
/// 2pt blur over `response` seconds, while "Delegating agent…" blurs away faster, so the
/// words barely overlap and nothing travels.
fn focus(response: f32) -> Transition {
    Transition::blur(2.0).springs(SpringPair::new(Spring::new(response, 0.0), Spring::new(0.2, 0.0)))
}

/// What the row says beyond its task comes into focus a beat behind it.
fn follow() -> Transition {
    focus(0.7)
}

/// What the agent is doing now, or how it ended, in a slot `h` tall: each new line rolls in
/// over the last, keyed by its step so the same words never swap in place. The fades sit in
/// the slot's spare height.
fn now(p: &Pal, r: &Run, font: TextStyle, h: f32) -> El {
    let (key, color) = match r.state {
        State::Running => (format!("step-{}", r.taken + r.steps.len()), p.faint),
        State::Done => ("result".to_owned(), p.faint),
        State::Failed => ("error".to_owned(), p.danger),
        State::Steered => ("steered".to_owned(), p.faint),
    };
    let line = text(r.line()).key(key).font(font).color(color).truncate_fade().transition(roll());
    row().key("now").h(h).grow(1.0).shrink().min_w(0.0).center_y().clip_fade(0.0, 5.0, 0.0, 5.0).child(line)
}

/// Makes `surface` open the call's session.
fn opens(parent: &str, c: &Call, surface: El) -> El {
    let Some(child) = c.child else { return surface };
    let (parent, child, title, agent) = (parent.to_owned(), child.to_owned(), c.title.to_owned(), c.agent.to_owned());
    let (visible, hover) = (child.clone(), child.clone());
    surface
        .role("button")
        .label(format!("{} {}", model::capitalize(&agent), title))
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        // Virtual rows mount around the viewport. Read the child while its card is being
        // read, rather than starting a tunnel round trip only after the press.
        .on_layout_tx(move |s: &mut App, _, tx| {
            tx.live_only();
            s.prefetch_session(&visible);
        })
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            tx.live_only();
            if inside { s.prefetch_session(&hover); }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.open_child(&parent, &child, &title, &agent, tx);
        })
}

/// One agent's live row, `h` tall: its mark, task, what it's doing now, type and time, and a
/// chevron on hover when there's a session to open.
fn live_row(p: &Pal, c: &Call, h: f32) -> El {
    let r = &c.run;
    let line = row()
        .w_full()
        .h(h)
        .gap(8.0)
        .center_y()
        .child(mark(p, c))
        .child(title(p, c, UI_MED.lh(20.0)));
    if c.pending {
        return line;
    }
    line.child(now(p, r, UI.lh(20.0), h - 6.0).transition(follow()))
        .child(tag(p, c.agent).transition(follow()))
        .children(time(p, &c.key, r.ms).map(|t| t.transition(follow())))
        .children(c.child.map(|_| icon(&ic::CHEVRON_RIGHT, 16.0).key("open").color(p.icon_faint).group_opacity(0.0, 1.0)))
}
