//! A video file open in the Files panel: Desktop's `ArtifactVideo` (`@opencode/plugin` file
//! `artifact-view.tsx:508-690`). The clip sits in the stage's frame (`p-6`), as wide as the frame,
//! on black with 4 pt corners and the artifact shadow, under the controls Chromium draws for a
//! `<video controls>`: play, the time, the mute button with its slider, the overflow menu and the
//! timeline, over a gradient. They show while paused and while the pointer moves over a playing
//! clip, and fade once it rests or leaves (Desktop's `VideoOverlayState`,
//! `artifact-view-machine.ts:256-399`). Desktop's own expand button sits where Chromium's
//! fullscreen button would.
//!
//! The clip streams through Rune's `Player`: a file on this machine's server is read where it
//! lies, a remote one over the server's file route. Nothing of it is downloaded first.

use std::cell::Cell;
use std::hash::{Hash, Hasher};

use rune::{
    Anim, Color, Curve, Easing, El, Family, Icon, Phase, Player, PlayerState, Shadow,
    SurfaceFit, TextStyle, Tx, el, icon, row, text,
};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Pal, kit};
pub use opencode_ui::video_overlay::{Effect, Event, Overlay, step};

use crate::FilesHost;

pub use opencode_kit::path::is_video;

/// Builds the `/api/fs/read/...` path and lowercase file extension for downloading remote media
/// via `Authorization` HTTP headers (never `auth_token` query parameters).
pub fn remote_read_request(directory: &str, path: &str) -> (String, String) {
    use opencode_search::query::{encode_fs_path, percent_encode};
    let req = format!(
        "/api/fs/read/{}?location%5Bdirectory%5D={}",
        encode_fs_path(path),
        percent_encode(directory)
    );
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    (req, ext)
}

/// Workspace movie source shared by the file viewer and chat; never fetch the entire file, never
/// embed credentials (`auth_token`) in URLs (`packages/app/src/runtime/server/api.ts:45-56`), and
/// verify local files stay inside `directory` without symlinks/junctions before handing a local
/// path to `rune::Player`. On native local servers (or with no server), a rejected path returns
/// `None` with no fallback to `/api/fs/read/...` so symlinks and junctions cannot bypass
/// confinement. On password-protected remote servers, returns `None` so callers download media
/// via `Authorization` headers (`TempMediaFile`) instead.
pub fn source(server: Option<&opencode_kit::client::Server>, directory: &str, path: &str) -> Option<String> {
    let stream_url = |s: &opencode_kit::client::Server| {
        let (req, _) = remote_read_request(directory, path);
        s.stream_url(&req)
    };
    #[cfg(target_arch = "wasm32")]
    {
        return server.and_then(stream_url);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(s) = server.filter(|s| !s.is_local()) {
            return stream_url(s);
        }
        let candidate = std::path::Path::new(directory).join(path).to_string_lossy().into_owned();
        browser_policy::verify_within_root(&candidate, directory, true)
    }
}

#[cfg(not(target_arch = "wasm32"))]
type AuthVideoDone = (String, String, Result<opencode_kit::client::TempMediaFile, String>);

/// Background authenticated remote video downloads (`Authorization` header -> owner-only
/// `TempMediaFile`, never `auth_token` query parameters).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct AuthVideoDownloads {
    tx: std::sync::mpsc::Sender<AuthVideoDone>,
    rx: std::sync::mpsc::Receiver<AuthVideoDone>,
    in_flight: std::collections::HashSet<(String, String)>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for AuthVideoDownloads {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { tx, rx, in_flight: std::collections::HashSet::new() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AuthVideoDownloads {
    pub(crate) fn start(
        &mut self,
        server: &opencode_kit::client::Server,
        directory: &str,
        sid: &str,
        path: &str,
        waker: Option<rune::Waker>,
    ) {
        let key = (sid.to_owned(), path.to_owned());
        if !self.in_flight.insert(key.clone()) {
            return;
        }
        let srv = server.clone();
        let (req, ext) = remote_read_request(directory, path);
        let sid = sid.to_owned();
        let path = path.to_owned();
        let tx = self.tx.clone();
        if std::thread::Builder::new()
            .name("opencode-auth-video".into())
            .spawn(move || {
                let res = srv.download_media_temp(&req, &ext);
                let _ = tx.send((sid, path, res));
                if let Some(w) = waker {
                    w();
                }
            })
            .is_err()
        {
            self.in_flight.remove(&key);
        }
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.in_flight.is_empty()
    }

    pub(crate) fn is_loading(&self, sid: &str, path: &str) -> bool {
        self.in_flight.iter().any(|(s, p)| s == sid && p == path)
    }

    pub(crate) fn drain(&mut self) -> Vec<AuthVideoDone> {
        let mut out = Vec::new();
        while let Ok(item) = self.rx.try_recv() {
            self.in_flight.remove(&(item.0.clone(), item.1.clone()));
            out.push(item);
        }
        out
    }
}

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
const TIME: TextStyle = TextStyle::new(14.0, 400.0)
    .family(Family::Named("Roboto-Regular, Roboto, Arial"))
    .lh(48.0);

/// A video file's player and its controls' state.
pub struct FileVideo {
    pub player: Player,
    /// Control visibility machine (`Overlay`), stored in a `Cell` so timer renewals on pointer
    /// move don't bump the `FILES` slot or trigger panel rebuilds when visibility didn't change.
    overlay: Cell<Overlay>,
    /// Last quarter-second `timeupdate` bucket shown while controls were visible.
    shown_tick: Cell<Option<u64>>,
    /// The timeline under a drag: the clip time it points at.
    pub scrub: Scrub,
    /// It can't play, and the file was read as any other instead (`poll`).
    read_instead: bool,
    /// Keeps an authenticated remote download alive on disk until this player drops.
    #[cfg(not(target_arch = "wasm32"))]
    _temp_file: Option<opencode_kit::client::TempMediaFile>,
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
        FileVideo {
            player,
            overlay: Cell::default(),
            shown_tick: Cell::default(),
            scrub: Scrub::Idle,
            read_instead: false,
            #[cfg(not(target_arch = "wasm32"))]
            _temp_file: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_temp(temp: opencode_kit::client::TempMediaFile, waker: Option<rune::Waker>) -> Option<FileVideo> {
        let path = temp.path_str()?.to_owned();
        let mut clip = Self::new(&path, waker);
        clip._temp_file = Some(temp);
        Some(clip)
    }

    /// It can't play (Desktop's `decode.failed`).
    pub fn failed(&self) -> bool {
        matches!(self.player.state(), PlayerState::Failed(_))
    }

    pub fn is_loading(&self) -> bool {
        self.overlay.get() == Overlay::Loading
    }

    /// Chromium's controls show (and so does Desktop's expand button).
    pub fn controls_shown(&self) -> bool {
        self.overlay.get().shown() || self.scrub != Scrub::Idle
    }

    /// Hashes only visible video presentation state (excluding idle-timer deadlines and hidden
    /// playback timestamps) so pointer moves and hidden playback don't rebuild the panel.
    pub fn version_hash(&self, h: &mut impl Hasher) {
        let p = &self.player;
        let shown = self.controls_shown();
        let scrub_bits = match self.scrub {
            Scrub::Idle => None,
            Scrub::Dragging { at, .. } => Some(at.to_bits()),
        };
        let tick = shown.then(|| (p.time() * 4.0) as u64);
        (
            std::mem::discriminant(&p.state()),
            self.is_loading(),
            shown,
            scrub_bits,
            p.is_playing(),
            p.muted(),
            p.has_sound(),
            p.natural_size(),
            p.duration().map(f64::to_bits),
            tick,
        )
            .hash(h);
    }

    fn step_overlay(&self, event: Event, tx: &mut Tx) -> bool {
        let mut state = self.overlay.get();
        let was_shown = state.shown();
        let was_loading = state == Overlay::Loading;
        if let Some(Effect::WakeAt(due)) = step(&mut state, event) {
            tx.wake_at(due);
        }
        self.overlay.set(state);
        was_shown != state.shown() || was_loading != (state == Overlay::Loading)
    }
}

/// The timeline's drag (Chromium's range input): where it points, where it started (for cancel),
/// and whether the clip played when it started, so it plays on once let go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Scrub {
    #[default]
    Idle,
    Dragging { at: f64, origin: f64, resume: bool },
}

fn video_ref<'a>(host: &'a dyn FilesHost, sid: &str, path: &str) -> Option<&'a FileVideo> {
    host.files().sessions.get(sid)?.videos.get(path)
}

fn video_mut<'a>(host: &'a mut dyn FilesHost, sid: &str, path: &str) -> Option<&'a mut FileVideo> {
    host.files_mut().sessions.get_mut(sid)?.videos.get_mut(path)
}

/// Folds each open video's playback into its overlay, and moves a playing one's time on.
pub fn poll(host: &mut dyn FilesHost, tx: &mut Tx) -> bool {
    let mut instant_changed = false;
    #[cfg(not(target_arch = "wasm32"))]
    if !host.files().auth_videos.is_idle() {
        let finished = host.files_mut().auth_videos.drain();
        for (sid, path, res) in finished {
            instant_changed = true;
            let waker = host.files().waker.clone();
            let wrap = host.files().wrap;
            match res.ok().and_then(|temp| FileVideo::with_temp(temp, waker)) {
                Some(clip) => {
                    let ws = host.files_mut().sessions.entry(sid).or_default();
                    let mut doc = wrap.doc(&path, "");
                    doc.set_binary(0);
                    ws.docs.insert(path.clone(), doc);
                    ws.videos.insert(path, clip);
                }
                None => {
                    if let Some(ws) = host.files_mut().sessions.get_mut(&sid) {
                        ws.docs.remove(&path);
                    }
                    crate::ops::read_file(host, &sid, &path, true);
                }
            }
        }
    }
    if host.files().sessions.values().all(|ws| ws.videos.is_empty()) {
        if instant_changed {
            tx.instant();
        }
        return instant_changed;
    }
    let now = tx.clock();
    let mut overlay_changed = false;
    let mut tick_changed = false;
    let mut read = Vec::new();
    for (sid, ws) in host.files().sessions.iter() {
        for (path, v) in ws.videos.iter() {
            if v.failed() && !v.read_instead {
                read.push((sid.clone(), path.clone()));
            }
            let paused = !v.player.is_playing();
            let event = match (v.overlay.get(), v.player.state()) {
                (Overlay::Loading, PlayerState::Loading) => None,
                (Overlay::Loading, _) => Some(Event::Loaded { paused, now }),
                _ => Some(Event::Playback { paused, now }),
            };
            if let Some(event) = event {
                overlay_changed |= v.step_overlay(event, tx);
            }
            overlay_changed |= v.step_overlay(Event::Tick { now }, tx);
            if !paused && v.controls_shown() {
                let tick = (v.player.time() * 4.0) as u64;
                if v.shown_tick.replace(Some(tick)) != Some(tick) {
                    tick_changed = true;
                }
                tx.wake_at(now + TICK);
            } else if !v.controls_shown() {
                v.shown_tick.set(None);
            }
        }
    }
    // A clip that can't play is read as the file it is, as Desktop shows a failed decode.
    for (sid, path) in read {
        if let Some(ws) = host.files_mut().sessions.get_mut(&sid) {
            if let Some(v) = ws.videos.get_mut(&path) {
                v.read_instead = true;
            }
            ws.docs.remove(&path);
        }
        crate::ops::ensure_file_loaded(host, &sid, &path);
        instant_changed = true;
    }
    if overlay_changed || tick_changed {
        host.touch_files();
    }
    if overlay_changed {
        tx.animated();
    } else if tick_changed || instant_changed {
        tx.instant();
    }
    overlay_changed || tick_changed || instant_changed
}

/// Toggles play/pause for the video at `(sid, path)`; returns `true` if a video was toggled.
pub(crate) fn toggle_play(host: &mut dyn FilesHost, sid: &str, path: &str, tx: &mut Tx) -> bool {
    let Some(v) = video_ref(host, sid, path) else { return false };
    if v.player.is_playing() {
        v.player.pause();
    } else {
        v.player.play();
    }
    let now = tx.clock();
    let paused = !v.player.is_playing();
    let vis_changed = v.step_overlay(Event::Playback { paused, now }, tx);
    if !paused && v.controls_shown() {
        v.shown_tick.set(Some((v.player.time() * 4.0) as u64));
        tx.wake_at(now + TICK);
    }
    host.touch_files();
    if vis_changed {
        tx.animated();
    } else {
        tx.instant();
    }
    true
}

fn on_stage_press(host: &mut dyn FilesHost, sid: &str, path: &str, tx: &mut Tx) {
    toggle_play(host, sid, path, tx);
}

fn on_stage_pointer_move(host: &mut dyn FilesHost, sid: &str, path: &str, tx: &mut Tx) {
    let now = tx.clock();
    let Some(v) = video_ref(host, sid, path) else {
        tx.live_only();
        return;
    };
    if v.step_overlay(Event::Moved { now }, tx) {
        if v.player.is_playing() && v.controls_shown() {
            v.shown_tick.set(Some((v.player.time() * 4.0) as u64));
            tx.wake_at(now + TICK);
        }
        host.touch_files();
        tx.animated();
    } else {
        tx.live_only();
    }
}

fn on_stage_hover_change(host: &mut dyn FilesHost, sid: &str, path: &str, inside: bool, tx: &mut Tx) {
    if !inside
        && let Some(v) = video_ref(host, sid, path)
        && v.step_overlay(Event::Left, tx)
    {
        v.shown_tick.set(None);
        host.touch_files();
        tx.animated();
    } else {
        tx.live_only();
    }
}

fn on_button_press(host: &mut dyn FilesHost, sid: &str, path: &str, press: fn(&mut FileVideo), tx: &mut Tx) {
    if let Some(v) = video_mut(host, sid, path) {
        press(v);
        let now = tx.clock();
        let paused = !v.player.is_playing();
        let mut vis_changed = v.step_overlay(Event::Playback { paused, now }, tx);
        if !paused {
            vis_changed |= v.step_overlay(Event::Moved { now }, tx);
            if v.controls_shown() {
                v.shown_tick.set(Some((v.player.time() * 4.0) as u64));
                tx.wake_at(now + TICK);
            }
        }
        if vis_changed {
            tx.animated();
        } else {
            tx.instant();
        }
    }
}

fn on_timeline_drag(
    host: &mut dyn FilesHost,
    sid: &str,
    path: &str,
    x: f32,
    phase: Phase,
    cancelled: bool,
    track: f32,
    tx: &mut Tx,
) {
    let Some(v) = video_mut(host, sid, path) else { return };
    let duration = v.player.duration().unwrap_or(0.0);
    let at = ((x - TRACK_INSET) / track.max(1.0)).clamp(0.0, 1.0) as f64 * duration;
    match (v.scrub, phase) {
        (Scrub::Dragging { origin, resume, .. }, Phase::Up) => {
            let target = if cancelled { origin } else { at };
            v.scrub = Scrub::Idle;
            v.player.seek(target);
            if resume {
                v.player.play();
            }
            let now = tx.clock();
            let paused = !v.player.is_playing();
            v.step_overlay(Event::Playback { paused, now }, tx);
            if !paused {
                v.step_overlay(Event::Moved { now }, tx);
                tx.wake_at(now + TICK);
            }
        }
        (Scrub::Idle, Phase::Up) => {
            v.player.seek(at);
        }
        (Scrub::Idle, _) => {
            let resume = v.player.is_playing();
            let origin = v.player.time();
            v.player.pause();
            v.scrub = Scrub::Dragging { at, origin, resume };
            v.player.seek(at);
        }
        (Scrub::Dragging { origin, resume, .. }, _) => {
            v.scrub = Scrub::Dragging { at, origin, resume };
            v.player.seek_fast(at);
        }
    }
    tx.instant();
}

/// The stage for `v`, filling the panel under the toolbar.
pub fn stage<H: FilesHost + 'static>(s: &H, p: &Pal, session_id: &str, path: &str, v: &FileVideo) -> El {
    let [w, h] = s.files().stage_size.get();
    let (nw, nh) = v.player.natural_size().unwrap_or((16, 9));
    // `w-full` in the `p-6` frame, its height from the clip's shape, at most the frame's
    // (`max-height: 100%` with `object-fit: contain`).
    let box_w = (w - 48.0).max(0.0);
    let box_h = (box_w * nh as f32 / nw.max(1) as f32).min((h - 48.0).max(0.0)).round();
    let shown = v.controls_shown();
    let loading = v.is_loading();
    let (press_sid, press_path) = (session_id.to_owned(), path.to_owned());
    let picture = el()
        .key("artifact-media")
        .w(box_w)
        .h(box_h)
        .radius(4.0)
        .clip_rounded(4.0)
        .fill(Color::BLACK)
        .box_shadow(Pal::ring_out(p.border))
        .box_shadow(Shadow {
            y: 12.0,
            blur: 32.0,
            spread: -12.0,
            color: p.text.alpha(0.25),
            ..Shadow::default()
        })
        .on_press_tx(move |s: &mut H, tx: &mut Tx| on_stage_press(s, &press_sid, &press_path, tx))
        .video(&v.player, SurfaceFit::Fit)
        .children((!loading).then(|| controls::<H>(session_id, path, v, box_w, box_h, shown)));
    let mut expand = expand_button(p, shown).absolute(box_w - 96.0, box_h - 72.0);
    if !shown {
        expand = expand.inert();
    }
    let frame = el()
        .key("artifact-video-frame")
        .w(box_w)
        .h(box_h)
        .child(picture)
        .child(expand);
    let sizer = s.files().stage_size.clone();
    let (move_sid, move_path) = (session_id.to_owned(), path.to_owned());
    let (hover_sid, hover_path) = (session_id.to_owned(), path.to_owned());
    el().key("artifact-stage")
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .fill(p.layer1)
        .center()
        .on_layout_tx(move |s: &mut H, [_, _, w, h], tx| {
            let prev = sizer.get();
            if (prev[0] - w).abs() > 0.5 || (prev[1] - h).abs() > 0.5 {
                sizer.set([w, h]);
                s.touch_files();
                tx.instant();
            } else {
                tx.live_only();
            }
        })
        .on_pointer_move_local(move |s: &mut H, _, tx| on_stage_pointer_move(s, &move_sid, &move_path, tx))
        .on_hover_change(move |s: &mut H, inside, tx| on_stage_hover_change(s, &hover_sid, &hover_path, inside, tx))
        .child(frame)
        .children(loading.then(|| {
            // `MediaLoading`: Desktop's Loader at 16 pt in `text-weak`, over the stage.
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .center()
                .inert()
                .child(rune_kit::Loader::new().size(16.0).build(kit(p)))
        }))
}

/// Chromium's controls over a `w` x `h` clip: the scrim, the button row and the timeline. They
/// fade out over 1 s when the clip plays on with the pointer resting, and back in at once.
fn controls<H: FilesHost + 'static>(sid: &str, path: &str, v: &FileVideo, w: f32, h: f32, shown: bool) -> El {
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
        .inert()
        .linear_gradient(std::f32::consts::PI, SCRIM_STOPS.map(|(at, a)| (at, Color::BLACK.alpha(a))));
    let buttons = row()
        .key("media-buttons")
        .pin(Some(0.0), None, Some(0.0), Some(TIMELINE))
        .h(BUTTON)
        .center_y()
        .child(button::<H>(
            sid,
            path,
            "media-play",
            if playing { &PAUSE } else { &PLAY },
            if playing { "Pause" } else { "Play" },
            |v| {
                if v.player.is_playing() { v.player.pause() } else { v.player.play() }
            },
        ))
        .child(text(clock(time)).font(TIME).color(Color::WHITE))
        .child(text(format!("/ {}", clock(duration))).font(TIME).color(Color::WHITE).ml(4.0))
        .child(el().grow(1.0))
        // A clip with no sound track greys its mute button, as Chromium disables it.
        .child(match player.has_sound() {
            Some(false) => disabled_button("media-mute", &SOUND, "Mute"),
            _ => button::<H>(
                sid,
                path,
                "media-mute",
                if player.muted() { &MUTED } else { &SOUND },
                if player.muted() { "Unmute" } else { "Mute" },
                |v| v.player.set_muted(!v.player.muted()),
            ),
        })
        // Chromium's fullscreen button keeps its place, hidden; Desktop's expand button lies over it.
        .child(el().w(BUTTON).h(BUTTON))
        .child(button::<H>(sid, path, "media-more", &MORE, "Show more media controls", |_| {}));
    let mut bar = el()
        .key("media-controls")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(Anim::new(fade))
        .child(scrim)
        .child(buttons)
        .child(timeline::<H>(sid, path, v, w, time, duration));
    if !shown {
        bar = bar.inert();
    }
    bar
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

fn button_shell(key: &'static str, glyph: &'static Icon, label: &'static str) -> El {
    el().key(key)
        .size(BUTTON, BUTTON)
        .center()
        .role("button")
        .label(label)
        .child(
            el().size(HOVER_DISC, HOVER_DISC)
                .radius(HOVER_DISC / 2.0)
                .hover(hover())
                .hover_motion(
                    Anim::new(Curve::tween(0.25, Easing::EASE)),
                    Anim::new(Curve::tween(0.25, Easing::EASE)),
                )
                .center()
                .child(icon(glyph, 20.0).color(Color::WHITE)),
        )
}

/// A 48 pt control with its 20 pt glyph, white, and the hover disc behind it.
fn button<H: FilesHost + 'static>(
    sid: &str,
    path: &str,
    key: &'static str,
    glyph: &'static Icon,
    label: &'static str,
    press: fn(&mut FileVideo),
) -> El {
    let (sid, path) = (sid.to_owned(), path.to_owned());
    button_shell(key, glyph, label)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| on_button_press(s, &sid, &path, press, tx))
}

/// A control Chromium disables (`:disabled`): its glyph at 40 % opacity, no hover disc, no press.
/// Rune composites a light glyph on dark brighter than Chromium's opacity does, so 0.32 here lands
/// on Chromium's pixels (158 against 161 on the scrim at 1.5x).
fn disabled_button(key: &'static str, glyph: &'static Icon, label: &'static str) -> El {
    el().key(key)
        .size(BUTTON, BUTTON)
        .center()
        .role("button")
        .label(label)
        .child(icon(glyph, 20.0).color(Color::WHITE).opacity(0.32))
}

fn timeline_shell(track: f32, played: f32, scrubbing: bool) -> El {
    el().key("media-timeline")
        .pin(Some(0.0), None, Some(0.0), Some(0.0))
        .h(TIMELINE)
        .role("slider")
        .label("Video time scrubber")
        .child(
            el().absolute(TRACK_INSET, 0.0)
                .w(track)
                .h(TRACK)
                .fill(Color::WHITE.alpha(0.3))
                .child(el().absolute(0.0, 0.0).w(track).h(TRACK).fill(Color::WHITE.alpha(0.54)))
                .child(el().absolute(0.0, 0.0).w(played).h(TRACK).fill(Color::WHITE))
                .child(
                    el().absolute(played - 6.0, -4.0)
                        .size(12.0, 12.0)
                        .radius(6.0)
                        .fill(Color::WHITE)
                        .opacity(if scrubbing { 1.0 } else { 0.0 }),
                ),
        )
}

/// The timeline: the track 16 pt in from each side along the top of the bottom 24 pt, white
/// where it played, 54 % white what's loaded (all of it, from a file), 30 % white the rest.
/// A drag scrubs, and the clip plays on from where it's let go if it was playing.
fn timeline<H: FilesHost + 'static>(sid: &str, path: &str, v: &FileVideo, w: f32, time: f64, duration: f64) -> El {
    let track = (w - 2.0 * TRACK_INSET).max(0.0);
    let played = if duration > 0.0 {
        (time / duration).clamp(0.0, 1.0) as f32 * track
    } else {
        0.0
    };
    let scrubbing = v.scrub != Scrub::Idle;
    let (sid, path) = (sid.to_owned(), path.to_owned());
    timeline_shell(track, played, scrubbing)
        .on_drag(move |s: &mut H, d, tx: &mut Tx| on_timeline_drag(s, &sid, &path, d.x, d.phase, d.cancelled(), track, tx))
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
        .role("button")
        .label_static(opencode_i18n::t!("plugin.file.view.fullscreen"))
        .help(opencode_i18n::t!("plugin.file.view.fullscreen").to_owned())
        .child(
            el().size(HOVER_DISC, HOVER_DISC)
                .radius(HOVER_DISC / 2.0)
                .hover(Color::BLACK.alpha(0.55))
                .center()
                .child(icon(&ic::OUTLINE_EXPAND, 20.0).color(Color::WHITE)),
        )
}

static PLAY: Icon = Icon {
    name: "media-play",
    view: 24.0,
    body: r#"<path d="M8 5v14l11-7z" fill="currentColor"/>"#,
};
static PAUSE: Icon = Icon {
    name: "media-pause",
    view: 24.0,
    body: r#"<path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z" fill="currentColor"/>"#,
};
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
