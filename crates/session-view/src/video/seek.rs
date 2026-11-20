//! Pointer position and playback position are separate. Hover reads a time without sending
//! a seek; a captured drag seeks the existing player and shows its frame in the loupe.
use std::{cell::Cell, rc::Rc};
use rune::{Align, Color, Drag, El, Key, Live, LiveText, Phase, Player, SurfaceFit, Tx, el, text};

use super::{Clip, transport::{clock, material}};
use crate::{TimelineHost, style::UI_SMALL};

const INSET: f32 = 14.0;
const MID: f32 = 35.0;
const GRIP: f32 = 12.0;
const STAMP_W: f32 = 64.0;
const FRAME_W: f32 = 124.0;
const FRAME_H: f32 = 78.0;

/// A small, aspect-fit copy of the frame. Don't put a second matte inside a wide movie.
fn frame_box(player: &Player) -> [f32; 2] {
    let (w, h) = player.natural_size().unwrap_or((16, 9));
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (FRAME_W / w).min(FRAME_H / h);
    [(w * scale).round().max(STAMP_W), (h * scale).round().max(1.0)]
}

#[derive(Clone, Copy)]
enum PreviewSeek { Sent(f64), Pending(f64) }

impl PreviewSeek {
    fn at(self) -> f64 { match self { Self::Sent(t) | Self::Pending(t) => t } }
}

#[derive(Default, Clone, Copy)]
enum Scrub {
    #[default]
    Idle,
    Pointing(f64),
    Dragging { origin: f64, preview: PreviewSeek, resume: bool },
}

enum Event {
    Point(f64),
    Leave,
    Grab { at: f64, origin: f64, playing: bool },
    Slide(f64),
    Frame,
    Release { at: f64, inside: bool, cancelled: bool },
}

struct Seek { at: Option<f64>, pause: bool, resume: bool }

impl Scrub {
    fn step(self, e: Event) -> (Self, Option<Seek>) {
        match (self, e) {
            (Self::Idle | Self::Pointing(_), Event::Point(at)) => (Self::Pointing(at), None),
            (Self::Pointing(_), Event::Leave) => (Self::Idle, None),
            (_, Event::Grab { at, origin, playing }) => (
                Self::Dragging { origin, preview: PreviewSeek::Sent(at), resume: playing },
                Some(Seek { at: Some(at), pause: playing, resume: false }),
            ),
            (Self::Dragging { origin, preview, resume }, Event::Slide(at)) if at != preview.at() =>
                (Self::Dragging { origin, preview: PreviewSeek::Pending(at), resume }, None),
            (Self::Dragging { origin, preview: PreviewSeek::Pending(at), resume }, Event::Frame) => (
                Self::Dragging { origin, preview: PreviewSeek::Sent(at), resume },
                Some(Seek { at: Some(at), pause: false, resume: false }),
            ),
            (Self::Dragging { origin, preview, resume }, Event::Release { at, inside, cancelled }) => {
                let to = if cancelled { origin } else { at };
                let sent = matches!(preview, PreviewSeek::Sent(t) if t == to);
                (if inside && !cancelled { Self::Pointing(at) } else { Self::Idle },
                 Some(Seek { at: (!sent).then_some(to), pause: false, resume }))
            }
            _ => (self, None),
        }
    }
}

pub(super) struct Track {
    state: Cell<Scrub>,
    width: Cell<f32>,
    rail: Live<[f32; 4]>,
    fill: Live<[f32; 4]>,
    grip: Live<[f32; 4]>,
    marker: Live<[f32; 2]>,
    peek_offset: Live<[f32; 2]>,
    peek_alpha: Live<f32>,
    seconds: Cell<Option<(u64, Option<u64>, bool)>>,
    pointed_second: Cell<Option<u64>>,
    pub(super) time: LiveText,
    stamp: LiveText,
}

impl Default for Track {
    fn default() -> Self {
        Self {
            state: Cell::default(), width: Cell::default(), rail: Live::new([0.0; 4]), fill: Live::new([0.0; 4]),
            grip: Live::new([0.0; 4]), marker: Live::new([0.0; 2]), peek_offset: Live::new([0.0; 2]),
            peek_alpha: Live::new(0.0), seconds: Cell::default(), pointed_second: Cell::default(),
            time: LiveText::new("0:00 / –:––"), stamp: LiveText::new("0:00"),
        }
    }
}

impl Track {
    pub(super) fn dragging(&self) -> bool { matches!(self.state.get(), Scrub::Dragging { .. }) }

    pub(super) fn continuing(&self, player: &Player) -> bool {
        match self.state.get() { Scrub::Dragging { resume, .. } => resume, _ => player.is_playing() }
    }

    pub(super) fn compact(&self, player: &Player) -> bool {
        let width = self.width.get();
        width > 0.0 && width < if player.duration().is_some_and(|s| s >= 3600.0) { 275.0 } else { 249.0 }
    }

    pub(super) fn position(&self, player: &Player) -> f64 {
        match self.state.get() { Scrub::Dragging { preview, .. } => preview.at(), _ => player.time() }
    }

    fn time_at(&self, x: f32, player: &Player) -> Option<f64> {
        let d = player.duration().filter(|v| v.is_finite() && *v > 0.0)?;
        Some(d * f64::from(((x - INSET) / (self.width.get() - INSET * 2.0).max(1.0)).clamp(0.0, 1.0)))
    }

    pub(super) fn sync(&self, player: &Player) {
        let at = self.position(player);
        let duration = player.duration();
        let seconds = (at as u64, duration.map(|v| v as u64), self.compact(player));
        if self.seconds.replace(Some(seconds)) != Some(seconds) {
            let elapsed = clock(seconds.0);
            self.time.set(if seconds.2 { elapsed } else {
                format!("{elapsed} / {}", seconds.1.map_or_else(|| "–:––".into(), clock))
            });
        }

        let width = self.width.get();
        let span = (width - 2.0 * INSET).max(0.0);
        let x = |at: f64| duration.filter(|v| *v > 0.0).map_or(0.0, |d| (at / d).clamp(0.0, 1.0) as f32 * span);
        let point = match self.state.get() {
            Scrub::Idle => None,
            Scrub::Pointing(target) => Some((target, STAMP_W)),
            Scrub::Dragging { preview, .. } => Some((preview.at(), frame_box(player)[0])),
        };
        let (thick, grip) = if point.is_some() { (4.0, GRIP) } else { (3.0, 8.0) };
        self.rail.set([INSET, MID - thick / 2.0, span, thick]);
        self.fill.set([INSET, MID - thick / 2.0, x(at), thick]);
        self.grip.set([INSET + x(at) - grip / 2.0, MID - grip / 2.0, grip, grip]);
        if let Some((target, bubble_width)) = point {
            let px = INSET + x(target);
            self.marker.set([px, 0.0]);
            self.peek_offset.set([(px - bubble_width / 2.0).clamp(0.0, (width - bubble_width).max(0.0)), 0.0]);
            if self.pointed_second.replace(Some(target as u64)) != Some(target as u64) {
                self.stamp.set(clock(target as u64));
            }
        }
        self.peek_alpha.set(if point.is_some() { 1.0 } else { 0.0 });
    }

    fn event(&self, player: &Player, e: Event) {
        let (next, effect) = self.state.get().step(e);
        self.state.set(next);
        if let Some(seek) = effect {
            if seek.pause { player.pause(); }
            if let Some(at) = seek.at { player.seek(at); }
            if seek.resume { player.play(); }
        }
        self.sync(player);
    }

    /// Many mouse samples can arrive before one display refresh. Paint each latest
    /// position directly but issue at most one preview seek per frame. Release flushes
    /// the final target immediately, even if the decoder hasn't sampled the move yet.
    pub(super) fn poll(&self, player: &Player) {
        if matches!(self.state.get(), Scrub::Dragging { preview: PreviewSeek::Pending(_), .. }) {
            self.event(player, Event::Frame);
        } else { self.sync(player); }
    }

    fn drag(&self, player: &Player, d: &Drag) {
        let Some(at) = self.time_at(d.x, player) else { return };
        let e = match d.phase {
            Phase::Down => Event::Grab { at, origin: player.time(), playing: player.is_playing() },
            Phase::Move => Event::Slide(at),
            Phase::Up => Event::Release { at, inside: !crate::style::hover_none() && (0.0..=d.w).contains(&d.x) && (0.0..=d.h).contains(&d.y), cancelled: d.cancelled() },
        };
        self.event(player, e);
    }
}

fn peek(c: &Clip) -> El {
    let frame = c.track.dragging();
    let [fw, fh] = frame_box(&c.player);
    let (w, h) = if frame { (fw, fh + 21.0) } else { (STAMP_W, 23.0) };
    let mut tip = material(el().key("peek").absolute(0.0, MID - h - 11.0).size(w, h), 7.0)
        .clip_rounded(7.0).live_offset(&c.track.peek_offset).live_opacity(&c.track.peek_alpha)
        .a11y_hidden().inert();
    if frame {
        // The two views sample the SAME decoded surface. No thumbnail download, second
        // player, audio track, or separate seek that can arrive after the user's hand.
        tip.push_child(el().key("frame").absolute(0.0, 0.0).size(w, fh)
            .fill(Color::BLACK).video(&c.player, SurfaceFit::Fit));
    }
    tip.child(el().pin(Some(0.0), None, Some(0.0), Some(0.0)).h(if frame { 21.0 } else { h }).center()
        .child(text("0:00:00").key("preview-time").w(w).align(Align::Center)
            .font(UI_SMALL.features("'tnum' 1")).color(Color::WHITE).live_text(&c.track.stamp)))
}

pub(super) fn view<S: TimelineHost>(c: &Rc<Clip>, name: &str) -> El {
    let t = &c.track;
    let line = |key, color, rect: &Live<[f32; 4]>, radius| {
        el().key(key).absolute(0.0, 0.0).size(1.0, 1.0).radius(radius).fill(color).live_rect(rect).inert()
    };
    // A message row may spring its children into place; the rail belongs to the
    // pointer, so never add that row motion to its live rects.
    let mut slider = el().key("seek").w_full().h(44.0).instant_changes().role("slider").focusable(true)
        .label(format!("{}: {name}", opencode_i18n::t!("rune.video.seek"))).cursor(rune::Cursor::Pointer)
        .range_value(t.position(&c.player), 0.0, c.player.duration().unwrap_or_default()).range_step(1.0)
        .focus_ring(1.0, -3.0, Color::WHITE.alpha(0.8))
        .child(line("rail", Color::WHITE.alpha(0.28), &t.rail, 2.0))
        .child(line("elapsed", Color::WHITE, &t.fill, 2.0))
        .child(line("thumb", Color::WHITE, &t.grip, GRIP / 2.0));
    if !t.dragging() {
        slider.push_child(el().key("hover-mark").absolute(-1.0, MID - 4.0).size(2.0, 8.0)
            .radius(1.0).fill(Color::WHITE.alpha(0.8)).live_offset(&t.marker).live_opacity(&t.peek_alpha).inert());
    }
    let layout = c.clone();
    let hover = c.clone();
    let leave = c.clone();
    let drag = c.clone();
    let step = c.clone();
    let limit = c.clone();
    let keyboard = c.clone();
    slider.child(peek(c))
        .on_layout_tx(move |s: &mut S, [_, _, w, _], tx| {
            let compact = layout.track.compact(&layout.player);
            layout.track.width.set(w); layout.track.sync(&layout.player);
            if compact != layout.track.compact(&layout.player) { layout.refresh(s, tx); } else { tx.live_only(); }
        })
        .on_pointer_move_any(move |p, tx| {
            if !crate::style::hover_none() && !hover.track.dragging() && let Some(at) = hover.track.time_at(p.x, &hover.player) {
                hover.track.event(&hover.player, Event::Point(at));
            }
            tx.live_only();
        })
        .on_hover_change_any(move |inside, tx| {
            if !inside { leave.track.event(&leave.player, Event::Leave); }
            tx.live_only();
        })
        .touch_drags_at_once().on_drag(move |s: &mut S, d, tx| {
            let was = drag.track.dragging();
            drag.track.drag(&drag.player, d);
            if d.phase == Phase::Move { tx.wake_at(tx.clock()); }
            if was != drag.track.dragging() { drag.refresh(s, tx); } else { tx.live_only(); }
        })
        .on_step(move |s: &mut S, dir, tx| {
            step.player.seek(step.player.time() + f64::from(dir)); step.track.sync(&step.player); step.refresh(s, tx);
        })
        .on_step_limit(move |s: &mut S, end, tx| {
            limit.player.seek(if end { limit.player.duration().unwrap_or_default() } else { 0.0 });
            limit.track.sync(&limit.player); limit.refresh(s, tx);
        })
        .on_key(move |s: &mut S, key, tx: &mut Tx| {
            if key.mods != rune::Mods::default() || !matches!(&key.key, Key::Text(t) if t == " ") { return false; }
            keyboard.toggle(); keyboard.refresh(s, tx); true
        })
}
