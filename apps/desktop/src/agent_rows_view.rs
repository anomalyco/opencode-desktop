//! A subagent call's row in the parent's timeline: agents started together share one card,
//! a live row each (`agent_rows.rs` has what each row knows).
//!
//! A new step rises in from below as the old one rises out, the time rolls like an odometer
//! (`odometer.rs`), the mark crossfades and scales between spinner, ✓ and ✕, cut-off words fade
//! out at the edge, and agents that join the card rise into place at the bottom (nothing above them
//! moves). Hover and click are instant.

use rune::{El, Spring, SpringPair, TextStyle, Transition, Tx, col, el, icon, row, text};

use opencode_kit::model::{self, ToolStatus};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::agent_rows::{Run, State};
use crate::odometer::odometer_end;
use crate::palette::Pal;
use crate::subagents::elapsed;
use crate::turns::Block;
use crate::view::{INSTANT, UI, UI_MED, UI_SMALL, activity_ring, activity_ring_on};

/// The widest a task gets, so what the agent is doing always shows beside it.
const TITLE_W: f32 = 260.0;

/// One call, ready to draw.
struct Call<'a> {
    key: String,
    agent: &'a str,
    title: &'a str,
    child: Option<&'a str>,
    run: Run,
}

/// The row for `blocks[i]`, a subagent call: one card holding it and the calls that follow it
/// (`skip` says which rows they took). A call still being written is Desktop's "Delegating
/// agent…" pill.
pub fn row_for<'a>(s: &'a App, p: &Pal, parent: &str, blocks: &'a [Block<'a>], i: usize) -> El {
    let call = |b: &'a Block<'a>| match b {
        Block::Agent { id, agent, title, status, session, .. } if **status != ToolStatus::Pending => {
            let child = crate::subagents::child_of(s, parent, *session, title);
            Some(Call { key: id.to_string(), agent, title, child, run: s.agent_run(child, status) })
        }
        _ => None,
    };
    let calls: Vec<Call> = blocks[i..].iter().map_while(call).collect();
    if calls.is_empty() {
        return delegating(p);
    }
    let mut card = col().key("agent-card").w_full().px(4.0).py(4.0).radius(10.0).fill(p.layer2);
    for c in &calls {
        let line = live_row(p, c, 32.0, p.layer2).key("agent-line").px(8.0).radius(6.0).hover(p.hover);
        card = card.child(row().key(format!("member-{}", c.key)).w_full().child(opens(parent, c, line)).transition(arrive()));
    }
    row().w_full().child(card)
}

/// Whether `blocks[i]` is drawn inside the card of the call above it.
pub fn skip(blocks: &[Block], i: usize) -> bool {
    let agent = |b: &Block| matches!(b, Block::Agent { status, .. } if **status != ToolStatus::Pending);
    i > 0 && agent(&blocks[i]) && agent(&blocks[i - 1])
}

fn delegating(p: &Pal) -> El {
    row().w_full().child(
        row()
            .key("delegating")
            .h(36.0)
            .px(12.0)
            .gap(8.0)
            .center_y()
            .radius(8.0)
            .fill(p.layer2)
            .child(activity_ring(p))
            .child(text("Delegating agent…").font(UI_MED).color(p.text)),
    )
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

/// The 16pt mark: a spinner, a ✓ or a ✕, each scaling and fading into the last one's place.
fn mark(p: &Pal, state: State, on: rune::Color) -> El {
    let (key, glyph) = match state {
        State::Running => ("spin", activity_ring_on(p, on, 14.0)),
        State::Done => ("done", icon(&ic::CHECK_SMALL, 16.0).color(p.success)),
        State::Failed => ("failed", icon(&ic::CIRCLE_X, 16.0).color(p.danger)),
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

/// The run time, rolling like an odometer, right-aligned in a slot wide enough for "59s", so
/// ticking never moves what sits beside it.
fn time(p: &Pal, call: &str, ms: Option<i64>) -> Option<El> {
    let ms = ms?;
    let t = odometer_end(format!("time-{call}"), elapsed(ms), (ms / 1000) as f64, UI_SMALL.lh(16.0), p.faint);
    Some(row().key("time").min_w(24.0).justify_end().motion(crate::odometer::SPRING).width_leads().child(t))
}

/// The words a task's title shows in, fading out where they're cut.
fn title(p: &Pal, c: &Call, font: TextStyle) -> El {
    text(c.title.to_owned()).key("title").font(font).color(p.text).truncate_fade().max_w(TITLE_W)
}

/// What the agent is doing now, or how it ended, in a slot `h` tall: each new line rolls in
/// over the last, keyed by its step so the same words never swap in place. The fades sit in
/// the slot's spare height.
fn now(p: &Pal, r: &Run, font: TextStyle, h: f32) -> El {
    let (key, color) = match r.state {
        State::Running => (format!("step-{}", r.taken + r.steps.len()), p.faint),
        State::Done => ("result".to_owned(), p.faint),
        State::Failed => ("error".to_owned(), p.danger),
    };
    let line = text(r.line()).key(key).font(font).color(color).truncate_fade().transition(roll());
    row().key("now").h(h).grow(1.0).shrink().min_w(0.0).center_y().clip_fade(0.0, 5.0, 0.0, 5.0).child(line)
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

/// One agent's live row, `h` tall: its mark, task, what it's doing now, type and time, and a
/// chevron on hover when there's a session to open.
fn live_row(p: &Pal, c: &Call, h: f32, on: rune::Color) -> El {
    let r = &c.run;
    row()
        .w_full()
        .h(h)
        .gap(8.0)
        .center_y()
        // The time widening ("9s" → "10s") eases, and what sits beside it slides over with it.
        .animate_layout()
        .child(mark(p, r.state, on))
        .child(title(p, c, UI_MED.lh(20.0)))
        .child(now(p, r, UI.lh(20.0), h - 6.0))
        .child(tag(p, c.agent))
        .children(time(p, &c.key, r.ms))
        .children(c.child.map(|_| icon(&ic::CHEVRON_RIGHT, 16.0).key("open").color(p.icon_faint).group_opacity(0.0, 1.0)))
}
