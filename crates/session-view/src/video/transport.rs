//! One picture, one floating controller. The file viewer's visibility machine handles
//! motion over the movie; Rune's hover group keeps the controller up under the hand or
//! keyboard focus, without a timer or Tab stop for each individual button.
use std::rc::Rc;

use opencode_ui::video_overlay::Event;
use rune::{Align, Color, Cursor, El, Icon, SurfaceFit, TextStyle, Tx, col, el, icon, row, text};

use super::{Clip, seek};
use crate::{TimelineHost, palette::Pal, style::{HOVER_FADE, INSTANT, UI_SMALL}};

const SKIP: f64 = 15.0;
const HUD_HEIGHT: f32 = 88.0;
pub(super) const PLAY: Icon = Icon { name: "movie-play", view: 24.0, body: r#"<path d="M6 4.7c0-.95 1.03-1.54 1.85-1.05l12.07 7.3a1.22 1.22 0 0 1 0 2.1l-12.07 7.3C7.03 20.84 6 20.25 6 19.3z" fill="currentColor"/>"# };
const PAUSE: Icon = Icon { name: "movie-pause", view: 24.0, body: r#"<rect x="5.4" y="3.7" width="4.7" height="16.6" rx="1.2" fill="currentColor"/><rect x="13.9" y="3.7" width="4.7" height="16.6" rx="1.2" fill="currentColor"/>"# };
const BACK: Icon = Icon { name: "movie-back-15", view: 24.0, body: r#"<path d="M3.7 10.4a8.55 8.55 0 1 0 8.3-6.2H9.3m2.9-2.8L9.3 4.2l2.9 2.9" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round"/>"# };
const AHEAD: Icon = Icon { name: "movie-forward-15", view: 24.0, body: r#"<path d="M20.3 10.4a8.55 8.55 0 1 1-8.3-6.2h2.7m-2.9-2.8 2.9 2.8-2.9 2.9" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round"/>"# };

pub(super) fn clock(s: u64) -> String {
    if s < 3600 { format!("{}:{:02}", s / 60, s % 60) }
    else { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) }
}

pub(super) fn material(e: El, radius: f32) -> El {
    e.radius(radius).continuous_corners().fill(Color::BLACK.alpha(0.78))
        .box_shadow(Pal::ring_in(Color::WHITE.alpha(0.15)))
}

fn skip_glyph(glyph: &'static Icon) -> El {
    el().size(26.0, 26.0).center().child(icon(glyph, 26.0).color(Color::WHITE))
        .child(text("15").absolute(0.0, 8.5).w(26.0).align(Align::Center)
            .font(TextStyle::new(9.0, 620.0).lh(12.0)).color(Color::WHITE).inert())
}

fn button<S: TimelineHost>(key: &'static str, label: String, content: El, width: f32, c: &Rc<Clip>, delta: Option<f64>) -> El {
    let clip = c.clone();
    let b = el().key(key).size(width, 44.0).center().role("button").label(label.clone()).help(label)
        .focusable(true).focus_ring(1.0, -3.0, Color::WHITE.alpha(0.9)).radius(22.0)
        .cursor(Cursor::Pointer).hover(Color::WHITE.alpha(0.10)).press(Color::WHITE.alpha(0.16)).press_scale(0.96)
        .child(content);
    if delta.is_some() && c.player.duration().is_none() { return b.disabled(true).opacity(0.35); }
    b.on_press_tx(move |s: &mut S, tx: &mut Tx| {
        if let Some(by) = delta {
            clip.player.seek(clip.player.time() + by);
            clip.track.sync(&clip.player);
        } else { clip.toggle(); }
        clip.refresh(s, tx);
    })
}

fn controls<S: TimelineHost>(c: &Rc<Clip>, name: &str) -> El {
    let playing = c.track.continuing(&c.player);
    let action = if playing { opencode_i18n::t!("lab.pause") } else { opencode_i18n::t!("lab.play") };
    let long = c.player.duration().is_some_and(|d| d >= 3600.0);
    let compact = c.track.compact(&c.player);
    let time_w = if compact { 52.0 } else if long { 102.0 } else { 75.0 };
    let mut readout = col().w(time_w).justify_center()
        .child(text("0:00").key("time").w_full().align(Align::End)
            .font(UI_SMALL.features("'tnum' 1")).color(Color::WHITE.alpha(0.82)).live_text(&c.track.time));
    if compact && let Some(d) = c.player.duration() {
        readout.push_child(text(format!("/ {}", clock(d as u64))).key("total").w_full().align(Align::End)
            .font(TextStyle::new(10.0, 480.0).lh(12.0).features("'tnum' 1")).color(Color::WHITE.alpha(0.62)));
    }
    // Equal flexible wings center the transport. The time's minimum nudges it left
    // on portrait clips, keeping the three targets and the readout apart.
    let buttons = row().key("buttons").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(44.0).px(12.0).center_y()
        .child(el().w(0.0).grow(1.0).min_w(0.0))
        .child(button::<S>("back-15", opencode_i18n::t!("rune.video.back15").into(), skip_glyph(&BACK), 44.0, c, Some(-SKIP)))
        .child(button::<S>("transport", format!("{action}: {name}"), icon(if playing { &PAUSE } else { &PLAY }, 25.0).color(Color::WHITE), 52.0, c, None))
        .child(button::<S>("forward-15", opencode_i18n::t!("rune.video.forward15").into(), skip_glyph(&AHEAD), 44.0, c, Some(SKIP)))
        .child(row().w(0.0).grow(1.0).min_w(time_w).justify_end().center_y().child(readout));
    let shown = c.shown();
    let hud = el().key("controls").w_full().h(HUD_HEIGHT)
        .group_opacity(if shown { 1.0 } else { 0.0 }, 1.0)
        .opacity_motion(if shown { INSTANT } else { HOVER_FADE })
        .child(material(el().key("material").pin(Some(0.0), Some(23.0), Some(0.0), Some(0.0)), 12.0)
            .on_background_press_tx(|_: &mut S, tx| tx.live_only()))
        .child(buttons)
        .child(seek::view::<S>(c, name).pin(Some(0.0), Some(0.0), Some(0.0), None));
    el().key("hud").pin(Some(10.0), None, Some(10.0), Some(8.0)).h(HUD_HEIGHT)
        .hover_group_focus().hover_motion(INSTANT, HOVER_FADE).child(hud)
}

pub(super) fn stage<S: TimelineHost>(p: &Pal, c: Rc<Clip>, name: &str) -> El {
    // Bound the stage, not the movie: ordinary clips get their exact aspect; very thin
    // clips get centered letterboxing so their controls still have room.
    let ratio = c.player.natural_size().filter(|&(w, h)| w > 0 && h > 0)
        .map_or(16.0 / 9.0, |(w, h)| w as f32 / h as f32).clamp(9.0 / 16.0, 3.0);
    let press = c.clone();
    let moved = c.clone();
    let leave = c.clone();
    let picture = el().key("picture").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .video(&c.player, SurfaceFit::Fit).label(name.to_owned()).cursor(Cursor::Pointer)
        .on_background_press_tx(move |s: &mut S, tx| { press.toggle(); press.refresh(s, tx); });
    el().w_full().max_w(420.0 * ratio).aspect_ratio(ratio).self_center()
        .fill(Color::BLACK).radius(10.0).clip_rounded(10.0).continuous_corners()
        .box_shadow(Pal::ring_in(p.border_muted))
        .on_pointer_move_local(move |s: &mut S, _, tx| {
            if moved.observe(Event::Moved { now: tx.clock() }, tx) { moved.refresh(s, tx); }
            else { tx.live_only(); }
        })
        .on_hover_change(move |s: &mut S, inside, tx| {
            if !inside && leave.observe(Event::Left, tx) { leave.refresh(s, tx); }
            else { tx.live_only(); }
        })
        .child(picture)
        .child(controls::<S>(&c, name))
}
