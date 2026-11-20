//! A clip's track is a live paint binding: neither playback ticks nor dragging reflow
//! the message. Rune's Player replaces a pending seek, so the decoder follows the latest
//! position rather than queueing a seek for every pointer sample.
use std::{cell::Cell, rc::Rc};
use rune::{Cursor, Drag, El, Icon, Live, LiveText, Phase, Player, Tx, el, icon, row, text};

use super::{Clip, Face};
use crate::{TimelineHost, palette::Pal, style::UI};

const SKIP: f64 = 15.0;
const PAD: f32 = 6.0;
const PLAY: Icon = Icon { name: "movie-play", view: 24.0, body: r#"<path d="M7 4.5v15l12-7.5z" fill="currentColor"/>"# };
const PAUSE: Icon = Icon { name: "movie-pause", view: 24.0, body: r#"<path d="M6 4h4v16H6zm8 0h4v16h-4z" fill="currentColor"/>"# };

#[derive(Default, Clone, Copy)]
enum Scrub {
    #[default]
    Idle,
    Dragging { origin: f64, target: f64, resume: bool },
}

struct Seek { at: f64, pause: bool, resume: bool }

impl Scrub {
    fn step(&mut self, phase: Phase, cancelled: bool, target: f64, origin: f64, playing: bool) -> Option<Seek> {
        let (next, seek) = match (*self, phase) {
            (_, Phase::Down) => (Self::Dragging { origin, target, resume: playing }, Seek { at: target, pause: playing, resume: false }),
            (Self::Dragging { origin, resume, .. }, Phase::Move) => (Self::Dragging { origin, target, resume }, Seek { at: target, pause: false, resume: false }),
            (Self::Dragging { origin, resume, .. }, Phase::Up) => (Self::Idle, Seek { at: if cancelled { origin } else { target }, pause: false, resume }),
            _ => return None,
        };
        *self = next;
        Some(seek)
    }
}

pub(super) struct Transport {
    scrub: Cell<Scrub>,
    width: Cell<f32>,
    fill: Live<[f32; 4]>,
    thumb: Live<[f32; 2]>,
    seconds: Cell<Option<(u64, u64)>>,
    clock: LiveText,
}

impl Default for Transport {
    fn default() -> Self {
        Self { scrub: Cell::new(Scrub::Idle), width: Cell::new(0.0), fill: Live::new([PAD, 20.0, 0.0, 4.0]),
            thumb: Live::new([0.0; 2]), seconds: Cell::new(None), clock: LiveText::new("0:00 / 0:00") }
    }
}

fn clock(s: u64) -> String {
    if s < 3600 { format!("{}:{:02}", s / 60, s % 60) }
    else { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) }
}

impl Transport {
    pub(super) fn sync(&self, player: &Player) {
        let at = match self.scrub.get() { Scrub::Idle => player.time(), Scrub::Dragging { target, .. } => target };
        self.paint(at, player.duration().unwrap_or_default());
    }

    fn paint(&self, at: f64, duration: f64) {
        let fraction = if duration > 0.0 { (at / duration).clamp(0.0, 1.0) } else { 0.0 };
        let x = fraction as f32 * (self.width.get() - 2.0 * PAD).max(0.0);
        self.fill.set([PAD, 20.0, x, 4.0]);
        self.thumb.set([x, 0.0]);
        let pair = (at as u64, duration as u64);
        if self.seconds.replace(Some(pair)) != Some(pair) {
            self.clock.set(format!("{} / {}", clock(pair.0), clock(pair.1)));
        }
    }

    fn drag(&self, player: &Player, d: &Drag) {
        let Some(duration) = player.duration().filter(|v| *v > 0.0) else { return };
        let target = duration * ((d.x - PAD) / (d.w - 2.0 * PAD).max(1.0)).clamp(0.0, 1.0) as f64;
        let mut state = self.scrub.get();
        if let Some(seek) = state.step(d.phase, d.cancelled(), target, player.time(), player.is_playing()) {
            self.scrub.set(state);
            if seek.pause { player.pause(); }
            player.seek(seek.at);
            self.paint(seek.at, duration);
            if seek.resume { player.play(); }
        }
    }
}

fn jump(c: &Clip, delta: f64) {
    if let Some(length) = c.player.duration() {
        let at = (c.player.time() + delta).clamp(0.0, length);
        c.player.seek(at);
        c.transport.paint(at, length);
    }
}

fn button<S: TimelineHost>(p: &Pal, key: &'static str, label: String, content: El, press: impl Fn() + 'static) -> El {
    el().key(key).role("button").focusable(true).label(label.clone()).help(label)
        .size(44.0, 44.0).center().radius(4.0).cursor(Cursor::Pointer).hover(p.hover).press(p.pressed)
        .child(content).on_press_tx(move |_: &mut S, tx: &mut Tx| { press(); tx.instant(); })
}

pub(super) fn controls<S: TimelineHost>(p: &Pal, c: Rc<Clip>, name: &str) -> El {
    let playing = c.face() == Face::Playing;
    let action = if playing { opencode_i18n::t!("lab.pause") } else { opencode_i18n::t!("lab.play") };
    let toggle = c.clone();
    let back = c.clone();
    let forward = c.clone();
    let layout = c.clone();
    let drag = c.clone();
    let step = c.clone();
    let timeline = el().key("seek").role("slider").label(format!("Seek: {name}")).focusable(true)
        .range_value(c.player.time(), 0.0, c.player.duration().unwrap_or_default()).range_step(SKIP)
        .on_step(move |_: &mut S, dir, tx| { jump(&step, f64::from(dir) * SKIP); tx.instant(); })
        .grow(1.0).min_w(0.0).h(44.0).cursor(Cursor::Pointer)
        .child(el().pin(Some(PAD), Some(20.0), Some(PAD), None).h(4.0).radius(2.0).fill(p.border))
        .child(el().absolute(PAD, 20.0).size(1.0, 4.0).radius(2.0).fill(p.text).live_rect(&c.transport.fill))
        .child(el().absolute(0.0, 16.0).size(12.0, 12.0).radius(6.0).fill(p.text).live_offset(&c.transport.thumb))
        .on_layout_tx(move |_: &mut S, [_, _, w, _], tx| {
            layout.transport.width.set(w);
            layout.transport.sync(&layout.player);
            tx.live_only();
        })
        .on_drag_any(move |d, tx| {
            drag.transport.drag(&drag.player, d);
            tx.live_only();
        });
    row().key("controls").w_full().px(4.0).center_y()
        .child(button::<S>(p, "transport", format!("{action}: {name}"), icon(if playing { &PAUSE } else { &PLAY }, 16.0).color(p.text), move || {
            if toggle.player.is_playing() { toggle.player.pause(); } else { toggle.player.play(); }
        }))
        .child(button::<S>(p, "back-15", "Back 15 seconds".into(), text("−15").font(UI).color(p.text), move || jump(&back, -SKIP)))
        .child(button::<S>(p, "forward-15", "Forward 15 seconds".into(), text("+15").font(UI).color(p.text), move || jump(&forward, SKIP)))
        .child(timeline)
        .child(text("0:00 / 0:00").key("time").w(if c.player.duration().unwrap_or_default() >= 3600.0 { 130.0 } else { 90.0 })
            .ml(6.0).font(UI.features("'tnum' 1")).color(p.muted).live_text(&c.transport.clock))
}
