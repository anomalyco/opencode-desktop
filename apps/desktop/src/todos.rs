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

pub const TICK: Knob = Knob::new("Todo tick", 0.0, 0.5, 0.2).step(0.01).label("Tick draws in").about("Seconds a finished todo's tick takes to grow");

pub const GROUP: Group = Group::new("Todo pill", "todos", &[TICK]).under("Composer");

rune_register::register!(crate::knobs::GROUP_LIST: Group = [GROUP]);

/// The open session's plan as last seen, and the ticks of todos that finished while it showed,
/// each growing in on its own track (t1150).
#[derive(Default)]
pub struct Ticks {
    session: String,
    seen: Vec<(String, Status)>,
    growing: HashMap<usize, Tween<f32>>,
}

/// Closes the list once its pill goes, so the next plan doesn't bring it back open (t1132), and
/// starts a tick for each todo that has just finished.
pub fn sync(s: &mut App) -> bool {
    let crate::Route::Session(id) = &s.route else { return false };
    let todos = s.shown_todos(id);
    let secs = s.knobs.get(&TICK);
    let ticks = &mut s.todo_ticks;
    if ticks.session != *id {
        *ticks = Ticks { session: id.clone(), ..Ticks::default() };
    } else if !s.reduced_motion && secs > 0.0 {
        for (k, t) in todos.iter().enumerate() {
            let was = ticks.seen.get(k).filter(|(c, _)| *c == t.content).map(|(_, st)| *st);
            if t.status == Status::Completed && was.is_some_and(|st| st != Status::Completed) {
                ticks.growing.insert(k, Tween::linear(0.0, 1.0, Duration::from_secs_f32(secs)));
            }
        }
    }
    ticks.seen = todos.iter().map(|t| (t.content.clone(), t.status)).collect();
    ticks.growing.retain(|k, _| todos.get(*k).is_some_and(|t| t.status == Status::Completed));
    if !s.todo_open || current(&todos).is_some() {
        return false;
    }
    s.todo_open = false;
    true
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
    let widest = todos.iter().map(|t| rune::text::measure(&t.content, &[], ROW, None).0).fold(0.0, f32::max);
    // Panel inset 4 + hairline 0.5, row inset 6, glyph 16 and its gap 8.
    let natural = 2.0 * (PAD + 0.5 + 6.0) + 16.0 + 8.0 + widest;
    let mut list = col().key("todo-list").w_full().padding(PAD);
    for (k, t) in todos.iter().enumerate() {
        let color = match t.status {
            Status::InProgress => p.text,
            Status::Pending => p.muted,
            Status::Completed | Status::Cancelled => p.faint,
        };
        let mut words = text(t.content.clone()).key("text").font(ROW).color(color).wrap().grow(1.0).shrink().min_w(0.0);
        if t.finished() {
            words = words.strike(0, t.content.len(), None);
        }
        list = list.child(
            row()
                .key(format!("todo-{k}"))
                .w_full()
                .px(6.0)
                .py(4.0)
                .gap(8.0)
                .child(el().h(ROW.line_height).center().child(glyph(p, t.status, s.todo_ticks.growing.get(&k))))
                .child(words),
        );
    }
    let rows = col()
        .key("todo-scroll")
        .w_full()
        .max_h(MAX_H)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(12.0)
        .scroll_margin(PAD)
        // What it holds comes in with the list, not on fades of its own.
        .instant_changes()
        .child(list);
    Some(
        col()
            .key("todo-panel")
            .role("dialog")
            .label(label(&todos))
            .above("todo-pill", 8.0)
            .w(natural.clamp(240.0, 400.0).ceil())
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

/// Desktop's 16 pt status glyph: a ring, half filled in progress, ticked when done, struck
/// through when cancelled. In progress is the warning colour; done is a step brighter than
/// pending and cancelled. A todo that has just finished grows its tick in on `growing`, from
/// the half circle it had, as its ring fills to done's colour.
fn glyph(p: &Pal, status: Status, growing: Option<&Tween<f32>>) -> El {
    let (shape, color) = match status {
        Status::Pending => (&PENDING, p.icon_faint),
        Status::InProgress => (&IN_PROGRESS, p.warning),
        Status::Completed => (&COMPLETED, p.icon_muted),
        Status::Cancelled => (&CANCELLED, p.icon_faint),
    };
    // The shape changes in place; only the growing tick mounts.
    let shape = if growing.is_some() { &PENDING } else { shape };
    el().key("todo-glyph").size(16.0, 16.0)
        .child(icon(shape, 16.0).key("shape").color(color))
        .children(growing.map(|t| icon(&TICK_MARK, 16.0).key("tick").color(color).absolute(0.0, 0.0).live_scale(t).live_opacity(t)))
}

static TICK_MARK: Icon = Icon {
    name: "todo-tick",
    view: 16.0,
    body: r#"<path d="M5 8 7.5 11 11 5.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

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
    body: r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor"/><path d="M5 8 7.5 11 11 5.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"/>"#,
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
/// A list row's words: 13 pt on the base line height.
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
const BADGE: TextStyle = TextStyle::new(11.0, 500.0).lh(16.0);

#[cfg(test)]
#[path = "todos_tests.rs"]
mod tests;
