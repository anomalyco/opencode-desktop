//! A subagent call's row in the parent's timeline: Desktop's compact chip (`tool-renderer.tsx`'s
//! `subagent` renderer, `basic-tool.css`'s `task-tool-card`), one per call. The agent's name
//! and its task on `layer-02`, with Desktop's spinner before them while it runs and a chevron
//! after them on hover when there's a session to open; "Delegating agent…" while the call is
//! still being written. A press opens the child session.

use rune::{Color, El, TextStyle, Tx, el, icon, row, text};

use opencode_kit::model::{self, ToolStatus};
use rune_theme_opencode::icons as ic;

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{INSTANT, activity_ring_sized};
use crate::turns::Block;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Running,
    Done,
    Failed,
    /// A later call to the same agent took over while this one's work was under way.
    Steered,
}

/// One call's run, as the host tracks it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub state: State,
    /// The latest steps, newest last ("Read backoff.ts"), and how many came before them.
    pub steps: Vec<String>,
    pub taken: usize,
    /// How long it has run, or ran, in ms.
    pub ms: Option<i64>,
    /// Its answer's first line, once done.
    pub result: Option<String>,
    /// What went wrong, once failed.
    pub error: Option<String>,
}

impl Run {
    pub fn from_status(status: &ToolStatus) -> Self {
        let state = match status {
            ToolStatus::Running | ToolStatus::Pending => State::Running,
            ToolStatus::Error => State::Failed,
            ToolStatus::Completed => State::Done,
        };
        Run { state, steps: Vec::new(), taken: 0, ms: None, result: None, error: None }
    }

    /// The one line that says where it is: its step while running, else its result or error.
    pub fn line(&self) -> String {
        match self.state {
            State::Running => self.steps.last().cloned().unwrap_or_else(|| opencode_i18n::t!("rune.subagent.starting").to_owned()),
            State::Done => self.result.clone().unwrap_or_else(|| opencode_i18n::t!("session.child.status.succeeded").to_owned()),
            State::Failed => self.error.clone().unwrap_or_else(|| opencode_i18n::t!("session.child.status.failed").to_owned()),
            State::Steered => opencode_i18n::t!("rune.subagent.steered").to_owned(),
        }
    }
}

/// "12s", "1m 05s", "1h 02m" (`formatElapsed`).
pub fn elapsed(ms: i64) -> String {
    let t = (ms / 1000).max(0);
    match t {
        0..60 => format!("{t}s"),
        60..3600 => format!("{}m {:02}s", t / 60, t % 60),
        _ => format!("{}h {:02}m", t / 3600, t / 60 % 60),
    }
}

/// A run time, "12s", in tabular digits. It changes at once, as Desktop's run times do: a digit
/// that rolls each second smears the number while it's read.
pub fn run_time(key: String, ms: i64, font: TextStyle, color: Color) -> El {
    text(elapsed(ms)).key(key).font(font.features("\"tnum\"")).color(color)
}

/// The chip's agent name (`task-tool-title`): 13/20 at 530, tracking -0.04 (`basic-tool.css:373-384`).
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
/// Its task (`basic-tool-tool-subtitle` in the card): 13/20 at 440 (`basic-tool.css:386-397`).
const TASK: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// `task-tool-surface`: 8 by 12 inside, 8 pt corners (`basic-tool.css:366-371`).
const PAD_Y: f32 = 8.0;
const PAD_X: f32 = 12.0;
const RADIUS: f32 = 8.0;
/// Between the spinner, the name and the task (`basic-tool-tool-info-main`'s gap).
const GAP: f32 = 8.0;
/// Desktop's activity spinner (`SessionProgressIndicatorV2`), and the chevron (`task-tool-action`).
const MARK: f32 = 16.0;

/// The row for `blocks[i]`, a subagent call.
pub fn row_for<'a, S: TimelineHost>(s: &'a S, p: &Pal, parent: &str, blocks: &'a [Block<'a>], i: usize) -> El {
    let Block::Agent { id, agent, title, status, session, metadata } = &blocks[i] else { return row() };
    let surface = row().key("agent-line").h(PAD_Y * 2.0 + 20.0).px(PAD_X).gap(GAP).center_y().radius(RADIUS).fill(p.layer2).shrink().min_w(0.0);
    // Still being written: Desktop's "Delegating agent…" chip, 36 pt with its spinner
    // (`task-tool-delegating`).
    if **status == ToolStatus::Pending {
        let words = crate::agent_start::words(p, TITLE).key("delegating").shrink().min_w(0.0).truncate_fade();
        let chip = surface.child(spinner(p, true)).child(words);
        return row().key(format!("member-{id}")).w_full().child(chip);
    }
    let child = s.resolve_subagent(parent, *session, title);
    let run = s.subagent_run(parent, id, child, status);
    let running = run.state == State::Running;
    // A finished call whose result says it's still running went to the background.
    let background = metadata["background"].as_bool() == Some(true) || (**status == ToolStatus::Completed && metadata["status"].as_str() == Some("running"));
    let task = if title.is_empty() { child.unwrap_or_default().to_owned() } else if background { opencode_i18n::t!("rune.subagent.background", title = title) } else { (*title).to_owned() };
    let opens = s.can_open_subagent() && child.is_some();
    let words = row().key("words").gap(GAP).center_y().shrink().min_w(0.0)
        .children(running.then(|| spinner(p, child.is_none())))
        .child(text(model::capitalize(agent)).key("title").font(TITLE).color(p.text))
        .children((!task.is_empty()).then(|| text(task.clone()).key("task").font(TASK).color(p.text).shrink().min_w(0.0).truncate_fade()));
    // The chevron's slot keeps its 8 pt gap at rest with no width, and takes 2 + 16 on hover
    // (`task-tool-action`): the chip's fill reaches 18 pt further right under the pointer, with
    // the chevron 2 pt past the gap.
    let chevron = opens.then(|| {
        let reach = 2.0 + MARK;
        let fill = el().key("open-fill").pin(None, Some(0.0), Some(-reach), Some(0.0)).w(reach + RADIUS).corners([0.0, RADIUS, RADIUS, 0.0]).fill(p.layer2);
        let glyph = el().key("open").pin(None, Some(PAD_Y + 2.0), Some(PAD_X - reach), None).size(MARK, MARK).child(icon(&ic::CHEVRON_RIGHT, MARK).color(p.faint));
        [fill.group_opacity(0.0, 1.0), glyph.group_opacity(0.0, 1.0)]
    });
    let surface = if opens { surface.pr(PAD_X + GAP) } else { surface };
    let chip = surface.children(chevron.into_iter().flatten()).child(words);
    let chip = match (opens, child) {
        (true, Some(child)) => clickable::<S>(parent, child, title, agent, &task, chip),
        _ => chip,
    };
    row().key(format!("member-{id}")).w_full().child(chip)
}

/// Desktop's spinner while the call runs; Kit's starting dots until its agent starts, waking
/// into the radar in place (`agent_start::spinner`).
fn spinner(p: &Pal, starting: bool) -> El {
    let mark = if starting { crate::agent_start::spinner(p, true) } else { activity_ring_sized(p, MARK) };
    el().key("mark").size(MARK, MARK).center().child(mark)
}

/// Makes `chip` open the call's session.
fn clickable<S: TimelineHost>(parent: &str, child: &str, title: &str, agent: &str, task: &str, chip: El) -> El {
    let (parent, child, title, agent) = (parent.to_owned(), child.to_owned(), title.to_owned(), agent.to_owned());
    let (visible, hover) = (child.clone(), child.clone());
    chip.role("link")
        .label(format!("{} {task}", model::capitalize(&agent)))
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        // Virtual rows mount around the viewport. Read the child while its row is being
        // read, rather than starting a tunnel round trip only after the press.
        .on_layout_tx(move |s: &mut S, _, tx| {
            tx.live_only();
            s.prefetch_subagent(&visible);
        })
        .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
            tx.live_only();
            if inside { s.prefetch_subagent(&hover); }
        })
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            s.open_subagent(&parent, &child, &title, &agent, tx);
        })
}
