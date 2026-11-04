//! When the lit model's efforts open beside its row, as in Desktop's model list
//! (`select-dialog.tsx`: `hover`, `move`, `follow`; `safe-triangle.ts`).
//!
//! - The first flyout waits 100 ms of the pointer on a row; once one has opened, the next opens
//!   at once.
//! - A safe triangle, from the pointer's last point on the flyout's row to the flyout's near
//!   corners, keeps the flyout while the pointer crosses other rows on its way in. Resting on a
//!   row inside it for 300 ms hands over to that row, as on macOS.
//! - A wheel scroll lights the row it brings under the pointer and moves the flyout beside it. The
//!   arrow keys close the flyout, and rows they scroll under a still pointer leave the highlight
//!   where the keys put it.

use std::time::Duration;

use rune::{El, PointerEvent, Tx};

use super::{Kind, PickerHost};

/// The first flyout's hover intent.
const INTENT: f64 = 0.1;
/// How long the pointer rests inside the triangle before the row under it takes over.
const SETTLE: f64 = 0.3;

/// A row waiting out a delay. Its due time is set by the next poll, which knows the time.
#[derive(Clone, Debug, PartialEq)]
struct Wait {
    key: String,
    due: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Hover {
    /// The arrow keys lit the row last.
    pub keyboard: bool,
    /// A wheel scroll moved the rows since the pointer last moved.
    wheel: bool,
    /// A flyout has opened since the list did.
    warm: bool,
    /// The pointer's last point on a row.
    pointer: Option<[f32; 2]>,
    /// The pointer's last point on the flyout's own row: the triangle's apex.
    apex: Option<[f32; 2]>,
    /// Where the list and the flyout draw, in window points ([`El::on_drawn_layout`]): the
    /// flyout flips beside its row where it doesn't fit, which layout doesn't know.
    list: Option<[f32; 4]>,
    flyout: Option<[f32; 4]>,
    open: Option<Wait>,
    settle: Option<Wait>,
}

impl Hover {
    /// As the list opens: the highlight is the keys' until the pointer moves.
    pub fn fresh() -> Hover {
        Hover { keyboard: true, ..Hover::default() }
    }

    /// A press decided the flyout, so no wait opens one later.
    pub fn cancel_wait(&mut self) {
        self.open = None;
        self.settle = None;
    }
}

fn contains(r: [f32; 4], [x, y]: [f32; 2]) -> bool {
    x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
}

/// Whether `p` is inside the triangle from `apex` to the flyout's corners nearest it.
pub fn in_triangle(apex: [f32; 2], flyout: [f32; 4], p: [f32; 2]) -> bool {
    let x = if flyout[0] > apex[0] { flyout[0] } else { flyout[0] + flyout[2] };
    let t = [apex, [x, flyout[1]], [x, flyout[1] + flyout[3]]];
    let side = |a: [f32; 2], b: [f32; 2]| (p[0] - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (p[1] - b[1]);
    let s = [side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0])];
    !(s.iter().any(|v| *v < 0.0) && s.iter().any(|v| *v > 0.0))
}

/// The model list's pointer, wheel and keys, for any [`PickerHost`].
pub trait ModelHover: PickerHost {
    /// The pointer moved on the model row `key` (Desktop's `move`).
    fn model_row_moved(&mut self, key: &str, at: [f32; 2], tx: &mut Tx) {
        let pk = self.pickers_mut();
        let h = &mut pk.hover;
        // Rows scrolling under a still pointer.
        if h.pointer == Some(at) { return; }
        h.pointer = Some(at);
        h.wheel = false;
        let other = pk.flyout.as_ref().is_some_and(|f| f.model != key);
        if other && let (Some(apex), Some(fly)) = (h.apex, h.flyout) && in_triangle(apex, fly, at) {
            h.settle = Some(Wait { key: key.to_owned(), due: None });
            tx.wake_after(Duration::ZERO);
            return;
        }
        h.settle = None;
        h.apex = Some(at);
        self.hover_model(key, tx);
    }

    /// The row `key` came under the pointer without it moving: a wheel scroll carried it there
    /// (Desktop's `follow`), or the keys or a filter did, which leave the highlight alone.
    fn model_row_entered(&mut self, key: &str) {
        let pk = self.pickers_mut();
        let h = &mut pk.hover;
        if h.keyboard || !h.wheel { return; }
        h.open = None;
        h.settle = None;
        h.apex = h.pointer;
        pk.activate(key.to_owned());
        if self.model_effort(key).is_some() { self.open_effort_flyout(key); } else { self.pickers_mut().flyout = None; }
    }

    /// A wheel scroll over the list: the rows it brings under the pointer light.
    fn model_list_wheeled(&mut self) {
        let h = &mut self.pickers_mut().hover;
        h.keyboard = false;
        h.wheel = true;
        h.open = None;
    }

    /// The arrow keys moved the highlight: the flyout closes.
    fn model_list_keyed(&mut self) {
        let pk = self.pickers_mut();
        let h = &mut pk.hover;
        h.keyboard = true;
        h.open = None;
        h.settle = None;
        pk.flyout = None;
    }

    /// The pointer anywhere in the window. Off the rows, the flyout and the triangle, the flyout
    /// closes.
    fn model_pointer_moved(&mut self, at: [f32; 2]) {
        let pk = self.pickers_mut();
        let h = &mut pk.hover;
        if h.list.is_some_and(|l| contains(l, at)) { return; }
        h.open = None;
        h.settle = None;
        if h.flyout.is_some_and(|f| contains(f, at)) { return; }
        if let (Some(apex), Some(fly)) = (h.apex, h.flyout) && in_triangle(apex, fly, at) { return; }
        pk.flyout = None;
    }

    /// Opens the model `key`'s efforts beside its row; the next flyout opens at once.
    fn open_effort_flyout(&mut self, key: &str) {
        let pk = self.pickers_mut();
        if pk.flyout.as_ref().is_none_or(|f| f.model != key) { pk.hover.flyout = None; }
        pk.flyout = Some(super::flyout::Flyout { model: key.to_owned(), lit: None });
        pk.hover.warm = true;
        pk.hover.open = None;
    }

    /// Lights the row `key` and opens its efforts, after the intent wait unless one has opened
    /// (Desktop's `hover`).
    fn hover_model(&mut self, key: &str, tx: &mut Tx) {
        let lit = self.picker_active().as_deref() == Some(key);
        let pk = self.pickers();
        let h = &pk.hover;
        let shown = pk.flyout.as_ref().is_some_and(|f| f.model == key) || h.open.as_ref().is_some_and(|w| w.key == key);
        if lit && !h.keyboard && shown { return; }
        tx.instant();
        crate::popover::jump_highlight();
        self.pickers_mut().activate(key.to_owned());
        let efforts = self.model_effort(key).is_some();
        let pk = self.pickers_mut();
        let h = &mut pk.hover;
        h.keyboard = false;
        h.open = None;
        if efforts && h.warm {
            self.open_effort_flyout(key);
            return;
        }
        if efforts {
            h.open = Some(Wait { key: key.to_owned(), due: None });
            tx.wake_after(Duration::ZERO);
        }
        pk.flyout = None;
    }
}

impl<S: PickerHost> ModelHover for S {}

/// `w`'s key once it's due, setting its due time `delay` from `now` on the first poll.
fn due(w: &mut Option<Wait>, now: f64, delay: f64, tx: &mut Tx) -> Option<String> {
    let wait = w.as_mut()?;
    let at = *wait.due.get_or_insert(now + delay);
    if now + 1e-6 < at {
        tx.wake_after(Duration::from_secs_f64(at - now));
        return None;
    }
    w.take().map(|w| w.key)
}

/// Opens a flyout whose intent wait is up, and hands over to a row the pointer rested on.
pub fn poll<S: PickerHost>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    if s.pickers().open != Some(Kind::Model) { return false; }
    let h = &mut s.pickers_mut().hover;
    let open = due(&mut h.open, now, INTENT, tx);
    let settle = due(&mut h.settle, now, SETTLE, tx);
    if open.is_none() && settle.is_none() { return false; }
    tx.instant();
    if let Some(key) = settle {
        s.pickers_mut().hover.apex = None;
        s.hover_model(&key, tx);
    }
    if let Some(key) = open.filter(|k| s.picker_active().as_ref() == Some(k)) {
        s.open_effort_flyout(&key);
    }
    true
}

/// The model list's row `key`: the pointer moving on it lights it and opens its efforts.
pub fn row<S: PickerHost>(e: El, key: &str) -> El {
    let key = key.to_owned();
    e.on_pointer_move_local(move |s: &mut S, p: PointerEvent, tx: &mut Tx| s.model_row_moved(&key, [p.window_x, p.window_y], tx))
}

/// The model list's scroll: a wheel scroll lights the rows it brings under the pointer.
pub fn scroll<S: PickerHost>(e: El) -> El {
    e.on_drawn_layout(|s: &mut S, r| s.pickers_mut().hover.list = Some(r)).on_wheel(|s: &mut S, _, _| {
        s.model_list_wheeled();
        false
    })
}

/// The flyout: where it draws, and the pointer reaching it.
pub fn flyout<S: PickerHost>(e: El) -> El {
    e.on_drawn_layout(|s: &mut S, r| s.pickers_mut().hover.flyout = Some(r))
        .on_hover_change(|s: &mut S, inside, _| if inside { s.pickers_mut().hover.settle = None; })
}

/// The layer under the open list: the pointer leaving the rows, the flyout and the triangle
/// closes the flyout.
pub fn watch<S: PickerHost>(e: El) -> El {
    e.on_pointer_move_local(|s: &mut S, p: PointerEvent, tx: &mut Tx| {
        let had = s.pickers().flyout.is_some();
        s.model_pointer_moved([p.window_x, p.window_y]);
        if had != s.pickers().flyout.is_some() { tx.instant(); }
    })
}
