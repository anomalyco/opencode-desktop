//! Dictation through Hex: the mic in the composer's toolbar, or ⇧⌘M, borrows Hex's microphone
//! and model and puts the words at the caret, ready to edit. A press starts and a second press
//! (or the mic) finishes; holding ⇧⌘M past `HOLD` talks until it comes up. Esc throws the
//! recording away. Nothing is ever sent.
//!
//! The words come from Hex's capture API (`hex_client`): the app owns the recording, so a pause
//! never ends it, the transcript comes back whole and Hex pastes nothing, so the composer keeps
//! its focus. Hex has no partial transcripts, so while it listens the toolbar shows "Listening…"
//! and the microphone's level. The mic shows while Hex's endpoint file is there (Hex removes it
//! when it quits). Tests, headless runs and the storybook use a fake Hex unless `SEER_HEX` names
//! an endpoint file, so they never open the microphone.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hex_client::Hex;
use rune::{El, Icon, Key, KeyEvent, Tx, el, icon, row, text};

use crate::App;
use crate::palette::Pal;
use crate::view::{UI, no_motion};

/// Who Hex says is recording.
const SOURCE: &str = "OpenCode";
/// Holding the shortcut longer than this is push-to-talk: the recording ends when it comes up.
const HOLD: f64 = 0.3;
/// How long the microphone stays open after the stop, so a last word said as he presses lands.
/// Hex's own shortcut keeps 200 ms after release, but its capture API stops at once.
const TAIL: Duration = Duration::from_millis(350);
/// Seconds an error stays beside the mic.
const ERROR_SECS: f64 = 4.0;
/// Seconds between looks for Hex's endpoint file while idle.
const LOOK_SECS: f64 = 2.0;
/// The fake's transcription time.
const FAKE_TRANSCRIBE: f64 = 0.4;
pub const FAKE_WORDS: &str = "keep the retry budget as it is and add a test for the reconnect";

const MIC: Icon = Icon {
    name: "outline-mic",
    view: 16.0,
    body: r#"<path d="M8 1.5C6.89543 1.5 6 2.39543 6 3.5V8C6 9.10457 6.89543 10 8 10C9.10457 10 10 9.10457 10 8V3.5C10 2.39543 9.10457 1.5 8 1.5Z" stroke="currentColor"/><path d="M3.5 7.5V8C3.5 10.4853 5.51472 12.5 8 12.5C10.4853 12.5 12.5 10.4853 12.5 8V7.5" stroke="currentColor" stroke-linecap="square"/><path d="M8 12.5V14.5" stroke="currentColor" stroke-linecap="square"/>"#,
};

enum Backend {
    /// In-process: words on stop, levels from the clock while `talking`.
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
    error: Option<(String, f64)>,
    hold: Hold,
    /// The meter as drawn, 0–1, eased toward the microphone's level.
    pub(crate) level: f32,
    /// The fake is hearing words (the storybook's W), since this time.
    talking: Option<f64>,
}

struct Rec {
    round: u64,
    /// Waiting for the words.
    stopping: bool,
    /// Tells the real Hex's thread to stop or cancel; None with the fake.
    cmd: Option<Sender<Cmd>>,
    /// The fake's words are due at this time.
    due: Option<f64>,
}

#[derive(Default)]
struct Shared {
    heard: Vec<(u64, Heard)>,
    /// The latest level reading in dBFS, and its recording.
    db: Option<(u64, f32)>,
}

enum Cmd {
    Stop,
    Cancel,
}

#[derive(Debug)]
enum Heard {
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
        Dictation { backend, installed, looked: None, rec: None, shared: Arc::default(), round: 0, now: 0.0, error: None, hold: Hold::None, level: 0.0, talking: None }
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

    /// The fake hears words from now on, so its meter moves.
    pub fn talk(&mut self) {
        if self.recording() {
            self.talking = Some(self.now);
        }
    }

    pub fn recording(&self) -> bool {
        self.rec.as_ref().is_some_and(|r| !r.stopping)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_ref().map(|(e, _)| e.as_str())
    }

    fn fail(&mut self, why: impl Into<String>) {
        self.rec = None;
        self.hold = Hold::None;
        self.talking = None;
        self.error = Some((why.into(), self.now));
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
    d.level = 0.0;
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
    d.rec = Some(Rec { round, stopping: false, cmd, due: None });
}

/// Stops the microphone; the words land at the caret when Hex has them.
pub fn stop(s: &mut App) {
    let d = &mut s.dictation;
    d.hold = Hold::None;
    d.talking = None;
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
    d.talking = None;
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

/// Each frame: looks for Hex, eases the meter, and puts Hex's words at the caret.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let d = &mut s.dictation;
    let dt = (now - d.now).max(0.0) as f32;
    d.now = now;
    let mut changed = false;
    if d.rec.is_none() && d.looked.is_none_or(|t| now - t >= LOOK_SECS) {
        d.looked = Some(now);
        let installed = d.backend.installed();
        changed |= installed != d.installed;
        d.installed = installed;
    }
    if d.error.as_ref().is_some_and(|(_, at)| now - at >= ERROR_SECS) {
        d.error = None;
        changed = true;
    }
    if let Some((_, at)) = &d.error {
        tx.wake_after(Duration::from_secs_f64((at + ERROR_SECS - now).max(0.0)));
    }
    let Some(rec) = &d.rec else { return changed };
    let (round, stopping, due) = (rec.round, rec.stopping, rec.due);
    if d.hold == Hold::Pressed {
        d.hold = Hold::Since(now);
    }
    tx.wake_after(Duration::from_millis(if stopping { 100 } else { 33 }));
    let heard: Vec<Heard> = {
        let mut sh = d.shared.lock().unwrap_or_else(|e| e.into_inner());
        let target = match (&d.backend, sh.db) {
            _ if stopping => 0.0,
            (Backend::Fake, _) => fake_level(d.talking.map(|t| now - t)),
            (Backend::Hex(_), Some((r, db))) if r == round => meter(db),
            _ => 0.0,
        };
        // The bar jumps up with a word and settles back more slowly, as a VU meter does.
        let rate = if target > d.level { 30.0 } else { 8.0 };
        d.level += (target - d.level) * (rate * dt).min(1.0);
        sh.heard.drain(..).filter(|(r, _)| *r == round).map(|(_, h)| h).collect()
    };
    let mut heard = heard;
    if due.is_some_and(|t| now >= t) {
        heard.push(Heard::Words(std::env::var("SEER_FAKE_WORDS").unwrap_or_else(|_| FAKE_WORDS.into())));
    }
    for h in heard {
        match h {
            Heard::Words(words) => {
                s.dictation.rec = None;
                insert(s, &words);
            }
            Heard::Failed(why) => s.dictation.fail(why),
        }
    }
    true
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

/// dBFS to the meter's 0–1: quiet rooms sit near the bottom, speech fills most of it.
fn meter(db: f32) -> f32 {
    ((db + 55.0) / 40.0).clamp(0.0, 1.0)
}

/// A voice-like level for the fake: syllables over a breath, silence when not talking.
fn fake_level(talking: Option<f64>) -> f32 {
    let Some(t) = talking else { return meter(-58.0) };
    let syllable = (t * 11.0).sin().abs() * (t * 2.3).sin().abs();
    meter(-46.0 + 34.0 * syllable as f32)
}

/// What sits before the Send button: the mic, and beside it "Listening…" with the level,
/// "Transcribing…", or what went wrong. Nothing while Hex isn't there.
pub fn toolbar(s: &App, p: &Pal) -> Option<El> {
    let d = &s.dictation;
    if !d.installed && d.rec.is_none() && d.error.is_none() {
        return None;
    }
    let listening = d.recording();
    let stopping = d.rec.as_ref().is_some_and(|r| r.stopping);
    // What it says changes at once and the row hugs it from the mic leftward: nothing glides.
    let still = no_motion().enter;
    let mut r = row().key("dictation").h(28.0).gap(8.0).center_y().shrink().motion(still);
    let label = |s: &str, color: rune::Color| text(s.to_owned()).font(UI).color(color).truncate().shrink().motion(still);
    if listening {
        r = r.child(el().key("dictation-dot").size(6.0, 6.0).radius(3.0).fill(p.danger).motion(still))
            .child(label("Listening…", p.muted))
            .child(
                el().key("dictation-meter").size(32.0, 4.0).radius(2.0).fill(p.layer3).motion(still)
                    // The fill follows the microphone frame by frame; a spring would lag the voice.
                    .child(el().key("dictation-level").w(32.0 * d.level).h(4.0).radius(2.0).fill(p.accent).motion(still)),
            );
    } else if stopping {
        r = r.child(label("Transcribing…", p.faint));
    } else if let Some(why) = d.error() {
        r = r.child(label(why, p.danger));
    }
    let mut mic = el()
        .key("mic")
        .role("button")
        .label(if listening { "Stop dictation" } else { "Dictate with Hex" })
        .help(if listening { "Stop dictation ⇧⌘M" } else { "Dictate with Hex ⇧⌘M" })
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .motion(still)
        .preserve_focus_on_press(true)
        .child(icon(&MIC, 16.0).color(if listening { p.text } else if stopping { p.icon_faint } else { p.icon_muted }));
    if listening {
        mic = mic.fill(p.hover);
    }
    if !stopping {
        mic = mic.hover(p.hover).press(p.pressed).on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            toggle(s);
        });
    }
    Some(r.child(mic))
}

/// The real Hex, on its own thread: borrows Hex's microphone, streams its level on another,
/// keeps the capture alive until Stop or Cancel, then waits for the whole transcript.
fn listen(file: &std::path::Path, rx: &mpsc::Receiver<Cmd>, shared: &Arc<Mutex<Shared>>, round: u64) -> Option<Heard> {
    let hex = match Hex::connect_at(file, SOURCE) {
        Ok(h) => h,
        Err(why) => return Some(Heard::Failed(why)),
    };
    let cap = match hex.start() {
        Ok(c) => c,
        Err(why) => return Some(Heard::Failed(why)),
    };
    if let Ok(levels) = Hex::connect_at(file, SOURCE) {
        let (cap, shared) = (cap.clone(), shared.clone());
        std::thread::spawn(move || {
            let _ = levels.levels(&cap, |db| {
                shared.lock().unwrap_or_else(|e| e.into_inner()).db = Some((round, db));
                true
            });
        });
    }
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
        start(s);
    } },
    rune_knobs::Action { cap: "W", label: "Say words", run: |s, _| s.dictation.talk() },
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

/// A draft with the caret after it, listening and hearing words.
pub fn story_open(s: &mut App, _: &mut Tx) {
    s.dictation = Dictation::fake();
    s.dictation.now = s.clock;
    s.composer = rune::TextEdit::new("Looks right. Before you merge,");
    start(s);
    s.dictation.talking = Some(s.clock);
}
