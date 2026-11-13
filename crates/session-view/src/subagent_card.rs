//! A subagent call's row in the parent's timeline: Desktop's compact chip (`tool-renderer.tsx`'s
//! `subagent` renderer, `basic-tool.css`'s `task-tool-card`), one per call. The agent's name
//! and its task on `layer-02`, with Desktop's spinner before them while it runs and a chevron
//! after them on hover when there's a session to open; "Delegating agent…" while the call is
//! still being written. A press opens the child session.

use rune::{Color, El, Icon, TextStyle, Tx, el, icon, row, text};

use opencode_kit::model::{self, ToolStatus};

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{HOVER_FADE, INSTANT, activity_ring_sized};
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

/// `chevron-right` at `size="normal"`: Desktop's new `chevron-up` turned a quarter right
/// (`new-icons.ts:31, 100`), round caps and joins on its 16 pt grid.
static CHEVRON: Icon = Icon {
    name: "oc-chevron-right-v2",
    view: 16.0,
    body: r#"<path d="M6.5 4.5L9.5 8L6.5 11.5" stroke-linecap="round" stroke-linejoin="round" stroke="currentColor"/>"#,
};

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
    // (`task-tool-action`, `basic-tool.css:286-303, 336-347`): the chip's fill reaches 18 pt
    // further right under the pointer, and the chevron comes in from 4 pt short of its place,
    // out of the slot growing under it, fading in; all of it over 0.15 s ease, both ways.
    let hovered = s.subagent_hover() == Some(*id);
    let chevron = opens.then(|| {
        let reach = 2.0 + MARK;
        // The slot growing: the fill slides out from under the chip's own end, whose colour
        // and corners it has, so only the part past the end shows.
        let shift = |rest: f32| if hovered { 0.0 } else { rest };
        let fill = el().key("open-fill").pin(None, Some(0.0), Some(-reach), Some(0.0)).w(reach + RADIUS).corners([0.0, RADIUS, RADIUS, 0.0]).fill(p.layer2)
            .offset(shift(-reach), 0.0).offset_motion(fade(s));
        // Centred in the growing slot, 4 pt back from its place (`translateX(-4px)`): 14 pt short
        // at rest, where the chip's end hides it under its fade.
        let glyph = el().key("open").pin(None, Some(PAD_Y + 2.0), Some(PAD_X - reach), None).size(MARK, MARK).child(icon(&CHEVRON, MARK).mirror_in_rtl().color(p.faint))
            .offset(shift(4.0 - reach), 0.0).offset_motion(fade(s));
        [fill, glyph.group_opacity(0.0, 1.0)]
    });
    let surface = if opens { surface.pr(PAD_X + GAP) } else { surface };
    let chip = surface.children(chevron.into_iter().flatten()).child(words);
    let chip = match (opens, child) {
        (true, Some(child)) => clickable::<S>(parent, id, child, title, agent, &task, chip.hover_motion(fade(s), fade(s))),
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

/// The chip's hover: 0.15 s ease both ways (`basic-tool.css:297-302`), at once under reduced
/// motion (`:root[data-motion="reduce"]`, `:315-317`).
fn fade<S: TimelineHost>(s: &S) -> rune::Anim {
    if s.reduced_motion() { INSTANT } else { HOVER_FADE }
}

/// Makes `chip`, call `call`'s, open the call's session.
fn clickable<S: TimelineHost>(parent: &str, call: &str, child: &str, title: &str, agent: &str, task: &str, chip: El) -> El {
    let (parent, child, title, agent) = (parent.to_owned(), child.to_owned(), title.to_owned(), agent.to_owned());
    let (visible, hover, call) = (child.clone(), child.clone(), call.to_owned());
    chip.role("link")
        .label(format!("{} {task}", model::capitalize(&agent)))
        .hover_group()
        // Virtual rows mount around the viewport. Read the child while its row is being
        // read, rather than starting a tunnel round trip only after the press.
        .on_layout_tx(move |s: &mut S, _, tx| {
            tx.live_only();
            s.prefetch_subagent(&visible);
        })
        .on_hover_change(move |s: &mut S, inside, _: &mut Tx| {
            if inside {
                s.prefetch_subagent(&hover);
                s.set_subagent_hover(Some(call.clone()));
            } else if s.subagent_hover() == Some(call.as_str()) {
                s.set_subagent_hover(None);
            }
        })
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            // A click with a modifier opens nothing (`tool-renderer.tsx` `navigate`).
            if crate::plain_click(tx) {
                tx.instant();
                s.open_subagent(&parent, &child, &title, &agent, tx);
            }
        })
}
