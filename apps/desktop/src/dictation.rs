//! Dictation through Hex: the mic in the composer's toolbar, or ⇧⌘M, records and puts the words
//! at the caret, ready to edit. A press starts and a second press (or the stop) finishes;
//! holding ⇧⌘M past `HOLD` talks until it comes up. Esc throws the recording away. Send or ⏎
//! while it listens finishes and sends with the words, as the iOS app's Send does; otherwise
//! nothing is sent.
//!
//! It looks like the OpenCode iOS app's composer: while it listens the mic is a quiet stop beside
//! Send, which shows a spinner while the words come; "Preparing dictation…", the words so far,
//! and what went wrong sit under the toolbar. `hex_client::dictate` runs each dictation on its
//! own thread over whatever this OS's Hex offers (its capture API on macOS, a live stream from
//! the app's own microphone on Windows, which also sends the words so far). Hex pastes nothing,
//! so the composer keeps its focus. An installed Hex that isn't running starts on the press.
//!
//! The mic shows while Hex is there (on Windows always, so a press without Hex offers its
//! download). Tests, headless runs and the storybook use a fake Hex unless `SEER_HEX` names an
//! endpoint file, so they never open the microphone.

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hex_client::{Cmd, Fix, Heard, Presence, Problem};
use rune::{El, Icon, Key, KeyEvent, Tx, el, icon, row, text};

use crate::App;
use crate::palette::Pal;
use crate::view::{UI_SMALL, activity_ring_sized};

/// Who Hex says is recording.
const SOURCE: &str = "OpenCode";
/// Holding the shortcut longer than this is push-to-talk: the recording ends when it comes up.
const HOLD: f64 = 0.3;
/// "Preparing dictation…" shows once Hex has taken this long to start, so a quick start shows
/// nothing but the stop.
const PREPARING_AFTER: f64 = 0.25;
/// Seconds between looks for Hex while idle.
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
    /// The real Hex: its endpoint file, and its data folder (where Windows' Hex is installed).
    Hex { file: PathBuf, support: PathBuf },
}

impl Backend {
    fn detect() -> Backend {
        let headless = crate::headless() || crate::arg("--scene").is_some() || crate::arg("--fixture").is_some();
        if std::env::var_os("SEER_HEX").is_none() && (cfg!(test) || headless) {
            return Backend::Fake;
        }
        let support = crate::dirs::hex_support().unwrap_or_default();
        Backend::Hex { file: hex_client::discovery(&support), support }
    }

    fn presence(&self) -> Presence {
        match self {
            Backend::Fake => Presence::Running,
            Backend::Hex { file, support } => hex_client::presence(file, support),
        }
    }
}

/// Where a dictation is. A press goes `Idle` (or `Failed`) → `Starting` → `Listening` →
/// `Finishing` → `Idle` with the words at the caret; Esc goes back to `Idle` from any of the
/// three, and Hex or the microphone refusing goes to `Failed`.
enum Phase {
    Idle,
    /// Pressed: Hex is starting, or taking the microphone. `slow` once that has taken
    /// `PREPARING_AFTER`, so "Preparing dictation…" shows.
    Starting { rec: Rec, slow: bool },
    /// Hex is listening; `heard` is the words so far, when its model sends them.
    Listening { rec: Rec, heard: Option<Words> },
    /// Stopped; the words are coming. The words so far stay until they land.
    Finishing { rec: Rec, heard: Option<Words> },
    /// What went wrong, until the next press or send (as iOS keeps it).
    Failed(Problem),
}

/// Words so far: `stable` won't change, `tentative` may.
#[derive(Clone)]
struct Words {
    stable: String,
    tentative: String,
}

struct Rec {
    round: u64,
    /// When the press started it.
    since: f64,
    /// Send or ⏎ finished it: the words are sent when they land (true for ⌘⏎).
    send_after: Option<bool>,
    /// Tells the real Hex's thread to stop or cancel; None with the fake.
    cmd: Option<Sender<Cmd>>,
    /// The fake's words are due at this time.
    due: Option<f64>,
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
    /// Where Hex was at the last look.
    presence: Presence,
    looked: Option<f64>,
    phase: Phase,
    /// What Hex's threads heard, tagged with their recording.
    heard: Arc<Mutex<Vec<(u64, Heard)>>>,
    round: u64,
    hold: Hold,
    /// Seconds the fake takes to start.
    prepare: f64,
}

impl Default for Dictation {
    fn default() -> Self {
        Self::with(Backend::detect())
    }
}

impl Dictation {
    fn with(backend: Backend) -> Dictation {
        let presence = backend.presence();
        Dictation { backend, presence, looked: None, phase: Phase::Idle, heard: Arc::default(), round: 0, hold: Hold::None, prepare: 0.0 }
    }

    /// The storybook's Hex, whatever the app runs on.
    pub fn fake() -> Dictation {
        Self::with(Backend::Fake)
    }

    /// The real Hex behind this endpoint file (tests point it at a stand-in).
    #[cfg(test)]
    pub fn at(file: PathBuf) -> Dictation {
        let support = file.parent().map(PathBuf::from).unwrap_or_default();
        Self::with(Backend::Hex { file, support })
    }

    fn rec(&self) -> Option<&Rec> {
        match &self.phase {
            Phase::Starting { rec, .. } | Phase::Listening { rec, .. } | Phase::Finishing { rec, .. } => Some(rec),
            Phase::Idle | Phase::Failed(_) => None,
        }
    }

    /// The microphone is open (or opening): starting or listening.
    pub fn recording(&self) -> bool {
        matches!(self.phase, Phase::Starting { .. } | Phase::Listening { .. })
    }

    /// A recording is on: preparing, listening or finishing.
    pub fn active(&self) -> bool {
        self.rec().is_some()
    }

    pub fn error(&self) -> Option<&str> {
        match &self.phase {
            Phase::Failed(p) => Some(&p.says),
            _ => None,
        }
    }

    /// The mic shows while Hex is there, or always where a press offers its download.
    fn shows_mic(&self) -> bool {
        self.presence != Presence::Missing || hex_client::MIC_WITHOUT_HEX || self.active()
    }

    /// The fake starts slowly from now on, as a Hex loading its model would.
    pub fn slow_fake(&mut self) {
        self.prepare = FAKE_SLOW_START;
    }

    #[cfg(test)]
    pub fn preparing_shown(&self) -> bool {
        matches!(self.phase, Phase::Starting { slow: true, .. })
    }

    fn fail(&mut self, why: Problem) {
        self.phase = Phase::Failed(why);
        self.hold = Hold::None;
    }
}

pub fn start(s: &mut App) {
    let now = s.clock;
    let d = &mut s.dictation;
    if d.active() {
        return;
    }
    d.presence = d.backend.presence();
    if d.presence == Presence::Missing {
        let why = if hex_client::MIC_WITHOUT_HEX { Problem::not_installed() } else { Problem::of(hex_client::NOT_RUNNING) };
        d.fail(why);
        return;
    }
    d.round += 1;
    let round = d.round;
    let cmd = match &d.backend {
        Backend::Fake => None,
        Backend::Hex { file, support } => {
            let (tx, rx) = mpsc::channel();
            let (file, support, heard) = (file.clone(), support.clone(), d.heard.clone());
            std::thread::spawn(move || {
                hex_client::dictate(&file, &support, SOURCE, &rx, &mut |h| heard.lock().unwrap_or_else(|e| e.into_inner()).push((round, h)));
            });
            Some(tx)
        }
    };
    let rec = Rec { round, since: now, send_after: None, cmd, due: None };
    d.phase = if rec.cmd.is_none() && d.prepare <= 0.0 { Phase::Listening { rec, heard: None } } else { Phase::Starting { rec, slow: false } };
}

/// Stops the microphone; the words land at the caret when Hex has them.
pub fn stop(s: &mut App) {
    let now = s.clock;
    let d = &mut s.dictation;
    d.hold = Hold::None;
    let (rec, heard) = match std::mem::replace(&mut d.phase, Phase::Idle) {
        Phase::Starting { rec, .. } => (rec, None),
        Phase::Listening { rec, heard } => (rec, heard),
        other => {
            d.phase = other;
            return;
        }
    };
    let mut rec = rec;
    match &rec.cmd {
        Some(cmd) => {
            let _ = cmd.send(Cmd::Stop);
        }
        None => rec.due = Some(now + FAKE_TRANSCRIBE),
    }
    d.phase = Phase::Finishing { rec, heard };
}

pub fn cancel(s: &mut App) {
    let d = &mut s.dictation;
    d.hold = Hold::None;
    if !d.active() {
        return;
    }
    if let Some(cmd) = d.rec().and_then(|r| r.cmd.as_ref()) {
        let _ = cmd.send(Cmd::Cancel);
    }
    d.phase = Phase::Idle;
}

/// The mic or the shortcut: starts, or finishes the recording that's listening.
fn toggle(s: &mut App) {
    match &s.dictation.phase {
        Phase::Idle | Phase::Failed(_) => start(s),
        Phase::Starting { .. } | Phase::Listening { .. } => stop(s),
        Phase::Finishing { .. } => {}
    }
}

/// Send or ⏎ (`alternate` for ⌘⏎): while it listens, finishes and sends with the words, as the
/// iOS app's Send does; while the words come, waits for them. True when dictation took it.
pub fn submit(s: &mut App, alternate: bool) -> bool {
    let d = &mut s.dictation;
    match &mut d.phase {
        Phase::Idle => false,
        Phase::Failed(_) => {
            d.phase = Phase::Idle;
            false
        }
        Phase::Finishing { .. } => true,
        Phase::Starting { rec, .. } | Phase::Listening { rec, .. } => {
            rec.send_after = Some(alternate);
            stop(s);
            true
        }
    }
}

fn shortcut(ev: &KeyEvent) -> bool {
    crate::keymap::Bind::Dictate.matches(ev)
}

/// ⇧⌘M starts or finishes; while a recording is on, Esc throws it away.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if shortcut(ev) {
        if !tx.key_repeat() {
            tx.instant();
            let starting = !s.dictation.active();
            toggle(s);
            if starting && s.dictation.active() {
                s.dictation.hold = Hold::Pressed;
            }
        }
        return true;
    }
    if ev.key == Key::Escape && s.dictation.active() {
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
        && s.clock - at >= HOLD
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
    let mut changed = false;
    if !d.active() && d.looked.is_none_or(|t| now - t >= LOOK_SECS) {
        d.looked = Some(now);
        let presence = d.backend.presence();
        changed |= presence != d.presence;
        d.presence = presence;
    }
    let Some(rec) = d.rec() else { return changed };
    let (round, since, fake, due) = (rec.round, rec.since, rec.cmd.is_none(), rec.due);
    if d.hold == Hold::Pressed {
        d.hold = Hold::Since(now);
    }
    let mut heard: Vec<Heard> = d.heard.lock().unwrap_or_else(|e| e.into_inner()).drain(..).filter(|(r, _)| *r == round).map(|(_, h)| h).collect();
    let starting = matches!(d.phase, Phase::Starting { .. });
    if fake && starting && now - since >= d.prepare {
        heard.push(Heard::Ready);
    }
    if due.is_some_and(|t| now >= t) {
        heard.push(Heard::Words(std::env::var("SEER_FAKE_WORDS").unwrap_or_else(|_| FAKE_WORDS.into())));
    }
    // Hex's thread can't wake the app, so look again soon; and on time for the slow-start line
    // and the fake's next step.
    let mut next = 0.1_f64;
    if starting {
        next = next.min(since + PREPARING_AFTER - now);
        if fake {
            next = next.min(since + d.prepare - now);
        }
    }
    if let Some(t) = due {
        next = next.min(t - now);
    }
    tx.wake_after(Duration::from_secs_f64(next.max(0.001)));
    if let Phase::Starting { slow, .. } = &mut d.phase
        && !*slow
        && now - since >= PREPARING_AFTER
    {
        *slow = true;
        changed = true;
    }
    for h in heard {
        changed = true;
        hear(s, h, tx);
    }
    changed
}

/// One thing Hex's thread (or the fake) heard, moving the phase on.
fn hear(s: &mut App, h: Heard, tx: &mut Tx) {
    let d = &mut s.dictation;
    match h {
        Heard::Ready => {
            if let Phase::Starting { .. } = d.phase
                && let Phase::Starting { rec, .. } = std::mem::replace(&mut d.phase, Phase::Idle)
            {
                d.phase = Phase::Listening { rec, heard: None };
            }
        }
        Heard::Partial { stable, tentative } => {
            if let Phase::Listening { heard, .. } | Phase::Finishing { heard, .. } = &mut d.phase {
                *heard = Some(Words { stable, tentative });
            }
        }
        Heard::Words(words) => {
            let send = d.rec().and_then(|r| r.send_after);
            d.phase = Phase::Idle;
            insert(s, &words);
            if let Some(alternate) = send {
                s.submit(alternate, tx);
            }
        }
        Heard::Failed(why) => d.fail(why),
    }
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
/// and a spinner in the stop while the words come. Nothing while Hex isn't there (except on
/// Windows, where the mic offers Hex's download).
pub fn toolbar(s: &App, p: &Pal) -> Option<El> {
    let d = &s.dictation;
    if !d.shows_mic() {
        return None;
    }
    let finishing = matches!(d.phase, Phase::Finishing { .. });
    // The mic and the stop swap at once on the press, as iOS's do.
    let still = rune::Spring::NONE;
    let mut b = el()
        .key("mic")
        .role("button")
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .motion(still)
        .motion_intent(rune::MotionIntent::CUT)
        .preserve_focus_on_press(true);
    b = match &d.phase {
        Phase::Idle | Phase::Failed(_) => b.label("Dictate with Hex").help(crate::keymap::Bind::Dictate.hint("Dictate with Hex")).child(icon(&MIC, 16.0).color(p.icon_muted).motion(still)),
        Phase::Finishing { .. } => b.label("Finishing dictation").fill(p.layer2).child(activity_ring_sized(p, 14.0)),
        // iOS's stop.fill in a tertiary fill; the same square as the run's Stop.
        Phase::Starting { .. } | Phase::Listening { .. } => b.label("Stop dictation").help(crate::keymap::Bind::Dictate.hint("Stop dictation")).fill(p.layer2)
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

/// Under the toolbar, as in iOS: "Preparing dictation…" while Hex is slow to start, the words so
/// far while it listens, or what went wrong with what to do about it.
pub fn status(s: &App, p: &Pal) -> Option<El> {
    let line = row().key("dictation-status").px(16.0).pb(12.0).gap(8.0).center_y();
    match &s.dictation.phase {
        Phase::Starting { slow: true, .. } => Some(line.child(activity_ring_sized(p, 12.0)).child(text("Preparing dictation…").font(UI_SMALL).color(p.muted))),
        Phase::Listening { heard: Some(w), .. } | Phase::Finishing { heard: Some(w), .. } => heard_line(line, p, w),
        Phase::Failed(why) => {
            // In a narrow composer the reason gives way, so its fix stays whole.
            let line = line.child(text(why.says.clone()).font(UI_SMALL).color(p.danger).truncate());
            Some(match why.fix {
                Some(fix) => line.child(fix_button(p, fix)),
                None => line,
            })
        }
        _ => None,
    }
}

/// The words so far: the settled text, cut at its start when the line is full, then the faint
/// tail Hex may still change.
fn heard_line(line: El, p: &Pal, w: &Words) -> Option<El> {
    if w.stable.is_empty() && w.tentative.is_empty() {
        return None;
    }
    let words = row().key("dictation-heard").shrink()
        .child(text(w.stable.clone()).font(UI_SMALL).color(p.muted).truncate_head())
        .child(text(w.tentative.clone()).font(UI_SMALL).color(p.faint));
    Some(line.child(words))
}

/// What a problem offers: Hex's download, Hex's window, or the microphone privacy page.
fn fix_button(p: &Pal, fix: Fix) -> El {
    row().key("dictation-fix").role("button").label(fix.label().to_owned()).h(20.0).px(4.0).center_y().radius(4.0).hover(p.hover).press(p.pressed)
        .preserve_focus_on_press(true)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            // The fake (storybook, headless) opens nothing.
            let Backend::Hex { support, .. } = &s.dictation.backend else { return };
            match fix {
                Fix::Install => {
                    let _ = tx.open_url(hex_client::DOWNLOAD);
                }
                Fix::OpenHex => {
                    if let Err(why) = hex_client::launch(&support.clone(), true) {
                        s.dictation.fail(Problem { says: why, fix: None });
                    }
                }
                Fix::MicSettings => hex_client::open_mic_settings(),
            }
        })
        .child(text(fix.label()).font(UI_SMALL).color(p.text))
}

// The storybook's "Composer › Dictation", on the fake Hex.

pub const STORY_KEYS: &[crate::stories::Trigger] = &[
    crate::stories::replay(story_open),
    crate::stories::Trigger { key: 's', label: "Start", run: |s, tx| {
        tx.instant();
        s.dictation.prepare = 0.0;
        start(s);
    } },
    crate::stories::Trigger { key: 'p', label: "Slow start", run: |s, tx| {
        tx.instant();
        cancel(s);
        s.dictation.slow_fake();
        start(s);
    } },
    crate::stories::Trigger { key: 'w', label: "Words so far", run: |s, tx| {
        tx.instant();
        if !s.dictation.recording() {
            s.dictation.prepare = 0.0;
            start(s);
        }
        let round = s.dictation.round;
        s.dictation.heard.lock().unwrap_or_else(|e| e.into_inner()).extend([
            (round, Heard::Ready),
            (round, Heard::Partial { stable: "keep the retry budget as it is".into(), tentative: " and add a".into() }),
        ]);
    } },
    crate::stories::Trigger { key: 'f', label: "Finish", run: |s, tx| {
        tx.instant();
        stop(s);
    } },
    crate::stories::Trigger { key: 'e', label: "Next error", run: |s, tx| {
        tx.instant();
        let next = match s.dictation.error() {
            Some(hex_client::NOT_RUNNING) => hex_client::BUSY,
            Some(hex_client::BUSY) => hex_client::OUTDATED,
            Some(hex_client::OUTDATED) => hex_client::NOT_INSTALLED,
            Some(hex_client::NOT_INSTALLED) => hex_client::MIC_BLOCKED,
            _ => hex_client::NOT_RUNNING,
        };
        cancel(s);
        s.dictation.fail(Problem::of(next));
    } },
];

/// A draft with the caret after it, then the loop iOS shows: the stop while it listens, the
/// spinner while the words come, the words at the caret.
fn story_open(s: &mut App, tx: &mut rune::Play<App>) {
    s.dictation = Dictation::fake();
    s.composer = rune::TextEdit::new("Looks right. Before you merge,");
    start(s);
    tx.after(1.6, |s, tx| {
        tx.instant();
        stop(s);
    });
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::composer("Composer/Dictation", STORY_KEYS),
]);
