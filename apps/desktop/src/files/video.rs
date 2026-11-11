//! A video file open in the Files panel: Desktop's `ArtifactVideo` (`@opencode/plugin` file
//! `artifact-view.tsx:508-690`). The clip sits in the stage's frame (`p-6`), as wide as the frame,
//! on black with 4 pt corners and the artifact shadow, under the controls Chromium draws for a
//! `<video controls>`: play, the time, the mute button with its slider, the overflow menu and the
//! timeline, over a gradient. They show while paused and while the pointer moves over a playing
//! clip, and fade once it rests or leaves (Desktop's `VideoOverlayState`,
//! `artifact-view-machine.ts:256-399`). Desktop's own expand button sits where Chromium's
//! fullscreen button would.
//!
//! The clip streams with Media Foundation through Rune's `Player`: a file on this machine's
//! server is read where it lies, a remote one over the server's file route. Nothing of it is
//! downloaded first.

use rune::{Anim, Color, Curve, Cursor, Easing, El, Family, Icon, Player, PlayerState, Shadow, SurfaceFit, TextStyle, Tx, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;

/// What Desktop shows as a video, by extension (`util/src/artifact.ts:46-51`).
pub fn is_video(path: &str) -> bool {
    let name = opencode_kit::path::basename(path);
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .is_some_and(|(_, e)| matches!(e.to_ascii_lowercase().as_str(), "mp4" | "m4v" | "webm" | "mov" | "ogv" | "mkv"))
}

/// `VIDEO_IDLE_MS`: how long a playing clip keeps its controls after the pointer last moved.
const IDLE: f64 = 2.5;
/// How often a playing clip's time and timeline move: Chromium's `timeupdate`.
const TICK: f64 = 0.25;
/// Chromium's controls: 48 pt buttons over a 24 pt timeline, a 112 pt scrim under them.
const BUTTON: f32 = 48.0;
const TIMELINE: f32 = 24.0;
const SCRIM: f32 = 112.0;
/// The timeline's track: 16 pt in from each side, 4 pt thick.
const TRACK_INSET: f32 = 16.0;
const TRACK: f32 = 4.0;
/// `--internal-media-controls-button-hover-background`: a 36 pt disc.
const HOVER_DISC: f32 = 36.0;
/// The time: Roboto at 14 px, white, on the buttons' 48 pt line.
const TIME: TextStyle = TextStyle::new(14.0, 400.0).family(Family::Named("Roboto-Regular, Roboto, Arial")).lh(48.0);

/// A video file's player and its controls' state.
pub struct FileVideo {
    pub player: Player,
    pub overlay: Overlay,
    /// The timeline under a drag: the clip time it points at.
    pub scrub: Scrub,
}

impl FileVideo {
    /// A clip at `source` (a path or a URL), paused at its first frame, as `preload="metadata"`.
    pub fn new(source: &str, waker: Option<rune::Waker>) -> FileVideo {
        let player = Player::new(source.into());
        player.set_muted(false);
        if let Some(waker) = waker {
            player.set_waker(waker);
        }
        player.preload();
        FileVideo { player, overlay: Overlay::default(), scrub: Scrub::Idle }
    }

    /// Chromium's controls show (and so does Desktop's expand button).
    fn controls_shown(&self) -> bool {
        matches!(self.overlay, Overlay::Paused | Overlay::Active { .. }) || self.scrub != Scrub::Idle
    }
}

/// Desktop's `VideoOverlayState` without the preview's `expanded`: loading until the first frame,
/// then paused, or playing with the controls up until `due`, or playing with them down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Overlay {
    #[default]
    Loading,
    Paused,
    /// Playing, the pointer moved lately: the controls go at `due` on the window clock.
    Active { due: f64 },
    /// Playing, the pointer rested or left.
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// The clip has a frame to show (`media.loaded`).
    Loaded { paused: bool, now: f64 },
    /// It started or stopped playing (`playback.changed`).
    Playback { paused: bool, now: f64 },
    /// The pointer moved over the stage (`pointer.moved`).
    Moved { now: f64 },
    /// The pointer left the stage (`pointer.left`).
    Left,
    /// The window clock reached `now` (`idle.elapsed`).
    Tick { now: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    WakeAt(f64),
}

/// `transitionVideoOverlay`, as one transition. A later move's timer replaces an earlier one's:
/// the state keeps only the newest `due`, so an older timer's tick changes nothing.
pub fn step(state: &mut Overlay, event: Event) -> Option<Effect> {
    let active = |now: f64| (Overlay::Active { due: now + IDLE }, Some(Effect::WakeAt(now + IDLE)));
    let (next, effect) = match (*state, event) {
        (Overlay::Loading, Event::Loaded { paused: true, .. }) => (Overlay::Paused, None),
        (Overlay::Loading, Event::Loaded { paused: false, now }) => active(now),
        (Overlay::Paused, Event::Playback { paused: false, now }) => active(now),
        (Overlay::Active { .. } | Overlay::Idle, Event::Playback { paused: true, .. }) => (Overlay::Paused, None),
        (Overlay::Active { .. } | Overlay::Idle, Event::Moved { now }) => active(now),
        (Overlay::Active { .. }, Event::Left) => (Overlay::Idle, None),
        (Overlay::Active { due }, Event::Tick { now }) if now >= due => (Overlay::Idle, None),
        _ => return None,
    };
    *state = next;
    effect
}

/// The timeline's drag (Chromium's range input): where it points, and whether the clip played
/// when it started, so it plays on once let go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Scrub {
    #[default]
    Idle,
    Dragging { at: f64, resume: bool },
}

/// The open video's state, for the stage's handlers: the active file tab's.
fn active(s: &mut App) -> Option<&mut FileVideo> {
    let (sid, path) = s.active_file_tab()?;
    s.files.sessions.get_mut(&sid)?.videos.get_mut(&path?)
}

fn apply(effect: Option<Effect>, tx: &mut Tx) {
    if let Some(Effect::WakeAt(due)) = effect {
        tx.wake_at(due);
    }
    tx.animated();
}

/// Folds each open video's playback into its overlay, and moves a playing one's time on.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let now = tx.clock();
    let mut changed = false;
    let mut playing = false;
    for ws in s.files.sessions.values_mut() {
        for v in ws.videos.values_mut() {
            let paused = !v.player.is_playing();
            let before = v.overlay;
            let event = match (v.overlay, v.player.state()) {
                (Overlay::Loading, PlayerState::Loading) => None,
                (Overlay::Loading, _) => Some(Event::Loaded { paused, now }),
                _ => Some(Event::Playback { paused, now }),
            };
            if let Some(event) = event {
                if let Some(Effect::WakeAt(due)) = step(&mut v.overlay, event) {
                    tx.wake_at(due);
                }
            }
            if let Some(Effect::WakeAt(due)) = step(&mut v.overlay, Event::Tick { now }) {
                tx.wake_at(due);
            }
            changed |= v.overlay != before;
            playing |= !paused;
        }
    }
    if playing {
        tx.wake_at(now + TICK);
        changed = true;
    }
    if changed {
        tx.animated();
    }
    changed
}

/// The stage for `v`, filling the panel under the toolbar.
pub fn stage(s: &App, p: &Pal, v: &FileVideo) -> El {
    let [w, h] = s.files.stage_size.get();
    let (nw, nh) = v.player.natural_size().unwrap_or((16, 9));
    // `w-full` in the `p-6` frame, its height from the clip's shape, at most the frame's
    // (`max-height: 100%` with `object-fit: contain`).
    let box_w = (w - 48.0).max(0.0);
    let box_h = (box_w * nh as f32 / nw.max(1) as f32).min((h - 48.0).max(0.0)).round();
    let shown = v.controls_shown();
    let loading = v.overlay == Overlay::Loading;
    let picture = el()
        .key("artifact-media")
        .w(box_w)
        .h(box_h)
        .radius(4.0)
        .clip_rounded(4.0)
        .fill(Color::BLACK)
        .box_shadow(Pal::ring_out(p.border))
        .box_shadow(Shadow { y: 12.0, blur: 32.0, spread: -12.0, color: p.text.alpha(0.25), ..Shadow::default() })
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            if let Some(v) = active(s) {
                if v.player.is_playing() { v.player.pause() } else { v.player.play() }
                let now = tx.clock();
                let paused = !v.player.is_playing();
                apply(step(&mut v.overlay, Event::Playback { paused, now }), tx);
            }
        })
        .video(&v.player, SurfaceFit::Fit)
        .children((!loading).then(|| controls(v, box_w, box_h, shown)));
    let expand = expand_button(p, shown).absolute(box_w - 96.0, box_h - 72.0);
    let frame = el().key("artifact-video-frame").w(box_w).h(box_h).child(picture).child(expand);
    let sizer = s.files.stage_size.clone();
    el().key("artifact-stage")
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .fill(p.layer1)
        .center()
        .on_layout_tx(move |_: &mut App, [_, _, w, h], tx| {
            sizer.set([w, h]);
            tx.instant();
        })
        .on_pointer_move_local(|s: &mut App, _, tx| {
            let now = tx.clock();
            if let Some(v) = active(s) {
                let effect = step(&mut v.overlay, Event::Moved { now });
                if effect.is_some() {
                    apply(effect, tx);
                }
            }
        })
        .on_hover_change(|s: &mut App, inside, tx| {
            if !inside && let Some(v) = active(s) && step(&mut v.overlay, Event::Left).is_none() && v.overlay == Overlay::Idle {
                tx.animated();
            }
        })
        .child(frame)
        .children(loading.then(|| {
            // `MediaLoading`: Desktop's Loader at 16 pt in `text-weak`, over the stage.
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center().child(rune_kit::Loader::new().size(16.0).build(crate::palette::kit(p)))
        }))
}

/// Chromium's controls over a `w` x `h` clip: the scrim, the button row and the timeline. They
/// fade out over 1 s when the clip plays on with the pointer resting, and back in at once.
fn controls(v: &FileVideo, w: f32, h: f32, shown: bool) -> El {
    let player = &v.player;
    let duration = player.duration().unwrap_or(0.0);
    let time = match v.scrub {
        Scrub::Dragging { at, .. } => at,
        Scrub::Idle => player.time(),
    };
    let playing = player.is_playing();
    let fade = if shown { Curve::tween(0.25, Easing::EASE) } else { Curve::tween(1.0, Easing::EASE) };
    let scrim = el()
        .pin(Some(0.0), None, Some(0.0), Some(0.0))
        .h(SCRIM.min(h))
        .linear_gradient(std::f32::consts::PI, SCRIM_STOPS.map(|(at, a)| (at, Color::BLACK.alpha(a))));
    let buttons = row()
        .key("media-buttons")
        .pin(Some(0.0), None, Some(0.0), Some(TIMELINE))
        .h(BUTTON)
        .center_y()
        .child(button("media-play", if playing { &PAUSE } else { &PLAY }, if playing { "Pause" } else { "Play" }, |v| {
            if v.player.is_playing() { v.player.pause() } else { v.player.play() }
        }))
        .child(text(clock(time)).font(TIME).color(Color::WHITE))
        .child(text(format!("/ {}", clock(duration))).font(TIME).color(Color::WHITE).ml(4.0))
        .child(el().grow(1.0))
        .child(button("media-mute", if player.muted() { &MUTED } else { &SOUND }, if player.muted() { "Unmute" } else { "Mute" }, |v| {
            v.player.set_muted(!v.player.muted());
        }))
        // Chromium's fullscreen button keeps its place, hidden; Desktop's expand button lies over it.
        .child(el().w(BUTTON).h(BUTTON))
        .child(button("media-more", &MORE, "Show more media controls", |_| {}));
    el().key("media-controls")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(Anim::new(fade))
        .child(scrim)
        .child(buttons)
        .child(timeline(v, w, time, duration))
}

/// The scrim's stops, bottom (1) to top (0): Chromium's eased black.
const SCRIM_STOPS: [(f32, f32); 16] = [
    (0.0, 0.0),
    (0.081, 0.01),
    (0.155, 0.047),
    (0.225, 0.106),
    (0.29, 0.176),
    (0.353, 0.26),
    (0.412, 0.353),
    (0.471, 0.45),
    (0.529, 0.55),
    (0.588, 0.647),
    (0.647, 0.74),
    (0.71, 0.824),
    (0.775, 0.894),
    (0.845, 0.953),
    (0.919, 0.99),
    (1.0, 1.0),
];

/// The hover disc's `rgba(32, 33, 36, 0.71)`.
fn hover() -> Color {
    rune::color::hex("#202124").unwrap_or(Color::BLACK).alpha(0.71)
}

/// `m:ss`, or `h:mm:ss` from an hour, as Chromium writes media times.
fn clock(seconds: f64) -> String {
    let t = if seconds.is_finite() { seconds.max(0.0) as u64 } else { 0 };
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// A 48 pt control with its 20 pt glyph, white, and the hover disc behind it.
fn button(key: &'static str, glyph: &'static Icon, label: &'static str, press: fn(&mut FileVideo)) -> El {
    el().key(key)
        .size(BUTTON, BUTTON)
        .center()
        .cursor(Cursor::Pointer)
        .role("button")
        .label(label)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if let Some(v) = active(s) {
                press(v);
                let now = tx.clock();
                let paused = !v.player.is_playing();
                apply(step(&mut v.overlay, Event::Playback { paused, now }), tx);
                tx.animated();
            }
        })
        .child(el().size(HOVER_DISC, HOVER_DISC).radius(HOVER_DISC / 2.0).hover(hover()).hover_motion(Anim::new(Curve::tween(0.25, Easing::EASE)), Anim::new(Curve::tween(0.25, Easing::EASE))).center().child(icon(glyph, 20.0).color(Color::WHITE)))
}

/// The timeline: the track 16 pt in from each side along the top of the bottom 24 pt, white
/// where it played, 54 % white what's loaded (all of it, from a file), 30 % white the rest.
/// A drag scrubs, and the clip plays on from where it's let go if it was playing.
fn timeline(v: &FileVideo, w: f32, time: f64, duration: f64) -> El {
    let track = (w - 2.0 * TRACK_INSET).max(0.0);
    let played = if duration > 0.0 { (time / duration).clamp(0.0, 1.0) as f32 * track } else { 0.0 };
    let scrubbing = v.scrub != Scrub::Idle;
    el().key("media-timeline")
        .pin(Some(0.0), None, Some(0.0), Some(0.0))
        .h(TIMELINE)
        .cursor(Cursor::Pointer)
        .role("slider")
        .label("Video time scrubber")
        .on_drag(move |s: &mut App, d, tx: &mut Tx| {
            let Some(v) = active(s) else { return };
            let duration = v.player.duration().unwrap_or(0.0);
            let at = ((d.x - TRACK_INSET) / track.max(1.0)).clamp(0.0, 1.0) as f64 * duration;
            v.scrub = match (v.scrub, d.phase == rune::Phase::Up) {
                (_, true) => {
                    if let Scrub::Dragging { resume: true, .. } = v.scrub {
                        v.player.play();
                    }
                    Scrub::Idle
                }
                (Scrub::Idle, false) => {
                    let resume = v.player.is_playing();
                    v.player.pause();
                    Scrub::Dragging { at, resume }
                }
                (Scrub::Dragging { resume, .. }, false) => Scrub::Dragging { at, resume },
            };
            v.player.seek(at);
            tx.instant();
        })
        .child(
            el().absolute(TRACK_INSET, 0.0)
                .w(track)
                .h(TRACK)
                .fill(Color::WHITE.alpha(0.3))
                .child(el().absolute(0.0, 0.0).w(track).h(TRACK).fill(Color::WHITE.alpha(0.54)))
                .child(el().absolute(0.0, 0.0).w(played).h(TRACK).fill(Color::WHITE))
                .child(el().absolute(played - 6.0, -4.0).size(12.0, 12.0).radius(6.0).fill(Color::WHITE).opacity(if scrubbing { 1.0 } else { 0.0 })),
        )
}

/// Desktop's expand button (`artifact-video-fullscreen`): a 48 pt hit area 96 in from the clip's
/// right and 72 up from its bottom, where Chromium's fullscreen button sits, with a 36 pt disc
/// that darkens to `rgb(0 0 0 / 0.55)` under the pointer and the white `outline-expand` glyph.
fn expand_button(p: &Pal, shown: bool) -> El {
    let _ = p;
    el().key("artifact-video-fullscreen")
        .size(BUTTON, BUTTON)
        .center()
        .opacity(if shown { 1.0 } else { 0.0 })
        .cursor(Cursor::Pointer)
        .role("button")
        .label(opencode_i18n::t!("plugin.file.view.fullscreen"))
        .help(opencode_i18n::t!("plugin.file.view.fullscreen").to_owned())
        .child(el().size(HOVER_DISC, HOVER_DISC).radius(HOVER_DISC / 2.0).hover(Color::BLACK.alpha(0.55)).center().child(icon(&ic::OUTLINE_EXPAND, 20.0).color(Color::WHITE)))
}

static PLAY: Icon = Icon { name: "media-play", view: 24.0, body: r#"<path d="M8 5v14l11-7z" fill="currentColor"/>"# };
static PAUSE: Icon = Icon { name: "media-pause", view: 24.0, body: r#"<path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z" fill="currentColor"/>"# };
static SOUND: Icon = Icon {
    name: "media-sound",
    view: 24.0,
    body: r#"<path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z" fill="currentColor"/>"#,
};
static MUTED: Icon = Icon {
    name: "media-muted",
    view: 24.0,
    body: r#"<path d="M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3L3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4L9.91 6.09 12 8.18V4z" fill="currentColor"/>"#,
};
static MORE: Icon = Icon {
    name: "media-more",
    view: 24.0,
    body: r#"<circle cx="12" cy="6" r="2" fill="currentColor"/><circle cx="12" cy="12" r="2" fill="currentColor"/><circle cx="12" cy="18" r="2" fill="currentColor"/>"#,
};
