//! The agent's plan as a pill over the composer, beside the subagent capsule, as Desktop shows it
//! (`app/src/session/composer/todo-pill.tsx`, `todos.ts`): the todo in hand, opening a list of
//! the whole plan. The `todowrite` tool keeps no server state, so the plan is the input of the
//! session's latest finished call before any staged revert. The timeline hides those calls.

use std::collections::HashMap;
use std::time::Duration;

use opencode_kit::model::{Message, Part, ToolStatus, Transcript};
use rune::{El, Icon, Shape, TextStyle, Tween, Tx, canvas, col, el, icon, row, text};
use rune_knobs::{Group, Knob, Knobs};
use serde_json::Value;
use session_view::focus::FocusExt;
use session_view::palette::{Elevate as _, Pal};
use session_view::style::{INSTANT, UI};

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

/// Object-safe host boundary for the todo pill and floating plan list over the composer.
pub trait TodosHost: opencode_kernel::AppEnv {
    fn is_session_running(&self, id: &str) -> bool;
    /// Transcript for session `id` (untracked read preferred to avoid unrelated rebuilds).
    fn transcript(&self, id: &str) -> Option<&Transcript>;
    /// Monotonic version of `id`'s transcript family for plan caching.
    fn transcript_version(&self, id: &str) -> u64;
    /// Active staged revert boundary for session `id`, if any.
    fn session_revert(&self, id: &str) -> Option<&str>;
    fn knobs(&self) -> &Knobs;
    fn todo_open(&self) -> bool;
    fn set_todo_open(&mut self, open: bool);
    fn todo_ticks(&self) -> &Ticks;
    fn todo_ticks_mut(&mut self) -> &mut Ticks;
    /// Called after the todo pill toggles (e.g. to close sibling composer popovers).
    fn on_todo_toggled(&mut self, open: bool) {
        let _ = open;
    }
    /// Base composer pill shell (desktop delegates to `subagents::chip` to share capsule knobs).
    fn todo_chip(&self, p: &Pal, key: &str, label: String, open: bool) -> El {
        default_chip(self.knobs(), p, key, label, open)
    }
}

type PlanCache = Option<((String, u64, Option<String>), Vec<Todo>)>;

thread_local! {
    /// The open session's plan, and the session, transcripts version and revert it was read at.
    static PLAN: std::cell::RefCell<PlanCache> = const { std::cell::RefCell::new(None) };
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

/// The plan the pill shows: only while the session runs and work is left, so a finished or
/// abandoned plan never lingers.
pub fn shown_todos(host: &dyn TodosHost, id: &str) -> Vec<Todo> {
    if !host.is_session_running(id) {
        return Vec::new();
    }
    let Some(t) = host.transcript(id) else { return Vec::new() };
    let revert = host.session_revert(id);
    // Every poll asks: the plan is read once per version of the transcripts, not by walking
    // a long session back to its last plan each frame.
    let version = (id.to_owned(), host.transcript_version(id), revert.map(str::to_owned));
    let todos = PLAN.with_borrow_mut(|kept| match kept.as_ref().filter(|(v, _)| *v == version) {
        Some((_, todos)) => todos.clone(),
        None => {
            let todos = session_todos(t, revert);
            *kept = Some((version, todos.clone()));
            todos
        }
    });
    if todos.iter().all(Todo::finished) { Vec::new() } else { todos }
}

pub const CLOSE: Knob = Knob::new("Todo ring", 0.0, 0.8, 0.4).step(0.01).label("Ring closes in").about("Seconds a finished todo's fill takes to close");
pub const BOUNCE: Knob = Knob::new("Todo bounce", 0.0, 0.7, 0.4).step(0.01).label("Ring bounce").about("How far the fill springs past full");
pub const GROUP: Group = Group::new("Todo pill", "todos", &[CLOSE, BOUNCE]).under("Composer");

/// The open session's plan as last seen, and the todos that finished while it showed, each
/// filling its circle and striking its words on tracks of its own (t1150).
#[derive(Default)]
pub struct Ticks {
    session: String,
    seen: Vec<(String, Status)>,
    growing: HashMap<usize, Fill>,
    /// The host's clock as of the last poll, which a filling todo is drawn at.
    now: f64,
}

/// A todo finishing: its fill sweeps clockwise from the top, from the half it had round to a
/// full disc, then springs out past full and back, all about its centre and drawn afresh each
/// frame from `at`, the host's clock. From halfway through, a clip over its struck words opens
/// left to right at the same pace.
struct Fill {
    from: Status,
    start: f64,
    secs: f32,
    bounce: f32,
    strike: Tween<[f32; 4]>,
}

impl Fill {
    fn new(from: Status, content: &str, wrap: f32, now: f64, secs: f32, bounce: f32) -> Self {
        let (w, _) = rune::text::measure(content, &[], ROW, Some(wrap));
        let w = w.max(1.0);
        // The clip runs on past the words to the widest a row can be, so a list that widens
        // later never cuts a finished strike; it opens at zero halfway through the fill.
        let (from_w, to_w) = (-w / 2.0, WIDEST);
        let strike = Duration::from_secs_f32(secs * (to_w - from_w) / w);
        Fill {
            from,
            start: now,
            secs,
            bounce,
            strike: Tween::linear([0.0, 0.0, from_w, TALLEST], [0.0, 0.0, to_w, TALLEST], strike),
        }
    }

    /// How far round the fill has swept (0 to 1) and its radius at `at`; `None` once settled.
    fn at(&self, at: f64) -> Option<(f32, f32)> {
        let t = (at - self.start).max(0.0) as f32;
        let close = self.secs * SWEEP_SHARE;
        if t < close {
            let u = 1.0 - t / close;
            return Some((1.0 - u * u, DISC_R));
        }
        let pop = rune::Spring::new(self.secs, self.bounce);
        let t = t - close;
        if t > pop.settling_duration_from(1.0, KICK, 1.0, 0.002) {
            return None;
        }
        Some((1.0, DISC_R * pop.sample(1.0, KICK, 1.0, t).0))
    }
}

/// Closes the list once its pill goes, so the next plan doesn't bring it back open (t1132),
/// starts a fill for each todo that has just finished, and draws each frame of those filling.
pub fn sync(host: &mut dyn TodosHost, now: f64, tx: &mut Tx) -> bool {
    let Some(id) = host.active_session_id().map(str::to_owned) else { return false };
    let todos = shown_todos(host, &id);
    let (secs, bounce) = (host.knobs().get(&CLOSE), host.knobs().get(&BOUNCE));
    // Every poll comes here: with no fill growing and the list as last seen, nothing below
    // changes, so the ticks aren't written.
    let seen = host.todo_ticks();
    let quiet = seen.session == id
        && seen.growing.is_empty()
        && seen.seen.len() == todos.len()
        && seen.seen.iter().zip(&todos).all(|((c, st), t)| *c == t.content && *st == t.status);
    if quiet {
        if !host.todo_open() || current(&todos).is_some() {
            return false;
        }
        host.set_todo_open(false);
        return true;
    }
    let reduced = host.reduced_motion();
    let ticks = host.todo_ticks_mut();
    let was_filling = ticks.growing.values().any(|f| f.at(ticks.now).is_some());
    ticks.now = now;
    if ticks.session != id {
        *ticks = Ticks { session: id, now, ..Ticks::default() };
    } else if !reduced && secs > 0.0 {
        let wrap = words_width(&todos);
        for (k, t) in todos.iter().enumerate() {
            let was = ticks.seen.get(k).filter(|(c, _)| *c == t.content).map(|(_, st)| *st);
            if let Some(was) = was.filter(|st| *st != Status::Completed && t.status == Status::Completed) {
                ticks.growing.insert(k, Fill::new(was, &t.content, wrap, now, secs, bounce));
            }
        }
    }
    ticks.seen = todos.iter().map(|t| (t.content.clone(), t.status)).collect();
    ticks.growing.retain(|k, _| todos.get(*k).is_some_and(|t| t.status == Status::Completed));
    let filling = ticks.growing.values().any(|f| f.at(now).is_some());
    if filling {
        tx.wake_after(Duration::ZERO);
    }
    let changed = filling || was_filling;
    if !host.todo_open() || current(&todos).is_some() {
        return changed;
    }
    host.set_todo_open(false);
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

/// "2 of 5 todos done", the pill's and list's name for screen readers (`todo-pill.tsx:31`).
fn label(todos: &[Todo]) -> String {
    let done = todos.iter().filter(|t| t.status == Status::Completed).count();
    opencode_i18n::plural!("session.todo.label", todos.len(), done = done)
}

/// Toggles the todo plan popover, notifies sibling popovers, and reveals the active todo.
pub fn toggle_pill(host: &mut dyn TodosHost, id: &str, tx: &mut Tx) {
    let open = !host.todo_open();
    host.set_todo_open(open);
    host.on_todo_toggled(open);
    session_view::popover::motion::toggled(host.knobs(), tx, open);
    if open {
        reveal_current(host, id, tx);
    }
}

/// The pill: the current todo's glyph and words, cut with a fade past 260, and "+2" when more
/// than one is in progress.
pub fn pill<H: TodosHost + 'static>(host: &H, p: &Pal, id: &str) -> Option<El> {
    let sid = id.to_owned();
    pill_shell(host, p, id).map(|chip| chip.on_press_tx(move |s: &mut H, tx: &mut Tx| toggle_pill(s, &sid, tx)))
}

fn pill_shell(host: &dyn TodosHost, p: &Pal, id: &str) -> Option<El> {
    let todos = shown_todos(host, id);
    let now = current(&todos)?;
    let more = todos.iter().filter(|t| t.status == Status::InProgress).count().saturating_sub(1);
    let mut chip = host.todo_chip(p, "todo-pill", label(&todos), host.todo_open())
        .pl(8.0).gap(6.0).shrink().min_w(0.0)
        .child(glyph(p, now.status, 0.0, None))
        .child(text(now.content.clone()).key("todo-now").dir(rune::Direction::Auto).font(UI).color(p.text).truncate_fade().max_w(PILL_TEXT_MAX).shrink().min_w(0.0));
    if more > 0 {
        // Desktop's compact `Badge`: 16 tall, a hairline over `layer-02`.
        let hairline = rune::Shadow { spread: 0.5, color: p.border, inset: true, ..rune::Shadow::default() };
        chip = chip.child(
            row().key("todo-more").h(16.0).px(4.0).center().radius(2.0).fill(p.layer2).box_shadow(hairline)
                .child(text(opencode_i18n::t!("session.todo.moreInProgress", count = more)).dir(rune::Direction::Ltr).font(BADGE).color(p.muted)),
        );
    }
    Some(chip)
}

/// The list (`session-todo-panel`), at once above the pill, start edges aligned: every todo
/// with its glyph, the one in progress in full text, finished ones struck through and faint.
/// At least 240 wide, at most 400, its words wrapping; past 232 it scrolls, keeping the todo in
/// progress in view.
pub fn panel<H: TodosHost + 'static>(host: &H, p: &Pal, id: &str) -> Option<El> {
    let content = build_panel_content(host, p, id)?;
    // `MenuScroll` (`todo-pill.tsx:85`). What it holds comes in with the list, not on fades of its
    // own, and changes at once; only a finishing todo's fill springs, on its own curves.
    let viewport = col().transition(rune::Transition::none()).w_full().max_h(MAX_H).scroll_edge_fade(12.0).scroll_margin(PAD).motion(rune::Spring::NONE);
    let rows = session_view::scroller::scroller("todo-scroll").bar(session_view::scroller::Bar::MenuScroll).build::<H>(p, viewport, content.list);
    Some(build_panel_frame(host.knobs(), p, content.width, content.label, rows).on_press(|_: &mut H| {}))
}

struct PanelContent {
    list: El,
    width: f32,
    label: String,
}

fn build_panel_content(host: &dyn TodosHost, p: &Pal, id: &str) -> Option<PanelContent> {
    if !host.todo_open() {
        return None;
    }
    let todos = shown_todos(host, id);
    if todos.is_empty() {
        return None;
    }
    let ticks = host.todo_ticks();
    let wrap = words_width(&todos);
    let mut list = col().key("todo-list").transition(rune::Transition::none()).w_full().padding(PAD);
    for (k, t) in todos.iter().enumerate() {
        let color = match t.status {
            Status::InProgress => p.text,
            Status::Pending => p.muted,
            Status::Completed | Status::Cancelled => p.faint,
        };
        let fill = ticks.growing.get(&k);
        list = list.child(
            row().key(format!("todo-{k}")).transition(rune::Transition::none()).w_full().px(6.0).py(4.0).gap(8.0)
                .child(el().h(ROW.line_height).center().transition(rune::Transition::none()).child(glyph(p, t.status, ticks.now, fill)))
                .child(words(t, color, wrap, fill)),
        );
    }
    Some(PanelContent { list, width: panel_width(&todos), label: label(&todos) })
}

fn build_panel_frame(knobs: &Knobs, p: &Pal, width: f32, label: String, rows: El) -> El {
    col().key("todo-panel").role("dialog").label(label).above("todo-pill", 8.0).w(width).radius(10.0)
        .fill(p.layer1).elevate(p.floating).clip_soft(0.0)
        .transition(session_view::popover::motion::transition(knobs, rune::Side::Top, rune::Align::Start))
        .child(rows)
}

/// Keeps the todo in progress in view when the list opens or the plan moves on.
pub fn reveal_current(host: &dyn TodosHost, id: &str, tx: &mut Tx) {
    if let Some(k) = shown_todos(host, id).iter().position(|t| t.status == Status::InProgress) {
        tx.scroll_into_view_in("todo-scroll", format!("todo-{k}"));
    }
}

fn default_chip(knobs: &Knobs, p: &Pal, key: &str, label: String, open: bool) -> El {
    let bg = match (open, p.dark) {
        (false, _) => session_view::composer::composer_fill(p),
        (true, false) => p.layer1,
        (true, true) => p.layer2,
    };
    let [far, near, ring, edge] = p.raised;
    let chip = row().key(key).role("button").label(label).expanded(open).cursor(rune::Cursor::Pointer).focus_default(p)
        .h(session_view::jump::chip_height())
        .px(10.0).gap(8.0).center_y().radius(knobs.get(&session_view::knobs::CAPSULE_RADIUS)).fill(bg)
        .box_shadow(edge).box_shadow(ring).box_shadow(near).box_shadow(far).hover_motion(INSTANT, INSTANT);
    if open { chip } else { chip.hover(p.hover).press(p.pressed) }
}

/// A row's words, struck through once finished. A todo finishing draws its strike: the same
/// words, struck with clear glyphs, over its own, in a clip that opens left to right.
fn words(t: &Todo, color: rune::Color, wrap: f32, fill: Option<&Fill>) -> El {
    let words = || text(t.content.clone()).dir(rune::Direction::Auto).font(ROW).wrap().w_full();
    let own = words().key("text").color(color);
    let own = if t.finished() && fill.is_none() { own.strikes(strikes(&t.content, wrap, None)) } else { own };
    el().key("words").grow(1.0).shrink().min_w(0.0).transition(rune::Transition::none()).instant_changes().child(own).children(fill.map(|fill| {
        el().key("strike").transition(rune::Transition::none()).absolute(0.0, 0.0).w_full().h_full().clip_soft(0.0).live_rect(&fill.strike)
            .child(words().color(rune::Color::TRANSPARENT).strikes(strikes(&t.content, wrap, Some(color))))
    }))
}

/// One strike per line the words wrap to at `wrap`, each from its line's first word to its last.
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
/// through when cancelled.
fn glyph(p: &Pal, status: Status, now: f64, fill: Option<&Fill>) -> El {
    let (shape, color) = match status {
        Status::Pending => (&PENDING, p.icon_faint),
        Status::InProgress => (&IN_PROGRESS, p.warning),
        Status::Completed => (&PENDING, p.icon_muted),
        Status::Cancelled => (&CANCELLED, p.icon_faint),
    };
    let filling = fill.and_then(|f| f.at(now).map(|(swept, r)| (f.from, swept, r)));
    let filling = filling.or((status == Status::Completed).then_some((Status::Completed, 1.0, DISC_R)));
    let shape = if filling.is_some() { &PENDING } else { shape };
    el().key("todo-glyph")
        .size(16.0, 16.0)
        .transition(rune::Transition::none())
        .child(icon(shape, 16.0).key("shape").color(color).transition(rune::Transition::none()).instant_changes())
        .children(filling.map(|(from, swept, r)| {
            canvas(disc(from, swept, r, color)).key("disc").absolute(0.0, 0.0).size(16.0, 16.0).transition(rune::Transition::none())
        }))
}

/// A finishing todo's fill, `r` points about the glyph's centre.
fn disc(from: Status, swept: f32, r: f32, color: rune::Color) -> Shape {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    let (start, rest) = if from == Status::InProgress { (FRAC_PI_2, PI) } else { (-FRAC_PI_2, TAU) };
    let end = -FRAC_PI_2 + rest * swept + if from == Status::InProgress { TAU } else { 0.0 };
    // Within a few degrees of closing it draws closed, so no hairline shows at the seam.
    let closed = end - start > TAU - 0.12;
    let (start, end) = if closed { (-FRAC_PI_2, -FRAC_PI_2 + TAU) } else { (start, end) };
    let steps = ((end - start) / TAU * 64.0).ceil().max(1.0) as usize;
    let at = |a: f32| [8.0 + r * a.cos(), 8.0 + r * a.sin()];
    let mut points: Vec<[f32; 2]> = (0..=steps).map(|i| at(start + (end - start) * i as f32 / steps as f32)).collect();
    if !closed {
        points.push([8.0, 8.0]);
    }
    Shape::new().fill(points, color)
}

const DISC_R: f32 = 4.0;
const KICK: f32 = 3.5;
const SWEEP_SHARE: f32 = 0.6;
const RING: &str = r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor"/>"#;
static PENDING: Icon = Icon { name: "todo-pending", view: 16.0, body: RING };
static IN_PROGRESS: Icon = Icon {
    name: "todo-in-progress",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor"/><path d="M8 4a4 4 0 0 0 0 8V4Z" fill="currentColor"/>"#,
};
static CANCELLED: Icon = Icon {
    name: "todo-cancelled",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor"/><path d="M3.5 12.5 12.5 3.5" stroke="currentColor"/>"#,
};
const PILL_TEXT_MAX: f32 = 260.0;
const PAD: f32 = 4.0;
const MAX_H: f32 = 232.0;
const WIDEST: f32 = 400.0;
const TALLEST: f32 = 2000.0;
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
const BADGE: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0).tracking(0.05).features("\"tnum\"");
