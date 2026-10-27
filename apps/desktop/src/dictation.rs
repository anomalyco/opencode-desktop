//! Dictation through Hex: the mic in the composer's toolbar, or ⇧⌘M, borrows Hex's microphone
//! and model and puts the words at the caret, ready to edit. A press starts and a second press
//! (or the stop) finishes; holding ⇧⌘M past `HOLD` talks until it comes up. Esc throws the
//! recording away. Send or ⏎ while it listens finishes and sends with the words, as the iOS
//! app's Send does; otherwise nothing is sent.
//!
//! It looks like the OpenCode iOS app's composer: while it listens the mic is a quiet stop beside
//! Send, which shows a spinner while the words come; "Preparing dictation…" and Hex's refusals
//! sit under the toolbar. The words come from Hex's capture API (`hex_client`): the app owns the
//! recording, so a pause never ends it, the transcript comes back whole (Hex has no partials) and
//! Hex pastes nothing, so the composer keeps its focus. The mic shows while Hex's endpoint file
//! is there (Hex removes it when it quits). Tests, headless runs and the storybook use a fake Hex
//! unless `SEER_HEX` names an endpoint file, so they never open the microphone.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hex_client::Hex;
use rune::{El, Icon, Key, KeyEvent, Tx, el, icon, row, text};

use crate::App;
use crate::palette::Pal;
use crate::view::{UI_SMALL, activity_ring_on, no_motion};

/// Who Hex says is recording.
const SOURCE: &str = "OpenCode";
/// Holding the shortcut longer than this is push-to-talk: the recording ends when it comes up.
const HOLD: f64 = 0.3;
/// How long the microphone stays open after the stop, so a last word said as he presses lands.
/// Hex's own shortcut keeps 200 ms after release, but its capture API stops at once.
const TAIL: Duration = Duration::from_millis(350);
/// "Preparing dictation…" shows once Hex has taken this long to start, so a quick start shows
/// nothing but the stop.
const PREPARING_AFTER: f64 = 0.25;
/// Seconds between looks for Hex's endpoint file while idle.
const LOOK_SECS: f64 = 2.0;
/// The fake's transcription time.
const FAKE_TRANSCRIBE: f64 = 0.6;
/// The fake's start when the storybook asks for a slow one.
const FAKE_SLOW_START: f64 = 1.2;
pub const FAKE_WORDS: &str = "keep the retry budget as it is and add a test for the reconnect";

const MIC: Icon = Icon {
    name: "outline-mic",
    view: 16.0,
    body: r#"<path d="M8 1.5C6.89543 1.5 6 2.39543 6 3.5V8C6 9.10457 6.89543 10 8 10C9.10457 10 10 9.10457 10 8V3.5C10 2.39543 9.10457 1.5 8 1.5Z" stroke="currentColor"/><path d="M3.5 7.5V8C3.5 10.4853 5.51472 12.5 8 12.5C10.4853 12.5 12.5 10.4853 12.5 8V7.5" stroke="currentColor" stroke-linecap="square"/><path d="M8 12.5V14.5" stroke="currentColor" stroke-linecap="square"/>"#,
};

enum Backend {
    /// In-process: ready after `Dictation::prepare`, words `FAKE_TRANSCRIBE` after the stop.
    Fake,
    /// The real Hex, found through this endpoint file.
    Hex(PathBuf),
}

impl Backend {
    fn detect() -> Backend {
        let headless = std::env::args().any(|a| matches!(a.as_str(), "--snap" | "--tree" | "--script" | "--video" | "--strip" | "--frames" | "--scene" | "--fixture"));
        if std::env::var_os("SEER_HEX").is_none() && (cfg!(test) || headless) { Backend::Fake } else { Backend::Hex(hex_client::discovery()) }
    }

    fn installed(&self) -> bool {
        match self {
            Backend::Fake => true,
            Backend::Hex(file) => file.exists(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Hold {
    None,
    /// The shortcut went down and started a recording; the next poll stamps the time.
    Pressed,
    Since(f64),
}

pub struct Dictation {
    backend: Backend,
    /// Hex's endpoint file was there at the last look, so the mic shows.
    installed: bool,
    looked: Option<f64>,
    rec: Option<Rec>,
    /// What Hex's threads heard, tagged with their recording.
    shared: Arc<Mutex<Shared>>,
    round: u64,
    /// Seconds since launch, from the last frame.
    now: f64,
    /// What Hex refused with, until the next press or send (as iOS keeps it).
    error: Option<String>,
    hold: Hold,
    /// Seconds the fake takes to start.
    prepare: f64,
}

struct Rec {
    round: u64,
    /// When the press started it.
    since: f64,
    /// Hex is listening.
    ready: bool,
    /// Waiting for the words.
    stopping: bool,
    /// Send or ⏎ finished it: the words are sent when they land (true for ⌘⏎).
    send_after: Option<bool>,
    /// Tells the real Hex's thread to stop or cancel; None with the fake.
    cmd: Option<Sender<Cmd>>,
    /// The fake's words are due at this time.
    due: Option<f64>,
}

#[derive(Default)]
struct Shared {
    heard: Vec<(u64, Heard)>,
}

enum Cmd {
    Stop,
    Cancel,
}

#[derive(Debug)]
enum Heard {
    /// Hex took the microphone.
    Ready,
    Words(String),
    Failed(String),
}

impl Default for Dictation {
    fn default() -> Self {
        Self::with(Backend::detect())
    }
}

impl Dictation {
    fn with(backend: Backend) -> Dictation {
        let installed = backend.installed();
        Dictation { backend, installed, looked: None, rec: None, shared: Arc::default(), round: 0, now: 0.0, error: None, hold: Hold::None, prepare: 0.0 }
    }

    /// The storybook's Hex, whatever the app runs on.
    pub fn fake() -> Dictation {
        Self::with(Backend::Fake)
    }

    /// The real Hex behind this endpoint file (tests point it at a stand-in).
    #[cfg(test)]
    pub fn at(file: PathBuf) -> Dictation {
        Self::with(Backend::Hex(file))
    }

    pub fn recording(&self) -> bool {
        self.rec.as_ref().is_some_and(|r| !r.stopping)
    }

    /// A recording is on: preparing, listening or finishing.
    pub fn active(&self) -> bool {
        self.rec.is_some()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The fake starts slowly from now on, as a Hex loading its model would.
    pub fn slow_fake(&mut self) {
        self.prepare = FAKE_SLOW_START;
    }

    #[cfg(test)]
    pub fn preparing_shown(&self) -> bool {
        self.preparing()
    }

    /// Hex is slow to start, so "Preparing dictation…" shows.
    fn preparing(&self) -> bool {
        self.rec.as_ref().is_some_and(|r| !r.ready && !r.stopping && self.now - r.since >= PREPARING_AFTER)
    }

    fn fail(&mut self, why: impl Into<String>) {
        self.rec = None;
        self.hold = Hold::None;
        self.error = Some(why.into());
    }
}

pub fn start(s: &mut App) {
    let d = &mut s.dictation;
    if d.rec.is_some() {
        return;
    }
    d.installed = d.backend.installed();
    if !d.installed {
        d.fail(hex_client::NOT_RUNNING);
        return;
    }
    d.error = None;
    d.round += 1;
    let round = d.round;
    let cmd = match &d.backend {
        Backend::Fake => None,
        Backend::Hex(file) => {
            let (tx, rx) = mpsc::channel();
            let (file, shared) = (file.clone(), d.shared.clone());
            std::thread::spawn(move || {
                let heard = listen(&file, &rx, &shared, round);
                if let Some(h) = heard {
                    shared.lock().unwrap_or_else(|e| e.into_inner()).heard.push((round, h));
                }
            });
            Some(tx)
        }
    };
    let fake = cmd.is_none();
    d.rec = Some(Rec { round, since: d.now, ready: fake && d.prepare <= 0.0, stopping: false, send_after: None, cmd, due: None });
}

/// Stops the microphone; the words land at the caret when Hex has them.
pub fn stop(s: &mut App) {
    let d = &mut s.dictation;
    d.hold = Hold::None;
    let now = d.now;
    let Some(rec) = d.rec.as_mut().filter(|r| !r.stopping) else { return };
    rec.stopping = true;
    match &rec.cmd {
        Some(cmd) => {
            let _ = cmd.send(Cmd::Stop);
        }
        None => rec.due = Some(now + FAKE_TRANSCRIBE),
    }
}

pub fn cancel(s: &mut App) {
    let d = &mut s.dictation;
    d.hold = Hold::None;
    if let Some(Rec { cmd: Some(cmd), .. }) = d.rec.take() {
        let _ = cmd.send(Cmd::Cancel);
    }
}

/// The mic or the shortcut: starts, or finishes the recording that's listening.
fn toggle(s: &mut App) {
    match &s.dictation.rec {
        None => start(s),
        Some(r) if !r.stopping => stop(s),
        Some(_) => {}
    }
}

/// Send or ⏎ (`alternate` for ⌘⏎): while it listens, finishes and sends with the words, as the
/// iOS app's Send does; while the words come, waits for them. True when dictation took it.
pub fn submit(s: &mut App, alternate: bool) -> bool {
    let d = &mut s.dictation;
    d.error = None;
    match &mut d.rec {
        None => false,
        Some(r) if r.stopping => true,
        Some(r) => {
            r.send_after = Some(alternate);
            stop(s);
            true
        }
    }
}

fn shortcut(ev: &KeyEvent) -> bool {
    ev.key == Key::Command('m') && ev.mods.shift && !ev.mods.alt && !ev.mods.ctrl
}

/// ⇧⌘M starts or finishes; while a recording is on, Esc throws it away.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if shortcut(ev) {
        if !tx.key_repeat() {
            tx.instant();
            let starting = s.dictation.rec.is_none();
            toggle(s);
            if starting && s.dictation.rec.is_some() {
                s.dictation.hold = Hold::Pressed;
            }
        }
        return true;
    }
    if ev.key == Key::Escape && s.dictation.rec.is_some() {
        tx.instant();
        cancel(s);
        return true;
    }
    false
}

/// The shortcut's M coming up (with ⌘ still down, or after it): a hold past `HOLD` was
/// push-to-talk, so the recording finishes.
pub fn key_up(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = matches!(&ev.key, Key::Command('m')) || matches!(&ev.key, Key::Text(t) if t.eq_ignore_ascii_case("m"));
    if !m || s.dictation.hold == Hold::None {
        return false;
    }
    if let Hold::Since(at) = s.dictation.hold
        && s.dictation.now - at >= HOLD
    {
        tx.instant();
        stop(s);
    }
    s.dictation.hold = Hold::None;
    true
}

/// Each frame: looks for Hex, follows Hex's thread, and puts Hex's words at the caret.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let d = &mut s.dictation;
    let was_preparing = d.preparing();
    d.now = now;
    let mut changed = false;
    if d.rec.is_none() && d.looked.is_none_or(|t| now - t >= LOOK_SECS) {
        d.looked = Some(now);
        let installed = d.backend.installed();
        changed |= installed != d.installed;
        d.installed = installed;
    }
    let Some(rec) = &mut d.rec else { return changed };
    let round = rec.round;
    if d.hold == Hold::Pressed {
        d.hold = Hold::Since(now);
    }
    let mut heard: Vec<Heard> = d.shared.lock().unwrap_or_else(|e| e.into_inner()).heard.drain(..).filter(|(r, _)| *r == round).map(|(_, h)| h).collect();
    if rec.cmd.is_none() && !rec.ready && now - rec.since >= d.prepare {
        heard.push(Heard::Ready);
    }
    if rec.due.is_some_and(|t| now >= t) {
        heard.push(Heard::Words(std::env::var("SEER_FAKE_WORDS").unwrap_or_else(|_| FAKE_WORDS.into())));
    }
    // Hex's thread can't wake the app, so look again soon; and on time for the slow-start line
    // and the fake's next step.
    let mut next = 0.1_f64;
    if !rec.ready && !rec.stopping {
        next = next.min(rec.since + PREPARING_AFTER - now).min(if rec.cmd.is_none() { rec.since + d.prepare - now } else { next });
    }
    if let Some(t) = rec.due {
        next = next.min(t - now);
    }
    tx.wake_after(Duration::from_secs_f64(next.max(0.001)));
    changed |= was_preparing != d.preparing();
    for h in heard {
        changed = true;
        match h {
            Heard::Ready => {
                if let Some(r) = s.dictation.rec.as_mut() {
                    r.ready = true;
                }
            }
            Heard::Words(words) => {
                let send = s.dictation.rec.take().and_then(|r| r.send_after);
                insert(s, &words);
                if let Some(alternate) = send {
                    s.submit(alternate, tx);
                }
            }
            Heard::Failed(why) => s.dictation.fail(why),
        }
    }
    changed
}

/// The words at the caret, replacing a selection, with a space from the text either side.
fn insert(s: &mut App, words: &str) {
    let words = words.trim();
    if words.is_empty() {
        return;
    }
    let c = &mut s.composer;
    let (start, end) = (c.anchor.min(c.focus), c.anchor.max(c.focus));
    let before = c.text[..start.min(c.text.len())].chars().next_back().is_some_and(|ch| !ch.is_whitespace());
    let after = c.text.get(end..).and_then(|t| t.chars().next()).is_some_and(|ch| !ch.is_whitespace());
    let mut put = String::new();
    if before {
        put.push(' ');
    }
    put.push_str(words);
    if after {
        put.push(' ');
    }
    c.insert(&put);
}

/// What sits before Send, as in the iOS app's composer: the mic, a quiet stop while it listens,
/// and a spinner in the stop while the words come. Nothing while Hex isn't there.
pub fn toolbar(s: &App, p: &Pal) -> Option<El> {
    let d = &s.dictation;
    if !d.installed && d.rec.is_none() {
        return None;
    }
    let finishing = d.rec.as_ref().is_some_and(|r| r.stopping);
    // The mic and the stop swap at once on the press, as iOS's do.
    let still = no_motion().enter;
    let mut b = el()
        .key("mic")
        .role("button")
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .motion(still)
        .preserve_focus_on_press(true);
    b = match &d.rec {
        None => b.label("Dictate with Hex").help("Dictate with Hex ⇧⌘M").child(icon(&MIC, 16.0).color(p.icon_muted).motion(still)),
        Some(_) if finishing => b.label("Finishing dictation").fill(p.layer2).child(activity_ring_on(p, p.layer2, 14.0)),
        // iOS's stop.fill in a tertiary fill; the same square as the run's Stop.
        Some(_) => b.label("Stop dictation").help("Stop dictation ⇧⌘M").fill(p.layer2)
            .child(el().key("stop-glyph").size(10.0, 10.0).radius(2.0).fill(p.text).motion(still)),
    };
    if !finishing {
        b = b.hover(p.hover).press(p.pressed).on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            toggle(s);
        });
    }
    Some(b)
}

/// Under the toolbar, as in iOS: "Preparing dictation…" while Hex is slow to start, or what Hex
/// refused with.
pub fn status(s: &App, p: &Pal) -> Option<El> {
    let d = &s.dictation;
    let line = row().key("dictation-status").px(16.0).pb(12.0).gap(8.0).center_y();
    if d.preparing() {
        let bg = crate::view::composer_fill(p);
        return Some(line.child(activity_ring_on(p, bg, 12.0)).child(text("Preparing dictation…").font(UI_SMALL).color(p.muted)));
    }
    d.error().map(|why| line.child(text(why.to_owned()).font(UI_SMALL).color(p.danger)))
}

/// The real Hex, on its own thread: borrows Hex's microphone, keeps the capture alive until Stop or Cancel, then waits for the whole transcript.
fn listen(file: &std::path::Path, rx: &mpsc::Receiver<Cmd>, shared: &Arc<Mutex<Shared>>, round: u64) -> Option<Heard> {
    let hex = match Hex::connect_at(file, SOURCE) {
        Ok(h) => h,
        Err(why) => return Some(Heard::Failed(why)),
    };
    let cap = match hex.start() {
        Ok(c) => c,
        Err(why) => return Some(Heard::Failed(why)),
    };
    shared.lock().unwrap_or_else(|e| e.into_inner()).heard.push((round, Heard::Ready));
    let mut beat = std::time::Instant::now();
    let mut missed = 0;
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Cmd::Stop) => break,
            Ok(Cmd::Cancel) | Err(RecvTimeoutError::Disconnected) => {
                hex.cancel(&cap);
                return None;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if beat.elapsed() >= Duration::from_secs(3) {
            beat = std::time::Instant::now();
            match hex.heartbeat(&cap) {
                Ok(()) => missed = 0,
                // Hex ended it (Escape in Hex, or it restarted): there's nothing to finish.
                Err(code) if code.starts_with("dictation-") => return Some(Heard::Failed("Hex stopped listening".into())),
                Err(_) => {
                    missed += 1;
                    if missed >= 3 {
                        return Some(Heard::Failed("Lost Hex while listening".into()));
                    }
                }
            }
        }
    }
    std::thread::sleep(TAIL);
    Some(match hex.finish(&cap) {
        Ok(words) if words.trim().is_empty() => Heard::Failed("Hex heard nothing".into()),
        Ok(words) => Heard::Words(words),
        Err(why) => Heard::Failed(why),
    })
}

// The storybook's "Composer › Dictation", on the fake Hex.

pub const STORY_KEYS: &[rune_knobs::Action<App>] = &[
    crate::stories::REPLAY,
    rune_knobs::Action { cap: "S", label: "Start", run: |s, tx| {
        tx.instant();
        s.dictation.prepare = 0.0;
        start(s);
    } },
    rune_knobs::Action { cap: "P", label: "Slow start", run: |s, tx| {
        tx.instant();
        cancel(s);
        s.dictation.slow_fake();
        start(s);
    } },
    rune_knobs::Action { cap: "F", label: "Finish", run: |s, tx| {
        tx.instant();
        stop(s);
    } },
    rune_knobs::Action { cap: "E", label: "Next error", run: |s, tx| {
        tx.instant();
        let next = match s.dictation.error() {
            Some(hex_client::NOT_RUNNING) => hex_client::BUSY,
            Some(hex_client::BUSY) => hex_client::OUTDATED,
            _ => hex_client::NOT_RUNNING,
        };
        cancel(s);
        s.dictation.fail(next);
    } },
];

/// A draft with the caret after it, then the loop iOS shows: the stop while it listens, the
/// spinner while the words come, the words at the caret.
pub fn story_open(s: &mut App, tx: &mut Tx) {
    s.dictation = Dictation::fake();
    s.dictation.now = s.clock;
    s.composer = rune::TextEdit::new("Looks right. Before you merge,");
    start(s);
    s.play.after(1.6, |s, tx| {
        tx.instant();
        stop(s);
    }, tx);
}
