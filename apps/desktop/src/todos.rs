//! The agent's plan as a pill over the composer, beside the subagent capsule, as Desktop shows it
//! (`app/src/session/composer/todo-pill.tsx`, `todos.ts`): the todo in hand, opening a list of
//! the whole plan. The `todowrite` tool keeps no server state, so the plan is the input of the
//! session's latest finished call before any staged revert. The timeline hides those calls.

use std::collections::HashMap;
use std::time::Duration;

use rune::{El, Icon, TextStyle, Tween, Tx, col, el, icon, row, text};
use rune_knobs::{Group, Knob};
use serde_json::Value;

use opencode_kit::model::{Message, Part, ToolStatus, Transcript};

use crate::App;
use crate::palette::Pal;
use crate::view::UI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    pub content: String,
    pub status: Status,
}

impl Todo {
    pub fn finished(&self) -> bool {
        matches!(self.status, Status::Completed | Status::Cancelled)
    }
}

/// The plan in a transcript: the latest completed `todowrite` call's todos, among the messages
/// a staged revert at `boundary` leaves.
pub fn session_todos(t: &Transcript, boundary: Option<&str>) -> Vec<Todo> {
    let input = t.messages[..t.visible_len(boundary)]
        .iter()
        .rev()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        })
        .flat_map(|a| a.parts.iter().rev())
        .find_map(|p| match p {
            Part::Tool { name, status: ToolStatus::Completed, input, .. } if name == "todowrite" => Some(input),
            _ => None,
        });
    input.map_or_else(Vec::new, parse)
}

fn parse(input: &Value) -> Vec<Todo> {
    let Some(items) = input["todos"].as_array() else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let content = item["content"].as_str().filter(|c| !c.is_empty())?;
            let status = match item["status"].as_str() {
                Some("in_progress") => Status::InProgress,
                Some("completed") => Status::Completed,
                Some("cancelled") => Status::Cancelled,
                _ => Status::Pending,
            };
            Some(Todo { content: content.to_owned(), status })
        })
        .collect()
}

/// The todo the agent is on: the one in progress, else the next pending one, else the last
/// finished one.
pub fn current(todos: &[Todo]) -> Option<&Todo> {
    todos
        .iter()
        .find(|t| t.status == Status::InProgress)
        .or_else(|| todos.iter().find(|t| t.status == Status::Pending))
        .or_else(|| todos.iter().rev().find(|t| t.status == Status::Completed))
        .or(todos.first())
}

impl App {
    /// The plan the pill shows: only while the session runs and work is left, so a finished or
    /// abandoned plan never lingers.
    pub fn shown_todos(&self, id: &str) -> Vec<Todo> {
        if !self.running.contains(id) {
            return Vec::new();
        }
        let Some(t) = self.transcripts.get(id) else { return Vec::new() };
        let todos = session_todos(t, self.session(id).and_then(|i| i.revert.as_deref()));
        if todos.iter().all(Todo::finished) { Vec::new() } else { todos }
    }
}

pub const CLOSE: Knob = Knob::new("Todo ring", 0.0, 0.8, 0.4).step(0.01).label("Ring closes in").about("Seconds a finished todo's fill takes to close");
pub const BOUNCE: Knob = Knob::new("Todo bounce", 0.0, 0.7, 0.5).step(0.01).label("Ring bounce").about("How much the closing fill springs past full");

pub const GROUP: Group = Group::new("Todo pill", "todos", &[CLOSE, BOUNCE]).under("Composer");

rune_register::register!(crate::knobs::GROUP_LIST: Group = [GROUP]);

/// The open session's plan as last seen, and the todos that finished while it showed, each
/// filling its circle and striking its words on tracks of its own (t1150).
#[derive(Default)]
pub struct Ticks {
    session: String,
    seen: Vec<(String, Status)>,
    growing: HashMap<usize, Fill>,
}

/// A todo finishing: its fill sweeps clockwise from the half it had round to a full disc and
/// springs out to size, both on a spring of `CLOSE` seconds; from halfway through, a clip over
/// its struck words opens left to right at the same pace. It mounts as it was (`swept` false)
/// and springs on the next frame.
struct Fill {
    from: Status,
    swept: bool,
    spring: rune::Spring,
    pop: rune::Spring,
    strike: Tween<[f32; 4]>,
}

impl Fill {
    fn new(from: Status, content: &str, wrap: f32, secs: f32, bounce: f32) -> Self {
        let (w, _) = rune::text::measure(content, &[], ROW, Some(wrap));
        let w = w.max(1.0);
        // The clip runs on past the words to the widest a row can be, so a list that widens
        // later never cuts a finished strike; it opens at zero halfway through the fill.
        let (from_w, to_w) = (-w / 2.0, WIDEST);
        let strike = Duration::from_secs_f32(secs * (to_w - from_w) / w);
        Fill {
            from,
            swept: false,
            spring: rune::Spring::new(secs, 0.0),
            pop: rune::Spring::new(secs, bounce),
            strike: Tween::linear([0.0, 0.0, from_w, TALLEST], [0.0, 0.0, to_w, TALLEST], strike),
        }
    }
}

/// Closes the list once its pill goes, so the next plan doesn't bring it back open (t1132), and
/// starts a fill for each todo that has just finished.
pub fn sync(s: &mut App, tx: &mut Tx) -> bool {
    let crate::Route::Session(id) = &s.route else { return false };
    let todos = s.shown_todos(id);
    let (secs, bounce) = (s.knobs.get(&CLOSE), s.knobs.get(&BOUNCE));
    let ticks = &mut s.todo_ticks;
    let mut changed = false;
    for fill in ticks.growing.values_mut().filter(|f| !f.swept) {
        fill.swept = true;
        changed = true;
    }
    if ticks.session != *id {
        *ticks = Ticks { session: id.clone(), ..Ticks::default() };
    } else if !s.reduced_motion && secs > 0.0 {
        let wrap = words_width(&todos);
        for (k, t) in todos.iter().enumerate() {
            let was = ticks.seen.get(k).filter(|(c, _)| *c == t.content).map(|(_, st)| *st);
            if let Some(was) = was.filter(|st| *st != Status::Completed && t.status == Status::Completed) {
                ticks.growing.insert(k, Fill::new(was, &t.content, wrap, secs, bounce));
                tx.wake_after(Duration::ZERO);
                changed = true;
            }
        }
    }
    ticks.seen = todos.iter().map(|t| (t.content.clone(), t.status)).collect();
    ticks.growing.retain(|k, _| todos.get(*k).is_some_and(|t| t.status == Status::Completed));
    if !s.todo_open || current(&todos).is_some() {
        return changed;
    }
    s.todo_open = false;
    true
}

/// The list's width for these todos: its widest todo's words on one line, from 240 to 400.
fn panel_width(todos: &[Todo]) -> f32 {
    let widest = todos.iter().map(|t| rune::text::measure(&t.content, &[], ROW, None).0).fold(0.0, f32::max);
    // Panel inset 4 + hairline 0.5, row inset 6, glyph 16 and its gap 8.
    let natural = 2.0 * (PAD + 0.5 + 6.0) + 16.0 + 8.0 + widest;
    natural.clamp(240.0, 400.0).ceil()
}

/// The width a row's words wrap to.
fn words_width(todos: &[Todo]) -> f32 {
    panel_width(todos) - 2.0 * (PAD + 0.5 + 6.0) - 16.0 - 8.0
}

/// "2 of 5 todos done", the pill's and list's name for screen readers.
fn label(todos: &[Todo]) -> String {
    let done = todos.iter().filter(|t| t.status == Status::Completed).count();
    let n = todos.len();
    format!("{done} of {n} todo{} done", if n == 1 { "" } else { "s" })
}

/// The pill: the current todo's glyph and words, cut with a fade past 260, and "+2" when more
/// than one is in progress.
pub fn pill(s: &App, p: &Pal, id: &str) -> Option<El> {
    let todos = s.shown_todos(id);
    let now = current(&todos)?;
    let more = todos.iter().filter(|t| t.status == Status::InProgress).count().saturating_sub(1);
    let sid = id.to_owned();
    let mut chip = crate::subagents::chip(s, p, "todo-pill", label(&todos), s.todo_open)
        .pl(8.0)
        .gap(6.0)
        .shrink()
        .min_w(0.0)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            s.todo_open = !s.todo_open;
            s.activity_open = false;
            crate::popover::motion::toggled(&s.knobs, tx, s.todo_open);
            if s.todo_open {
                reveal_current(s, &sid, tx);
            }
        })
        .child(glyph(p, now.status, None))
        .child(text(now.content.clone()).key("todo-now").font(UI).color(p.text).truncate_fade().max_w(PILL_TEXT_MAX).shrink().min_w(0.0));
    if more > 0 {
        chip = chip.child(
            row().key("todo-more").h(18.0).px(5.0).center().radius(4.0).fill(p.layer2)
                .child(text(format!("+{more}")).font(BADGE).color(p.muted)),
        );
    }
    Some(chip)
}

/// The list (`session-todo-panel`), at once above the pill, start edges aligned: every todo
/// with its glyph, the one in progress in full text, finished ones struck through and faint.
/// At least 240 wide, at most 400, its words wrapping; past 232 it scrolls, keeping the todo in
/// progress in view.
pub fn panel(s: &App, p: &Pal, id: &str) -> Option<El> {
    if !s.todo_open {
        return None;
    }
    let todos = s.shown_todos(id);
    if todos.is_empty() {
        return None;
    }
    let wrap = words_width(&todos);
    let mut list = col().key("todo-list").transition(rune::Transition::none()).w_full().padding(PAD);
    for (k, t) in todos.iter().enumerate() {
        let color = match t.status {
            Status::InProgress => p.text,
            Status::Pending => p.muted,
            Status::Completed | Status::Cancelled => p.faint,
        };
        let fill = s.todo_ticks.growing.get(&k);
        list = list.child(
            row()
                .key(format!("todo-{k}"))
                .transition(rune::Transition::none())
                .w_full()
                .px(6.0)
                .py(4.0)
                .gap(8.0)
                .child(el().h(ROW.line_height).center().transition(rune::Transition::none()).child(glyph(p, t.status, fill)))
                .child(words(t, color, wrap, fill)),
        );
    }
    let rows = col()
        .key("todo-scroll")
        .transition(rune::Transition::none())
        .w_full()
        .max_h(MAX_H)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(12.0)
        .scroll_margin(PAD)
        // What it holds comes in with the list, not on fades of its own, and changes at once;
        // only a finishing todo's fill springs, on its own curves. An `instant_changes` here
        // would land those too, so each piece around the fill says so itself.
        .motion(rune::Spring::NONE)
        .child(list);
    Some(
        col()
            .key("todo-panel")
            .role("dialog")
            .label(label(&todos))
            .above("todo-pill", 8.0)
            .w(panel_width(&todos))
            .radius(10.0)
            .fill(p.layer1)
            .border(0.5, p.ring)
            .shadow(8.0, 16.0, p.shadow_far)
            .shadow(4.0, 8.0, p.shadow)
            .clip_soft(0.0)
            .transition(crate::popover::motion::transition(&s.knobs, rune::Side::Top, rune::Align::Start))
            .on_press(|_: &mut App| {})
            .child(rows),
    )
}

/// Keeps the todo in progress in view when the list opens or the plan moves on.
pub fn reveal_current(s: &App, id: &str, tx: &mut Tx) {
    if let Some(k) = s.shown_todos(id).iter().position(|t| t.status == Status::InProgress) {
        tx.scroll_into_view_in("todo-scroll", format!("todo-{k}"));
    }
}

/// A row's words, struck through once finished. A todo finishing draws its strike: the same
/// words, struck with clear glyphs, over its own, in a clip that opens left to right.
fn words(t: &Todo, color: rune::Color, wrap: f32, fill: Option<&Fill>) -> El {
    let words = || text(t.content.clone()).font(ROW).wrap().w_full();
    let own = words().key("text").color(color);
    let own = if t.finished() && fill.is_none() { own.strikes(strikes(&t.content, wrap, None)) } else { own };
    el().key("words").grow(1.0).shrink().min_w(0.0).transition(rune::Transition::none()).instant_changes().child(own).children(fill.map(|fill| {
        el().key("strike").transition(rune::Transition::none()).absolute(0.0, 0.0).w_full().h_full().clip_soft(0.0).live_rect(&fill.strike)
            .child(words().color(rune::Color::TRANSPARENT).strikes(strikes(&t.content, wrap, Some(color))))
    }))
}

/// One strike per line the words wrap to at `wrap`, each from its line's first word to its
/// last: a strike across the whole text would run on through the space each line ends at.
fn strikes(words: &str, wrap: f32, color: Option<rune::Color>) -> Vec<(usize, usize, Option<rune::Color>)> {
    if rune::text::measure(words, &[], ROW, None).0 <= wrap {
        return vec![(0, words.len(), color)];
    }
    let lines = |end: usize| (rune::text::measure(&words[..end], &[], ROW, Some(wrap)).1 / ROW.line_height).round();
    let mut out = Vec::new();
    let (mut start, mut end, mut line) = (None::<usize>, 0, 1.0);
    for (at, word) in words.split_inclusive(char::is_whitespace).scan(0, |at, w| {
        let here = *at;
        *at += w.len();
        Some((here, w.trim_end()))
    }) {
        if word.is_empty() {
            continue;
        }
        let n = lines(at + word.len());
        if n > line {
            out.extend(start.map(|s| (s, end, color)));
            (start, line) = (None, n);
        }
        start.get_or_insert(at);
        end = at + word.len();
    }
    out.extend(start.map(|s| (s, end, color)));
    out
}

/// Desktop's 16 pt status glyph: a ring, half filled in progress, filled when done, struck
/// through when cancelled. In progress is the warning colour; done is a step brighter than
/// pending and cancelled. A todo finishing shows its ring in done's colour, and inside it the
/// half fill it had, a little smaller, as the other half sweeps round and the whole disc
/// springs out to size.
fn glyph(p: &Pal, status: Status, fill: Option<&Fill>) -> El {
    let (shape, color) = match status {
        Status::Pending => (&PENDING, p.icon_faint),
        Status::InProgress => (&IN_PROGRESS, p.warning),
        Status::Completed => (&COMPLETED, p.icon_muted),
        Status::Cancelled => (&CANCELLED, p.icon_faint),
    };
    // The shape changes in place; only the closing disc mounts.
    let shape = if fill.is_some() { &PENDING } else { shape };
    el().key("todo-glyph").size(16.0, 16.0).transition(rune::Transition::none())
        .child(icon(shape, 16.0).key("shape").color(color).transition(rune::Transition::none()).instant_changes())
        .children(fill.map(|f| disc(f, color)))
}

/// A finishing todo's fill: the left half, and the right half turning in from behind it,
/// clipped to the right so it sweeps clockwise from the top. Nothing in it fades in.
fn disc(f: &Fill, color: rune::Color) -> El {
    let none = rune::Transition::none;
    let sweep = icon(&SWEEP, 16.0).key("sweep").transition(none()).color(color).absolute(-8.0, 0.0)
        .rotate(if f.swept { 0.0 } else { -std::f32::consts::PI })
        .rotate_motion(f.spring);
    el().key("disc").absolute(0.0, 0.0).size(16.0, 16.0).transition(none())
        .scale(if f.swept { 1.0 } else { POP_FROM })
        .scale_motion(f.pop)
        .child(icon(&LEFT_HALF, 16.0).transition(none()).color(color))
        .child(el().transition(none()).absolute(8.0, 0.0).size(8.0, 16.0).clip_soft(0.0).child(sweep))
}

/// How small a finishing todo's fill starts, so it springs out as it closes.
const POP_FROM: f32 = 0.75;

// Half a point past the middle, under the sweep, so no seam shows where they meet.
static LEFT_HALF: Icon = Icon { name: "todo-left-half", view: 16.0, body: r#"<path d="M8.5 4H8a4 4 0 0 0 0 8h.5V4Z" fill="currentColor"/>"# };
// The right half and 10° more, so the spring's last creep to rest leaves no gap at the bottom.
static SWEEP: Icon = Icon { name: "todo-sweep", view: 16.0, body: r#"<path d="M8 8V4a4 4 0 1 1-.695 7.939Z" fill="currentColor"/>"# };

const RING: &str = r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor"/>"#;

static PENDING: Icon = Icon { name: "todo-pending", view: 16.0, body: RING };
static IN_PROGRESS: Icon = Icon {
    name: "todo-in-progress",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor"/><path d="M8 4a4 4 0 0 0 0 8V4Z" fill="currentColor"/>"#,
};
static COMPLETED: Icon = Icon {
    name: "todo-completed",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor"/><circle cx="8" cy="8" r="4" fill="currentColor"/>"#,
};
static CANCELLED: Icon = Icon {
    name: "todo-cancelled",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor"/><path d="M3.5 12.5 12.5 3.5" stroke="currentColor"/>"#,
};

/// Desktop's `max-w-[260px]` on the pill's words.
const PILL_TEXT_MAX: f32 = 260.0;
const PAD: f32 = 4.0;
/// Desktop's `max-h-[232px]`.
const MAX_H: f32 = 232.0;
/// The widest a row's words can be, and taller than any wraps: the drawing strike's clip.
const WIDEST: f32 = 400.0;
const TALLEST: f32 = 2000.0;
/// A list row's words: 13 pt on the base line height.
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
const BADGE: TextStyle = TextStyle::new(11.0, 500.0).lh(16.0);

#[cfg(test)]
#[path = "todos_tests.rs"]
mod tests;
