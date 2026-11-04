//! The overlay scroll bar at a scroll viewport's right edge, as macOS draws one: a thin thumb that
//! shows while the pane scrolls or the pointer comes to the edge, fades after a rest, widens
//! under the pointer, drags, and pages on a click in its track. System Settings' "Show scroll
//! bars: Always" keeps it showing.
//!
//! It follows the viewport through `on_scroll_offset` with live values only, so a scroll runs no
//! view. What it draws is mounted only while it shows: Rune lays a resized window out again
//! without running the view only while no node holds a live value. The thumb is two layers so it never jumps: a plain scroll moves the outer layer exactly
//! with the content, and a change in the content's length (rows measuring, a reply streaming,
//! an older page landing) moves and resizes the inner one on a spring.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rune::{Color, Cursor, Drag, El, Live, Phase, ScrollPos, Spring, Tx, Waker, el};
use rune_knobs::{Group, Knob, Knobs};

use crate::App;
use crate::palette::Pal;

#[cfg(test)]
mod tests;
pub mod stories;

pub const WIDTH: Knob = Knob::new("Scroll bar width", 2.0, 10.0, 4.0).step(0.5).about("Thumb width at rest");
pub const HOT_WIDTH: Knob = Knob::new("Scroll bar hover width", 2.0, 14.0, 6.0).step(0.5).about("Thumb width under the pointer");
pub const FADE_DELAY: Knob = Knob::new("Scroll bar fade delay", 0.2, 3.0, 0.9).step(0.05).about("Seconds at rest before the bar fades");

rune_register::register!(crate::knobs::GROUP_LIST: Group = [
    Group::new("Scroll bar", "scroll-bar", &[WIDTH, HOT_WIDTH, FADE_DELAY]).under("Timeline"),
]);

/// Space between the thumb and the track's ends and right edge.
const INSET: f32 = 2.0;
/// The shortest the thumb gets, however long the content.
const MIN_LEN: f32 = 28.0;
/// How far left of the thumb the pointer counts as at the edge.
const REACH: f32 = 6.0;
/// The thumb's moves when the content changes length, and its widening: a spring's response in
/// seconds, with no bounce. The poll steps it, so a hidden bar takes the change at once.
const SETTLE: f32 = 0.16;
const FADE: Spring = Spring::new(0.2, 0.0);
/// How long a viewport that just opened takes to settle where it was left.
const OPENING: f64 = 0.4;
/// How long after the content changes length the viewport may still be finding its place
/// (an anchor holding what's on screen after a fold shuts above it).
const RESHAPE: f64 = 0.05;
/// How long after it starts to fade the bar's drawing leaves, once [`FADE`] has run out.
const UNMOUNT: f64 = 0.6;

/// Every scroll bar's live state, by the key of the viewport it follows.
#[derive(Default)]
pub struct Bars {
    bars: RefCell<HashMap<String, Rc<Bar>>>,
    /// The poll clock, for handlers that have none.
    clock: Rc<Cell<f64>>,
    wake: Rc<RefCell<Option<Waker>>>,
    /// macOS "Show scroll bars: Always".
    always: Arc<AtomicBool>,
}

impl Bars {
    pub fn set_waker(&mut self, waker: Waker) {
        *self.wake.borrow_mut() = Some(waker);
    }

    /// Reads System Settings' scroll bar choice off the main thread, once.
    pub fn read_system_setting(&self) {
        #[cfg(target_os = "macos")]
        {
            let (always, wake) = (self.always.clone(), self.wake.borrow().clone());
            std::thread::spawn(move || {
                let out = std::process::Command::new("defaults").args(["read", "-g", "AppleShowScrollBars"]).output();
                if out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "Always") {
                    always.store(true, Ordering::Relaxed);
                    if let Some(wake) = wake { wake(); }
                }
            });
        }
    }

    #[cfg(test)]
    pub fn set_always(&self, on: bool) {
        self.always.store(on, Ordering::Relaxed);
    }

    fn bar(&self, key: &str) -> Rc<Bar> {
        self.bars.borrow_mut().entry(key.to_owned()).or_insert_with(|| Rc::new(Bar::new(key, self))).clone()
    }

    /// For the `on_scroll_offset` of the viewport keyed `key`.
    pub fn watch(&self, key: &str) -> impl Fn(ScrollPos) + 'static {
        let bar = self.bar(key);
        move |at| bar.seen(at)
    }

    /// The viewport keyed `key` is on its way to its end on its own (Jump to latest): like a
    /// reply streaming in, that isn't Kit scrolling, so the bar stays as it is.
    pub fn follow(&self, key: &str) {
        self.bar(key).following.set(true);
    }

    /// Each poll: fades out a bar that has rested long enough, mounts or unmounts what bars
    /// draw, and wakes for the next one due. True when the view has to run again.
    pub fn poll(&self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        let dt = (now - self.clock.replace(now)).clamp(0.0, 0.05) as f32;
        let always = self.always.load(Ordering::Relaxed);
        for bar in self.bars.borrow().values() {
            if bar.always.replace(always) != always {
                bar.place(true);
            }
            if bar.step(dt) {
                tx.wake_after(Duration::ZERO);
            }
            if let Some(left) = bar.rest(now) {
                tx.wake_after(Duration::from_secs_f64(left));
            }
            changed |= bar.mount(now, tx);
        }
        changed
    }

    /// Whether the bar on `key` is showing or on its way there.
    #[cfg(test)]
    pub fn shown(&self, key: &str) -> bool {
        self.bars.borrow().get(key).is_some_and(|b| b.shown.get())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mounting {
    Still,
    /// Draw it on the next commit, at nothing.
    Mount,
    /// Drawn: fade it in.
    Raise,
    /// Faded: stop drawing it at this poll clock.
    Leave(f64),
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Look {
    width: f32,
    hot_width: f32,
    delay: f64,
    rest: Color,
    hot: Color,
}

struct Bar {
    key: String,
    clock: Rc<Cell<f64>>,
    wake: Rc<RefCell<Option<Waker>>>,
    look: Cell<Look>,
    /// The track's height: the viewport's, in layout points.
    track_h: Cell<f32>,
    /// The last offset and max seen, once the viewport has been laid out.
    pos: Cell<Option<(f32, f32)>>,
    /// The viewport sat at its end and has only followed it since (a reply streaming in).
    following: Cell<bool>,
    hovered: Cell<bool>,
    dragging: Cell<bool>,
    always: Cell<bool>,
    shown: Cell<bool>,
    /// Whether the view draws the bar, and what the poll has to do about it.
    mounted: Cell<bool>,
    mounting: Cell<Mounting>,
    /// The poll clock at the last scroll, hover or drag.
    active: Cell<f64>,
    /// Until this poll clock, the viewport is settling where it opened, not being scrolled.
    opening: Cell<f64>,
    /// The poll clock when the content last changed length.
    reshaped: Cell<f64>,
    /// Where the thumb should be, `[x, y, w, h]` in the track.
    target: Cell<[f32; 4]>,
    /// The outer layer's offset: every plain scroll's move, added up.
    carry: Cell<f32>,
    outer: Live<[f32; 2]>,
    inner: Live<[f32; 4]>,
    /// The inner layer's goal, and its spring's position and velocity while it glides there.
    goal: Cell<[f32; 4]>,
    glide: Cell<Option<([f32; 4], [f32; 4])>>,
    still: Cell<bool>,
    opacity: Live<f32>,
    fill: Live<Color>,
}

impl Bar {
    fn new(key: &str, bars: &Bars) -> Bar {
        Bar {
            key: key.to_owned(),
            clock: bars.clock.clone(),
            wake: bars.wake.clone(),
            look: Cell::default(),
            track_h: Cell::new(0.0),
            pos: Cell::new(None),
            following: Cell::new(true),
            hovered: Cell::new(false),
            dragging: Cell::new(false),
            always: Cell::new(bars.always.load(Ordering::Relaxed)),
            shown: Cell::new(false),
            mounted: Cell::new(false),
            mounting: Cell::new(Mounting::Still),
            active: Cell::new(0.0),
            opening: Cell::new(0.0),
            reshaped: Cell::new(f64::NEG_INFINITY),
            target: Cell::new([0.0; 4]),
            carry: Cell::new(0.0),
            outer: Live::new([0.0; 2]),
            inner: Live::new([0.0; 4]),
            goal: Cell::new([0.0; 4]),
            glide: Cell::new(None),
            still: Cell::new(false),
            opacity: Live::new(0.0),
            fill: Live::new(Color::TRANSPARENT),
        }
    }

    fn scrolls(&self) -> bool {
        self.pos.get().is_some_and(|(_, max)| max >= 1.0) && self.track_h.get() > 2.0 * INSET + MIN_LEN
    }

    fn hot(&self) -> bool {
        self.hovered.get() || self.dragging.get()
    }

    /// The thumb's rect in the track for the geometry as it is now.
    fn rect(&self) -> [f32; 4] {
        let Some((offset, max)) = self.pos.get() else { return [0.0; 4] };
        let look = self.look.get();
        let w = if self.hot() { look.hot_width } else { look.width };
        let (y, h) = thumb(self.track_h.get(), offset, max);
        [REACH + look.hot_width - w, y, w, h].map(|v| v.max(0.0))
    }

    /// Moves the thumb to where the geometry puts it now: on the spring when the content
    /// changed length or the thumb changed width, else exactly with the scroll.
    fn place(&self, settle: bool) {
        let look = self.look.get();
        self.fill.set(if self.hot() { look.hot } else { look.rest });
        let show = self.scrolls() && (self.always.get() || self.hot() || self.shown.get());
        self.present(show);
        let to = self.rect();
        let was = self.target.replace(to);
        if settle || was[2] != to[2] || was[3] != to[3] || was[0] != to[0] {
            self.settle_to([to[0], to[1] - self.carry.get(), to[2], to[3]]);
        } else {
            let carry = self.carry.get() + to[1] - was[1];
            self.carry.set(carry);
            self.outer.set([0.0, carry]);
        }
    }

    /// Sends the inner layer to `goal`: at once while the bar is hidden, else on the spring.
    fn settle_to(&self, goal: [f32; 4]) {
        self.goal.set(goal);
        if !self.shown.get() || self.still.get() {
            self.glide.set(None);
            self.inner.set(goal);
        } else if self.glide.get().is_none() && self.inner.get() != goal {
            self.glide.set(Some((self.inner.get(), [0.0; 4])));
            self.wake_soon();
        }
    }

    /// Steps the inner layer's glide by `dt` seconds: true while it has further to go.
    fn step(&self, dt: f32) -> bool {
        let Some((mut x, mut v)) = self.glide.get() else { return false };
        let goal = self.goal.get();
        let w = std::f32::consts::TAU / SETTLE;
        let n = (dt * 240.0).ceil().max(1.0);
        let h = dt / n;
        for _ in 0..n as usize {
            for i in 0..4 {
                v[i] += (-w * w * (x[i] - goal[i]) - 2.0 * w * v[i]) * h;
                x[i] += v[i] * h;
            }
        }
        let done = (0..4).all(|i| (x[i] - goal[i]).abs() < 0.05 && v[i].abs() < 2.0);
        self.inner.set(if done { goal } else { x });
        self.glide.set((!done).then_some((x, v)));
        !done
    }

    /// Shows the bar and restarts its rest.
    fn poke(&self) {
        self.active.set(self.clock.get());
        if !self.shown.get() && self.scrolls() {
            self.present(true);
            // The scroll may end on this frame; the poll that fades the bar has to come.
            if let Some(wake) = self.wake.borrow().as_ref() { wake(); }
        }
    }

    /// The viewport's offset this frame.
    fn seen(&self, at: ScrollPos) {
        let (offset, max) = (at.offset[1], at.max[1]);
        let was = self.pos.replace(Some((offset, max)));
        let mut settle = true;
        if let Some((was_offset, was_max)) = was {
            let same = (max - was_max).abs() < 0.5;
            settle = !same;
            let moved = (offset - was_offset).abs() > 0.5;
            let up = offset < was_offset - 0.5;
            if up && offset < max - 1.0 {
                self.following.set(false);
            }
            let now = self.clock.get();
            if !same {
                self.reshaped.set(now);
            }
            // Kit scrolling: the content held its length and the viewport moved, other than to
            // follow its end as a reply streams in, or to find its place as it opens or as the
            // content changes length.
            let settling = now < self.opening.get() || now - self.reshaped.get() < RESHAPE;
            if !settling && same && moved && !self.following.get() {
                self.poke();
            }
        }
        if offset >= max - 1.0 {
            self.following.set(true);
        }
        self.place(settle);
    }

    /// Fades the bar once it has rested `delay`: the seconds left until then, if it's showing.
    fn rest(&self, now: f64) -> Option<f64> {
        if !self.shown.get() || self.always.get() || self.hot() {
            return None;
        }
        let left = self.active.get() + self.look.get().delay - now;
        if left > 0.0 {
            return Some(left);
        }
        self.present(false);
        self.settle_to(self.goal.get());
        None
    }

    /// The viewport is new this frame (a session opening, or coming back to its tab): what it
    /// was is gone, and it takes a moment to settle where it was left.
    fn open(&self) {
        self.pos.set(None);
        self.following.set(false);
        self.opening.set(self.clock.get() + OPENING);
    }

    fn hover(&self, on: bool) {
        self.hovered.set(on);
        self.active.set(self.clock.get());
        if on { self.poke(); }
        self.place(true);
        if !on { self.wake_soon(); }
    }

    /// Shows or hides the bar: a bar not drawn yet is mounted first, at nothing, and fades in
    /// from the next frame; a hidden one leaves once its fade has run out.
    fn present(&self, show: bool) {
        self.shown.set(show);
        let mounting = self.mounting.get();
        if !show {
            self.opacity.set(0.0);
            if self.mounted.get() && !matches!(mounting, Mounting::Leave(_)) {
                self.mounting.set(Mounting::Leave(self.clock.get() + UNMOUNT));
            } else if !self.mounted.get() {
                self.mounting.set(Mounting::Still);
            }
        } else if self.mounted.get() && mounting != Mounting::Raise {
            self.mounting.set(Mounting::Still);
            self.opacity.set(1.0);
        } else if !self.mounted.get() && mounting != Mounting::Mount {
            self.mounting.set(Mounting::Mount);
            self.wake_soon();
        }
    }

    /// The poll's part in [`Bar::present`]: true when the view has to draw the bar or stop.
    fn mount(&self, now: f64, tx: &mut Tx) -> bool {
        match self.mounting.get() {
            Mounting::Still => false,
            Mounting::Mount => {
                self.mounted.set(true);
                self.mounting.set(Mounting::Raise);
                tx.wake_after(Duration::ZERO);
                true
            }
            Mounting::Raise => {
                self.mounting.set(Mounting::Still);
                self.opacity.set(1.0);
                false
            }
            Mounting::Leave(at) if now < at => {
                tx.wake_after(Duration::from_secs_f64(at - now));
                false
            }
            Mounting::Leave(_) => {
                self.mounted.set(false);
                self.mounting.set(Mounting::Still);
                true
            }
        }
    }

    fn wake_soon(&self) {
        if let Some(wake) = self.wake.borrow().as_ref() { wake(); }
    }

    fn drag(&self, d: &mut Drag, tx: &mut Tx) {
        let Some((offset, max)) = self.pos.get() else { return };
        let track = self.track_h.get();
        let (top, len) = thumb(track, offset, max);
        match d.phase {
            Phase::Down => {
                if (top..top + len).contains(&d.y) {
                    d.data = d.y - top;
                    self.dragging.set(true);
                    self.place(true);
                } else {
                    d.data = f32::NAN;
                    tx.scroll_page(self.key.clone(), if d.y < top { -1 } else { 1 });
                }
                self.poke();
            }
            Phase::Move if !d.data.is_nan() => {
                let travel = (track - 2.0 * INSET - len).max(1.0);
                let at = ((d.y - d.data - INSET) / travel).clamp(0.0, 1.0);
                tx.scroll_to_y(self.key.clone(), at * max);
                self.active.set(self.clock.get());
            }
            Phase::Up => {
                self.dragging.set(false);
                self.active.set(self.clock.get());
                self.place(true);
                self.wake_soon();
            }
            Phase::Move => {}
        }
        tx.live_only();
    }
}

/// The thumb's top and length in a track `track` high, for a viewport scrolled `offset` of
/// `max`: its length is the viewport's share of the content, and it never runs shorter than
/// [`MIN_LEN`] or past the track's ends.
pub fn thumb(track: f32, offset: f32, max: f32) -> (f32, f32) {
    let room = (track - 2.0 * INSET).max(0.0);
    if max < 1.0 || room <= 0.0 {
        return (INSET, room);
    }
    let len = (room * track / (track + max)).clamp(MIN_LEN.min(room), room);
    (INSET + (room - len) * (offset / max).clamp(0.0, 1.0), len)
}

/// The scroll bar over the right edge of the viewport keyed `key`, for a parent that holds the
/// viewport `bottom` points above its own bottom.
pub fn bar(s: &App, p: &Pal, key: &str, bottom: f32) -> El {
    let b = s.bars.bar(key);
    let look = look(&s.knobs, p);
    // Only whether the viewport was laid out before, so its resizes re-run no view.
    if rune::memo::layout_with(key, |_| ()).is_none() {
        b.open();
    }
    b.still.set(s.reduced_motion);
    if b.look.replace(look) != look { b.place(true); }
    let width = REACH + look.hot_width + INSET;
    let (hover, drag, size) = (b.clone(), b.clone(), b.clone());
    let strip = el().key(format!("{key}-bar"))
        .pin(None, Some(0.0), Some(0.0), Some(bottom))
        .w(width)
        .cursor(Cursor::Default)
        // As tall as the viewport, in layout points at any zoom.
        .on_layout_tx(move |_: &mut App, r: [f32; 4], tx: &mut Tx| {
            if size.track_h.replace(r[3]) != r[3] { size.place(true); }
            tx.live_only();
        })
        .on_hover_change(move |_: &mut App, on, tx: &mut Tx| {
            hover.hover(on);
            tx.live_only();
        })
        .on_drag(move |_: &mut App, d: &mut Drag, tx: &mut Tx| drag.drag(d, tx));
    if !b.mounted.get() {
        return strip;
    }
    strip
        // A transparent node takes no input, so the fade is on what's drawn, not the strip.
        .child(el().key("bar-face").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).inert()
            .live_opacity(b.opacity.smooth(if s.reduced_motion { Spring::NONE } else { FADE }))
        .child(el().key("bar-carrier").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).live_offset(&b.outer)
            .child(el().key("bar-thumb").absolute(0.0, 0.0).size(1.0, 1.0).radius(99.0).live_fill(&b.fill)
                .live_rect(&b.inner))))
}

fn look(k: &Knobs, p: &Pal) -> Look {
    Look {
        width: k.get(&WIDTH),
        hot_width: k.get(&HOT_WIDTH).max(k.get(&WIDTH)),
        delay: crate::knobs::secs(k, &FADE_DELAY),
        rest: p.text.alpha(if p.dark { 0.2 } else { 0.24 }),
        hot: p.text.alpha(if p.dark { 0.32 } else { 0.36 }),
    }
}
