//! What a widget's view keeps between steps that the widget template never sees: text being typed
//! (sent as one `change` when committed), a slider's value while it's dragged (sent on
//! release), and each carousel's page. One [`Local`] per widget; the host keeps it beside the
//! widget's lifecycle machine.
//!
//! Each control's behaviour is its own explicit machine:
//! - [`Draft`]: an input is synced with the view's value, or being typed in until its debounce
//!   is due, it loses focus, Return commits it, or another event flushes it first.
//! - [`Drag`]: no slider is dragged (`Idle`), or one is, with its live value.
//! - [`Wheel`]: a carousel's trackpad gesture is `Idle`, `Tracking` its sideways travel, or
//!   `Spent` once it turned a page, until the gesture (and its momentum) ends.

use std::collections::HashMap;

use opencode_widget::{Command, Node, WidgetEvent};
use opencode_widget::draw::{Frame, Rect};

use crate::canvas::state::{Canvas, Event as CanvasEvent, Motion};
use rune::{GesturePhase, TextEdit, WheelEvent};

/// How long typing rests before an input's text goes to the widget template (the contract's >= 300 ms).
pub const DEBOUNCE_S: f64 = 0.3;

/// Sideways trackpad travel that turns a carousel's page, in points.
const WHEEL_PAGE: f32 = 48.0;

/// An input's typing: `sent` is the text the widget template last had, so a commit that changes
/// nothing sends nothing.
#[derive(Clone, Debug)]
pub struct Draft {
    pub edit: TextEdit,
    sent: String,
    phase: DraftPhase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DraftPhase {
    Synced,
    /// Typed in since; committed at `due` (window clock, seconds), on blur, or on Return.
    Editing { due: f64 },
}

impl Draft {
    fn synced(value: &str) -> Draft {
        Draft { edit: TextEdit::new(value), sent: value.to_owned(), phase: DraftPhase::Synced }
    }

    pub fn editing(&self) -> bool {
        matches!(self.phase, DraftPhase::Editing { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Drag {
    #[default]
    Idle,
    Slider { id: String, value: f64 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Wheel {
    #[default]
    Idle,
    Tracking(f32),
    Spent,
}

/// Which side a carousel's page came in from, for its slide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Came {
    #[default]
    Still,
    FromRight,
    FromLeft,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pager {
    pub page: usize,
    pub came: Came,
    /// The widget template's `index` this page was last synced with.
    synced: Option<usize>,
    wheel: Wheel,
}

/// The canvas a live program's frames are for.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveCanvas {
    pub id: Option<String>,
    /// Its logical size, once laid out.
    pub size: Option<[f32; 2]>,
    /// Where the pointer is over it.
    pub pointer: Option<opencode_widget::draw::Pointer>,
    /// Full screen, the close button's area over it (`frame.close`).
    pub close: Option<Rect>,
}

/// One widget's view-local state.
#[derive(Clone, Debug, Default)]
pub struct Local {
    drafts: HashMap<String, Draft>,
    drag: Drag,
    pagers: HashMap<String, Pager>,
    /// Each canvas's drawing, by its path.
    canvases: HashMap<String, Canvas>,
    /// Bumped on every change a build reads, for hosts that memoize the widget's row.
    version: u64,
    /// The widget is on screen (a live program runs only while it is).
    shown: bool,
    /// Builds of the widget's frame: one that stops counting sits in a window that doesn't draw.
    builds: std::cell::Cell<u64>,
    /// When each shader last sent a pointer `move`, by path.
    moved: HashMap<String, f64>,
    /// Each shader's logical size, by path, once laid out.
    sizes: HashMap<String, [f32; 2]>,
    /// Where each shader was laid out, in window points, by path.
    origins: HashMap<String, [f32; 2]>,
    /// The widget is full screen (`frame.fullscreen`).
    fullscreen: bool,
    /// Full screen, the close button's area `[x, y, w, h]` in window points.
    close: Option<[f32; 4]>,
    /// It has keyboard focus (a ring and "Esc to release"), and holds the pointer captured.
    focused: bool,
    captured: bool,
}

impl Local {
    /// A number that moves whenever anything a build reads here changed.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Whether the widget is on screen.
    pub fn shown(&self) -> bool {
        self.shown
    }

    /// The widget came on screen, or left it.
    pub fn set_shown(&mut self, on: bool) {
        self.shown = on;
    }

    /// A live program drew canvas `id` of `view`: it shows `commands` until the next draw.
    pub fn canvas_posted(&mut self, view: &Node, id: &str, commands: Vec<Command>) {
        let Some((path, ..)) = canvases(view).into_iter().find(|(_, i, _)| i.as_deref() == Some(id)) else { return };
        self.canvases.entry(path).or_default().phase = crate::canvas::state::Phase::Settled { commands: std::rc::Rc::new(commands) };
        self.version += 1;
    }

    /// The first canvas of `view`, which a live program's frame is for. None when the view has
    /// no canvas.
    pub fn live_canvas(&self, view: &Node) -> Option<LiveCanvas> {
        let (path, id, _) = canvases(view).into_iter().next()?;
        let c = self.canvases.get(&path);
        Some(LiveCanvas { id, size: c.and_then(|c| c.size), pointer: c.and_then(|c| c.pointer), close: self.canvas_close(&path) })
    }

    /// Full screen, the close button's area over canvas `path`, in its coordinates.
    pub fn canvas_close(&self, path: &str) -> Option<Rect> {
        let c = self.canvases.get(path)?;
        self.close_over(c.origin?, c.size?)
    }

    /// Full screen, the close button's area over the shader at `path`, in its coordinates.
    pub fn shader_close(&self, path: &str) -> Option<Rect> {
        self.close_over(*self.origins.get(path)?, *self.sizes.get(path)?)
    }

    fn close_over(&self, origin: [f32; 2], size: [f32; 2]) -> Option<Rect> {
        let [x, y, width, height] = self.close?.map(f64::from);
        Rect { x, y, width, height }.over(origin.map(f64::from), size.map(f64::from))
    }

    /// Full screen, the close button's area in window points; `None` inline. Its canvases draw
    /// again, as `frame.close` changed.
    pub fn set_close(&mut self, close: Option<[f32; 4]>) {
        if self.close != close {
            self.close = close;
            for c in self.canvases.values_mut() {
                c.step(CanvasEvent::Changed);
            }
            self.version += 1;
        }
    }

    /// Canvas `path` was laid out at `rect` (window points). Full screen, moving it (a scroll)
    /// draws it again, as the close button's area over it moved.
    pub fn canvas_laid(&mut self, path: &str, [x, y, w, h]: [f32; 4]) {
        let close = self.close.is_some();
        let c = self.canvases.entry(path.to_owned()).or_default();
        let moved = c.origin != Some([x, y]);
        c.origin = Some([x, y]);
        c.step(CanvasEvent::Resized([w, h]));
        if close && moved {
            c.step(CanvasEvent::Changed);
        }
        self.version += 1;
    }

    /// The shader at `path` was laid out at `[x, y]` (window points).
    pub fn set_origin(&mut self, path: &str, at: [f32; 2]) {
        self.origins.insert(path.to_owned(), at);
    }

    /// When the shader at `path` last sent a pointer `move`.
    pub fn moved_at(&self, path: &str) -> f64 {
        self.moved.get(path).copied().unwrap_or(f64::NEG_INFINITY)
    }

    pub fn set_moved(&mut self, path: &str, at: f64) {
        self.moved.insert(path.to_owned(), at);
    }

    /// The shader at `path` was laid out `size` big.
    pub fn set_size(&mut self, path: &str, size: [f32; 2]) {
        self.sizes.insert(path.to_owned(), size);
    }

    pub fn size(&self, path: &str) -> Option<[f32; 2]> {
        self.sizes.get(path).copied()
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    pub fn captured(&self) -> bool {
        self.captured
    }

    /// The host's keyboard focus and pointer capture for this widget.
    pub fn set_input(&mut self, focused: bool, captured: bool) {
        if (self.focused, self.captured) != (focused, captured) {
            (self.focused, self.captured) = (focused, captured);
            self.version += 1;
        }
    }

    pub fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// The widget entered or left full screen: its canvases draw again, sized anew.
    pub fn set_fullscreen(&mut self, on: bool) {
        if self.fullscreen != on {
            self.fullscreen = on;
            for c in self.canvases.values_mut() {
                c.step(CanvasEvent::Changed);
            }
            self.version += 1;
        }
    }

    /// The widget's frame was built (drawn).
    pub fn built(&self) {
        self.builds.set(self.builds.get() + 1);
    }

    pub fn builds(&self) -> u64 {
        self.builds.get()
    }

    /// Catches up with a new view from a step: inputs not being typed in take the view's value,
    /// carousels whose `index` changed go to it, and controls no longer shown are let go.
    pub fn sync(&mut self, view: &Node) {
        let mut inputs: Vec<(&str, &str)> = Vec::new();
        let mut carousels: Vec<(String, usize)> = Vec::new();
        walk(view, "w", &mut |path, n| match n {
            Node::Input { id, value, .. } => inputs.push((id, value)),
            Node::Carousel { index: Some(i), .. } => carousels.push((path.to_owned(), *i)),
            _ => {}
        });
        self.drafts.retain(|id, _| inputs.iter().any(|(i, _)| i == id));
        let canvases = canvases(view);
        self.canvases.retain(|path, _| canvases.iter().any(|(p, ..)| p == path));
        for (path, ..) in canvases {
            self.canvases.entry(path).or_default().step(CanvasEvent::Changed);
        }
        for (id, value) in inputs {
            match self.drafts.get_mut(id) {
                Some(d) if d.phase == DraftPhase::Synced => {
                    if d.edit.text != value {
                        d.edit.replace_text(value);
                    }
                    value.clone_into(&mut d.sent);
                }
                Some(_) => {}
                None => {
                    self.drafts.insert(id.to_owned(), Draft::synced(value));
                }
            }
        }
        for (path, index) in carousels {
            let p = self.pagers.entry(path).or_default();
            if p.synced != Some(index) {
                p.synced = Some(index);
                p.came = match index.cmp(&p.page) {
                    std::cmp::Ordering::Greater => Came::FromRight,
                    std::cmp::Ordering::Less => Came::FromLeft,
                    std::cmp::Ordering::Equal => p.came,
                };
                p.page = index;
            }
        }
        self.version += 1;
    }

    pub fn draft(&self, id: &str) -> Option<&Draft> {
        self.drafts.get(id)
    }

    /// Input `id`'s edit, for the field's `on_edit` lens.
    pub fn edit_mut(&mut self, id: &str) -> &mut TextEdit {
        self.version += 1;
        &mut self.drafts.entry(id.to_owned()).or_insert_with(|| Draft::synced("")).edit
    }

    /// Input `id` was typed in at `now`: its commit is due [`DEBOUNCE_S`] later. Returns when.
    pub fn edited(&mut self, id: &str, now: f64) -> f64 {
        let due = now + DEBOUNCE_S;
        self.drafts.entry(id.to_owned()).or_insert_with(|| Draft::synced("")).phase = DraftPhase::Editing { due };
        self.version += 1;
        due
    }

    /// Commits input `id` now (Return): its `change`, when its text moved from what the
    /// widget template last had.
    pub fn commit(&mut self, id: &str) -> Option<WidgetEvent> {
        let draft = self.drafts.get_mut(id)?;
        if draft.phase == DraftPhase::Synced {
            return None;
        }
        draft.phase = DraftPhase::Synced;
        self.version += 1;
        if draft.edit.text == draft.sent {
            return None;
        }
        draft.edit.text.clone_into(&mut draft.sent);
        Some(WidgetEvent::change(id, draft.edit.text.clone()))
    }

    /// Commits every input being typed in, before another event steps the widget template, so that
    /// step sees what was typed.
    pub fn flush(&mut self) -> Vec<WidgetEvent> {
        let ids: Vec<String> = self.drafts.iter().filter(|(_, d)| d.editing()).map(|(id, _)| id.clone()).collect();
        ids.iter().filter_map(|id| self.commit(id)).collect()
    }

    /// The commits due at `now`: debounces that ran out, and inputs that lost focus.
    pub fn poll(&mut self, now: f64) -> Vec<WidgetEvent> {
        let ids: Vec<String> = self
            .drafts
            .iter()
            .filter(|(_, d)| matches!(d.phase, DraftPhase::Editing { due } if now >= due || !d.edit.focused))
            .map(|(id, _)| id.clone())
            .collect();
        ids.iter().filter_map(|id| self.commit(id)).collect()
    }

    /// When the next debounce is due, for the host's wake.
    pub fn next_due(&self) -> Option<f64> {
        self.drafts.values().filter_map(|d| if let DraftPhase::Editing { due } = d.phase { Some(due) } else { None }).reduce(f64::min)
    }

    pub fn has_pending(&self) -> bool {
        self.drafts.values().any(Draft::editing)
    }

    /// Slider `id`'s value while it's dragged.
    pub fn dragging(&self, id: &str) -> Option<f64> {
        match &self.drag {
            Drag::Slider { id: d, value } if d == id => Some(*value),
            _ => None,
        }
    }

    /// Slider `id` is dragged to `value` (a press starts the drag).
    pub fn drag_to(&mut self, id: &str, value: f64) {
        self.drag = Drag::Slider { id: id.to_owned(), value };
        self.version += 1;
    }

    /// The drag ended: the value it released at, unless it was cancelled.
    pub fn drag_end(&mut self, cancelled: bool) -> Option<(String, f64)> {
        let drag = std::mem::take(&mut self.drag);
        self.version += 1;
        match drag {
            Drag::Slider { id, value } if !cancelled => Some((id, value)),
            _ => None,
        }
    }

    pub fn canvas(&self, path: &str) -> Option<&Canvas> {
        self.canvases.get(path)
    }

    /// Something happened to canvas `path` (laid out, seen, pointed at).
    pub fn canvas_event(&mut self, path: &str, event: CanvasEvent) {
        self.canvases.entry(path.to_owned()).or_default().step(event);
        self.version += 1;
    }

    /// Canvas `path` sent a pointer `move` at clock `at`.
    pub fn canvas_moved(&mut self, path: &str, at: f64) {
        if let Some(c) = self.canvases.get_mut(path) {
            c.moved = at;
        }
    }

    /// The canvases of `view` that want a draw at clock `now`, with their frames.
    pub fn canvases_due(&mut self, view: &Node, now: f64, scale: f64, dark: bool) -> Vec<(String, Frame)> {
        canvases(view)
            .into_iter()
            .filter_map(|(path, id, motion)| {
                let mut frame = self.canvases.entry(path.clone()).or_default().due(now, motion, id.as_deref(), scale, dark)?;
                frame.fullscreen = self.fullscreen;
                frame.close = self.canvas_close(&path);
                Some((path, frame))
            })
            .collect()
    }

    /// Whether a canvas of `view` wants a draw at clock `now` (without touching anything).
    pub fn canvases_want(&self, view: &Node, now: f64) -> bool {
        canvases(view).iter().any(|(path, id, motion)| self.canvases.get(path).is_some_and(|c| c.due(now, *motion, id.as_deref(), 1.0, false).is_some()))
    }

    /// Canvas `path`'s draw at clock `at` came back.
    pub fn canvas_drawn(&mut self, view: &Node, path: &str, at: f64, result: Result<Vec<Command>, String>) {
        let Some((_, _, motion)) = canvases(view).into_iter().find(|(p, ..)| p == path) else { return };
        if let Some(c) = self.canvases.get_mut(path) {
            c.step(CanvasEvent::Drawn { at, result, motion });
            self.version += 1;
        }
    }

    /// When the next canvas frame of `view` is due, for the host's wake.
    pub fn canvas_next(&self, view: &Node) -> Option<f64> {
        canvases(view).iter().filter_map(|(path, _, motion)| self.canvases.get(path)?.next(*motion)).reduce(f64::min)
    }

    pub fn pager(&self, key: &str) -> Pager {
        self.pagers.get(key).copied().unwrap_or_default()
    }

    /// Carousel `key` (of `count` pages) goes to page `to`; whether it moved.
    pub fn go(&mut self, key: &str, to: usize, count: usize) -> bool {
        let p = self.pagers.entry(key.to_owned()).or_default();
        let to = to.min(count.saturating_sub(1));
        if to == p.page {
            return false;
        }
        p.came = if to > p.page { Came::FromRight } else { Came::FromLeft };
        p.page = to;
        self.version += 1;
        true
    }

    /// A trackpad scroll over carousel `key`: whether it takes the delta, and the page it turns
    /// to (for the caller to [`Local::go`] to). A gesture that starts more vertical than sideways is left to scroll the page.
    pub fn wheel(&mut self, key: &str, ev: &WheelEvent, count: usize) -> (bool, Option<usize>) {
        let p = self.pagers.entry(key.to_owned()).or_default();
        let (wheel, take, turn) = match (p.wheel, ev.phase) {
            (_, GesturePhase::Ended) if !ev.momentum => (Wheel::Idle, !matches!(p.wheel, Wheel::Idle), None),
            (Wheel::Idle, _) if ev.momentum => (Wheel::Idle, false, None),
            (Wheel::Idle, _) if ev.dx.abs() <= ev.dy.abs() => (Wheel::Idle, false, None),
            (Wheel::Idle, _) => step_wheel(ev.dx),
            (Wheel::Tracking(dx), _) => step_wheel(dx + ev.dx),
            // One page per gesture: the rest of it, momentum too, is spent.
            (Wheel::Spent, _) if ev.momentum && ev.phase == GesturePhase::Ended => (Wheel::Idle, true, None),
            (Wheel::Spent, _) => (Wheel::Spent, true, None),
        };
        p.wheel = wheel;
        let page = p.page;
        let to = match turn {
            Some(next) if next => page + 1,
            Some(_) => page.wrapping_sub(1),
            None => return (take, None),
        };
        (take, (to < count && to != page).then_some(to))
    }
}

/// Sideways travel `dx` so far: tracking, or spent with the page it turns (`true`: next). Content
/// moving left (`dx < 0`, fingers swiping left) brings the next page.
fn step_wheel(dx: f32) -> (Wheel, bool, Option<bool>) {
    if dx <= -WHEEL_PAGE {
        (Wheel::Spent, true, Some(true))
    } else if dx >= WHEEL_PAGE {
        (Wheel::Spent, true, Some(false))
    } else {
        (Wheel::Tracking(dx), true, None)
    }
}

/// `view`'s canvases: path, id and how they animate.
fn canvases(view: &Node) -> Vec<(String, Option<String>, Motion)> {
    let mut out = Vec::new();
    walk(view, "w", &mut |path, n| {
        // A GPU canvas is drawn by its shader, never by `draw`.
        if let Node::Canvas { id, fps, duration, wgsl: None, .. } = n {
            out.push((path.to_owned(), id.clone(), Motion::new(*fps, *duration)));
        }
    });
    out
}

/// Calls `f` with each node under `n` and its path, as the renderer keys them (`w`, `w.0`, `w.0.1`…).
pub fn walk<'a>(n: &'a Node, path: &str, f: &mut dyn FnMut(&str, &'a Node)) {
    f(path, n);
    for (i, c) in n.children().iter().enumerate() {
        walk(c, &format!("{path}.{i}"), f);
    }
}
